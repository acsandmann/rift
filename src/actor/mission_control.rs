use std::time::Instant;

use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::{
    CGEventFlags, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
};
use objc2_foundation::MainThreadMarker;

use crate::actor::{self, reactor};
use crate::common::config::MissionControlSettings;
use crate::ui::mission_control::{OverviewAction, OverviewSession, PreviewRenderers};

#[derive(Debug)]
pub enum Event {
    PumpPreviews,
    ShowAll,
    Dismiss,
    RefreshCurrentWorkspace,
    DropFinished(super::app::WindowId, bool),
    ActivationTarget(super::app::WindowId, Instant, Option<CGRect>),
    Configure(MissionControlSettings, bool),
    Input(Input),
}

/// Semantic input; native CGEvents stay on the existing input thread.
#[derive(Debug)]
pub enum Input {
    Dismiss,
    Left,
    Right,
    Up,
    Down,
    Activate,
    ZoomStep(i8),
    Cycle(bool),
    Click(CGPoint),
    PointerDown(CGPoint),
    PointerDrag(CGPoint),
    PointerUp(CGPoint),
    Move(CGPoint),
    BeginPan(CGPoint),
    Pan(CGPoint),
    EndPan(CGPoint),
    Scroll { point: CGPoint, delta: CGPoint },
    Zoom { point: CGPoint, delta: CGPoint },
    Magnify { point: CGPoint, factor: f64 },
}

impl Input {
    pub(crate) fn from_keycode(keycode: u16, flags: CGEventFlags) -> Option<Self> {
        if !flags.intersects(CGEventFlags::MaskControl | CGEventFlags::MaskAlternate) {
            match keycode {
                24 => return Some(Self::ZoomStep(1)),
                27 => return Some(Self::ZoomStep(-1)),
                29 => return Some(Self::ZoomStep(0)),
                _ => {}
            }
        }
        if flags.intersects(
            CGEventFlags::MaskCommand | CGEventFlags::MaskControl | CGEventFlags::MaskAlternate,
        ) {
            return None;
        }
        if (123..=126).contains(&keycode) && flags.contains(CGEventFlags::MaskShift) {
            return None;
        }
        Some(match keycode {
            53 => Self::Dismiss,
            123 => Self::Left,
            124 => Self::Right,
            125 => Self::Down,
            126 => Self::Up,
            36 | 76 => Self::Activate,
            48 => Self::Cycle(!flags.contains(CGEventFlags::MaskShift)),
            _ => return None,
        })
    }
}

/// Collapse queued motion without crossing clicks, direction changes, or display targets.
fn merge_motion(event: &mut Event, next: Event) -> Result<(), Event> {
    match (event, next) {
        (Event::RefreshCurrentWorkspace, Event::RefreshCurrentWorkspace) => Ok(()),
        (Event::Input(Input::Move(point)), Event::Input(Input::Move(next)))
        | (Event::Input(Input::PointerDrag(point)), Event::Input(Input::PointerDrag(next)))
        | (Event::Input(Input::Pan(point)), Event::Input(Input::Pan(next))) => {
            *point = next;
            Ok(())
        }
        (
            Event::Input(Input::Scroll { point, delta }),
            Event::Input(Input::Scroll {
                point: next_point,
                delta: next_delta,
            }),
        ) if *point == next_point
            && (delta.y.abs() >= delta.x.abs()) == (next_delta.y.abs() >= next_delta.x.abs())
            && delta.x * next_delta.x >= 0.0
            && delta.y * next_delta.y >= 0.0 =>
        {
            delta.x += next_delta.x;
            delta.y += next_delta.y;
            Ok(())
        }
        (
            Event::Input(Input::Zoom { point, delta }),
            Event::Input(Input::Zoom {
                point: next_point,
                delta: next_delta,
            }),
        ) if *point == next_point
            && (delta.y.abs() >= delta.x.abs()) == (next_delta.y.abs() >= next_delta.x.abs())
            && delta.x * next_delta.x >= 0.0
            && delta.y * next_delta.y >= 0.0 =>
        {
            delta.x += next_delta.x;
            delta.y += next_delta.y;
            Ok(())
        }
        (
            Event::Input(Input::Magnify { point, factor }),
            Event::Input(Input::Magnify {
                point: next_point,
                factor: next_factor,
            }),
        ) if *point == next_point && (*factor - 1.) * (next_factor - 1.) >= 0. => {
            *factor *= next_factor;
            Ok(())
        }
        (_, next) => Err(next),
    }
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

pub fn channel_if_enabled(settings: &MissionControlSettings) -> Option<(Sender, Receiver)> {
    settings.enabled.then(actor::channel)
}

pub struct MissionControlActor {
    settings: MissionControlSettings,
    shared: bool,
    rx: Receiver,
    tx: Sender,
    cache: Option<PreviewRenderers>,
    reactor: reactor::ReactorHandle,
    session: Option<OverviewSession>,
    mtm: MainThreadMarker,
    input_tx: super::input::Sender,
}

impl MissionControlActor {
    pub fn new(
        settings: MissionControlSettings,
        shared: bool,
        rx: Receiver,
        tx: Sender,
        reactor: reactor::ReactorHandle,
        mtm: MainThreadMarker,
        input_tx: super::input::Sender,
    ) -> Self {
        if settings.enabled && settings.window_previews && CGPreflightScreenCaptureAccess() {
            let _ = cgs::PreviewCompositor::prepare();
        }
        Self {
            settings,
            shared,
            rx,
            tx,
            cache: None,
            reactor,
            session: None,
            mtm,
            input_tx,
        }
    }

    pub async fn run(mut self) {
        let mut pending_event = None;
        loop {
            tokio::select! {
                event = async {
                    if pending_event.is_some() { pending_event.take() } else { self.rx.recv().await }
                } => {
                    let Some((span, mut event)) = event else { break };
                    if matches!(event, Event::RefreshCurrentWorkspace | Event::Input(Input::Scroll { .. } | Input::Zoom { .. } | Input::Magnify { .. } | Input::Move(_) | Input::PointerDrag(_) | Input::Pan(_))) {
                        for _ in 0..32 {
                            let Ok((next_span, next)) = self.rx.try_recv() else { break };
                            if let Err(next) = merge_motion(&mut event, next) {
                                pending_event = Some((next_span, next));
                                break;
                            }
                        }
                    }
                    let _guard = span.enter();
                    self.handle(event);
                    if !self.settings.enabled { break; }
                }
            }
        }
        self.close();
    }

    fn close(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.retain_previews(&mut self.cache);
            drop(session);
        }
        self.input_tx
            .send(super::input::Request::SetMissionControlActive(false, Vec::new()));
    }

    fn handle(&mut self, event: Event) {
        if !matches!(event, Event::PumpPreviews | Event::Input(Input::Move(_))) {
            tracing::debug!(?event, "Overview event");
        }
        match event {
            Event::PumpPreviews => {
                if let Some(session) = self.session.as_mut() {
                    session.pump_previews();
                    if session.handoff_ready() {
                        self.close();
                    }
                }
            }
            Event::ActivationTarget(window, stamp, frame) => {
                if let Some(session) = &mut self.session {
                    session.activation_target(window, stamp, frame);
                }
            }
            Event::DropFinished(window, changed) => {
                tracing::debug!(?window, changed, "Overview drop completed");
                if let Some(session) = &mut self.session {
                    session.refresh(&self.reactor, Some(window));
                }
            }
            Event::ShowAll => {
                tracing::debug!(
                    open = self.session.is_some(),
                    enabled = self.settings.enabled,
                    previews = self.settings.window_previews,
                    permission = CGPreflightScreenCaptureAccess(),
                    "Overview toggle"
                );
                if self.session.is_some() {
                    if self.session.as_ref().is_some_and(OverviewSession::needs_handoff) {
                        self.session.as_mut().unwrap().toggle();
                    } else {
                        self.close();
                    }
                } else if self.settings.enabled {
                    if self.settings.window_previews && !CGPreflightScreenCaptureAccess() {
                        CGRequestScreenCaptureAccess();
                        return;
                    }
                    if self.settings.window_previews {
                        let _ = cgs::PreviewCompositor::prepare();
                    }
                    self.session = OverviewSession::new(
                        &self.reactor,
                        self.mtm,
                        &self.settings,
                        self.shared,
                        &mut self.cache,
                    );
                    if let Some(session) = &mut self.session {
                        session.start_previews(
                            self.settings.window_previews,
                            self.tx.clone(),
                            self.cache.as_mut(),
                        );
                    }
                    self.input_tx.send(super::input::Request::SetMissionControlActive(
                        self.session.is_some(),
                        self.session.as_ref().map_or_else(Vec::new, OverviewSession::input_bounds),
                    ));
                }
            }
            Event::Dismiss => self.handle(Event::Input(Input::Dismiss)),
            Event::RefreshCurrentWorkspace => {
                if let Some(session) = &mut self.session {
                    session.refresh(&self.reactor, None);
                    self.input_tx.send(super::input::Request::SetMissionControlActive(
                        !session.is_empty(),
                        session.input_bounds(),
                    ));
                }
                if self.session.as_ref().is_some_and(OverviewSession::is_empty) {
                    self.close();
                }
            }
            Event::Configure(settings, shared) => {
                self.close();
                if !settings.enabled || !settings.window_previews {
                    self.cache = None;
                }
                self.settings = settings;
                self.shared = shared;
            }

            Event::Input(input) => {
                let action = self.session.as_mut().and_then(|session| session.input(input));
                if let Some(intent) = self.session.as_mut().and_then(OverviewSession::take_drop) {
                    let window = intent.window;
                    let (reply, rx) = std::sync::mpsc::sync_channel(1);
                    self.reactor.send(reactor::Event::OverviewDrop { intent, reply });
                    let tx = self.tx.clone();
                    // Reactor application remains asynchronous; never wait on the input owner.
                    std::thread::spawn(move || {
                        let changed = rx.recv().unwrap_or(false);
                        tx.send(Event::DropFinished(window, changed));
                    });
                }
                if matches!(action, Some(OverviewAction::Dismiss)) {
                    self.close();
                } else if let Some(OverviewAction::Activate { display, workspace, selection }) =
                    action
                {
                    let transition =
                        self.session.as_ref().and_then(OverviewSession::activation_stamp);
                    self.reactor.send(reactor::Event::OverviewSelectWorkspace {
                        display,
                        workspace,
                        window: selection.window,
                        transition,
                    });
                    if transition.is_none() {
                        self.close();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_settings_do_not_create_a_channel() {
        assert!(channel_if_enabled(&MissionControlSettings::default()).is_none());
        assert!(!MissionControlSettings::default().window_previews);
        let settings: MissionControlSettings =
            toml::from_str("enabled = true\nwindow_previews = false").unwrap();
        assert!(channel_if_enabled(&settings).is_some());
        assert!(!settings.window_previews);
    }

    #[test]
    fn queued_scroll_preserves_distance_and_input_boundaries() {
        let mut refresh = Event::RefreshCurrentWorkspace;
        assert!(merge_motion(&mut refresh, Event::RefreshCurrentWorkspace).is_ok());
        assert!(merge_motion(&mut refresh, Event::Dismiss).is_err());
        let pivot = CGPoint::new(120., 90.);
        let mut pinch = Event::Input(Input::Magnify { point: pivot, factor: 1.1 });
        assert!(
            merge_motion(
                &mut pinch,
                Event::Input(Input::Magnify { point: pivot, factor: 1.2 })
            )
            .is_ok()
        );
        assert!(
            matches!(pinch, Event::Input(Input::Magnify { factor, .. }) if (factor - 1.32).abs() < 1e-12)
        );
        assert!(
            merge_motion(
                &mut pinch,
                Event::Input(Input::Magnify { point: pivot, factor: 0.9 })
            )
            .is_err()
        );

        let point = CGPoint::new(40.0, 50.0);
        let scroll = |y| {
            Event::Input(Input::Scroll {
                point,
                delta: CGPoint::new(0.0, y),
            })
        };
        let mut event = scroll(-2.5);
        assert!(merge_motion(&mut event, scroll(-1.25)).is_ok());
        assert!(matches!(&event, Event::Input(Input::Scroll { delta, .. }) if delta.y == -3.75));
        assert!(merge_motion(&mut event, scroll(1.0)).is_err());
        assert!(merge_motion(&mut event, Event::Input(Input::PointerUp(point))).is_err());
        assert!(merge_motion(&mut event, Event::Dismiss).is_err());
        assert!(
            merge_motion(
                &mut event,
                Event::Input(Input::Scroll {
                    point: CGPoint::new(500.0, 50.0),
                    delta: CGPoint::new(0.0, -1.0)
                })
            )
            .is_err()
        );
    }

    #[test]
    fn existing_keycodes_route_to_semantic_input() {
        assert!(matches!(
            Input::from_keycode(53, CGEventFlags::empty()),
            Some(Input::Dismiss)
        ));
        assert!(matches!(
            Input::from_keycode(36, CGEventFlags::empty()),
            Some(Input::Activate)
        ));
        assert!(matches!(
            Input::from_keycode(48, CGEventFlags::empty()),
            Some(Input::Cycle(true))
        ));
        assert!(matches!(
            Input::from_keycode(48, CGEventFlags::MaskShift),
            Some(Input::Cycle(false))
        ));
        for code in [123, 124, 125, 126] {
            assert!(Input::from_keycode(code, CGEventFlags::empty()).is_some());
            assert!(Input::from_keycode(code, CGEventFlags::MaskControl).is_none());
            assert!(Input::from_keycode(code, CGEventFlags::MaskShift).is_none());
        }
        assert!(Input::from_keycode(0, CGEventFlags::empty()).is_none());
    }
}
