use objc2_core_foundation::CGRect;
use tracing::debug;

use super::{Reactor, WindowId, WindowInfo, WindowState};
use crate::sys::window_server;
#[cfg(not(test))]
use crate::sys::window_server::WindowServerId;

fn same_tab_frame(a: CGRect, b: CGRect) -> bool {
    (a.origin.x - b.origin.x).abs() <= 1.0
        && (a.origin.y - b.origin.y).abs() <= 1.0
        && (a.size.width - b.size.width).abs() <= 1.0
        && (a.size.height - b.size.height).abs() <= 1.0
}

impl Reactor {
    /// Native tabs have distinct WindowServer IDs but occupy the same frame.
    /// Require AX tab evidence and one outgoing predecessor; same PID alone
    /// would incorrectly combine independent windows or separate tab groups.
    pub(super) fn replace_native_tab(&mut self, incoming: WindowId, info: &mut WindowInfo) {
        if !info.is_standard || info.is_minimized || info.sys_id.is_none() {
            return;
        }
        if self.layout_manager.layout_engine.space_with_window(incoming).is_some() {
            return;
        }
        let native_frame = std::cell::OnceCell::<Option<CGRect>>::new();
        let candidates: Vec<_> = self
            .state
            .windows
            .window_ids_for_pid(incoming.pid)
            .filter(|old| *old != incoming)
            .filter_map(|old| {
                let window = self.state.windows.window(old)?;
                let wsid = window.info.sys_id?;
                let space = self.layout_manager.layout_engine.space_with_window(old)?;
                if !self.is_space_active(space)
                    || self.window_in_non_active_workspace(space, old)
                    || !(info.has_native_tabs || window.info.has_native_tabs)
                    || !(same_tab_frame(window.frame_monotonic, info.frame)
                        || native_frame
                            .get_or_init(|| {
                                #[cfg(not(test))]
                                {
                                    info.sys_id
                                        .and_then(window_server::get_window)
                                        .map(|window| window.frame)
                                }
                                #[cfg(test)]
                                {
                                    None
                                }
                            })
                            .is_some_and(|frame| same_tab_frame(window.frame_monotonic, frame)))
                    || !(window_server::window_ordered_in(wsid) == Some(false)
                        // AppKit can announce the incoming tab before ordering
                        // out the focused tab. Do not tile it provisionally.
                        || (info.has_native_tabs
                            && self.layout_manager.layout_engine.focused_window() == Some(old)))
                {
                    return None;
                }
                Some((old, window.frame_monotonic, window.manage_override, wsid))
            })
            .collect();
        let [(old, frame, manage_override, old_wsid)] = candidates.as_slice() else {
            return;
        };
        let (old, frame, manage_override, old_wsid) = (*old, *frame, *manage_override, *old_wsid);
        info.frame = frame;
        let mut replacement = WindowState::from(info.clone());
        replacement.manage_override = manage_override;
        self.state.windows.insert_window(incoming, replacement);
        self.state.windows.transfer_persistent_window_metadata(old, incoming);
        self.layout_manager
            .layout_engine
            .transfer_persistent_window_identity(old, incoming);
        self.state.windows.remove_window(old);
        self.transaction_manager.remove_for_window(old_wsid);
        debug!(?old, ?incoming, "Replaced native tab in existing layout slot");
    }

    /// Keep the old slot until the coalesced AX inventory can identify the new
    /// tab. A failed/empty native match takes the normal window-close path, and
    /// a successful inventory without a matching tab retires the old slot normally.
    pub(super) fn retain_native_tab_slot_on_departure(&mut self, old: WindowId) -> bool {
        let Some(window) = self.state.windows.window(old) else {
            return false;
        };
        let Some(space) = self.layout_manager.layout_engine.space_with_window(old) else {
            return false;
        };
        if !self.is_space_active(space) || self.window_in_non_active_workspace(space, old) {
            return false;
        }
        #[cfg(not(test))]
        let successor = window_server::key_focused_window(space).and_then(|native| {
            let frame = window_server::get_window(WindowServerId::new(native.idx.get()))?.frame;
            Some((native, frame))
        });
        #[cfg(test)]
        let successor = self.native_tab_successor;
        let Some((native, frame)) = successor else { return false };
        if native.pid != old.pid || native == old || !same_tab_frame(window.frame_monotonic, frame)
        {
            return false;
        }
        let wsid = window.info.sys_id;
        if let Some(wsid) = wsid {
            if wsid.as_u32() == native.idx.get() {
                return false;
            }
            self.state.windows.mark_window_hidden(wsid);
        }
        self.request_window_inventory(old.pid);
        debug!(?old, ?native, "Retaining native tab slot until AX inventory");
        true
    }
}
