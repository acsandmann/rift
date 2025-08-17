use std::sync::Arc;

use objc2::MainThreadMarker;
use tracing::instrument;

use crate::actor;
use crate::common::config::Config;
use crate::model::VirtualWorkspaceId;
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::menubar::MenuIcon;
use crate::sys::screen::SpaceId;

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
            icon: config.settings.ui.menubar.then(|| MenuIcon::new(mtm)),
            config,
            rx,
        }
    }

    pub async fn run(mut self) {
        if self.icon.is_none() {
            return;
        }
        while let Some((span, event)) = self.rx.recv().await {
            let _guard = span.enter();
            self.handle_event(event);
        }
    }

    #[instrument(skip(self))]
    fn handle_event(&mut self, event: Event) {
        let Some(icon) = &mut self.icon else { return };
        let Event::Update {
            active_space,
            workspaces,
            active_workspace,
            windows,
        } = event;

        let show_all = self.config.settings.ui.menubar_show_all_workspaces;
        icon.update(active_space, workspaces, active_workspace, windows, show_all);
    }
}
