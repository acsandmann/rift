use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dispatchr::qos::QoS;
use dispatchr::queue::{self, Unmanaged};
use objc2::MainThreadMarker;
use rustc_hash::FxHasher;
use tokio::sync::Notify;
use tracing::instrument;

use crate::actor;
use crate::common::config::Config;
use crate::model::VirtualWorkspaceId;
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::dispatch::{NamedQueueExt, TimerSource};
use crate::sys::screen::SpaceId;
use crate::ui::menu_bar::MenuIcon;
#[derive(Debug, Clone)]
pub enum Event {
    Update {
        active_space: SpaceId,
        workspaces: Vec<WorkspaceData>,
        active_workspace: Option<VirtualWorkspaceId>,
        windows: Vec<WindowData>,
    },
}

pub struct Menu {
    config: Config,
    rx: Receiver,
    icon: Option<MenuIcon>,
    last_signature: Option<u64>,
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

impl Menu {
    pub fn new(config: Config, rx: Receiver, mtm: MainThreadMarker) -> Self {
        Self {
            icon: config.settings.ui.menu_bar.enabled.then(|| MenuIcon::new(mtm)),
            config,
            rx,
            last_signature: None,
        }
    }

    pub async fn run(mut self) {
        if self.icon.is_none() {
            return;
        }

        const DEBOUNCE_MS: u64 = 150;

        let mut pending: Option<Event> = None;
        let notify = Arc::new(Notify::new());
        let armed = Arc::new(AtomicBool::new(false));

        let q = Unmanaged::named("git.acsandmann.rift.menu_bar")
            .unwrap_or_else(|| queue::global(QoS::Utility).unwrap_or_else(|| queue::main()));

        let mut timer = TimerSource::new(q);

        {
            let notify_cl = Arc::clone(&notify);
            let armed_cl = Arc::clone(&armed);
            timer.set_handler(move || {
                armed_cl.store(false, Ordering::Release);
                notify_cl.notify_one();
            });
        }
        timer.resume();

        let schedule_timer = |t: &mut TimerSource, armed: &AtomicBool| {
            if !armed.swap(true, Ordering::AcqRel) {
                t.schedule_after_ms(DEBOUNCE_MS);
            }
        };

        loop {
            tokio::select! {
                maybe = self.rx.recv() => {
                    match maybe {
                        Some((span, event)) => {
                            let _enter = span.enter();
                            pending = Some(event);
                            schedule_timer(&mut timer, &armed);
                        }
                        None => {
                            if let Some(ev) = pending.take() {
                                self.handle_event(ev);
                            }
                            drop(timer);
                            break;
                        }
                    }
                }
                _ = notify.notified(), if pending.is_some() => {
                    if let Some(ev) = pending.take() {
                        self.handle_event(ev);
                    }
                }
            }
        }
    }

    #[instrument(name = "menu_bar::handle_event", skip(self))]
    fn handle_event(&mut self, event: Event) {
        let Some(icon) = &mut self.icon else { return };
        let Event::Update {
            active_space,
            workspaces,
            active_workspace,
            windows,
        } = event;

        let mut hasher = FxHasher::default();
        active_space.get().hash(&mut hasher);
        if let Some(ws) = active_workspace {
            ws.hash(&mut hasher);
        }
        for w in windows.iter() {
            w.id.hash(&mut hasher);
            hasher.write_u64(w.frame.origin.x.to_bits());
            hasher.write_u64(w.frame.origin.y.to_bits());
            hasher.write_u64(w.frame.size.width.to_bits());
            hasher.write_u64(w.frame.size.height.to_bits());
        }
        let sig = hasher.finish();
        if self.last_signature == Some(sig) {
            return;
        }
        self.last_signature = Some(sig);

        let show_all = self.config.settings.ui.menu_bar.show_empty;
        icon.update(active_space, workspaces, active_workspace, windows, show_all);
    }
}
