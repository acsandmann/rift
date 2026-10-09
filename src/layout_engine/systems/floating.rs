use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use rift_protocol::DirectionalDistance;
use serde::{Deserialize, Serialize};

use super::{LayoutSystem, WindowLayoutConstraints};
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::HashMap;
use crate::common::config::WindowInsertionPoint;
use crate::layout_engine::{
    Direction, LayoutId, LayoutKind, ResizeOrientation, TraditionalLayoutSystem,
};
use crate::model::tree::NodeId;

/// Tree membership supplies focus and stacks; frames remain independent of tiling.
#[serde_with::serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub struct FloatingLayoutSystem {
    inner: TraditionalLayoutSystem,
    #[serde(default)]
    #[serde_as(as = "HashMap<_, crate::sys::geometry::CGRectDef>")]
    frames: HashMap<WindowId, CGRect>,
    #[serde(default)]
    #[serde_as(as = "HashMap<_, crate::sys::geometry::CGRectDef>")]
    independent_frames: HashMap<WindowId, CGRect>,
    #[serde(default)]
    spread_on_initialization: bool,
}
impl Default for FloatingLayoutSystem {
    fn default() -> Self { Self::new(WindowInsertionPoint::default()) }
}

impl FloatingLayoutSystem {
    pub fn new(point: WindowInsertionPoint) -> Self {
        Self {
            inner: TraditionalLayoutSystem::new(point, false),
            frames: HashMap::default(),
            independent_frames: HashMap::default(),
            spread_on_initialization: false,
        }
    }

    pub fn set_window_insertion_point(&mut self, point: WindowInsertionPoint) {
        self.inner.set_window_insertion_point(point);
    }

    pub(crate) fn collect_group_containers(
        &self,
        layout: LayoutId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        selection_path_only: bool,
    ) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
        let selection = self.inner.selection(layout);
        let root = self.inner.root(layout);
        root.traverse_preorder(self.inner.map())
            .filter(|node| self.inner.layout(*node).is_stacked())
            .filter(|node| {
                !selection_path_only
                    || selection.ancestors(self.inner.map()).any(|ancestor| ancestor == *node)
            })
            .filter_map(|node| {
                let children: Vec<_> = node.children(self.inner.map()).collect();
                let selected =
                    self.inner.local_selection(node).or_else(|| children.first().copied())?;
                let index = children.iter().position(|child| *child == selected)?;
                let wid = self.inner.visible_windows_in_subtree(selected).first().copied()?;
                let window = self.inner.window_node(layout, wid)?;
                if self.inner.fullscreen_frame(window, screen, gaps).is_some() {
                    return None;
                }
                let frame = self.frame(wid).unwrap_or_else(|| {
                    Self::default_frame(screen, WindowLayoutConstraints::default())
                });
                Some(self.inner.stack_group_container_info(
                    node,
                    self.inner.layout(node),
                    frame,
                    &children,
                    index,
                ))
            })
            .collect()
    }

    fn active_window(&self, layout: LayoutId) -> Option<WindowId> {
        let mut node = self.inner.selection(layout);
        loop {
            if let Some(wid) = self.inner.window_at(node) {
                return Some(wid);
            }
            node = self
                .inner
                .local_selection(node)
                .or_else(|| node.first_child(self.inner.map()))?;
        }
    }

    fn members(&self, container: NodeId) -> Vec<WindowId> {
        container
            .traverse_preorder(self.inner.map())
            .filter_map(|node| self.inner.window_at(node))
            .collect()
    }

    fn selected_container(&self, layout: LayoutId) -> Option<NodeId> {
        let selection = self.inner.selection(layout);
        let container = if self.inner.window_at(selection).is_some() {
            selection.parent(self.inner.map())?
        } else {
            selection
        };
        (container != self.inner.root(layout)).then_some(container)
    }

    fn restore_independent_members(&mut self, layout: LayoutId, members: &[WindowId]) {
        // The tree observer collapses one-member containers during reparenting.
        // Restore only the affected members that were promoted to the root.
        let root = self.inner.root(layout);
        for &wid in members {
            if self
                .inner
                .window_node(layout, wid)
                .and_then(|node| node.parent(self.inner.map()))
                == Some(root)
                && let Some(frame) = self.independent_frames.remove(&wid)
            {
                self.frames.insert(wid, frame);
            }
        }
    }

    pub(crate) fn cycle_windows(&self, layout: LayoutId) -> Vec<WindowId> {
        if let Some(wid) = self.active_window(layout)
            && let Some(node) = self.inner.window_node(layout, wid)
            && let Some(parent) = node.parent(self.inner.map())
            && self.inner.layout(parent).is_stacked()
        {
            return self.members(parent);
        }
        self.inner.visible_windows_in_layout(layout)
    }

    fn directional_window(
        &self,
        layout: LayoutId,
        direction: Direction,
        exclude: Option<NodeId>,
    ) -> Option<WindowId> {
        let focused = self.active_window(layout)?;
        let source = self.frames.get(&focused)?;
        let origin = source.mid();
        let horizontal = matches!(direction, Direction::Left | Direction::Right);
        self.inner
            .visible_windows_in_layout(layout)
            .into_iter()
            .filter(|wid| *wid != focused)
            .filter(|wid| {
                !exclude.is_some_and(|parent| {
                    self.inner.window_node(layout, *wid).is_some_and(|node| {
                        node.ancestors(self.inner.map()).any(|ancestor| ancestor == parent)
                    })
                })
            })
            .filter_map(|wid| {
                let frame = self.frames.get(&wid)?;
                let candidate = frame.mid();
                let (across, overlaps) = if horizontal {
                    (
                        candidate.y - origin.y,
                        source.origin.y < frame.max().y && frame.origin.y < source.max().y,
                    )
                } else {
                    (
                        candidate.x - origin.x,
                        source.origin.x < frame.max().x && frame.origin.x < source.max().x,
                    )
                };
                let along = (origin.x, origin.y)
                    .distance_in_direction((candidate.x, candidate.y), direction)?;
                (along > 0.0).then_some((wid, !overlaps, along * along + across * across))
            })
            .min_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2)))
            .map(|(wid, _, _)| wid)
    }

    fn focus_target(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        let focused = self.active_window(layout)?;
        let node = self.inner.window_node(layout, focused)?;
        let parent = node.parent(self.inner.map())?;
        let kind = self.inner.layout(parent);
        let cycles_stack = match direction {
            Direction::Left | Direction::Right => kind == LayoutKind::HorizontalStack,
            Direction::Up | Direction::Down => kind == LayoutKind::VerticalStack,
        };
        let members = self.inner.layout(parent).is_stacked().then(|| self.members(parent));
        let forward = matches!(direction, Direction::Right | Direction::Down);
        let stack_target = members.as_ref().and_then(|members| {
            let index = members.iter().position(|wid| *wid == focused)?;
            let next = if forward {
                index.checked_add(1)?
            } else {
                index.checked_sub(1)?
            };
            members.get(next).copied()
        });
        // Walk hidden members before leaving along the stack axis, but let the
        // end of a stack reach neighboring windows instead of trapping focus.
        if cycles_stack && stack_target.is_some() {
            return stack_target;
        }
        self.directional_window(layout, direction, None).or_else(|| {
            stack_target.or_else(|| {
                let members = members.as_ref()?;
                if forward {
                    members.first().copied()
                } else {
                    members.last().copied()
                }
            })
        })
    }

    fn default_frame(screen: CGRect, limits: WindowLayoutConstraints) -> CGRect {
        let limits = limits.normalized();
        let dimension = |value: f64, horizontal| {
            if let Some(fixed) = limits.fixed_for_axis(horizontal) {
                return fixed;
            }
            let value = value.max(limits.min_for_axis(horizontal));
            let max = limits.max_for_axis(horizontal);
            if max > 0.0 { value.min(max) } else { value }
        };
        let size = CGSize::new(
            dimension(screen.size.width * 0.7, true),
            dimension(screen.size.height * 0.7, false),
        );
        CGRect::new(
            CGPoint::new(
                screen.mid().x - size.width / 2.,
                screen.mid().y - size.height / 2.,
            ),
            size,
        )
    }

    pub(crate) fn spread_initial_windows(&mut self) { self.spread_on_initialization = true; }

    fn cascade_frame(frame: CGRect, screen: CGRect, index: usize, count: usize) -> CGRect {
        if count <= 1 {
            return frame;
        }
        let slots = (count - 1) as f64;
        let available_x = (screen.size.width - frame.size.width).max(0.0);
        let available_y = (screen.size.height - frame.size.height).max(0.0);
        let span_x = (slots * 36.0).min(available_x);
        let span_y = (slots * 36.0).min(available_y);
        let fraction = index as f64 / slots;
        CGRect::new(
            CGPoint::new(
                screen.origin.x + available_x / 2.0 - span_x / 2.0 + span_x * fraction,
                screen.origin.y + available_y / 2.0 - span_y / 2.0 + span_y * fraction,
            ),
            frame.size,
        )
    }

    fn initial_frame(
        &self,
        screen: CGRect,
        limits: WindowLayoutConstraints,
        live: Option<CGRect>,
        index: usize,
        count: usize,
    ) -> CGRect {
        let default = Self::default_frame(screen, limits);
        let mut frame = live.unwrap_or(default);
        let size = CGSize::new(
            frame.size.width.min(default.size.width),
            frame.size.height.min(default.size.height),
        );
        if size != frame.size {
            frame = CGRect::new(
                CGPoint::new(
                    screen.mid().x - size.width / 2.,
                    screen.mid().y - size.height / 2.,
                ),
                size,
            );
        }
        if self.spread_on_initialization {
            Self::cascade_frame(frame, screen, index, count)
        } else {
            frame
        }
    }

    pub(crate) fn initialize_frames(
        &mut self,
        layout: LayoutId,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        get_frame: &impl Fn(WindowId) -> Option<CGRect>,
    ) {
        let windows = self.inner.all_windows_in_layout(layout);
        let count = windows.len();
        for (index, wid) in windows.into_iter().enumerate() {
            if self.frames.contains_key(&wid) {
                continue;
            }
            let frame = self.initial_frame(
                screen,
                constraints.get(&wid).copied().unwrap_or_default(),
                get_frame(wid),
                index,
                count,
            );
            self.frames.insert(wid, frame);
        }
        self.spread_on_initialization = false;
    }

    pub fn frame(&self, wid: WindowId) -> Option<CGRect> { self.frames.get(&wid).copied() }

    pub fn store_frame(&mut self, wid: WindowId, frame: CGRect) {
        let layouts = self.inner.layouts_for_window(wid);
        if !layouts.iter().any(|layout| self.inner.window_is_visible(*layout, wid)) {
            return;
        }
        self.frames.insert(wid, frame);
        for layout in layouts {
            let Some(node) = self.inner.window_node(layout, wid) else {
                continue;
            };
            // The outermost stack contains all members that share this frame.
            let stack = node
                .ancestors(self.inner.map())
                .filter(|parent| self.inner.layout(*parent).is_stacked())
                .last();
            if let Some(stack) = stack {
                for member in self.members(stack) {
                    self.frames.insert(member, frame);
                }
            }
        }
    }

    fn align_members(&mut self, members: &[WindowId], anchor: WindowId, save: bool) {
        let frame = self.frame(anchor);
        for &wid in members {
            if save && let Some(original) = self.frame(wid) {
                self.independent_frames.entry(wid).or_insert(original);
            }
            if let Some(frame) = frame {
                self.frames.insert(wid, frame);
            }
        }
    }

    pub fn contains_floating_window(&self, wid: WindowId) -> bool {
        self.inner.contains_any_window(wid)
    }
}
impl LayoutSystem for FloatingLayoutSystem {
    delegate_traditional_layout_system!(@tree);

    fn create_layout(&mut self) -> LayoutId { self.inner.create_layout() }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId { self.inner.clone_layout(layout) }

    fn remove_layout(&mut self, layout: LayoutId) {
        let windows = self.inner.all_windows_in_layout(layout);
        self.inner.remove_layout(layout);
        for wid in windows {
            if !self.inner.contains_any_window(wid) {
                self.frames.remove(&wid);
                self.independent_frames.remove(&wid);
            }
        }
    }

    fn draw_tree(&self, layout: LayoutId) -> String { self.inner.draw_tree(layout) }

    fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        self.inner.container_tree(layout)
    }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        _stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        _stack_line_thickness: f64,
        _stack_line_horiz: crate::common::config::HorizontalPlacement,
        _stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        let windows = self.inner.all_windows_in_layout(layout);
        let count = windows.len();
        windows
            .into_iter()
            .enumerate()
            .map(|(index, wid)| {
                let frame = self
                    .inner
                    .window_node(layout, wid)
                    .and_then(|node| self.inner.fullscreen_frame(node, screen, gaps))
                    .or_else(|| self.frames.get(&wid).copied())
                    .unwrap_or_else(|| {
                        self.initial_frame(
                            screen,
                            constraints.get(&wid).copied().unwrap_or_default(),
                            None,
                            index,
                            count,
                        )
                    });
                (wid, frame)
            })
            .collect()
    }

    fn all_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.inner.all_windows_in_layout(layout)
    }

    fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        let target = self.focus_target(layout, direction);
        if let Some(wid) = target {
            self.inner.select_window(layout, wid);
        }
        (target, target.into_iter().collect())
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.focus_target(layout, direction)
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        // New windows remain independent until an explicit join command.
        let root = self.inner.root(layout);
        let anchor = (self.inner.window_insertion_point() == WindowInsertionPoint::NextToSelection)
            .then(|| {
                self.inner
                    .selection(layout)
                    .ancestors(self.inner.map())
                    .find(|node| node.parent(self.inner.map()) == Some(root))
            })
            .flatten();
        let node = if let Some(anchor) = anchor {
            let node = self.inner.tree.mk_node().insert_after(anchor);
            self.inner.tree.data.window.set_window(layout, node, wid);
            node
        } else {
            self.inner.add_window_under(layout, root, wid)
        };
        self.inner.select(node);
    }

    fn replace_window(&mut self, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }
        for frames in [&mut self.frames, &mut self.independent_frames] {
            let frame = frames.remove(&from);
            frames.remove(&to);
            if let Some(frame) = frame {
                frames.insert(to, frame);
            }
        }
        self.inner.replace_window(from, to);
    }

    fn remove_window(&mut self, wid: WindowId) {
        self.frames.remove(&wid);
        self.independent_frames.remove(&wid);
        self.inner.remove_window(wid);
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        self.frames.retain(|wid, _| wid.pid != pid);
        self.independent_frames.retain(|wid, _| wid.pid != pid);
        self.inner.remove_windows_for_app(pid)
    }

    fn windows_for_app(&self, layout: LayoutId, pid: pid_t) -> Vec<WindowId> {
        self.inner.windows_for_app(layout, pid)
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        let delta =
            super::reconcile_app_membership(pid, self.inner.windows_for_app(layout, pid), desired);
        for wid in delta.removals {
            self.remove_window(wid);
        }
        for wid in delta.additions {
            self.add_window_after_selection(layout, wid);
        }
    }

    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.inner.has_windows_for_app(layout, pid)
    }

    fn on_window_resized(
        &mut self,
        _layout: LayoutId,
        wid: WindowId,
        _old_frame: CGRect,
        new_frame: CGRect,
        _screen: CGRect,
        _gaps: &crate::common::config::GapSettings,
    ) {
        self.store_frame(wid, new_frame);
    }

    fn move_selection(&mut self, _layout: LayoutId, _direction: Direction) -> bool { false }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: LayoutId,
        to_layout: LayoutId,
    ) {
        self.inner.move_selection_to_layout_after_selection(from_layout, to_layout)
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) {
        let Some(focused) = self.active_window(layout) else {
            return;
        };
        let Some(selection) = self.inner.window_node(layout, focused) else {
            return;
        };
        let root = self.inner.root(layout);
        let parent = selection.parent(self.inner.map()).unwrap_or(root);
        let exclude = (parent != root).then_some(parent);
        let target = self.directional_window(layout, direction, exclude);
        let Some(target) = target else {
            return;
        };
        let Some(target_node) = self.inner.window_node(layout, target) else {
            return;
        };
        let container = if parent == root {
            let kind = match direction {
                Direction::Left | Direction::Right => LayoutKind::Horizontal,
                _ => LayoutKind::Vertical,
            };
            self.inner.split_selection(layout, kind);
            selection.parent(self.inner.map()).unwrap()
        } else {
            parent
        };
        let previous_members = target_node
            .parent(self.inner.map())
            .filter(|parent| *parent != root)
            .map(|parent| self.members(parent))
            .unwrap_or_default();
        if self.inner.layout(container).is_stacked() {
            self.align_members(&[target], focused, true);
        } else {
            self.independent_frames.remove(&target);
        }
        target_node.detach(&mut self.inner.tree).push_back(container);
        self.restore_independent_members(layout, &previous_members);
        self.inner.select(selection);
    }

    fn consume_or_expel_selection(&mut self, layout: LayoutId, direction: Direction) {
        if self.selected_container(layout).is_some() {
            self.unjoin_selection(layout);
        } else {
            self.join_selection_with_direction(layout, direction);
        }
    }

    fn apply_stacking_to_parent_of_selection(
        &mut self,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        let Some(focused) = self.active_window(layout) else {
            return Vec::new();
        };
        // Only an explicitly joined container can become a floating stack.
        let Some(container) = self.selected_container(layout) else {
            return Vec::new();
        };
        let members = self.members(container);
        let save = !self.inner.layout(container).is_stacked();
        self.align_members(&members, focused, save);
        let _ = self.inner.apply_stacking_to_parent_of_selection(layout, default_orientation);
        self.inner.select_window(layout, focused);
        self.inner.visible_windows_under_selection(layout)
    }

    fn unstack_parent_of_selection(
        &mut self,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        let focused = self.active_window(layout);
        let selection = self.inner.selection(layout);
        let container = selection
            .ancestors(self.inner.map())
            .find(|node| self.inner.layout(*node).is_stacked());
        let Some(container) = container else {
            return Vec::new();
        };
        let members = self.members(container);
        let visible = self.inner.unstack_parent_of_selection(layout, default_orientation);
        for wid in members {
            if let Some(frame) = self.independent_frames.remove(&wid) {
                if Some(wid) != focused {
                    self.frames.insert(wid, frame);
                }
            }
        }
        if let Some(wid) = focused {
            self.inner.select_window(layout, wid);
        }
        visible
    }

    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        self.inner.parent_of_selection_is_stacked(layout)
    }

    fn unjoin_selection(&mut self, layout: LayoutId) {
        let Some(wid) = self.active_window(layout) else {
            return;
        };
        let Some(selection) = self.inner.window_node(layout, wid) else {
            return;
        };
        let Some(parent) = selection.parent(self.inner.map()) else {
            return;
        };
        if parent == self.inner.root(layout) {
            return;
        }
        let previous_members = self.members(parent);
        // Detach only the focused member. The remaining stack stays intact.
        selection.detach(&mut self.inner.tree).insert_after(parent);
        if let Some(frame) = self.independent_frames.remove(&wid) {
            self.frames.insert(wid, frame);
        }
        self.restore_independent_members(layout, &previous_members);
        self.inner.select(selection);
    }

    fn resize_selection_by(
        &mut self,
        _layout: LayoutId,
        _amount: f64,
        _orientation: ResizeOrientation,
    ) {
    }

    fn toggle_tile_orientation(&mut self, _layout: LayoutId) {
        self.inner.toggle_tile_orientation(_layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::geometry::CGRectExt;

    fn w(index: u32) -> WindowId { WindowId::new(1, index) }
    fn rect(x: f64, y: f64) -> CGRect { CGRect::new(CGPoint::new(x, y), CGSize::new(200., 150.)) }
    fn fixture() -> (FloatingLayoutSystem, LayoutId) {
        let mut system = FloatingLayoutSystem::default();
        let layout = system.create_layout();
        for index in 1..=4 {
            system.add_window_after_selection(layout, w(index));
            system.store_frame(w(index), rect(20. + (index - 1) as f64 * 250., 40.));
        }
        system.select_window(layout, w(1));
        (system, layout)
    }
    fn stack(system: &mut FloatingLayoutSystem, layout: LayoutId) {
        system.apply_stacking_to_parent_of_selection(
            layout,
            crate::common::config::default_stack_orientation(),
        );
    }

    #[test]
    fn explicit_stacks_share_geometry_and_restore_only_their_members() {
        let (mut system, layout) = fixture();
        stack(&mut system, layout);
        assert_eq!(system.visible_windows_in_layout(layout).len(), 4);
        system.join_selection_with_direction(layout, Direction::Right);
        assert_ne!(system.frame(w(1)), system.frame(w(2)));
        stack(&mut system, layout);
        system.join_selection_with_direction(layout, Direction::Right);
        assert_eq!(system.cycle_windows(layout), vec![w(1), w(2), w(3)]);
        system.add_window_after_selection(layout, w(5));
        system.set_windows_for_app(layout, 1, (1..=6).map(w).collect());
        assert_eq!(system.visible_windows_in_layout(layout).len(), 4);
        system.select_window(layout, w(2));
        system.store_frame(w(2), rect(100., 80.));
        system.store_frame(w(1), rect(999., 999.));
        assert_eq!(system.frame(w(1)), Some(rect(100., 80.)));
        system.independent_frames.insert(w(4), rect(999., 999.));
        system.unjoin_selection(layout);
        assert_eq!(system.frame(w(2)), Some(rect(270., 40.)));
        assert_eq!(system.frame(w(4)), Some(rect(770., 40.)));
        system.select_window(layout, w(1));
        system.unstack_parent_of_selection(
            layout,
            crate::common::config::default_stack_orientation(),
        );
        assert_eq!(system.frame(w(1)), Some(rect(100., 80.)));
        assert_eq!(system.frame(w(3)), Some(rect(520., 40.)));
    }

    #[test]
    fn directional_focus_reaches_hidden_members_and_neighboring_windows() {
        let (mut system, layout) = fixture();
        system.join_selection_with_direction(layout, Direction::Right);
        assert!(system.ascend_selection(layout));
        stack(&mut system, layout);
        assert_eq!(system.selected_window(layout), Some(w(1)));
        system.store_frame(w(3), rect(20., 400.));
        for (direction, expected) in [
            (Direction::Down, w(2)),
            (Direction::Down, w(3)),
            (Direction::Up, w(2)),
            (Direction::Up, w(1)),
        ] {
            assert_eq!(system.move_focus(layout, direction).0, Some(expected));
        }
        system.remove_window(w(3));
        system.remove_window(w(4));
        assert_eq!(system.move_focus(layout, Direction::Right).0, Some(w(2)));
        assert_eq!(system.move_focus(layout, Direction::Right).0, Some(w(1)));
        system.unjoin_selection(layout);
        assert_eq!(system.all_windows_in_layout(layout).len(), 2);
        assert!(!system.parent_of_selection_is_stacked(layout));
    }

    #[test]
    fn geometry_initialization_is_bounded_and_only_happens_once() {
        let (mut system, layout) = fixture();
        system.frames.clear();
        let screen = CGRect::new(CGPoint::new(1000., 50.), CGSize::new(1000., 800.));
        system.spread_initial_windows();
        system.initialize_frames(layout, screen, &HashMap::default(), &|_| Some(screen));
        let before = system.frames.clone();
        assert!(before.values().all(|frame| screen.contains_rect(*frame)));
        assert_ne!(system.frame(w(1)), system.frame(w(2)));
        system.initialize_frames(layout, screen, &HashMap::default(), &|_| Some(screen));
        assert_eq!(system.frames, before);
        system.frames.clear();
        system.initialize_frames(layout, screen, &HashMap::default(), &|_| Some(rect(1020., 80.)));
        assert_eq!(system.frame(w(1)), Some(rect(1020., 80.)));
        system.toggle_fullscreen_of_selection(layout);
        let frames = system.calculate_layout(
            layout,
            screen,
            0.,
            &HashMap::default(),
            &Default::default(),
            0.,
            Default::default(),
            Default::default(),
        );
        assert_eq!(
            frames.iter().find(|(wid, _)| *wid == w(2)).unwrap().1,
            rect(1020., 80.)
        );
    }

    #[test]
    fn saved_stack_frames_survive_identity_changes_and_layout_clones() {
        let (mut system, layout) = fixture();
        system.join_selection_with_direction(layout, Direction::Right);
        stack(&mut system, layout);
        system.select_window(layout, w(3));
        system.join_selection_with_direction(layout, Direction::Right);
        stack(&mut system, layout);
        system.select_window(layout, w(1));
        system.join_selection_with_direction(layout, Direction::Right);
        assert_eq!(system.cycle_windows(layout), vec![w(1), w(2), w(3)]);
        assert_eq!(system.frame(w(4)), Some(rect(770., 40.)));
        let bytes = ron::to_string(&system).unwrap();
        let mut restored: FloatingLayoutSystem = ron::from_str(&bytes).unwrap();
        assert_eq!(restored.frames, system.frames);
        assert_eq!(restored.independent_frames, system.independent_frames);
        restored.replace_window(w(2), w(9));
        assert_eq!(restored.frame(w(9)), system.frame(w(2)));
        assert_eq!(restored.independent_frames[&w(9)], rect(270., 40.));
        let clone = restored.clone_layout(layout);
        restored.remove_layout(layout);
        assert!(restored.frame(w(9)).is_some());
        restored.remove_layout(clone);
        assert!(restored.frames.is_empty());
        assert!(restored.independent_frames.is_empty());
    }
}
