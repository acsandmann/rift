use crate::common::collections::HashMap;
use crate::sys::app::{WindowInfo, is_ax_transient, pid_t};
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

#[derive(Default)]
pub(super) struct TransientFocusGuard {
    pub windows: HashMap<WindowServerId, Transient>,
}

pub(super) struct Transient {
    pub pid: pid_t,
    pub space: SpaceId,
    pub visible: bool,
    // AX recovery may supplement native evidence, but must not erase it.
    pub native: bool,
}

impl TransientFocusGuard {
    pub fn native(&mut self, id: WindowServerId, pid: pid_t, space: SpaceId, transient: bool) {
        if transient {
            self.windows.insert(id, Transient {
                pid,
                space,
                visible: true,
                native: true,
            });
        } else {
            self.windows.remove(&id);
        }
    }

    pub fn ax(&mut self, id: WindowServerId, pid: pid_t, info: &WindowInfo, space: SpaceId) {
        if self.windows.get(&id).is_some_and(|t| t.native && t.pid == pid) {
            return;
        }
        if is_ax_transient(info)
            && matches!(
                info.ax_role.as_deref(),
                Some("AXWindow" | "AXSheet" | "AXPopover" | "AXMenu")
            )
        {
            self.windows.insert(id, Transient {
                pid,
                space,
                visible: !info.is_minimized,
                native: false,
            });
        } else {
            self.windows.remove(&id);
        }
    }

    pub fn visibility(&mut self, id: WindowServerId, visible: bool) {
        if let Some(t) = self.windows.get_mut(&id) {
            t.visible = visible;
        }
    }

    pub fn depart(&mut self, id: WindowServerId, departed: SpaceId, resolved: Option<SpaceId>) {
        if let Some(t) = self.windows.get_mut(&id) {
            if let Some(space) = resolved.filter(|space| *space != departed) {
                t.space = space;
                t.visible = true;
            } else if t.space == departed {
                t.visible = false;
            }
        }
    }
}
