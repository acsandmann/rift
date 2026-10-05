use std::path::Path;
use std::process::Command as ProcessCommand;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use objc2::MainThreadMarker;
use tokio::sync::mpsc::UnboundedSender;

use crate::actor::{config, reactor};
use crate::common::config::{Config, ConfigCommand};
use crate::layout_engine::LayoutCommand;
use crate::model::server::RuntimeWorkspaceData;
use crate::sys::screen::SpaceId;
use crate::ui::menu_bar::{MenuAction, MenuIcon};
use crate::{actor, common};

/// Menu-bar-only projection; workspace indices remain local to each space.
#[derive(Debug, Clone)]
pub struct DisplayWorkspaces {
    pub display_uuid: String,
    pub space: SpaceId,
    pub is_active_context: bool,
    pub workspaces: Vec<RuntimeWorkspaceData>,
}

#[derive(Debug, Clone)]
pub struct Update {
    pub active_space_is_activated: bool,
    pub displays: Vec<DisplayWorkspaces>,
}

impl Update {
    pub fn context_workspaces(&self) -> &[RuntimeWorkspaceData] {
        self.displays
            .iter()
            .find(|display| display.is_active_context)
            .map(|display| display.workspaces.as_slice())
            .unwrap_or_default()
    }
}

pub enum Event {
    OpenSettings,
    Update(Update),
    ConfigUpdated(Config),
}

enum DebounceCommand {
    Arm,
    Shutdown,
}

pub struct Menu {
    config: Config,
    rx: Receiver,
    reactor_tx: reactor::Sender,
    config_tx: config::Sender,
    action_tx: UnboundedSender<MenuAction>,
    action_rx: tokio::sync::mpsc::UnboundedReceiver<MenuAction>,
    icon: Option<MenuIcon>,
    settings: Option<crate::ui::settings::Settings>,
    settings_requests: tokio::sync::mpsc::UnboundedReceiver<crate::ui::settings::Request>,
    settings_request_tx: tokio::sync::mpsc::UnboundedSender<crate::ui::settings::Request>,
    mtm: MainThreadMarker,
    config_path: std::path::PathBuf,
    last_signature: Option<u64>,
    last_update: Option<Update>,
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

impl Menu {
    pub fn new(
        config: Config,
        rx: Receiver,
        reactor_tx: reactor::Sender,
        config_tx: config::Sender,
        mtm: MainThreadMarker,
        config_path: std::path::PathBuf,
    ) -> Self {
        let (settings_request_tx, settings_requests) = tokio::sync::mpsc::unbounded_channel();
        let (action_tx, action_rx) = tokio::sync::mpsc::unbounded_channel();
        let layout_folder = config.settings.ui.menu_bar.resolved_layout_folder();
        let mut icon = config
            .settings
            .ui
            .menu_bar
            .enabled
            .then(|| MenuIcon::new(mtm, action_tx.clone(), reactor_tx.clone(), &layout_folder));
        if let Some(icon) = &mut icon {
            icon.update_config(&config.settings.ui.menu_bar, &config.keys);
        }
        Self {
            icon,
            settings: None,
            settings_requests,
            settings_request_tx,
            config,
            rx,
            reactor_tx,
            config_tx,
            action_tx,
            action_rx,
            mtm,
            config_path,
            last_signature: None,
            last_update: None,
        }
    }

    pub async fn run(mut self) {
        const DEBOUNCE: Duration = Duration::from_millis(150);

        let mut pending: Option<Event> = None;
        let (tick_tx, mut tick_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let debounce_tx = Self::spawn_debouncer(DEBOUNCE, tick_tx);

        loop {
            tokio::select! {
                maybe_tick = tick_rx.recv() => {
                    if maybe_tick.is_none() {
                        if let Some(ev) = pending.take() {
                            self.handle_event(ev);
                        }
                        break;
                    }

                    if let Some(ev) = pending.take() {
                        self.handle_event(ev);
                    }
                }

                maybe = self.rx.recv() => {
                    match maybe {
                        Some((span, event)) => {
                            let _enter = span.enter();
                            match event {
                                Event::Update(_) => {
                                    pending = Some(event);
                                    let _ = debounce_tx.send(DebounceCommand::Arm);
                                }
                                Event::ConfigUpdated(cfg) => {
                                    self.handle_config_updated(cfg);
                                    if self.settings.as_ref().is_some_and(|s| s.visible()) {
                                        self.sync_settings().await;
                                    }
                                }
                                Event::OpenSettings => self.open_settings().await,
                            }
                        }
                        None => {
                            let _ = debounce_tx.send(DebounceCommand::Shutdown);
                            if let Some(ev) = pending.take() {
                                self.handle_event(ev);
                            }
                            break;
                        }
                    }
                }

                Some(request) = self.settings_requests.recv() => {
                    let (response, result) = tokio::sync::oneshot::channel();
                    self.config_tx.send(match request.action {
                        crate::ui::settings::Action::Edit(edit) => config::Event::EditSource { edit, response },
                        crate::ui::settings::Action::Reload => config::Event::ReloadSource(response),
                        crate::ui::settings::Action::RefreshRuntime => {
                            let (displays, applications) = self.settings_runtime().await;
                            if let Some(settings) = &self.settings {
                                settings.refresh_applications(applications);
                                settings.refresh_displays(displays);
                            }
                            config::Event::QuerySource(response)
                        },
                    });
                    let result = result.await.unwrap_or_else(|_| Err("Configuration service unavailable".into()));
                    let source = result.as_ref().ok().cloned();
                    (request.finish)(result);
                    if let Some(source) = source {
                        if let Some(settings) = &self.settings { settings.synchronize(source); }
                    } else {
                        self.sync_settings().await;
                    }
                }
                maybe_action = self.action_rx.recv() => {
                    if let Some(action) = maybe_action {
                        if matches!(action, MenuAction::OpenSettings) { self.open_settings().await; }
                        else { self.handle_action(action); }
                    }
                }
            }
        }
    }

    async fn source_snapshot(&self) -> Result<crate::common::config::ConfigSource, String> {
        let (response, result) = tokio::sync::oneshot::channel();
        self.config_tx.send(config::Event::QuerySource(response));
        result.await.map_err(|_| "Configuration service unavailable".to_string())?
    }

    async fn settings_runtime(
        &self,
    ) -> (
        Vec<crate::sys::screen::ScreenInfo>,
        Vec<rift_protocol::ApplicationData>,
    ) {
        let (displays_tx, displays_rx) = tokio::sync::oneshot::channel();
        let (apps_tx, apps_rx) = tokio::sync::oneshot::channel();
        self.reactor_tx.send(reactor::Event::Query(reactor::QueryRequest::DisplaysAsync(
            displays_tx,
        )));
        self.reactor_tx
            .send(reactor::Event::Query(reactor::QueryRequest::ApplicationsAsync(
                apps_tx,
            )));
        let (displays, applications) = tokio::join!(displays_rx, apps_rx);
        (
            displays.unwrap_or_default().into_iter().map(|d| d.info).collect(),
            applications.unwrap_or_default(),
        )
    }

    async fn open_settings(&mut self) {
        match self.source_snapshot().await {
            Ok(source) => {
                let (displays, applications) = self.settings_runtime().await;
                if let Some(settings) = &self.settings {
                    settings.refresh_applications(applications);
                    settings.synchronize(source);
                    settings.refresh_displays(displays);
                } else {
                    self.settings = Some(crate::ui::settings::Settings::new(
                        cgs::Ui::new(self.mtm),
                        source,
                        self.config_path.clone(),
                        displays,
                        applications,
                        self.settings_request_tx.clone(),
                        {
                            let actions = self.action_tx.clone();
                            move || {
                                let _ = actions.send(MenuAction::SettingsClosed);
                            }
                        },
                    ));
                }
                let settings = self.settings.as_ref().unwrap();
                settings.show();
                self.sync_settings().await;
            }
            Err(error) => {
                tracing::error!(%error, "Could not open Settings");
                cgs::Alert::new(&cgs::Ui::new(self.mtm), "Could not open Settings", &error)
                    .button("OK")
                    .ns_alert()
                    .runModal();
            }
        }
    }

    async fn sync_settings(&self) {
        if let Ok(source) = self.source_snapshot().await {
            if let Some(settings) = &self.settings {
                settings.synchronize(source);
            }
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::OpenSettings => {}
            Event::Update(update) => self.handle_update(update),
            Event::ConfigUpdated(cfg) => self.handle_config_updated(cfg),
        }
    }

    fn handle_update(&mut self, update: Update) {
        self.apply_update(&update);
        self.last_update = Some(update);
    }

    fn apply_update(&mut self, update: &Update) {
        let Some(icon) = &mut self.icon else { return };

        let sig = sig(update);
        if self.last_signature == Some(sig) {
            return;
        }
        self.last_signature = Some(sig);

        icon.sync_workspace_topology(update.context_workspaces(), &self.config.keys);
        icon.update_menu_state(update.active_space_is_activated, update.context_workspaces());
        icon.update_status_icon(&update.displays, &self.config.settings.ui.menu_bar);
    }

    fn handle_config_updated(&mut self, new_config: Config) {
        let should_enable = new_config.settings.ui.menu_bar.enabled;

        self.config = new_config;

        if should_enable && self.icon.is_none() {
            let layout_folder = self.config.settings.ui.menu_bar.resolved_layout_folder();
            self.icon = Some(MenuIcon::new(
                self.mtm,
                self.action_tx.clone(),
                self.reactor_tx.clone(),
                &layout_folder,
            ));
        } else if !should_enable && self.icon.is_some() {
            self.icon = None;
        }

        if let Some(icon) = &mut self.icon {
            icon.update_config(&self.config.settings.ui.menu_bar, &self.config.keys);
        }

        self.last_signature = None;
        if let Some(update) = self.last_update.take() {
            self.handle_update(update);
        }
    }

    fn handle_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::SetLayout(mode) => self
                .send_layout_command(LayoutCommand::SetWorkspaceLayout { workspace: None, mode }),
            MenuAction::NextWorkspace => {
                self.send_layout_command(LayoutCommand::NextWorkspace(None));
            }
            MenuAction::PrevWorkspace => {
                self.send_layout_command(LayoutCommand::PrevWorkspace(None));
            }
            MenuAction::SwitchToWorkspace(workspace) => {
                self.send_layout_command(LayoutCommand::SwitchToWorkspace(workspace));
            }
            MenuAction::RestoreLayout { path, scope, source } => {
                self.reactor_tx.send(reactor::Event::Command(reactor::Command::Reactor(
                    reactor::ReactorCommand::RestoreLayout { path, scope, source },
                )));
            }
            MenuAction::RestoreMasterFile(scope) => {
                self.reactor_tx.send(reactor::Event::Command(reactor::Command::Reactor(
                    reactor::ReactorCommand::RestoreLayout {
                        path: common::config::restore_file(),
                        scope,
                        source: crate::layout_engine::RestoreSource::CurrentSpace,
                    },
                )));
            }
            MenuAction::SaveLayout(path) => {
                self.reactor_tx.send(reactor::Event::Command(reactor::Command::Reactor(
                    reactor::ReactorCommand::SaveLayout { path },
                )));
                let action_tx = self.action_tx.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(150));
                    let _ = action_tx.send(MenuAction::RefreshLayoutFiles);
                });
            }
            MenuAction::SaveMasterFile => {
                self.reactor_tx.send(reactor::Event::Command(reactor::Command::Reactor(
                    reactor::ReactorCommand::SaveLayout {
                        path: common::config::restore_file(),
                    },
                )));
            }
            MenuAction::ToggleSpaceActivated => {
                self.reactor_tx.send(reactor::Event::Command(reactor::Command::Reactor(
                    reactor::ReactorCommand::ToggleSpaceActivated,
                )));
            }
            MenuAction::OpenGitHub => {
                Self::open_path_or_url("https://github.com/acsandmann/rift");
            }
            MenuAction::OpenDocumentation => {
                Self::open_path_or_url("https://acsandmann.github.io/rift-docs/");
            }
            MenuAction::OpenMatrix => {
                Self::open_path_or_url("https://matrix.to/#/#rift:matrix.org");
            }
            MenuAction::OpenSponsor => {
                Self::open_path_or_url("https://github.com/sponsors/acsandmann");
            }
            MenuAction::OpenSettings => {}
            // Run after windowWillClose returns, rather than dropping AppKit's active delegate.
            MenuAction::SettingsClosed => self.settings = None,
            MenuAction::OpenConfig => {
                Self::open_path_or_url(common::config::config_file());
            }
            MenuAction::ReloadConfig => self.reload_config(),
            MenuAction::RefreshLayoutFiles => {
                if let Some(icon) = &mut self.icon {
                    icon.refresh_layout_library();
                }
            }
            MenuAction::QuitRift => {
                self.reactor_tx.send(reactor::Event::Command(reactor::Command::Reactor(
                    reactor::ReactorCommand::SaveAndExit,
                )));
            }
        }
    }

    fn send_layout_command(&self, command: LayoutCommand) {
        self.reactor_tx.send(reactor::Event::Command(reactor::Command::Layout(command)));
    }

    fn open_path_or_url(target: impl AsRef<Path>) {
        let _ = ProcessCommand::new("open").arg(target.as_ref()).spawn();
    }

    fn reload_config(&self) {
        let (response, _result) = std::sync::mpsc::sync_channel(1);
        let msg = config::Event::ApplyConfig {
            cmd: ConfigCommand::ReloadConfig,
            response,
        };
        self.config_tx.send(msg);
    }

    fn spawn_debouncer(
        period: Duration,
        tick_tx: UnboundedSender<()>,
    ) -> mpsc::Sender<DebounceCommand> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<DebounceCommand>();

        std::thread::spawn(move || {
            loop {
                match cmd_rx.recv() {
                    Ok(DebounceCommand::Arm) => loop {
                        match cmd_rx.recv_timeout(period) {
                            Ok(DebounceCommand::Arm) => continue,
                            Ok(DebounceCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                                return;
                            }
                            Err(RecvTimeoutError::Timeout) => {
                                if tick_tx.send(()).is_err() {
                                    return;
                                }
                                break;
                            }
                        }
                    },
                    Ok(DebounceCommand::Shutdown) | Err(_) => return,
                }
            }
        });

        cmd_tx
    }
}

fn sig(update: &Update) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    update.active_space_is_activated.hash(&mut hash);
    update.displays.len().hash(&mut hash);
    for display in &update.displays {
        display.display_uuid.hash(&mut hash);
        display.space.hash(&mut hash);
        display.is_active_context.hash(&mut hash);
        display.workspaces.len().hash(&mut hash);
        for workspace in &display.workspaces {
            workspace.id.hash(&mut hash);
            workspace.index.hash(&mut hash);
            workspace.name.hash(&mut hash);
            workspace.layout_mode.hash(&mut hash);
            workspace.is_active.hash(&mut hash);
            workspace.window_count.hash(&mut hash);
            workspace.windows.len().hash(&mut hash);
            for window in &workspace.windows {
                let frame = window.info.frame;
                [
                    frame.origin.x.to_bits(),
                    frame.origin.y.to_bits(),
                    frame.size.width.to_bits(),
                    frame.size.height.to_bits(),
                ]
                .hash(&mut hash);
            }
        }
    }
    hash.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::server::RuntimeWorkspaceData;

    fn workspace(layout_mode: &str) -> RuntimeWorkspaceData {
        RuntimeWorkspaceData {
            workspace_id: crate::model::VirtualWorkspaceId::default(),
            space: crate::sys::screen::SpaceId::new(1),
            id: "VirtualWorkspaceId(1v1)".to_string(),
            index: 0,
            name: "main".to_string(),
            layout_mode: layout_mode.to_string(),
            is_active: true,
            window_count: 1,
            windows: Vec::new(),
        }
    }

    fn update(workspaces: Vec<RuntimeWorkspaceData>) -> Update {
        Update {
            active_space_is_activated: true,
            displays: vec![DisplayWorkspaces {
                display_uuid: "a".into(),
                space: SpaceId::new(1),
                is_active_context: true,
                workspaces,
            }],
        }
    }

    #[test]
    fn grouped_signature_and_command_context() {
        let mut base = update(vec![workspace("bsp")]);
        base.displays.push(DisplayWorkspaces {
            display_uuid: "b".into(),
            space: SpaceId::new(2),
            is_active_context: false,
            workspaces: vec![workspace("stack"), workspace("floating")],
        });
        let before = sig(&base);
        assert_eq!(before, sig(&base.clone()));
        assert_eq!(base.context_workspaces().len(), 1);
        let mut changed = base.clone();
        changed.displays[1].workspaces[0].is_active = false;
        assert_ne!(before, sig(&changed));
        changed = base.clone();
        changed.displays.reverse();
        assert_ne!(before, sig(&changed));
        assert_eq!(changed.context_workspaces().len(), 1);
        changed.displays[0].is_active_context = true;
        changed.displays[1].is_active_context = false;
        assert_eq!(changed.context_workspaces().len(), 2);
        changed = base.clone();
        changed.displays.pop();
        assert_ne!(before, sig(&changed));
    }

    #[test]
    fn signature_changes_when_workspace_layout_mode_changes() {
        let base = vec![workspace("bsp")];
        let changed = vec![workspace("master_stack")];

        let before = sig(&update(base));
        let after = sig(&update(changed));

        assert_ne!(before, after);
    }
}
