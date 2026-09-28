use objc2_core_foundation::CGPoint;
use objc2_core_graphics::CGEventFlags;
use objc2_foundation::MainThreadMarker;

use crate::actor::{self, reactor};
use crate::common::config::MissionControlSettings;
use crate::ui::mission_control::OverviewSession;

#[derive(Debug)]
pub enum Event {
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
    Move(CGPoint),
}

impl Input {
    pub(crate) fn from_keycode(keycode: u16, flags: CGEventFlags) -> Option<Self> {
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

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

pub fn channel_if_enabled(settings: &MissionControlSettings) -> Option<(Sender, Receiver)> {
    settings.enabled.then(actor::channel)
}

pub struct MissionControlActor {
    settings: MissionControlSettings,
    rx: Receiver,
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
        reactor: reactor::ReactorHandle,
        mtm: MainThreadMarker,
        input_tx: super::input::Sender,
    ) -> Self {
        Self {
            settings,
            rx,
            reactor,
            session: None,
            generation: 0,
            mtm,
            input_tx,
        }
    }

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                event = self.rx.recv() => {
                    let Some((span, event)) = event else { break };
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
                        if let Some(session) = &mut self.session { session.preview_ready(result, current.as_ref()); }
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
                    );
                    self.input_tx.send(super::input::Request::SetMissionControlActive(
                        self.session.is_some(),
                    ));
                }
            }
            Event::Dismiss | Event::RefreshCurrentWorkspace => self.close(),
            Event::Configure(settings) => {
                self.close();
                self.settings = settings;
            }
            Event::Input(Input::Dismiss) => self.close(),
            Event::Input(input) => {
                let action = self.session.as_mut().and_then(|session| session.input(input));
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
    fn disabled_settings_do_not_create_a_channel() {
        assert!(channel_if_enabled(&MissionControlSettings::default()).is_none());
        assert!(!MissionControlSettings::default().window_previews);
        let settings: MissionControlSettings = toml::from_str("enabled = true").unwrap();
        assert!(channel_if_enabled(&settings).is_some());
        assert!(!settings.window_previews);
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
        }
        assert!(Input::from_keycode(0, CGEventFlags::empty()).is_none());
    }
}
