use std::sync::Arc;
use std::time::Duration;

use objc2::MainThreadMarker;
use tracing::instrument;

use crate::actor;
use crate::common::config::Config;
use crate::model::VirtualWorkspaceId;
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::screen::SpaceId;
use crate::sys::timer::Timer;
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
    config: Arc<Config>,
    rx: Receiver,
    icon: Option<MenuIcon>,
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

impl Menu {
    pub fn new(config: Arc<Config>, rx: Receiver, mtm: MainThreadMarker) -> Self {
        Self {
            icon: config.settings.ui.menu_bar.enabled.then(|| MenuIcon::new(mtm)),
            config,
            rx,
        }
    }

    pub async fn run(mut self) {
        if self.icon.is_none() {
            return;
        }

        const DEBOUNCE_MS: u64 = 150;

        let mut pending: Option<Event> = None;

        loop {
            if pending.is_none() {
                match self.rx.recv().await {
                    Some((span, event)) => {
                        let _ = span.enter();
                        pending = Some(event);
                    }
                    None => break,
                }
            } else {
                tokio::select! {
                    maybe_msg = self.rx.recv() => {
                        match maybe_msg {
                            Some((span, event)) => {
                                let _ = span.enter();
                                pending = Some(event);
                            }
                            None => {
                                if let Some(ev) = pending.take() {
                                    self.handle_event(ev);
                                }
                                break;
                            }
                        }
                    }
                    _ = Timer::sleep(Duration::from_millis(DEBOUNCE_MS)) => {
                        if let Some(ev) = pending.take() {
                            self.handle_event(ev);
                        }
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

        let show_all = self.config.settings.ui.menu_bar.show_empty;
        icon.update(active_space, workspaces, active_workspace, windows, show_all);
    }
}
