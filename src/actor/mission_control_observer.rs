use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nix::libc::pid_t;
use objc2_app_kit::NSWorkspace;
use tracing::{info, instrument};

use crate::actor::reactor;
use crate::actor::reactor::Event;
use crate::sys::app::NSRunningApplicationExt;
use crate::sys::axuielement::AXUIElement;
use crate::sys::observer::Observer;

const K_AX_EXPOSE_SHOW_ALL_WINDOWS: &str = "AXExposeShowAllWindows";
const K_AX_EXPOSE_SHOW_FRONT_WINDOWS: &str = "AXExposeShowFrontWindows";
const K_AX_EXPOSE_SHOW_DESKTOP: &str = "AXExposeShowDesktop";
const K_AX_EXPOSE_EXIT: &str = "AXExposeExit";

#[derive(Debug)]
pub enum Request {
    Stop,
}

pub type Sender = crate::actor::Sender<Request>;
pub type Receiver = crate::actor::Receiver<Request>;

pub struct NativeMissionControl {
    rx: Receiver,
    events_tx: reactor::Sender,
    observer: Option<Observer>,
    app_elem: Option<AXUIElement>,
    active: Arc<AtomicBool>,
}

impl NativeMissionControl {
    pub fn new(events_tx: reactor::Sender, rx: Receiver) -> Self {
        Self {
            rx,
            events_tx,
            observer: None,
            app_elem: None,
            active: Arc::new(AtomicBool::new(false)),
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
        let active_clone = self.active.clone();
        let observer = builder.install(move |_elem: AXUIElement, notif: &str| match notif {
            K_AX_EXPOSE_SHOW_ALL_WINDOWS
            | K_AX_EXPOSE_SHOW_FRONT_WINDOWS
            | K_AX_EXPOSE_SHOW_DESKTOP => {
                active_clone.store(true, Ordering::SeqCst);
                let _ = tx_clone.send(Event::MissionControlNativeEntered);
            }
            K_AX_EXPOSE_EXIT => {
                active_clone.store(false, Ordering::SeqCst);
                let _ = tx_clone.send(Event::MissionControlNativeExited);
            }
            _ => (),
        });

        let elem = AXUIElement::application(pid);

        let _ = observer.add_notification(&elem, K_AX_EXPOSE_SHOW_ALL_WINDOWS);
        let _ = observer.add_notification(&elem, K_AX_EXPOSE_SHOW_FRONT_WINDOWS);
        let _ = observer.add_notification(&elem, K_AX_EXPOSE_SHOW_DESKTOP);
        let _ = observer.add_notification(&elem, K_AX_EXPOSE_EXIT);

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
        self.active.store(false, Ordering::SeqCst);
    }

    pub fn is_active(&self) -> bool { self.active.load(Ordering::SeqCst) }
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
