use std::path::Path;
use std::process::Command as ProcessCommand;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use objc2::MainThreadMarker;
use tokio::sync::mpsc::UnboundedSender;

use crate::actor::{config, reactor};
use crate::common::config::{Config, ConfigCommand};
use crate::layout_engine::LayoutCommand;
use crate::model::VirtualWorkspaceId;
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::screen::SpaceId;
use crate::ui::menu_bar::{MenuAction, MenuIcon};
use crate::{actor, common};

#[derive(Debug, Clone)]
pub struct Update {
    pub active_space: SpaceId,
    pub active_space_is_activated: bool,
    pub workspaces: Vec<WorkspaceData>,
    pub active_workspace_idx: Option<u64>,
    pub active_workspace: Option<VirtualWorkspaceId>,
    pub windows: Vec<WindowData>,
}

pub enum Event {
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
    mtm: MainThreadMarker,
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
    ) -> Self {
        let (action_tx, action_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            icon: config
                .settings
                .ui
                .menu_bar
                .enabled
                .then(|| MenuIcon::new(mtm, action_tx.clone())),
            config,
            rx,
            reactor_tx,
            config_tx,
            action_tx,
            action_rx,
            mtm,
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
                                Event::ConfigUpdated(cfg) => self.handle_config_updated(cfg),
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

                maybe_action = self.action_rx.recv() => {
                    if let Some(action) = maybe_action {
                        self.handle_action(action);
                    }
                }
            }
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
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

        let sig = sig(
            update.active_space.get() as u64,
            update.active_space_is_activated,
            update.active_workspace_idx,
            &update.workspaces,
            &update.windows,
        );
        if self.last_signature == Some(sig) {
            return;
        }
        self.last_signature = Some(sig);

        let menu_bar_settings = &self.config.settings.ui.menu_bar;
        icon.update(
            update.active_space,
            update.active_space_is_activated,
            &update.workspaces,
            update.active_workspace,
            &update.windows,
            menu_bar_settings,
            &self.config.keys,
        );
    }

    fn handle_config_updated(&mut self, new_config: Config) {
        let should_enable = new_config.settings.ui.menu_bar.enabled;

        self.config = new_config;

        if should_enable && self.icon.is_none() {
            self.icon = Some(MenuIcon::new(self.mtm, self.action_tx.clone()));
        } else if !should_enable && self.icon.is_some() {
            self.icon = None;
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
            MenuAction::ToggleSpaceActivated => {
                self.reactor_tx.send(reactor::Event::Command(reactor::Command::Reactor(
                    reactor::ReactorCommand::ToggleSpaceActivated,
                )));
            }
            MenuAction::OpenGitHub => {
                Self::open_path_or_url("https://github.com/acsandmann/rift");
            }
            MenuAction::OpenDocumentation => {
                Self::open_path_or_url("https://github.com/acsandmann/rift#readme");
            }
            MenuAction::OpenMatrix => {
                Self::open_path_or_url("https://matrix.to/#/#rift:matrix.org");
            }
            MenuAction::OpenConfig => {
                Self::open_path_or_url(common::config::config_file());
            }
            MenuAction::ReloadConfig => self.reload_config(),
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
        let (response, _fut) = r#continue::continuation();
        let msg = config::Event::ApplyConfig {
            cmd: ConfigCommand::ReloadConfig,
            response,
        };
        if let Err(e) = self.config_tx.try_send(msg) {
            let tokio::sync::mpsc::error::SendError((_span, msg)) = e;
            match msg {
                config::Event::ApplyConfig { response, .. } => std::mem::forget(response),
                config::Event::QueryConfig(response) => std::mem::forget(response),
            }
        }
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

// this is kind of reinventing the wheel but oh well i am using my brain
#[inline(always)]
fn sig(
    active_space: u64,
    active_space_is_activated: bool,
    active_workspace: Option<u64>,
    workspaces: &[WorkspaceData],
    windows: &[WindowData],
) -> u64 {
    let mut x = active_space
        ^ (windows.len() as u64).rotate_left(7)
        ^ (workspaces.len() as u64).rotate_left(13);
    if active_space_is_activated {
        x ^= 0x9E37_79B9_7F4A_7C15u64;
    }
    let mut s = active_space
        .wrapping_add(windows.len() as u64)
        .wrapping_add((workspaces.len() as u64).rotate_left(5));

    if let Some(ws) = active_workspace {
        let ws_tag = ws ^ 0xA5A5_A5A5_A5A5_A5A5u64;
        x ^= ws_tag;
        s = s.wrapping_add(ws_tag);
    }

    for ws in workspaces {
        let v = workspace_sig(ws);
        x ^= v.rotate_left(9);
        s = s.wrapping_add(v);
    }

    for w in windows {
        let v = window_sig(w);

        x ^= v;
        s = s.wrapping_add(v);
    }

    x ^ s.rotate_left(29) ^ (s >> 17)
}

#[inline(always)]
fn workspace_sig(ws: &WorkspaceData) -> u64 {
    let mut x = (ws.index as u64).rotate_left(3)
        ^ (ws.window_count as u64).rotate_left(19)
        ^ hash_str(&ws.id).rotate_left(11)
        ^ hash_str(&ws.name).rotate_left(17)
        ^ hash_str(&ws.layout_mode).rotate_left(23);
    if ws.is_active {
        x ^= 0xD6E8_FEB8_6659_FD93u64;
    }
    let mut s = x ^ (ws.windows.len() as u64).rotate_left(7);
    for w in &ws.windows {
        let v = window_sig(w).rotate_left(13);
        x ^= v;
        s = s.wrapping_add(v);
    }
    x ^ s.rotate_left(21) ^ (s >> 11)
}

#[inline(always)]
fn window_sig(w: &WindowData) -> u64 {
    (w.id.idx.get() as u64)
        ^ w.info.frame.origin.x.to_bits().rotate_left(11)
        ^ w.info.frame.origin.y.to_bits().rotate_left(23)
        ^ w.info.frame.size.width.to_bits().rotate_left(37)
        ^ w.info.frame.size.height.to_bits().rotate_left(51)
}

#[inline(always)]
fn hash_str(s: &str) -> u64 {
    let mut x = 0xcbf2_9ce4_8422_2325u64;
    for &b in s.as_bytes() {
        x ^= b as u64;
        x = x.wrapping_mul(0x0000_0100_0000_01B3);
    }
    x
}

#[cfg(test)]
mod tests {
    use super::sig;
    use crate::model::server::WorkspaceData;

    fn workspace(layout_mode: &str) -> WorkspaceData {
        WorkspaceData {
            id: "VirtualWorkspaceId(1v1)".to_string(),
            index: 0,
            name: "main".to_string(),
            layout_mode: layout_mode.to_string(),
            is_active: true,
            window_count: 1,
            windows: Vec::new(),
        }
    }

    #[test]
    fn signature_changes_when_workspace_layout_mode_changes() {
        let base = vec![workspace("bsp")];
        let changed = vec![workspace("master_stack")];

        let before = sig(1, true, Some(0), &base, &[]);
        let after = sig(1, true, Some(0), &changed, &[]);

        assert_ne!(before, after);
    }
}
