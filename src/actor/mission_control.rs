use std::rc::Rc;

use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::MainThreadMarker;
use tracing::instrument;

use crate::actor::{self, reactor};
use crate::common::config::Config;
use crate::sys::event::current_cursor_location;
use crate::sys::geometry::CGRectExt;
use crate::sys::screen::{NSScreenExt, ScreenCache, get_active_space_number};
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
    reactor: reactor::ReactorHandle,
    overlay: Option<MissionControlOverlay>,
    mtm: MainThreadMarker,
    mission_control_active: bool,
    current_view_mode: Option<MissionControlViewMode>,
}

impl MissionControlActor {
    pub fn new(
        config: Config,
        rx: Receiver,
        reactor: reactor::ReactorHandle,
        mtm: MainThreadMarker,
    ) -> Self {
        Self {
            config,
            rx,
            reactor,
            overlay: None,
            mtm,
            mission_control_active: false,
            current_view_mode: None,
        }
    }

    pub async fn run(mut self) {
        if self.config.settings.ui.mission_control.enabled {
            let _ = self.ensure_overlay();
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
            let (frame, scale) = self.initial_overlay_geometry();
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

    fn initial_overlay_geometry(&self) -> (CGRect, f64) {
        let fallback = (
            CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1280.0, 800.0)),
            1.0,
        );
        let mut cache = ScreenCache::new(self.mtm);
        let Some((screens, _)) = cache.refresh() else {
            return fallback;
        };

        let selected = current_cursor_location()
            .ok()
            .and_then(|cursor| screens.iter().find(|screen| screen.frame.contains(cursor)))
            .or_else(|| {
                let active_space = get_active_space_number()?;
                screens.iter().find(|screen| screen.space == Some(active_space))
            })
            .or_else(|| screens.first());

        let Some(selected) = selected else {
            return fallback;
        };

        let scale = NSScreen::screens(self.mtm)
            .iter()
            .find_map(|ns| {
                let id = ns.get_number().ok()?;
                if id == selected.id {
                    Some(ns.backingScaleFactor())
                } else {
                    None
                }
            })
            .unwrap_or(1.0);

        (selected.frame, scale)
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
                let _ = self.reactor.try_send(reactor::Event::Command(reactor::Command::Layout(
                    crate::layout_engine::LayoutCommand::SwitchToWorkspace(index),
                )));
                self.dispose_overlay();
            }
            MissionControlAction::FocusWindow { window_id, window_server_id } => {
                let _ = self.reactor.try_send(reactor::Event::Command(reactor::Command::Reactor(
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

        let resp = self.reactor.query_workspaces(None);
        let overlay = self.ensure_overlay();
        overlay.update(MissionControlMode::AllWorkspaces(resp));
    }

    fn show_current_workspace(&mut self) {
        self.mission_control_active = true;
        self.current_view_mode = Some(MissionControlViewMode::CurrentWorkspace);
        {
            let overlay = self.ensure_overlay();
            overlay.update(MissionControlMode::CurrentWorkspace(Vec::new()));
        }

        let windows = self.reactor.query_windows(None);

        let overlay = self.ensure_overlay();
        overlay.update(MissionControlMode::CurrentWorkspace(windows));
    }

    fn refresh_all_workspaces_highlight(&mut self) {
        let active_workspace = self.reactor.query_active_workspace(None);
        if let Some(overlay) = self.overlay.as_ref() {
            overlay.refresh_active_workspace(active_workspace);
        }
    }
}
