use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use cgs::MainThreadMarker;
use objc2_core_foundation::CGRect;
use tokio::sync::mpsc::UnboundedSender;

use crate::actor::{config, reactor};
use crate::common::config::{Config, ConfigCommand};
use crate::layout_engine::LayoutCommand;
use crate::sys::screen::SpaceId;
use crate::ui::menu_bar::{MenuAction, MenuIcon};
use crate::ui::settings::{Action, Finish, Request};
use crate::{actor, common};

type UpdateCheck = (
    tokio::sync::oneshot::Receiver<Result<String, String>>,
    crate::ui::settings::updates::Completion,
);

/// Data consumed by the workspace menu and miniature layout indicator.
/// Window titles, application metadata, and logical layout snapshots stay in IPC queries.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: String,
    pub index: usize,
    pub name: String,
    pub layout_mode: String,
    pub is_active: bool,
    pub window_count: usize,
    pub windows: Vec<CGRect>,
}

/// Menu-bar-only projection; workspace indices remain local to each space.
#[derive(Debug, Clone)]
pub struct DisplayWorkspaces {
    pub display_uuid: String,
    pub space: SpaceId,
    pub is_active_context: bool,
    pub workspaces: Vec<Workspace>,
}

#[derive(Debug, Clone)]
pub struct Update {
    pub active_space_is_activated: bool,
    pub displays: Vec<DisplayWorkspaces>,
}

impl Update {
    pub fn context_workspaces(&self) -> &[Workspace] {
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
    ConfigUpdated(Box<Config>),
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
    settings_source_revision: std::cell::Cell<Option<u64>>,
    settings_requests: tokio::sync::mpsc::UnboundedReceiver<crate::ui::settings::Request>,
    settings_request_tx: tokio::sync::mpsc::UnboundedSender<crate::ui::settings::Request>,
    /// Completions awaiting installed-application discovery; `Some` while a scan runs.
    app_scan: Option<Vec<Finish>>,
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
        cgs::Application::shared(&cgs::Ui::new(mtm)).install_close_window_command();
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
            settings_source_revision: std::cell::Cell::new(None),
            settings_requests,
            settings_request_tx,
            app_scan: None,
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

        let mut update_check: Option<UpdateCheck> = None;
        loop {
            tokio::select! {
                // A queued close must retire its owner before another open or UI request.
                biased;
                maybe_action = self.action_rx.recv() => {
                    if let Some(action) = maybe_action {
                        if matches!(action, MenuAction::OpenSettings) { self.open_settings().await; }
                        else { self.handle_action(action); }
                    }
                }
                result = async {
                    match &mut update_check {
                        Some((receive, _)) => receive.await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Some((_, done)) = update_check.take() {
                        done(result.unwrap_or_else(|_| Err("Couldn’t check for updates. Try again.".into())));
                    }
                }
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
                                    self.handle_config_updated(*cfg);
                                    if self.settings.is_some() {
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
                    self.handle_settings_request(request, &mut update_check).await;
                }
            }
        }
    }

    async fn handle_settings_request(
        &mut self,
        request: Request,
        update_check: &mut Option<UpdateCheck>,
    ) {
        let (response, receive) = tokio::sync::oneshot::channel();
        match request.action {
            Action::CheckUpdates(done) => {
                *update_check = Some((crate::ui::settings::updates::start(), done));
                (request.finish)(Ok(()));
                return;
            }
            Action::RefreshRuntime => {
                self.discover_applications(request.finish);
                return;
            }
            Action::Edit(edit) => self.config_tx.send(config::Event::EditSource { edit, response }),
            Action::Reload => self.config_tx.send(config::Event::ReloadSource(response)),
        }
        let result = receive
            .await
            .unwrap_or_else(|_| Err("Configuration service unavailable".into()));
        let failed = result.is_err();
        (request.finish)(result.map(|snapshot| {
            self.settings_source_revision.set(Some(snapshot.revision));
            if let Some(settings) = &self.settings {
                settings.synchronize(snapshot.source);
            }
        }));
        if failed {
            self.sync_settings().await;
        }
    }

    async fn source_snapshot(&self) -> Result<crate::common::config::ConfigSource, String> {
        let (response, result) = tokio::sync::oneshot::channel();
        self.config_tx
            .send(config::Event::QuerySourceSince { revision: None, response });
        let snapshot = result
            .await
            .map_err(|_| "Configuration service unavailable".to_string())??
            .ok_or_else(|| "Missing configuration snapshot".to_string())?;
        self.settings_source_revision.set(Some(snapshot.revision));
        Ok(snapshot.source)
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
                self.settings.as_ref().unwrap().show();
            }
            Err(error) => {
                tracing::error!(%error, "Could not open Settings");
                cgs::Alert::new(&cgs::Ui::new(self.mtm), "Could not open Settings", &error)
                    .button("OK")
                    .run_modal();
            }
        }
    }

    /// Scan installed applications off the main thread; the result returns as a `MenuAction`
    /// so the menu actor keeps handling events and no closed window or model is retained.
    fn discover_applications(&mut self, finish: Finish) {
        if self
            .settings
            .as_ref()
            .is_none_or(|settings| settings.has_installed_applications())
        {
            finish(Ok(()));
            return;
        }
        let actions = &self.action_tx;
        let waiting = self.app_scan.get_or_insert_with(|| {
            let actions = actions.clone();
            std::thread::spawn(move || {
                let apps = crate::sys::installed_apps::installed();
                let _ = actions.send(MenuAction::InstalledApplications(apps));
            });
            Vec::new()
        });
        waiting.push(finish);
    }

    async fn sync_settings(&self) {
        if self.settings.is_none() {
            return;
        }
        let (response, result) = tokio::sync::oneshot::channel();
        self.config_tx.send(config::Event::QuerySourceSince {
            revision: self.settings_source_revision.get(),
            response,
        });
        if let Ok(Ok(Some(snapshot))) = result.await {
            self.settings_source_revision.set(Some(snapshot.revision));
            if let Some(settings) = &self.settings {
                settings.synchronize(snapshot.source);
            }
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::OpenSettings => {}
            Event::Update(update) => self.handle_update(update),
            Event::ConfigUpdated(cfg) => self.handle_config_updated(*cfg),
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
            MenuAction::SettingsClosed => {
                objc2::rc::autoreleasepool(|_| {
                    drop(self.settings.take());
                    // Keep the in-flight marker so reopening cannot start a duplicate scan.
                    if let Some(waiting) = &mut self.app_scan {
                        waiting.clear();
                    }
                });
            }
            // A reopened window uses an in-flight scan only if it requested the inventory.
            MenuAction::InstalledApplications(apps) => {
                let waiting = self.app_scan.take().unwrap_or_default();
                if let Some(settings) = &self.settings
                    && !waiting.is_empty()
                {
                    settings.set_installed_applications(apps);
                    waiting.into_iter().for_each(|finish| finish(Ok(())));
                }
            }
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
        cgs::Application::open(&target.as_ref().to_string_lossy());
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

pub(crate) fn sig(update: &Update) -> u64 {
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
                let frame = *window;
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

    fn workspace(layout_mode: &str) -> Workspace {
        Workspace {
            id: "VirtualWorkspaceId(1v1)".to_string(),
            index: 0,
            name: "main".to_string(),
            layout_mode: layout_mode.to_string(),
            is_active: true,
            window_count: 1,
            windows: Vec::new(),
        }
    }

    fn update(workspaces: Vec<Workspace>) -> Update {
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
