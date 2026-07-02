use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use dispatchr::queue;
use dispatchr::time::Time;
use nix::libc::pid_t;
use objc2_app_kit::NSWorkspace;
use tracing::{info, instrument};

#[cfg(test)]
use crate::actor;
use crate::actor::reactor;
use crate::actor::reactor::Event;
use crate::sys::app::NSRunningApplicationExt;
use crate::sys::axuielement::AXUIElement;
use crate::sys::dispatch::DispatchExt;
use crate::sys::observer::Observer;
use crate::sys::window_server;

const K_AX_EXPOSE_SHOW_ALL_WINDOWS: &str = "AXExposeShowAllWindows";
const K_AX_EXPOSE_SHOW_FRONT_WINDOWS: &str = "AXExposeShowFrontWindows";
const K_AX_EXPOSE_SHOW_DESKTOP: &str = "AXExposeShowDesktop";
const K_AX_EXPOSE_EXIT: &str = "AXExposeExit";
const MISSION_CONTROL_EXIT_POLL_DELAY_NS: i64 = 100_000_000;

/// Native Mission Control has two distinct signal families:
///
/// 1. WindowServer event `1204` marks Mission Control entry. This is the same
///    event yabai treats as the canonical start signal.
/// 2. Dock AX expose notifications describe the active expose mode and provide
///    an exit hint, but `AXExposeExit` can arrive before the Dock overlay is
///    actually gone.
///
/// Rift therefore enters Mission Control on the first enter signal from either
/// source, then confirms exit by polling the Dock-owned layer-18 overlay via
/// `window_server::mission_control_dock_overlay_visible()`.
#[derive(Copy, Clone)]
enum MissionControlNotification {
    Enter = 1,
    Exit = 2,
}

#[derive(Copy, Clone, Debug)]
enum ExitProbeAction {
    None,
    Reschedule(i64),
    EmitExit,
}

#[derive(Debug)]
pub enum Request {
    WindowServerEnter,
    Stop,
}

pub type Sender = crate::actor::Sender<Request>;
pub type Receiver = crate::actor::Receiver<Request>;

struct MissionControlState {
    active: AtomicBool,
    stopped: AtomicBool,
    epoch: AtomicU64,
}

pub struct NativeMissionControl {
    rx: Receiver,
    events_tx: reactor::Sender,
    observer: Option<Observer>,
    app_elem: Option<AXUIElement>,
    state: Arc<MissionControlState>,
}

impl NativeMissionControl {
    pub fn new(events_tx: reactor::Sender, rx: Receiver) -> Self {
        Self {
            rx,
            events_tx,
            observer: None,
            app_elem: None,
            state: Arc::new(MissionControlState {
                active: AtomicBool::new(false),
                stopped: AtomicBool::new(false),
                epoch: AtomicU64::new(0),
            }),
        }
    }

    #[instrument(skip(self))]
    pub async fn run(mut self) {
        info!("Starting native mission-control monitor (must run on main thread)");
        self.observe();

        while let Some((_span, req)) = self.rx.recv().await {
            match req {
                Request::WindowServerEnter => {
                    Self::handle_enter_notification(&self.state, &self.events_tx);
                }
                Request::Stop => break,
            }
        }

        self.unobserve();
    }

    pub fn observe(&mut self) {
        if self.observer.is_some() {
            return;
        }

        let pid = find_dock_pid();
        if pid == 0 {
            // Dock not found
            return;
        }

        let builder = match Observer::new(pid) {
            Ok(b) => b,
            Err(_) => return,
        };

        let tx_clone = self.events_tx.clone();
        let state_clone = self.state.clone();
        let observer = builder.install(move |_elem: AXUIElement, data: usize| match data {
            value if value == MissionControlNotification::Enter as usize => {
                Self::handle_enter_notification(&state_clone, &tx_clone);
            }
            value if value == MissionControlNotification::Exit as usize => {
                Self::handle_exit_notification(&state_clone, &tx_clone);
            }
            _ => (),
        });

        let elem = AXUIElement::application(pid);

        let enter = MissionControlNotification::Enter as usize;
        let exit = MissionControlNotification::Exit as usize;
        let _ = observer.add_notification_with_data(&elem, K_AX_EXPOSE_SHOW_ALL_WINDOWS, enter);
        let _ = observer.add_notification_with_data(&elem, K_AX_EXPOSE_SHOW_FRONT_WINDOWS, enter);
        let _ = observer.add_notification_with_data(&elem, K_AX_EXPOSE_SHOW_DESKTOP, enter);
        let _ = observer.add_notification_with_data(&elem, K_AX_EXPOSE_EXIT, exit);

        self.observer = Some(observer);
        self.app_elem = Some(elem);
    }

    pub fn unobserve(&mut self) {
        if self.observer.is_none() {
            return;
        }

        if let (Some(observer), Some(elem)) = (self.observer.as_ref(), self.app_elem.as_ref()) {
            let _ = observer.remove_notification(elem, K_AX_EXPOSE_SHOW_ALL_WINDOWS);
            let _ = observer.remove_notification(elem, K_AX_EXPOSE_SHOW_FRONT_WINDOWS);
            let _ = observer.remove_notification(elem, K_AX_EXPOSE_SHOW_DESKTOP);
            let _ = observer.remove_notification(elem, K_AX_EXPOSE_EXIT);
        }

        self.observer = None;
        self.app_elem = None;
        self.state.stopped.store(true, Ordering::SeqCst);
        self.state.active.store(false, Ordering::SeqCst);
        self.state.epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub fn is_active(&self) -> bool { self.state.active.load(Ordering::SeqCst) }

    fn handle_enter_notification(state: &Arc<MissionControlState>, events_tx: &reactor::Sender) {
        state.stopped.store(false, Ordering::SeqCst);
        if state
            .active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let epoch = state.epoch.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        let _ = events_tx.try_send(Event::MissionControlNativeEntered);
        Self::schedule_exit_probe(
            state.clone(),
            events_tx.clone(),
            epoch,
            MISSION_CONTROL_EXIT_POLL_DELAY_NS,
        );
    }

    fn handle_exit_notification(state: &Arc<MissionControlState>, events_tx: &reactor::Sender) {
        if !state.active.load(Ordering::SeqCst) {
            return;
        }
        let epoch = state.epoch.load(Ordering::SeqCst);
        Self::schedule_exit_probe(state.clone(), events_tx.clone(), epoch, 0);
    }

    fn schedule_exit_probe(
        state: Arc<MissionControlState>,
        events_tx: reactor::Sender,
        epoch: u64,
        delay_ns: i64,
    ) {
        queue::main().after_f_s(
            Time::new_after(Time::NOW, delay_ns),
            (state, events_tx, epoch),
            |(state, events_tx, epoch)| Self::run_exit_probe(state, events_tx, epoch),
        );
    }

    fn run_exit_probe(state: Arc<MissionControlState>, events_tx: reactor::Sender, epoch: u64) {
        let overlay_visible = window_server::mission_control_dock_overlay_visible();
        match Self::handle_exit_probe_result(&state, &events_tx, epoch, overlay_visible) {
            ExitProbeAction::None => {}
            ExitProbeAction::Reschedule(delay_ns) => {
                Self::schedule_exit_probe(state, events_tx, epoch, delay_ns);
            }
            ExitProbeAction::EmitExit => {
                let _ = events_tx.try_send(Event::MissionControlNativeExited);
            }
        }
    }

    fn handle_exit_probe_result(
        state: &Arc<MissionControlState>,
        _events_tx: &reactor::Sender,
        epoch: u64,
        overlay_visible: bool,
    ) -> ExitProbeAction {
        if state.stopped.load(Ordering::SeqCst)
            || !state.active.load(Ordering::SeqCst)
            || state.epoch.load(Ordering::SeqCst) != epoch
        {
            return ExitProbeAction::None;
        }

        if overlay_visible {
            return ExitProbeAction::Reschedule(MISSION_CONTROL_EXIT_POLL_DELAY_NS);
        }

        if state.active.swap(false, Ordering::SeqCst) {
            ExitProbeAction::EmitExit
        } else {
            ExitProbeAction::None
        }
    }
}

fn find_dock_pid() -> pid_t {
    let workspace = NSWorkspace::sharedWorkspace();
    for app in workspace.runningApplications().into_iter() {
        if let Some(bid) = app.bundle_id() {
            if bid.to_string() == "com.apple.dock" {
                return app.processIdentifier();
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recv_event(rx: &mut actor::Receiver<reactor::Event>) -> reactor::Event {
        rx.try_recv().expect("expected reactor event").1
    }

    fn assert_no_event(rx: &mut actor::Receiver<reactor::Event>) {
        assert!(rx.try_recv().is_err(), "expected no reactor event");
    }

    #[test]
    fn enter_emits_enter_and_exit_probe_only_exits_when_overlay_disappears() {
        let (_req_tx, req_rx) = actor::channel();
        let (events_tx, mut events_rx) = actor::channel();
        let observer = NativeMissionControl::new(events_tx.clone(), req_rx);

        NativeMissionControl::handle_enter_notification(&observer.state, &events_tx);
        assert!(matches!(
            recv_event(&mut events_rx),
            reactor::Event::MissionControlNativeEntered
        ));
        assert!(observer.is_active());

        let epoch = observer.state.epoch.load(Ordering::SeqCst);
        let action = NativeMissionControl::handle_exit_probe_result(
            &observer.state,
            &events_tx,
            epoch,
            true,
        );
        assert!(matches!(action, ExitProbeAction::Reschedule(_)));
        assert!(observer.is_active());
        assert_no_event(&mut events_rx);

        let action = NativeMissionControl::handle_exit_probe_result(
            &observer.state,
            &events_tx,
            epoch,
            false,
        );
        assert!(matches!(action, ExitProbeAction::EmitExit));
        assert!(!observer.is_active());
    }

    #[test]
    fn duplicate_enter_does_not_reemit_or_advance_epoch() {
        let (_req_tx, req_rx) = actor::channel();
        let (events_tx, mut events_rx) = actor::channel();
        let observer = NativeMissionControl::new(events_tx.clone(), req_rx);

        NativeMissionControl::handle_enter_notification(&observer.state, &events_tx);
        let epoch = observer.state.epoch.load(Ordering::SeqCst);
        assert!(matches!(
            recv_event(&mut events_rx),
            reactor::Event::MissionControlNativeEntered
        ));

        NativeMissionControl::handle_enter_notification(&observer.state, &events_tx);
        assert_eq!(observer.state.epoch.load(Ordering::SeqCst), epoch);
        assert!(observer.is_active());
        assert_no_event(&mut events_rx);
    }

    #[test]
    fn exit_notification_only_requests_probe() {
        let (_req_tx, req_rx) = actor::channel();
        let (events_tx, mut events_rx) = actor::channel();
        let observer = NativeMissionControl::new(events_tx.clone(), req_rx);

        NativeMissionControl::handle_enter_notification(&observer.state, &events_tx);
        let _ = recv_event(&mut events_rx);

        NativeMissionControl::handle_exit_notification(&observer.state, &events_tx);
        assert!(observer.is_active());
        assert_no_event(&mut events_rx);
    }
}
