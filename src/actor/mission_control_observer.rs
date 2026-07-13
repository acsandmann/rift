use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nix::libc::pid_t;
use objc2_app_kit::NSRunningApplication;
use objc2_foundation::ns_string;
use tracing::{error, info, instrument, warn};

use crate::actor::reactor;
use crate::actor::reactor::Event;
use crate::sys::app::NSRunningApplicationExt;
use crate::sys::axuielement::AXUIElement;
use crate::sys::observer::Observer;

const K_AX_EXPOSE_SHOW_ALL_WINDOWS: &str = "AXExposeShowAllWindows";
const K_AX_EXPOSE_SHOW_FRONT_WINDOWS: &str = "AXExposeShowFrontWindows";
const K_AX_EXPOSE_SHOW_DESKTOP: &str = "AXExposeShowDesktop";
const K_AX_EXPOSE_EXIT: &str = "AXExposeExit";

const NOTIFICATIONS: &[&str] = &[
    K_AX_EXPOSE_EXIT,
    K_AX_EXPOSE_SHOW_ALL_WINDOWS,
    K_AX_EXPOSE_SHOW_FRONT_WINDOWS,
    K_AX_EXPOSE_SHOW_DESKTOP,
];

#[derive(Debug)]
pub enum Request {
    Stop,
}

pub type Sender = crate::actor::Sender<Request>;
pub type Receiver = crate::actor::Receiver<Request>;

pub struct NativeMissionControl {
    rx: Receiver,
    observer: Option<Observer>,
    app_elem: Option<AXUIElement>,
    active: Arc<AtomicBool>,
    events_tx: reactor::Sender,
}

struct State {
    events_tx: reactor::Sender,
    active: Arc<AtomicBool>,
}

impl NativeMissionControl {
    pub fn new(events_tx: reactor::Sender, rx: Receiver) -> Self {
        Self {
            rx,
            observer: None,
            app_elem: None,
            active: Arc::new(AtomicBool::new(false)),
            events_tx,
        }
    }

    #[instrument(skip(self))]
    pub async fn run(mut self) {
        info!("Starting native mission-control monitor (must run on main thread)");
        self.observe();

        while let Some((_span, req)) = self.rx.recv().await {
            match req {
                Request::Stop => break,
            }
        }

        self.unobserve();
    }

    pub fn observe(&mut self) {
        if self.observer.is_some() {
            return;
        }

        let Some(pid) = find_dock_pid() else {
            warn!("Could not find the Dock process; Mission Control observer disabled");
            return;
        };

        let builder = match Observer::new_with_notification(pid) {
            Ok(builder) => builder,
            Err(err) => {
                warn!(?err, pid, "Could not create Dock accessibility observer");
                return;
            }
        };

        let state = Rc::new(RefCell::new(State {
            events_tx: self.events_tx.clone(),
            active: self.active.clone(),
        }));
        let callback_state = state.clone();
        let observer = builder.install_with_notification(move |_elem, notification| {
            callback_state.borrow_mut().handle_notification(notification);
        });

        let elem = AXUIElement::application(pid);
        for notification in NOTIFICATIONS {
            if let Err(err) = observer.add_notification(&elem, notification) {
                warn!(?err, notification, "Could not observe Dock notification");
            }
        }

        self.observer = Some(observer);
        self.app_elem = Some(elem);
    }

    pub fn unobserve(&mut self) {
        if self.observer.is_none() {
            return;
        }

        if let (Some(observer), Some(elem)) = (self.observer.as_ref(), self.app_elem.as_ref()) {
            for notification in NOTIFICATIONS {
                let _ = observer.remove_notification(elem, notification);
            }
        }

        self.observer = None;
        self.app_elem = None;
        self.active.store(false, Ordering::SeqCst);
    }

    pub fn is_active(&self) -> bool { self.active.load(Ordering::SeqCst) }
}

impl State {
    #[instrument(skip(self))]
    fn handle_notification(&mut self, notification: &'static str) {
        match notification {
            K_AX_EXPOSE_SHOW_ALL_WINDOWS
            | K_AX_EXPOSE_SHOW_FRONT_WINDOWS
            | K_AX_EXPOSE_SHOW_DESKTOP => {
                self.active.store(true, Ordering::SeqCst);
                self.events_tx.send(Event::MissionControlNativeEntered);
            }
            K_AX_EXPOSE_EXIT => {
                self.active.store(false, Ordering::SeqCst);
                self.events_tx.send(Event::MissionControlNativeExited);
            }
            _ => error!(?notification, "Unhandled notification from Dock"),
        }
    }
}

fn find_dock_pid() -> Option<pid_t> {
    let apps =
        NSRunningApplication::runningApplicationsWithBundleIdentifier(ns_string!("com.apple.dock"))
            .to_vec();
    let [app] = apps.as_slice() else {
        return None;
    };
    Some(app.pid())
}
