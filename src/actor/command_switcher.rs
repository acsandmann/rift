use std::rc::Rc;
use std::time::Duration;

use r#continue::continuation;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::MainThreadMarker;
use tracing::{instrument, warn};

use crate::actor::{self, reactor};
use crate::common::config::{CommandSwitcherDisplayMode, CommandSwitcherSettings, Config};
use crate::model::server::{WindowData, WorkspaceData, WorkspaceQueryResponse};
use crate::sys::dispatch::block_on;
use crate::sys::screen;
use crate::ui::command_switcher::{
    CommandSwitcherAction, CommandSwitcherMode, CommandSwitcherOverlay,
};

#[derive(Debug)]
pub enum Event {
    Show(CommandSwitcherDisplayMode),
    Dismiss,
    UpdateConfig(Config),
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

pub struct CommandSwitcherActor {
    config: Config,
    settings: CommandSwitcherSettings,
    rx: Receiver,
    reactor_tx: reactor::Sender,
    overlay: Option<CommandSwitcherOverlay>,
    mtm: MainThreadMarker,
    active: bool,
    last_mode: Option<CommandSwitcherDisplayMode>,
}

impl CommandSwitcherActor {
    pub fn new(
        config: Config,
        rx: Receiver,
        reactor_tx: reactor::Sender,
        mtm: MainThreadMarker,
    ) -> Self {
        let settings = config.settings.ui.command_switcher.clone();
        Self {
            config,
            settings,
            rx,
            reactor_tx,
            overlay: None,
            mtm,
            active: false,
            last_mode: None,
        }
    }

    pub async fn run(mut self) {
        while let Some((span, event)) = self.rx.recv().await {
            let _guard = span.enter();
            self.handle_event(event);
        }
    }

    #[instrument(skip(self))]
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::UpdateConfig(config) => self.apply_config(config),
            Event::Dismiss => self.hide_overlay(),
            Event::Show(mode) => {
                if self.settings.enabled {
                    let _ = self.show_contents(mode);
                }
            }
        }
    }

    fn apply_config(&mut self, config: Config) {
        self.config = config.clone();
        self.settings = config.settings.ui.command_switcher.clone();
        if !self.settings.enabled {
            self.hide_overlay();
            self.overlay = None;
        } else if self.active {
            if let Some(mode) = self.last_mode {
                let _ = self.show_contents(mode);
            }
        }
    }

    fn show_contents(&mut self, mode: CommandSwitcherDisplayMode) -> bool {
        let Some(payload) = self.fetch_mode_data(mode) else {
            self.hide_overlay();
            return false;
        };
        let Some(overlay) = self.ensure_overlay() else {
            return false;
        };
        overlay.update(payload);
        self.active = true;
        self.last_mode = Some(mode);
        true
    }

    fn ensure_overlay(&mut self) -> Option<&CommandSwitcherOverlay> {
        if self.overlay.is_none() {
            let (frame, scale) = if let Some(screen) = NSScreen::mainScreen(self.mtm) {
                (screen.frame(), screen.backingScaleFactor())
            } else {
                (
                    CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1280.0, 800.0)),
                    1.0,
                )
            };
            let overlay = CommandSwitcherOverlay::new(self.config.clone(), self.mtm, frame, scale);
            let self_ptr: *mut CommandSwitcherActor = self;
            overlay.set_action_handler(Rc::new(move |action| unsafe {
                let this: &mut CommandSwitcherActor = &mut *self_ptr;
                this.handle_overlay_action(action);
            }));
            self.overlay = Some(overlay);
        }
        self.overlay.as_ref()
    }

    fn fetch_mode_data(&mut self, mode: CommandSwitcherDisplayMode) -> Option<CommandSwitcherMode> {
        match mode {
            CommandSwitcherDisplayMode::CurrentWorkspace => {
                let active_space = screen::get_active_space_number();
                let (tx, fut) = continuation::<Vec<WindowData>>();
                let _ = self.reactor_tx.try_send(reactor::Event::QueryWindows {
                    space_id: active_space,
                    response: tx,
                });
                match block_on(fut, Duration::from_millis(750)) {
                    Ok(windows) => Some(CommandSwitcherMode::CurrentWorkspace(windows)),
                    Err(_) => {
                        warn!("command switcher: windows query timed out");
                        None
                    }
                }
            }
            CommandSwitcherDisplayMode::AllWindows => {
                let (tx, fut) = continuation::<WorkspaceQueryResponse>();
                let _ = self.reactor_tx.try_send(reactor::Event::QueryWorkspaces(tx));
                match block_on(fut, Duration::from_millis(750)) {
                    Ok(resp) => {
                        Some(CommandSwitcherMode::AllWindows(flatten_windows(resp.workspaces)))
                    }
                    Err(_) => {
                        warn!("command switcher: workspace query timed out");
                        None
                    }
                }
            }
            CommandSwitcherDisplayMode::Workspaces => {
                let (tx, fut) = continuation::<WorkspaceQueryResponse>();
                let _ = self.reactor_tx.try_send(reactor::Event::QueryWorkspaces(tx));
                match block_on(fut, Duration::from_millis(750)) {
                    Ok(resp) => Some(CommandSwitcherMode::Workspaces(filter_workspaces(
                        resp.workspaces,
                    ))),
                    Err(_) => {
                        warn!("command switcher: workspace query timed out");
                        None
                    }
                }
            }
        }
    }

    fn handle_overlay_action(&mut self, action: CommandSwitcherAction) {
        match action {
            CommandSwitcherAction::Dismiss => self.hide_overlay(),
            CommandSwitcherAction::SwitchToWorkspace(index) => {
                let _ =
                    self.reactor_tx.try_send(reactor::Event::Command(reactor::Command::Layout(
                        crate::layout_engine::LayoutCommand::SwitchToWorkspace(index),
                    )));
                self.hide_overlay();
            }
            CommandSwitcherAction::FocusWindow { window_id, window_server_id } => {
                let _ =
                    self.reactor_tx.try_send(reactor::Event::Command(reactor::Command::Reactor(
                        reactor::ReactorCommand::FocusWindow { window_id, window_server_id },
                    )));
                self.hide_overlay();
            }
        }
    }

    fn hide_overlay(&mut self) {
        if let Some(overlay) = self.overlay.as_ref() {
            overlay.hide();
        }
        self.active = false;
    }
}

fn flatten_windows(workspaces: Vec<WorkspaceData>) -> Vec<WindowData> {
    let mut active = Vec::new();
    let mut others = Vec::new();
    for mut workspace in workspaces {
        if workspace.is_active {
            active.append(&mut workspace.windows);
        } else {
            others.append(&mut workspace.windows);
        }
    }
    active.extend(others);
    active
}

fn filter_workspaces(workspaces: Vec<WorkspaceData>) -> Vec<WorkspaceData> {
    workspaces
        .into_iter()
        .filter(|ws| ws.is_active || ws.is_last_active || !ws.windows.is_empty())
        .collect()
}
