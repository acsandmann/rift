use objc2_core_foundation::CGPoint;
use objc2_core_graphics::CGEventFlags;
use objc2_foundation::MainThreadMarker;

use crate::actor::{self, reactor};
use crate::common::config::MissionControlSettings;
use crate::sys::dispatch::DispatchExt;
use crate::sys::timer::Timer;
use crate::ui::mission_control::{OverviewSession, RememberedPreviewCache};

#[derive(Debug)]
pub enum Event {
    StartPreviews(u64),
    ShowAll,
    ShowCurrent,
    Dismiss,
    RefreshCurrentWorkspace,
    Configure(MissionControlSettings),
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
    Cycle(bool),
    Click(CGPoint),
    PointerDown(CGPoint),
    PointerDrag(CGPoint),
    PointerUp(CGPoint),
    Move(CGPoint),
    Scroll { point: CGPoint, delta: CGPoint },
}

impl Input {
    pub(crate) fn from_keycode(keycode: u16, flags: CGEventFlags) -> Option<Self> {
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
        (Event::Input(Input::Move(point)), Event::Input(Input::Move(next)))
        | (Event::Input(Input::PointerDrag(point)), Event::Input(Input::PointerDrag(next))) => {
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
        (_, next) => Err(next),
    }
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

pub fn channel_if_enabled(settings: &MissionControlSettings) -> Option<(Sender, Receiver)> {
    settings.enabled.then(actor::channel)
}

/// Keep one run-loop timer across input events; dropping it invalidates the native timer.
fn update_edge_timer(timer: &mut Option<Timer>, active: bool) {
    if active {
        timer.get_or_insert_with(|| Timer::sleep(std::time::Duration::from_millis(150)));
    } else {
        *timer = None;
    }
}

pub struct MissionControlActor {
    settings: MissionControlSettings,
    rx: Receiver,
    tx: Sender,
    remembered: Option<RememberedPreviewCache>,
    reactor: reactor::ReactorHandle,
    session: Option<OverviewSession>,
    generation: u64,
    mtm: MainThreadMarker,
    input_tx: super::input::Sender,
}

impl MissionControlActor {
    pub fn new(
        settings: MissionControlSettings,
        rx: Receiver,
        tx: Sender,
        reactor: reactor::ReactorHandle,
        mtm: MainThreadMarker,
        input_tx: super::input::Sender,
    ) -> Self {
        Self {
            settings,
            rx,
            tx,
            remembered: None,
            reactor,
            session: None,
            generation: 0,
            mtm,
            input_tx,
        }
    }

    pub async fn run(mut self) {
        let mut edge_timer = None;
        let mut pending_event = None;
        loop {
            let edge_active = self.session.as_ref().is_some_and(OverviewSession::edge_active);
            update_edge_timer(&mut edge_timer, edge_active);
            tokio::select! {
                _ = async { if let Some(timer) = &mut edge_timer { timer.await } else { std::future::pending().await } } => {
                    edge_timer = None;
                    if let Some(session) = &mut self.session { session.edge_tick(self.remembered.as_ref()); }
                }
                event = async {
                    if pending_event.is_some() { pending_event.take() } else { self.rx.recv().await }
                } => {
                    let Some((span, mut event)) = event else { break };
                    if matches!(event, Event::Input(Input::Scroll { .. } | Input::Move(_) | Input::PointerDrag(_))) {
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
                result = async {
                    if let Some(previews) = self.session.as_mut().and_then(|s| s.previews.as_mut()) {
                        previews.rx.recv().await
                    } else { std::future::pending().await }
                } => {
                    if let Some((_, result)) = result {
                        let current = result.window_id().and_then(|id| self.reactor.query_window_info(id));
                        if let Some(session) = &mut self.session { session.preview_ready(result, current.as_ref(), &mut self.remembered); }
                    }
                }
            }
        }
        self.close();
    }

    fn close(&mut self) {
        self.session = None;
        self.input_tx.send(super::input::Request::SetMissionControlActive(false));
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::StartPreviews(generation) => {
                if let Some(session) = &mut self.session {
                    session.start_previews(
                        generation,
                        self.settings.window_previews,
                        self.remembered.as_ref(),
                    );
                }
            }
            Event::ShowAll | Event::ShowCurrent => {
                if self.session.is_some() {
                    self.close();
                } else if self.settings.enabled {
                    self.generation = self.generation.wrapping_add(1);
                    self.session = OverviewSession::new(
                        &self.reactor,
                        self.mtm,
                        &self.settings,
                        self.generation,
                        &mut self.remembered,
                    );
                    if self.session.is_some() && self.settings.window_previews {
                        dispatchr::queue::main().after_f_s(
                            dispatchr::time::Time::NOW,
                            (self.tx.clone(), self.generation),
                            |(tx, generation)| tx.send(Event::StartPreviews(generation)),
                        );
                    }
                    self.input_tx.send(super::input::Request::SetMissionControlActive(
                        self.session.is_some(),
                    ));
                }
            }
            Event::Dismiss => self.close(),
            Event::RefreshCurrentWorkspace => {
                if let Some(session) = &mut self.session {
                    session.refresh(&self.reactor, self.remembered.as_ref(), None);
                }
                if self.session.as_ref().is_some_and(OverviewSession::is_empty) {
                    self.close();
                }
            }
            Event::Configure(settings) => {
                self.close();
                if !settings.enabled || !settings.window_previews {
                    self.remembered = None;
                }
                self.settings = settings;
            }
            Event::Input(Input::Dismiss) => self.close(),
            Event::Input(input) => {
                let action = self
                    .session
                    .as_mut()
                    .and_then(|session| session.input(input, self.remembered.as_ref()));
                if let Some(intent) = self.session.as_mut().and_then(OverviewSession::take_drop) {
                    let window = intent.window;
                    let (reply, rx) = std::sync::mpsc::sync_channel(1);
                    self.reactor.send(reactor::Event::OverviewDrop { intent, reply });
                    let changed = rx.recv().unwrap_or(false);
                    if let Some(session) = &mut self.session {
                        session.refresh(
                            &self.reactor,
                            self.remembered.as_ref(),
                            changed.then_some(window),
                        );
                    }
                }
                if let Some((display, selection, sys_id)) = action {
                    self.close();
                    self.reactor.send(reactor::Event::Command(reactor::Command::Reactor(
                        reactor::ReactorCommand::FocusDisplay(
                            rift_protocol::DisplaySelector::Uuid(display),
                        ),
                    )));
                    self.reactor.send(reactor::Event::Command(reactor::Command::Layout(
                        crate::layout_engine::LayoutCommand::SwitchToWorkspace(selection.workspace),
                    )));
                    if let Some(window_id) = selection.window {
                        self.reactor.send(reactor::Event::Command(reactor::Command::Reactor(
                            reactor::ReactorCommand::FocusWindow {
                                window_id: window_id.into(),
                                window_server_id: sys_id.map(Into::into),
                            },
                        )));
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
    fn edge_timer_runs_without_tokio_and_input_does_not_restart_it() {
        // Ordinary test + Rift executor: no Tokio runtime is created or entered.
        crate::sys::executor::Executor::run(async {
            let mut timer = None;
            let mut input_events = 0;
            loop {
                update_edge_timer(&mut timer, true);
                tokio::select! {
                    _ = timer.as_mut().unwrap() => break,
                    _ = Timer::sleep(std::time::Duration::from_millis(10)) => {
                        input_events += 1;
                        assert!(input_events < 40, "input must not postpone the edge tick");
                    }
                }
            }
            assert!(input_events > 0);
            update_edge_timer(&mut timer, false);
            assert!(timer.is_none());
        });
    }

    #[test]
    fn inactive_edge_scrolling_has_no_timer() {
        let mut timer = None;
        update_edge_timer(&mut timer, false);
        assert!(timer.is_none());
        update_edge_timer(&mut timer, true);
        assert!(timer.is_some());
        update_edge_timer(&mut timer, false);
        assert!(timer.is_none());
    }

    #[test]
    fn disabled_settings_do_not_create_a_channel() {
        assert!(channel_if_enabled(&MissionControlSettings::default()).is_none());
        assert!(!MissionControlSettings::default().window_previews);
        let settings: MissionControlSettings = toml::from_str("enabled = true").unwrap();
        assert!(channel_if_enabled(&settings).is_some());
        assert!(!settings.window_previews);
    }

    #[test]
    fn queued_scroll_preserves_distance_and_input_boundaries() {
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
    fn empty_workspace_setting_is_opt_in() {
        let settings: MissionControlSettings = toml::from_str("enabled = true").unwrap();
        assert!(!settings.show_empty_workspaces);
        let settings: MissionControlSettings =
            toml::from_str("show_empty_workspaces = true").unwrap();
        assert!(settings.show_empty_workspaces);
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
