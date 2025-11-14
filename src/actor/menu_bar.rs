use nix::libc;
use objc2::MainThreadMarker;
use tokio::sync::mpsc::UnboundedSender;

use crate::actor;
use crate::common::config::Config;
use crate::model::VirtualWorkspaceId;
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::screen::SpaceId;
use crate::ui::menu_bar::MenuIcon;

#[derive(Debug, Clone)]
pub struct Update {
    pub active_space: SpaceId,
    pub workspaces: Vec<WorkspaceData>,
    pub active_workspace_idx: Option<u64>,
    pub active_workspace: Option<VirtualWorkspaceId>,
    pub windows: Vec<WindowData>,
}

pub enum Event {
    Update(Update),
    ConfigUpdated(Config),
}

pub struct Menu {
    config: Config,
    rx: Receiver,
    icon: Option<MenuIcon>,
    mtm: MainThreadMarker,
    last_signature: Option<u64>,
    last_update: Option<Update>,
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

impl Menu {
    pub fn new(config: Config, rx: Receiver, mtm: MainThreadMarker) -> Self {
        Self {
            icon: config.settings.ui.menu_bar.enabled.then(|| MenuIcon::new(mtm)),
            config,
            rx,
            mtm,
            last_signature: None,
            last_update: None,
        }
    }

    pub async fn run(mut self) {
        const DEBOUNCE_MS: u64 = 150;

        let mut pending: Option<Event> = None;

        let (tick_tx, mut tick_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

        Self::spawn_kqueue_timer(DEBOUNCE_MS as i64, tick_tx);

        loop {
            tokio::select! {
                maybe_tick = tick_rx.recv() => {
                    if maybe_tick.is_none() {
                        if let Some(ev) = pending.take() {
                            self.handle_event(ev);
                        }
                        break;
                    }

                    if let Some(ev) = pending.take() {
                        self.handle_event(ev);
                    }
                }

                maybe = self.rx.recv() => {
                    match maybe {
                        Some((span, event)) => {
                            let _enter = span.enter();
                            match event {
                                Event::Update(_) => pending = Some(event),
                                Event::ConfigUpdated(cfg) => self.handle_config_updated(cfg),
                            }
                        }
                        None => {
                            if let Some(ev) = pending.take() {
                                self.handle_event(ev);
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Update(update) => self.handle_update(update),
            Event::ConfigUpdated(cfg) => self.handle_config_updated(cfg),
        }
    }

    fn handle_update(&mut self, update: Update) {
        self.apply_update(&update);
        self.last_update = Some(update);
    }

    fn apply_update(&mut self, update: &Update) {
        let Some(icon) = &mut self.icon else { return };

        let sig = sig(
            update.active_space.get() as u64,
            update.active_workspace_idx,
            &update.windows,
        );
        if self.last_signature == Some(sig) {
            return;
        }
        self.last_signature = Some(sig);

        let menu_bar_settings = &self.config.settings.ui.menu_bar;
        icon.update(
            update.active_space,
            update.workspaces.clone(),
            update.active_workspace,
            update.windows.clone(),
            menu_bar_settings,
        );
    }

    fn handle_config_updated(&mut self, new_config: Config) {
        let should_enable = new_config.settings.ui.menu_bar.enabled;

        self.config = new_config;

        if should_enable && self.icon.is_none() {
            self.icon = Some(MenuIcon::new(self.mtm));
        } else if !should_enable && self.icon.is_some() {
            self.icon = None;
        }

        self.last_signature = None;
        if let Some(update) = self.last_update.take() {
            self.handle_update(update);
        }
    }

    fn spawn_kqueue_timer(period_ms: i64, tx: UnboundedSender<()>) {
        std::thread::spawn(move || unsafe {
            let kq = libc::kqueue();
            if kq < 0 {
                return;
            }

            let mut change: libc::kevent = std::mem::zeroed();
            change.ident = 1 as libc::uintptr_t;
            change.filter = libc::EVFILT_TIMER as i16;
            change.flags = (libc::EV_ADD | libc::EV_ENABLE) as u16;
            change.fflags = 0;
            change.data = period_ms as libc::intptr_t;
            change.udata = std::ptr::null_mut();

            let reg = libc::kevent(
                kq,
                &change as *const libc::kevent,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            );
            if reg < 0 {
                let _ = libc::close(kq);
                return;
            }

            let mut event: libc::kevent = std::mem::zeroed();

            loop {
                let n = libc::kevent(
                    kq,
                    std::ptr::null(),
                    0,
                    &mut event as *mut libc::kevent,
                    1,
                    std::ptr::null(),
                );

                if n <= 0 {
                    break;
                }

                if tx.send(()).is_err() {
                    break;
                }
            }

            let _ = libc::close(kq);
        });
    }
}

// this is kind of reinventing the wheel but oh well i am using my brain
#[inline(always)]
fn sig(active_space: u64, active_workspace: Option<u64>, windows: &[WindowData]) -> u64 {
    let mut x = active_space ^ (windows.len() as u64).rotate_left(7);
    let mut s = active_space.wrapping_add(windows.len() as u64);

    if let Some(ws) = active_workspace {
        let ws_tag = ws ^ 0xA5A5_A5A5_A5A5_A5A5u64;
        x ^= ws_tag;
        s = s.wrapping_add(ws_tag);
    }

    for w in windows {
        let v = (w.id.idx.get() as u64)
            ^ w.frame.origin.x.to_bits().rotate_left(11)
            ^ w.frame.origin.y.to_bits().rotate_left(23)
            ^ w.frame.size.width.to_bits().rotate_left(37)
            ^ w.frame.size.height.to_bits().rotate_left(51);

        x ^= v;
        s = s.wrapping_add(v);
    }

    x ^ s.rotate_left(29) ^ (s >> 17)
}
