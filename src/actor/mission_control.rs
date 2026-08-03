use std::rc::Rc;

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::MainThreadMarker;
use tracing::instrument;

use crate::actor::{self, reactor};
use crate::common::config::Config;
use crate::model::server::RuntimeWorkspaceData;
use crate::ui::mission_control::{MissionControlAction, MissionControlMode, MissionControlOverlay};

#[derive(Debug)]
pub enum Event {
    ShowAll,
    ShowCurrent,
    Dismiss,
    RefreshCurrentWorkspace,
    PreviewReady,
    Action(MissionControlAction),
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
    reactor: reactor::ReactorHandle,
    tx: Sender,
    overlay: Option<MissionControlOverlay>,
    mtm: MainThreadMarker,
    mission_control_active: bool,
    current_view_mode: Option<MissionControlViewMode>,
    workspaces: Vec<RuntimeWorkspaceData>,
}

impl MissionControlActor {
    pub fn new(
        config: Config,
        rx: Receiver,
        tx: Sender,
        reactor: reactor::ReactorHandle,
        mtm: MainThreadMarker,
    ) -> Self {
        Self {
            config,
            rx,
            reactor,
            tx,
            overlay: None,
            mtm,
            mission_control_active: false,
            current_view_mode: None,
            workspaces: Vec::new(),
        }
    }

    pub async fn run(mut self) {
        if self.config.settings.ui.mission_control.enabled {
            self.refresh_snapshot();
        }
        while let Some((span, event)) = self.rx.recv().await {
            let _guard = span.enter();
            if self.config.settings.ui.mission_control.enabled {
                self.handle_event(event);
            }
        }
    }

    fn ensure_overlay(&mut self) -> &MissionControlOverlay {
        if self.overlay.is_none() {
            let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1280.0, 800.0));
            let preview_tx = self.tx.clone();
            let overlay = MissionControlOverlay::new(
                self.config.clone(),
                self.mtm,
                frame,
                1.0,
                std::sync::Arc::new(move || preview_tx.send(Event::PreviewReady)),
            );
            let action_tx = self.tx.clone();
            overlay.set_action_handler(Rc::new(move |action| {
                action_tx.send(Event::Action(action));
            }));
            self.overlay = Some(overlay);
        }
        self.overlay.as_ref().unwrap()
    }

    fn dispose_overlay(&mut self) {
        if let Some(overlay) = self.overlay.as_ref() {
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
                let _ = self.reactor.try_send(reactor::Event::Command(reactor::Command::Layout(
                    crate::layout_engine::LayoutCommand::SwitchToWorkspace(index),
                )));
                self.dispose_overlay();
            }
            MissionControlAction::FocusWindow { window_id, window_server_id } => {
                let _ = self.reactor.try_send(reactor::Event::Command(reactor::Command::Reactor(
                    reactor::ReactorCommand::FocusWindow {
                        window_id: window_id.into(),
                        window_server_id: window_server_id.map(Into::into),
                    },
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
            Event::PreviewReady => {
                if let Some(overlay) = self.overlay.as_ref() {
                    overlay.refresh_previews();
                }
            }
            Event::Action(action) => self.handle_overlay_action(action),
            Event::RefreshCurrentWorkspace => {
                self.refresh_snapshot();
                if self.mission_control_active {
                    match self.current_view_mode {
                        Some(MissionControlViewMode::CurrentWorkspace) => {
                            self.show_current_workspace();
                        }
                        Some(MissionControlViewMode::AllWorkspaces) => {
                            self.show_all_workspaces();
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
        let resp = self.workspaces.clone();
        let overlay = self.ensure_overlay();
        overlay.update(MissionControlMode::AllWorkspaces(resp));
    }

    fn show_current_workspace(&mut self) {
        self.mission_control_active = true;
        self.current_view_mode = Some(MissionControlViewMode::CurrentWorkspace);
        let windows = self
            .workspaces
            .iter()
            .find(|workspace| workspace.is_active)
            .map(|workspace| workspace.windows.clone())
            .unwrap_or_default();
        let overlay = self.ensure_overlay();
        overlay.update(MissionControlMode::CurrentWorkspace(windows));
    }

    fn refresh_snapshot(&mut self) { self.workspaces = self.reactor.query_workspaces(None); }
}
