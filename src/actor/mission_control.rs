use std::rc::Rc;

use r#continue::continuation;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::MainThreadMarker;
use tracing::instrument;

use crate::actor::{self, reactor};
use crate::common::config::Config;
use crate::model::server::{WindowData, WorkspaceData};
use crate::model::virtual_workspace::VirtualWorkspaceId;
use crate::sys::dispatch::block_on;
use crate::ui::mission_control::{MissionControlAction, MissionControlMode, MissionControlOverlay};

#[derive(Debug)]
pub enum Event {
    ShowAll,
    ShowCurrent,
    Dismiss,
    RefreshCurrentWorkspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissionControlViewMode {
    AllWorkspaces,
    CurrentWorkspace,
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

pub struct MissionControlActor {
    config: Config,
    rx: Receiver,
    reactor_tx: reactor::Sender,
    overlay: Option<MissionControlOverlay>,
    mtm: MainThreadMarker,
    mission_control_active: bool,
    current_view_mode: Option<MissionControlViewMode>,
}

impl MissionControlActor {
    pub fn new(
        config: Config,
        rx: Receiver,
        reactor_tx: reactor::Sender,
        mtm: MainThreadMarker,
    ) -> Self {
        Self {
            config,
            rx,
            reactor_tx,
            overlay: None,
            mtm,
            mission_control_active: false,
            current_view_mode: None,
        }
    }

    pub async fn run(mut self) {
        if self.config.settings.ui.mission_control.enabled {
            let _ = self.ensure_overlay();

            while let Some((span, event)) = self.rx.recv().await {
                let _guard = span.enter();
                self.handle_event(event);
            }
        }
    }

    fn ensure_overlay(&mut self) -> &MissionControlOverlay {
        if self.overlay.is_none() {
            let (frame, scale) = if let Some(screen) = NSScreen::mainScreen(self.mtm) {
                let frame = screen.frame();
                let scale = screen.backingScaleFactor();
                (frame, scale)
            } else {
                (
                    CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1280.0, 800.0)),
                    1.0,
                )
            };
            let overlay = MissionControlOverlay::new(self.config.clone(), self.mtm, frame, scale);
            let self_ptr: *mut MissionControlActor = self as *mut _;
            overlay.set_action_handler(Rc::new(move |action| unsafe {
                let this: &mut MissionControlActor = &mut *self_ptr;
                this.handle_overlay_action(action);
            }));
            self.overlay = Some(overlay);
        }
        self.overlay.as_ref().unwrap()
    }

    fn dispose_overlay(&mut self) {
        if let Some(overlay) = self.overlay.take() {
            overlay.hide();
        }
        self.mission_control_active = false;
        self.current_view_mode = None;
    }

    fn handle_overlay_action(&mut self, action: MissionControlAction) {
        match action {
            MissionControlAction::Dismiss => {
                self.dispose_overlay();
            }
            MissionControlAction::SwitchToWorkspace(index) => {
                let _ =
                    self.reactor_tx.try_send(reactor::Event::Command(reactor::Command::Layout(
                        crate::layout_engine::LayoutCommand::SwitchToWorkspace(index),
                    )));
                self.dispose_overlay();
            }
            MissionControlAction::FocusWindow { window_id, window_server_id } => {
                let _ =
                    self.reactor_tx.try_send(reactor::Event::Command(reactor::Command::Reactor(
                        reactor::ReactorCommand::FocusWindow { window_id, window_server_id },
                    )));
                self.dispose_overlay();
            }
        }
    }

    #[instrument(skip(self))]
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::ShowAll => {
                if self.mission_control_active {
                    self.dispose_overlay();
                } else {
                    self.show_all_workspaces();
                }
            }
            Event::ShowCurrent => {
                if self.mission_control_active {
                    self.dispose_overlay();
                } else {
                    self.show_current_workspace();
                }
            }
            Event::Dismiss => self.dispose_overlay(),
            Event::RefreshCurrentWorkspace => {
                if self.mission_control_active {
                    match self.current_view_mode {
                        Some(MissionControlViewMode::CurrentWorkspace) => {
                            self.show_current_workspace();
                        }
                        Some(MissionControlViewMode::AllWorkspaces) => {
                            self.refresh_all_workspaces_highlight();
                        }
                        None => {}
                    }
                }
            }
        }
    }

    fn show_all_workspaces(&mut self) {
        self.mission_control_active = true;
        self.current_view_mode = Some(MissionControlViewMode::AllWorkspaces);
        {
            let overlay = self.ensure_overlay();
            overlay.update(MissionControlMode::AllWorkspaces(Vec::new()));
        }

        let (tx, fut) = continuation::<Vec<WorkspaceData>>();
        let _ = self
            .reactor_tx
            .try_send(reactor::Event::QueryWorkspaces { space_id: None, response: tx });
        match block_on(fut, std::time::Duration::from_secs_f32(0.75)) {
            Ok(resp) => {
                let overlay = self.ensure_overlay();
                overlay.update(MissionControlMode::AllWorkspaces(resp));
            }
            Err(_) => tracing::warn!("workspace query timed out"),
        }
    }

    fn show_current_workspace(&mut self) {
        self.mission_control_active = true;
        self.current_view_mode = Some(MissionControlViewMode::CurrentWorkspace);
        {
            let overlay = self.ensure_overlay();
            overlay.update(MissionControlMode::CurrentWorkspace(Vec::new()));
        }

        let active_space = crate::sys::screen::get_active_space_number();
        let (tx, fut) = continuation::<Vec<WindowData>>();
        let _ = self.reactor_tx.try_send(reactor::Event::QueryWindows {
            space_id: active_space,
            response: tx,
        });
        let windows = match block_on(fut, std::time::Duration::from_secs_f32(0.75)) {
            Ok(windows) => windows,
            Err(_) => {
                tracing::warn!("windows query timed out");
                return;
            }
        };

        let overlay = self.ensure_overlay();
        overlay.update(MissionControlMode::CurrentWorkspace(windows));
    }

    fn refresh_all_workspaces_highlight(&mut self) {
        let (tx, fut) = continuation::<Option<VirtualWorkspaceId>>();
        let _ = self
            .reactor_tx
            .try_send(reactor::Event::QueryActiveWorkspace { space_id: None, response: tx });
        match block_on(fut, std::time::Duration::from_secs_f32(0.75)) {
            Ok(active_workspace) => {
                if let Some(overlay) = self.overlay.as_ref() {
                    overlay.refresh_active_workspace(active_workspace);
                }
            }
            Err(_) => {
                tracing::warn!("active workspace query timed out");
            }
        }
    }
}
