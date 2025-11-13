use objc2::MainThreadMarker;
use tokio::time::Duration;

use crate::actor;
use crate::common::config::Config;
use crate::model::VirtualWorkspaceId;
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::screen::SpaceId;
use crate::sys::timer::Timer;
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
        const DEBOUNCE: Duration = Duration::from_millis(150);

        let mut pending: Option<Event> = None;
        let mut timer = Timer::manual();

        loop {
            tokio::select! {
                _ = &mut timer, if pending.is_some() => {
                    if let Some(ev) = pending.take() {
                        self.handle_event(ev);
                    }
                }

                maybe = self.rx.recv() => {
                    match maybe {
                        Some((span, event)) => {
                            let _enter = span.enter();
                            match event {
                                Event::Update(_) => {
                                    pending = Some(event);
                                    timer.set_next_fire(DEBOUNCE);
                                }
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
