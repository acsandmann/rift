use super::{Reactor, SpaceId, WindowId, WindowServerId, window_server};
use crate::actor::app::Request;
use crate::common::config::LayoutMode;
use crate::layout_engine::systems::LayoutSystem;

impl Reactor {
    pub(super) fn repair_stack_after_focus(
        &self,
        space: SpaceId,
        focused: WindowId,
    ) -> Option<WindowId> {
        let workspaces = self.layout_manager.layout_engine.workspaces();
        let (workspace, layout) = workspaces.active_layout_for_space(space)?;
        let workspace = &workspaces[workspace];
        if workspace.layout_mode != LayoutMode::Stack
            || self.config.settings.layout.stack.stack_offset == 0.0
        {
            return None;
        }

        let system = &workspace.layout_system;
        let members = system.stack_members(layout, focused);
        if members.len() < 3 || system.has_any_fullscreen_node(layout) {
            return None;
        }

        let order: Vec<_> =
            window_server::space_window_list_for_connection(&[space.get()], 0, false)
                .into_iter()
                .filter_map(|id| self.state.windows.tracked_window_id(WindowServerId::new(id)))
                .filter(|id| members.contains(id))
                .collect();
        if order.len() != members.len() {
            return None;
        }

        // The key-window notification can precede the corresponding reorder.
        if order.first() != Some(&focused) {
            return Some(focused);
        }

        let window = background_raises(&members, focused, &order).into_iter().next()?;
        // AXRaise can return before WindowServer changes the order. Each
        // reorder wakes the focus resolver; advance only from the observed order.
        if self.pending_stack_raise != Some(window) {
            self.app_manager
                .apps
                .get(&window.pid)?
                .handle
                .send(Request::RaiseStackWindow { window, focused, space })
                .ok()?;
        }

        Some(window)
    }
}

fn background_raises(stack: &[WindowId], focused: WindowId, order: &[WindowId]) -> Vec<WindowId> {
    let Some(index) = stack.iter().position(|&id| id == focused) else {
        return Vec::new();
    };

    let mut raises = Vec::new();
    append_covered(stack[..index].iter().copied(), order, &mut raises);
    append_covered(stack[index + 1..].iter().rev().copied(), order, &mut raises);

    // AXRaise on a sibling in the key app can transfer keyboard focus.
    if raises.iter().any(|id| id.pid == focused.pid) {
        return Vec::new();
    }

    raises
}

fn append_covered(
    outer_to_inner: impl Iterator<Item = WindowId>,
    order: &[WindowId],
    raises: &mut Vec<WindowId>,
) {
    // Keep the outermost run already stacked back-to-front. Raising the rest
    // from the outside in exposes every edge on this side of the key window.
    let mut native = order.iter().rev();
    raises.extend(outer_to_inner.skip_while(|id| native.any(|window| window == id)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_only_covered_edges_after_a_focus_jump() {
        let [a, b, c, d, e] = [1, 2, 3, 4, 5].map(|id| WindowId::new(id, 1));
        let stack = [a, b, c, d, e];

        assert_eq!(background_raises(&stack, e, &[e, a, b, c, d]), [b, c, d]);
        assert_eq!(background_raises(&stack, a, &[a, e, d, c, b]), [d, c, b]);
        assert!(background_raises(&stack, c, &[c, d, b, a, e]).is_empty());
    }

    #[test]
    fn skips_the_whole_repair_if_a_target_shares_keyboard_focus() {
        let a = WindowId::new(1, 1);
        let b = WindowId::new(2, 1);
        let c = WindowId::new(3, 1);
        let d = WindowId::new(4, 1);
        let e = WindowId::new(3, 2);
        let stack = [a, b, c, d, e];

        assert!(background_raises(&stack, e, &[e, a, b, c, d]).is_empty());
        assert_eq!(background_raises(&stack, e, &[e, c, b, a, d]), [d]);
    }
}
