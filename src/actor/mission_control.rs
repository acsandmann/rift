use std::rc::Rc;
use std::sync::Arc;

use r#continue::continuation;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::MainThreadMarker;
use tracing::instrument;

use crate::actor::{self, reactor};
use crate::common::config::Config;
use crate::model::server::{WindowData, WorkspaceQueryResponse};
use crate::sys::dispatch::block_on;
use crate::ui::mission_control::{MissionControlAction, MissionControlMode, MissionControlOverlay};

#[derive(Debug)]
pub enum Event {
    ShowAll,
    ShowCurrent,
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

pub struct MissionControlActor {
    _config: Arc<Config>,
    rx: Receiver,
    reactor_tx: reactor::Sender,
    overlay: Option<MissionControlOverlay>,
    mtm: MainThreadMarker,
}

impl MissionControlActor {
    pub fn new(
        config: Arc<Config>,
        rx: Receiver,
        reactor_tx: reactor::Sender,
        mtm: MainThreadMarker,
    ) -> Self {
        Self {
            _config: config,
            rx,
            reactor_tx,
            overlay: None,
            mtm,
        }
    }

    pub async fn run(mut self) {
        while let Some((span, event)) = self.rx.recv().await {
            let _guard = span.enter();
            self.handle_event(event);
        }
    }

    fn ensure_overlay(&mut self) -> &MissionControlOverlay {
        if self.overlay.is_none() {
            let frame = if let Some(screen) = NSScreen::mainScreen(self.mtm) {
                screen.frame()
            } else {
                CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1280.0, 800.0))
            };
            let overlay = MissionControlOverlay::new(self.mtm, frame);
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
            MissionControlAction::FocusWindow(window_id) => {
                let _ = self.reactor_tx.try_send(reactor::Event::Command(
                    reactor::Command::Reactor(reactor::ReactorCommand::FocusWindow(window_id)),
                ));
                self.dispose_overlay();
            }
        }
    }

    #[instrument(skip(self))]
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::ShowAll => self.show_all_workspaces(),
            Event::ShowCurrent => self.show_current_workspace(),
        }
    }

    fn show_all_workspaces(&mut self) {
        let (tx, fut) = continuation::<WorkspaceQueryResponse>();
        let _ = self.reactor_tx.try_send(reactor::Event::QueryWorkspaces(tx));
        match block_on(fut, std::time::Duration::from_secs_f32(0.75)) {
            Ok(resp) => {
                let overlay = self.ensure_overlay();
                overlay.update(MissionControlMode::AllWorkspaces(resp.workspaces));
            }
            Err(_) => tracing::warn!("workspace query timed out"),
        }
    }

    fn show_current_workspace(&mut self) {
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
}
