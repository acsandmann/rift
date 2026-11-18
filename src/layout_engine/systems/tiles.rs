use std::ops::ControlFlow;

use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};
use slotmap::Key;

use super::{EditResult, Focus, FocusResult, WindowChange, WindowQuery};
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::layout_engine::systems::constraints::{
    AxisConstraints, ConstraintCache, axis_slot, constrain_frame, container_axis_constraints,
    solve_axis_lengths, traditional_leaf_axis_constraints,
};
use crate::layout_engine::systems::{WindowLayoutConstraints, reconcile_app_membership};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation, ResizeOrientation};
use crate::model::selection::*;
use crate::model::tree::{
    LayoutInfo, NodeId, NodeMap, Placement, Tree, WindowIndex, WindowNodeInfoVec,
};
use crate::sys::geometry::Round;

fn perpendicular(orientation: Orientation) -> Orientation {
    match orientation {
        Orientation::Horizontal => Orientation::Vertical,
        Orientation::Vertical => Orientation::Horizontal,
    }
}

fn group_orientation(
    base: Orientation,
    configured: crate::common::config::StackDefaultOrientation,
) -> Orientation {
    use crate::common::config::StackDefaultOrientation::*;
    match configured {
        Same => base,
        Perpendicular => perpendicular(base),
        Horizontal => Orientation::Horizontal,
        Vertical => Orientation::Vertical,
    }
}

/// The forest owns roots. The saved tuple retains compatibility with named root guards.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(
    try_from = "(Option<NodeId>, String)",
    into = "(Option<NodeId>, &'static str)"
)]
pub(crate) struct LayoutRoot(pub(super) NodeId);

impl TryFrom<(Option<NodeId>, String)> for LayoutRoot {
    type Error = &'static str;

    fn try_from((root, _): (Option<NodeId>, String)) -> Result<Self, Self::Error> {
        root.map(Self).ok_or("saved layout has no root")
    }
}

impl From<LayoutRoot> for (Option<NodeId>, &'static str) {
    fn from(root: LayoutRoot) -> Self { (Some(root.0), "layout_root") }
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub(super) struct TileLayouts {
    #[serde(skip)]
    pub(super) inactive: HashSet<WindowId>,
    #[serde(deserialize_with = "deserialize_tile_tree")]
    pub(super) tree: Tree,
    pub(super) layout_roots: slotmap::SlotMap<LayoutId, LayoutRoot>,
}

pub(super) fn deserialize_tile_tree<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Tree, D::Error> {
    #[derive(Deserialize, Default)]
    struct LegacyLayout {
        info: slotmap::SecondaryMap<NodeId, LayoutInfo>,
    }
    #[derive(Deserialize)]
    struct StoredIndex {
        #[serde(default)]
        windows: slotmap::SecondaryMap<NodeId, WindowId>,
        window_nodes: crate::common::collections::BTreeMap<WindowId, WindowNodeInfoVec>,
    }
    #[derive(Deserialize)]
    struct StoredData {
        #[serde(default)]
        selection: Selection,
        #[serde(default)]
        layout: LegacyLayout,
        window: StoredIndex,
    }
    #[derive(Deserialize)]
    struct StoredTree {
        map: NodeMap,
        #[serde(default, deserialize_with = "super::present")]
        data: Option<StoredData>,
        #[serde(default, deserialize_with = "super::present")]
        window: Option<WindowIndex>,
    }
    let StoredTree { mut map, data, window } = StoredTree::deserialize(deserializer)?;
    let Some(data) = data else {
        return Ok(Tree {
            map,
            window: window.unwrap_or_default(),
        });
    };
    for (node, layout) in data.layout.info {
        map[node].layout = layout;
    }
    for (node, selection) in data.selection.nodes {
        *map[node].selection_mut() = Some(selection);
    }
    for (node, window) in data.window.windows {
        map[node].set_window(window);
    }
    Ok(Tree {
        map,
        window: WindowIndex {
            window_nodes: data.window.window_nodes,
        },
    })
}

pub(super) struct StackLayoutResult {
    container_rect: CGRect,
    stack_offset: f64,
    is_horizontal: bool,
    window_width: f64,
    window_height: f64,
}

impl StackLayoutResult {
    fn new(
        container_rect: CGRect,
        window_count: usize,
        stack_offset: f64,
        is_horizontal: bool,
    ) -> Self {
        let total_offset_space = if window_count > 0 {
            (window_count - 1) as f64 * stack_offset
        } else {
            0.0
        };
        let (window_width, window_height) = if is_horizontal {
            (
                (container_rect.size.width - total_offset_space).max(100.0),
                container_rect.size.height.max(100.0),
            )
        } else {
            (
                container_rect.size.width.max(100.0),
                (container_rect.size.height - total_offset_space).max(100.0),
            )
        };
        Self {
            container_rect,
            stack_offset,
            is_horizontal,
            window_width,
            window_height,
        }
    }

    pub(super) fn get_frame_for_index(&self, index: usize) -> CGRect {
        use objc2_core_foundation::{CGPoint, CGSize};
        let offset_amount = index as f64 * self.stack_offset;
        let (x_offset, y_offset) = if self.is_horizontal {
            (offset_amount, 0.0)
        } else {
            (0.0, offset_amount)
        };
        let container = &self.container_rect;
        let width = self.window_width.min(container.size.width);
        let height = self.window_height.min(container.size.height);
        let min_x = container.origin.x;
        let max_x = (container.origin.x + container.size.width - width).max(min_x);
        let min_y = container.origin.y;
        let max_y = (container.origin.y + container.size.height - height).max(min_y);
        CGRect {
            origin: CGPoint {
                x: (container.origin.x + x_offset).clamp(min_x, max_x),
                y: (container.origin.y + y_offset).clamp(min_y, max_y),
            },
            size: CGSize { width, height },
        }
        .round()
    }
}

impl NodeMap {
    pub(super) fn focus_target(
        &self,
        node: NodeId,
        participates: &impl Fn(WindowId) -> bool,
    ) -> Option<(NodeId, WindowId)> {
        if let Some(window) = self.window(node) {
            return participates(window).then_some((node, window));
        }
        let selected = self.last_selection(node);
        selected.and_then(|child| self.focus_target(child, participates)).or_else(|| {
            node.children(self)
                .filter(|&child| Some(child) != selected)
                .find_map(|child| self.focus_target(child, participates))
        })
    }

    pub(super) fn children_total(&self, parent: NodeId) -> f32 {
        parent.children(self).map(|child| self[child].layout.size.max(0.0)).sum()
    }

    // Leaf removal keeps sibling proportions while restoring their average weight to one.
    pub(crate) fn normalize_after_removal(&mut self, node: NodeId) {
        let parent = node.parent(self).unwrap();
        let siblings = || parent.children(self).filter(|&child| child != node);
        let (count, total) = siblings().fold((0, 0.0_f32), |(count, total), child| {
            (count + 1, total + self[child].layout.size.max(0.0))
        });
        let mut current = parent.first_child(self);
        while let Some(child) = current {
            current = child.next_sibling(self);
            if child == node {
                continue;
            }
            self[child].layout.size = if total.is_finite() && total > f32::EPSILON {
                self[child].layout.size * (count as f32 / total)
            } else {
                1.0
            };
        }
    }

    pub(crate) fn assume_size_of(&mut self, new: NodeId, old: NodeId) {
        assert_eq!(new.parent(self), old.parent(self));
        self[new].layout.size = core::mem::replace(&mut self[old].layout.size, 0.0);
    }

    pub(super) fn set_kind(&mut self, node: NodeId, kind: LayoutKind) {
        self[node].layout.kind = kind;
    }

    pub(super) fn kind(&self, node: NodeId) -> LayoutKind { self[node].layout.kind }

    fn proportion(&self, node: NodeId) -> Option<f64> {
        let Some(parent) = node.parent(self) else { return None };
        let total = self.children_total(parent);
        (total.is_finite() && total > f32::EPSILON)
            .then(|| f64::from(self[node].layout.size.max(0.0)) / f64::from(total))
    }

    fn take_share(&mut self, node: NodeId, from: NodeId, share: f32) {
        assert_eq!(node.parent(self), from.parent(self));
        const MIN_NODE_SIZE: f32 = 0.05;
        let share = share.min(self[from].layout.size - MIN_NODE_SIZE);
        let share = share.max(MIN_NODE_SIZE - self[node].layout.size);
        self[from].layout.size -= share;
        self[node].layout.size += share;
        let parent = node.parent(self).unwrap();
        let (count, total) = parent.children(self).fold((0, 0.0_f32), |(count, total), child| {
            (count + 1, total + self[child].layout.size.max(0.0))
        });
        if total > f32::EPSILON && total.is_finite() {
            let scale = count as f32 / total;
            let mut current = parent.first_child(self);
            while let Some(child) = current {
                current = child.next_sibling(self);
                self[child].layout.size *= scale;
            }
        }
    }

    fn is_effectively_fullscreen(&self, node: NodeId) -> bool {
        let info = &self[node].layout;
        info.is_fullscreen || info.is_fullscreen_within_gaps
    }

    fn debug(&self, node: NodeId, is_container: bool) -> String {
        let info = &self[node].layout;
        if is_container {
            format!(
                "{:?} [size {} total={}]",
                info.kind,
                info.size,
                self.children_total(node)
            )
        } else {
            format!("[size {}]", info.size)
        }
    }

    fn node_axis_constraints(
        &self,
        prepared: &ConstraintCache,
        node: NodeId,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        stack_offset: f64,
        stack_line: crate::common::config::StackLineReserve,
        gaps: &crate::common::config::GapSettings,
        horizontal: bool,
    ) -> (f64, Option<f64>, Option<f64>, bool) {
        if let Some(wid) = self.window(node) {
            if let Some(c) = constraints.get(&wid).copied() {
                return traditional_leaf_axis_constraints(c, horizontal);
            }
            return (0.0, None, None, true);
        }

        let kind = self[node].layout.kind;
        container_axis_constraints(
            node.children(self)
                .filter_map(|child| prepared.get(&child))
                .map(|axes| axes[horizontal as usize]),
            kind,
            horizontal,
            if horizontal {
                gaps.inner.horizontal
            } else {
                gaps.inner.vertical
            },
            stack_offset,
            stack_line.on_axis(kind, horizontal),
        )
    }
}

struct Projection<'a> {
    prepared: &'a ConstraintCache,
    map: &'a NodeMap,
    screen: CGRect,
    stack_offset: f64,
    constraints: &'a HashMap<WindowId, WindowLayoutConstraints>,
    gaps: &'a crate::common::config::GapSettings,
    stack_line: crate::common::config::StackLineReserve,
}

impl Projection<'_> {
    fn apply(&self, node: NodeId, rect: CGRect, sizes: &mut Vec<(WindowId, CGRect)>) {
        let Self {
            prepared,
            map,
            screen,
            stack_offset,
            constraints,
            gaps,
            stack_line,
        } = *self;
        if !prepared.contains_key(&node) {
            return;
        }
        let info = &map[node].layout;
        let rect = if info.is_fullscreen {
            screen
        } else if info.is_fullscreen_within_gaps {
            compute_tiling_area(screen, gaps)
        } else {
            rect
        };
        if let Some(wid) = map.window(node) {
            debug_assert!(
                node.children(map).next().is_none(),
                "non-leaf node with window id"
            );
            let final_rect = constraints.get(&wid).copied().map_or(rect, |c| {
                constrain_frame(rect, |horizontal| {
                    traditional_leaf_axis_constraints(c, horizontal)
                })
            });
            sizes.push((wid, final_rect));
            return;
        }
        use LayoutKind::*;
        match info.kind {
            HorizontalStack | VerticalStack => {
                let children: Vec<_> =
                    node.children(map).filter(|child| prepared.contains_key(child)).collect();
                if children.is_empty() {
                    return;
                }
                let is_horizontal = matches!(info.kind, HorizontalStack);
                let focused_child = map
                    .local_selection(node)
                    .filter(|child| prepared.contains_key(child))
                    .unwrap_or_else(|| children[0]);
                let focused_idx = children.iter().position(|&c| c == focused_child).unwrap_or(0);
                let (min, fixed, _, _) = prepared[&children[focused_idx]][is_horizontal as usize];
                let layout = stack_layout_result(
                    rect,
                    children.len(),
                    stack_offset_with_required_size(
                        rect,
                        is_horizontal,
                        children.len(),
                        stack_offset,
                        fixed.unwrap_or(min),
                    ),
                    is_horizontal,
                    stack_line,
                );
                for (idx, child) in children.into_iter().enumerate() {
                    self.apply(child, layout.get_frame_for_index(idx), sizes);
                }
            }
            Horizontal | Vertical => self.layout_axis(node, rect, sizes, info.kind == Horizontal),
        }
    }

    fn layout_axis(
        &self,
        node: NodeId,
        rect: CGRect,
        sizes: &mut Vec<(WindowId, CGRect)>,
        horizontal: bool,
    ) {
        let Self { prepared, map, gaps, .. } = *self;
        let children: Vec<_> =
            node.children(map).filter(|child| prepared.contains_key(child)).collect();
        if children.is_empty() {
            return;
        }
        let min_size = 0.05;
        let mut needs_normalization = false;
        let mut actual_total = 0.0;
        for &child in &children {
            let sz = map[child].layout.size;
            actual_total += sz;
            if !sz.is_finite() || sz < min_size - f32::EPSILON {
                needs_normalization = true;
            }
        }
        let normalize_sizes =
            !actual_total.is_finite() || actual_total <= f32::EPSILON || needs_normalization;
        let (mut offset, usable_axis, inner_gap) =
            axis_span(rect, horizontal, gaps, children.len());
        let axis_constraints: Vec<AxisConstraints> = children
            .iter()
            .map(|&child| {
                let (min, fixed, max, can_grow) = prepared[&child][horizontal as usize];
                AxisConstraints {
                    min,
                    fixed,
                    max,
                    weight: f64::from(if normalize_sizes {
                        1.0
                    } else {
                        map[child].layout.size.max(0.0)
                    }),
                    can_grow,
                }
            })
            .collect();
        let seg_lens = solve_axis_lengths(axis_constraints, usable_axis);
        for (child, seg_len) in children.into_iter().zip(seg_lens) {
            self.apply(
                child,
                axis_slot(rect, horizontal, offset, seg_len).round(),
                sizes,
            );
            offset += seg_len;
            offset += inner_gap;
        }
    }
}

fn axis_span(
    rect: CGRect,
    horizontal: bool,
    gaps: &crate::common::config::GapSettings,
    count: usize,
) -> (f64, f64, f64) {
    let (origin, length, gap) = if horizontal {
        (rect.origin.x, rect.size.width, gaps.inner.horizontal)
    } else {
        (rect.origin.y, rect.size.height, gaps.inner.vertical)
    };
    let usable = if gap == 0.0 {
        length
    } else {
        (length - count.saturating_sub(1) as f64 * gap).max(0.0)
    };
    (origin, usable, gap)
}

pub(super) fn stack_offset_with_required_size(
    rect: CGRect,
    horizontal: bool,
    count: usize,
    offset: f64,
    required: f64,
) -> f64 {
    if count <= 1 {
        return offset;
    }
    let axis = if horizontal {
        rect.size.width
    } else {
        rect.size.height
    };
    let required = required.clamp(0.0, axis.max(0.0));
    offset.min((axis - required).max(0.0) / (count - 1) as f64)
}

pub(super) fn stack_layout_result(
    rect: CGRect,
    child_count: usize,
    stack_offset: f64,
    is_horizontal: bool,
    stack_line: crate::common::config::StackLineReserve,
) -> StackLayoutResult {
    StackLayoutResult::new(
        if child_count > 1 {
            stack_line.inset(rect, is_horizontal)
        } else {
            rect
        },
        child_count,
        stack_offset,
        is_horizontal,
    )
}

impl TileLayouts {
    pub(super) fn update_existing(&mut self, change: WindowChange<'_>) {
        match change {
            WindowChange::Remove(window) | WindowChange::RemoveAndRebalance(window) => {
                let rebalance = matches!(change, WindowChange::RemoveAndRebalance(_));
                self.inactive.remove(&window);
                for node in self.tree.window.take_nodes_for(window) {
                    let parent = node.parent(self.map());
                    self.tree.remove(node);
                    if rebalance
                        && let Some(parent) = parent.filter(|&node| self.map().contains(node))
                    {
                        self.rebalance_node(parent);
                    }
                }
            }
            WindowChange::Replace(from, to) => {
                super::replace_participation(&mut self.inactive, from, to);
                self.tree.window.replace_window(&mut self.tree.map, from, to);
            }
            WindowChange::CloseApp(pid) => {
                self.inactive.retain(|w| w.pid != pid);
                for node in self.tree.window.take_nodes_for_app(pid) {
                    self.tree.remove(node);
                }
            }
            WindowChange::Resize {
                layout,
                window,
                old,
                new,
                screen,
                gaps,
            } => {
                let wid = window;
                let old_frame = old;
                let new_frame = new;

                if !self.window_participates(wid) {
                    return;
                }
                if let Some(node) = self.tree.window.node_for(layout, wid) {
                    let info = &mut self.tree.map[node].layout;
                    if !super::track_fullscreen_resize(
                        &mut info.is_fullscreen,
                        &mut info.is_fullscreen_within_gaps,
                        old_frame,
                        new_frame,
                        screen,
                        gaps,
                    ) {
                        self.set_frame_from_resize(node, old_frame, new_frame, screen);
                    }
                }
            }
            WindowChange::Add(..) | WindowChange::Reconcile(..) => {
                unreachable!("placement is owned by the layout")
            }
        }
    }

    pub(super) fn toggle_orientation(&mut self, layout: LayoutId) -> EditResult {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return EditResult::Unchanged;
        }
        use crate::layout_engine::LayoutKind;

        let map = self.map();
        let selection_node = self.selection(layout);

        let target_node = match selection_node.parent(map) {
            Some(p) => p,
            None => self.root(layout),
        };

        let current_kind = self.layout(target_node);

        if current_kind.is_group() {
            return EditResult::Unchanged;
        }

        let new_kind = match current_kind {
            LayoutKind::Horizontal => LayoutKind::Vertical,
            LayoutKind::Vertical => LayoutKind::Horizontal,
            other => other,
        };

        // Split weights are axis-independent. Rotating a container should preserve the
        // user's ratios; `rebalance` is the explicit operation that resets them.
        self.set_layout(target_node, new_kind);
        EditResult::Changed
    }

    pub(super) fn resize_selection(
        &mut self,
        layout: LayoutId,
        amount: f64,
        orientation: ResizeOrientation,
    ) -> EditResult {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return EditResult::Unchanged;
        }
        if amount == 0.0 {
            return EditResult::Unchanged;
        }
        let selection = self.selection(layout);
        if self.window_at(selection).is_none() {
            return EditResult::Unchanged;
        }
        let changed = match orientation {
            ResizeOrientation::Horizontal => [Direction::Right, Direction::Left]
                .into_iter()
                .any(|direction| self.resize_internal(selection, amount, direction)),
            ResizeOrientation::Vertical => [Direction::Down, Direction::Up]
                .into_iter()
                .any(|direction| self.resize_internal(selection, amount, direction)),
            ResizeOrientation::Smart => {
                let mut node = selection;
                while let Some(parent) = node.parent(self.map()) {
                    let kind = self.layout(parent);
                    if !kind.is_group() {
                        let directions = match kind.orientation() {
                            Orientation::Horizontal => [Direction::Right, Direction::Left],
                            Orientation::Vertical => [Direction::Down, Direction::Up],
                        };
                        if directions
                            .into_iter()
                            .any(|direction| self.resize_internal(node, amount, direction))
                        {
                            return EditResult::Changed;
                        }
                    }
                    node = parent;
                }
                false
            }
        };
        EditResult::from(changed)
    }

    pub(super) fn toggle_fullscreen(
        &mut self,
        layout: LayoutId,
        within_gaps: bool,
    ) -> Vec<WindowId> {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return Vec::new();
        }
        let node = self.selection(layout);
        let info = &mut self.tree.map[node].layout;
        if super::toggle_fullscreen_mode(
            &mut info.is_fullscreen,
            &mut info.is_fullscreen_within_gaps,
            within_gaps,
        ) {
            self.visible_windows_under_internal(node)
        } else {
            Vec::new()
        }
    }

    pub(super) fn split_selection(&mut self, layout: LayoutId, kind: LayoutKind) -> EditResult {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return EditResult::Unchanged;
        }
        let selection = self.selection(layout);
        self.nest_in_container_internal(layout, selection, kind);
        EditResult::Changed
    }

    pub(super) fn transfer_selection(&mut self, layout: LayoutId, to: LayoutId) -> EditResult {
        if layout == to || !self.contains_layout(layout) || !self.contains_layout(to) {
            return EditResult::Unchanged;
        }
        let from_sel = self.selection(layout);
        let to_sel = self.selection(to);
        if self
            .windows_from(from_sel, Some(from_sel), true)
            .any(|window| self.contains_window(to, window))
        {
            return EditResult::Unchanged;
        }

        let map = &self.tree.map;
        let Some(old_parent) = from_sel.parent(map) else {
            return EditResult::Unchanged;
        };
        let is_selection = self.tree.map.local_selection(old_parent) == Some(from_sel);
        if to_sel.parent(self.map()).is_none() {
            self.tree.move_node(from_sel, Placement::Back(to_sel));
        } else {
            self.tree.move_node(from_sel, Placement::After(to_sel));
        }
        self.tree.window.reassign_subtree(&self.tree.map, from_sel, to);
        if is_selection {
            let mut current = Some(from_sel);
            while let Some(node) = current.filter(|&node| node != old_parent) {
                current = node.parent(&self.tree.map);
                self.tree.map.select_locally(node);
            }
        };
        EditResult::Changed
    }

    pub(super) fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        self.container_tree_with_roles(layout, &HashMap::default())
    }

    pub(super) fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        crate::layout_engine::systems::node_slot(
            self.window_node(layout, window)?,
            self.root(layout),
            self.map(),
        )
    }

    fn get_ascii_tree_with_labels(
        &self,
        node: NodeId,
        labels: Option<&HashMap<NodeId, &'static str>>,
    ) -> ascii_tree::Tree {
        let status = match node.parent(&self.tree.map) {
            None => "",
            Some(parent) if self.tree.map.local_selection(parent) == Some(node) => "☒ ",
            _ => "☐ ",
        };
        let desc = format!("{status}{node:?}");
        let desc = match self.window_at(node) {
            Some(wid) => format!("{desc} {:?} {}", wid, self.tree.map.debug(node, false)),
            None => format!("{desc} {}", self.tree.map.debug(node, true)),
        };
        let desc = if let Some(label) = labels.and_then(|labels| labels.get(&node).copied()) {
            format!("{desc} [{label}]")
        } else {
            desc
        };
        let children: Vec<_> = node
            .children(&self.tree.map)
            .map(|c| self.get_ascii_tree_with_labels(c, labels))
            .collect();
        if children.is_empty() {
            ascii_tree::Tree::Leaf(vec![desc])
        } else {
            ascii_tree::Tree::Node(desc, children)
        }
    }

    pub(super) fn add_window_under(
        &mut self,
        layout: LayoutId,
        parent: NodeId,
        wid: WindowId,
    ) -> NodeId {
        let node = self.tree.insert(Placement::Back(parent));
        self.tree.window.set_window(&mut self.tree.map, layout, node, wid);
        node
    }

    pub(super) fn window_at(&self, node: NodeId) -> Option<WindowId> { self.tree.map.window(node) }

    pub(super) fn container_tree_with_roles(
        &self,
        layout: LayoutId,
        roles: &HashMap<NodeId, &'static str>,
    ) -> rift_protocol::ContainerTreeNode {
        fn snapshot(
            system: &TileLayouts,
            node: NodeId,
            selected: NodeId,
            roles: &HashMap<NodeId, &'static str>,
        ) -> rift_protocol::ContainerTreeNode {
            let window = system.window_at(node);
            let info = system.tree.map[node].layout;
            rift_protocol::ContainerTreeNode {
                node_id: node.data().as_ffi(),
                node_type: if window.is_some() {
                    rift_protocol::ContainerNodeType::Window
                } else {
                    rift_protocol::ContainerNodeType::Container
                },
                frame: Default::default(),
                layout_kind: window.is_none().then_some(info.kind),
                weight: node.parent(system.map()).map(|_| f64::from(info.size)),
                window_id: window.map(Into::into),
                is_selected: node == selected,
                is_fullscreen: info.is_fullscreen,
                is_fullscreen_within_gaps: info.is_fullscreen_within_gaps,
                role: roles.get(&node).map(|role| (*role).to_owned()),
                pending_split: None,
                children: node
                    .children(system.map())
                    .map(|child| snapshot(system, child, selected, roles))
                    .collect(),
            }
        }

        snapshot(self, self.root(layout), self.selection(layout), roles)
    }

    pub(super) fn draw_tree(
        &self,
        layout: LayoutId,
        labels: Option<&HashMap<NodeId, &'static str>>,
    ) -> String {
        let tree = self.get_ascii_tree_with_labels(self.root(layout), labels);
        let mut out = String::new();
        ascii_tree::write_tree(&mut out, &tree).unwrap();
        out
    }

    pub(super) fn window_in_direction_from(
        &self,
        node: NodeId,
        direction: Direction,
    ) -> Option<WindowId> {
        if let Some(window) = self.window_at(node) {
            return self.window_participates(window).then_some(window);
        }
        // Along the movement axis, enter at the near edge. Across a split,
        // reuse its selected branch so a return trip retraces the focus path.
        // Stacks always present their selected member.
        let kind = self.tree.map.kind(node);
        let selected = (kind.is_stacked() || kind.orientation() != direction.orientation())
            .then(|| self.tree.map.local_selection(node))
            .flatten();
        if let Some(window) =
            selected.and_then(|child| self.window_in_direction_from(child, direction))
        {
            return Some(window);
        }
        let alternative = |child: &NodeId| Some(*child) != selected;
        match direction {
            Direction::Left | Direction::Up => node
                .children_rev(self.map())
                .filter(alternative)
                .find_map(|child| self.window_in_direction_from(child, direction)),
            Direction::Right | Direction::Down => node
                .children(self.map())
                .filter(alternative)
                .find_map(|child| self.window_in_direction_from(child, direction)),
        }
    }

    pub(super) fn rebalance_node(&mut self, node: NodeId) {
        let map = &self.tree.map;
        let children: Vec<_> = node.children(map).collect();
        if children.is_empty() {
            return;
        }

        for &child in &children {
            self.tree.map[child].layout.size = 1.0;
        }
        for child in children {
            self.rebalance_node(child);
        }
    }

    pub(super) fn select(&mut self, selection: NodeId) { self.tree.map.select(selection) }

    pub(super) fn traverse_internal(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let map = &self.tree.map;
        if let Some(sibling) = self.move_over(from, direction) {
            return Some(sibling);
        }
        let node = from.ancestors(map).skip(1).find_map(|ancestor| {
            if let Some(target) = self.move_over(ancestor, direction) {
                Some(self.descend_into_target(target, direction, map))
            } else {
                None
            }
        });
        node.flatten()
    }

    pub(super) fn descend_into_target(
        &self,
        target: NodeId,
        direction: Direction,
        map: &NodeMap,
    ) -> Option<NodeId> {
        let active = self.participating_nodes(target);
        let participates = |child: &NodeId| self.inactive.is_empty() || active.contains(child);
        let mut current = target;
        loop {
            let kind = self.tree.map.kind(current);
            let selected = self.tree.map.local_selection(current).filter(participates);
            let edge = || match direction {
                Direction::Left | Direction::Up => current.children(map).find(participates),
                Direction::Right | Direction::Down => current.children_rev(map).find(participates),
            };
            let next = if kind.is_stacked() {
                selected.or_else(|| current.children(map).find(participates))
            } else {
                selected.or_else(edge)
            };
            match next {
                Some(child) => current = child,
                None => return Some(current),
            }
        }
    }

    pub(super) fn visible_windows_under_internal(&self, node: NodeId) -> Vec<WindowId> {
        let mut windows = Vec::new();
        self.visit_visible_windows(node, &mut |_, window| {
            windows.push(window);
            ControlFlow::<()>::Continue(())
        })
        .continue_value();
        windows
    }

    pub(super) fn first_visible_window(&self, node: NodeId) -> Option<(NodeId, WindowId)> {
        self.visit_visible_windows(node, &mut |node, window| ControlFlow::Break((node, window)))
            .break_value()
    }

    pub(super) fn visit_visible_windows<B>(
        &self,
        node: NodeId,
        visit: &mut impl FnMut(NodeId, WindowId) -> ControlFlow<B>,
    ) -> ControlFlow<B, bool> {
        if let Some(window) = self.window_at(node) {
            let active = self.window_participates(window);
            if active {
                visit(node, window)?;
            }
            return ControlFlow::Continue(active);
        }
        let grouped = self.layout(node).is_group();
        let selected = grouped.then(|| self.tree.map.local_selection(node)).flatten();
        if let Some(selected) = selected {
            let visited = self.visit_visible_windows(selected, visit)?;
            if visited || self.inactive.is_empty() {
                return ControlFlow::Continue(visited);
            }
        }
        let mut visited = false;
        for child in node.children(self.map()).filter(|&child| Some(child) != selected) {
            visited |= self.visit_visible_windows(child, visit)?;
            if grouped && (visited || self.inactive.is_empty()) {
                break;
            }
        }
        ControlFlow::Continue(visited)
    }

    pub(super) fn move_over(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let Some(parent) = from.parent(&self.tree.map) else {
            return None;
        };
        if self.tree.map.kind(parent).orientation() != direction.orientation() {
            return None;
        }
        let sibling = |node: NodeId| match direction {
            Direction::Left | Direction::Up => node.prev_sibling(&self.tree.map),
            Direction::Right | Direction::Down => node.next_sibling(&self.tree.map),
        };
        let mut next = sibling(from);
        while let Some(node) = next {
            if self.inactive.is_empty() || self.find_best_focus_target(node).is_some() {
                return Some(node);
            }
            next = sibling(node);
        }
        None
    }

    pub(super) fn resize_internal(
        &mut self,
        node: NodeId,
        screen_ratio: f64,
        direction: Direction,
    ) -> bool {
        let can_resize = |&node: &NodeId| -> bool {
            let Some(parent) = node.parent(&self.tree.map) else {
                return false;
            };
            !self.tree.map.kind(parent).is_group() && self.move_over(node, direction).is_some()
        };
        let Some(resizing_node) = node.ancestors(&self.tree.map).find(can_resize) else {
            return false;
        };
        let sibling = self.move_over(resizing_node, direction).unwrap();
        let exchange_rate = resizing_node
            .ancestors(&self.tree.map)
            .skip(1)
            .try_fold(1.0, |r, node| match node.parent(&self.tree.map) {
                Some(parent)
                    if self.tree.map.kind(parent).orientation() == direction.orientation()
                        && !self.tree.map.kind(parent).is_group() =>
                {
                    self.tree.map.proportion(node).map(|p| r * p)
                }
                _ => Some(r),
            })
            .unwrap_or(1.0);
        let local_ratio = f64::from(screen_ratio)
            * f64::from(
                self.tree.map.children_total(resizing_node.parent(&self.tree.map).unwrap()),
            )
            / exchange_rate;
        self.tree.map.take_share(resizing_node, sibling, local_ratio as f32);
        true
    }

    pub(super) fn can_resize_towards(&self, node: NodeId, direction: Direction) -> bool {
        let can_resize = |candidate: NodeId| -> bool {
            let Some(parent) = candidate.parent(&self.tree.map) else {
                return false;
            };
            !self.tree.map.kind(parent).is_group() && self.move_over(candidate, direction).is_some()
        };
        node.ancestors(&self.tree.map).any(can_resize)
    }

    pub(super) fn set_frame_from_resize(
        &mut self,
        node: NodeId,
        old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
    ) {
        const RESIZE_DELTA_EPSILON: f64 = 1.0;
        let choose_edge = |mut edges: [(Direction, f64); 2]| {
            for (_, delta) in &mut edges {
                if delta.abs() < RESIZE_DELTA_EPSILON {
                    *delta = 0.0;
                }
            }
            if !(edges[0].1.abs() >= edges[1].1.abs()) {
                edges.swap(0, 1);
            }
            edges.into_iter().find(|&(direction, delta)| {
                delta != 0.0 && self.can_resize_towards(node, direction)
            })
        };
        let horizontal = choose_edge([
            (Direction::Left, old_frame.min().x - new_frame.min().x),
            (Direction::Right, new_frame.max().x - old_frame.max().x),
        ]);
        let vertical = choose_edge([
            (Direction::Up, old_frame.min().y - new_frame.min().y),
            (Direction::Down, new_frame.max().y - old_frame.max().y),
        ]);
        for (edge, extent) in [
            (horizontal, screen.size.width),
            (vertical, screen.size.height),
        ] {
            if let Some((direction, delta)) = edge {
                self.resize_internal(node, delta / extent, direction);
            }
        }
    }

    pub(super) fn nest_in_container_internal(
        &mut self,
        layout: LayoutId,
        node: NodeId,
        kind: LayoutKind,
    ) -> NodeId {
        let old_parent = node.parent(&self.tree.map);
        let parent = if node.prev_sibling(&self.tree.map).is_none()
            && node.next_sibling(&self.tree.map).is_none()
            && old_parent.is_some()
        {
            old_parent.unwrap()
        } else {
            let new_parent = if let Some(old_parent) = old_parent {
                let is_selection = self.tree.map.local_selection(old_parent) == Some(node);
                let new_parent = self.tree.insert(Placement::Before(node));
                self.tree.map.assume_size_of(new_parent, node);
                self.tree.move_node(node, Placement::Back(new_parent));
                if is_selection {
                    self.tree.map.select_locally(new_parent);
                }
                new_parent
            } else {
                let new_root = self.tree.create_root();
                self.layout_roots[layout] = LayoutRoot(new_root);
                self.tree.move_node(node, Placement::Back(new_root));
                new_root
            };
            self.tree.map.select_locally(node);
            new_parent
        };
        self.tree.map.set_kind(parent, kind);
        parent
    }

    pub(super) fn stack_group_container_info(
        &self,
        node: NodeId,
        kind: crate::layout_engine::LayoutKind,
        rect: CGRect,
        children: &[NodeId],
        selected_index: usize,
    ) -> crate::layout_engine::engine::GroupContainerInfo {
        // One representative per non-empty child, in child order: left to right for a
        // horizontal stack, top to bottom for a vertical one.
        let mut window_ids = Vec::with_capacity(children.len());
        let mut representative_selection = 0;
        for (i, &child) in children.iter().enumerate() {
            if let Some((_, wid)) = self.find_best_focus_target(child) {
                if i == selected_index {
                    representative_selection = window_ids.len();
                }
                window_ids.push(wid);
            }
        }
        let selected_index = representative_selection;

        crate::layout_engine::engine::GroupContainerInfo {
            node_id: node,
            container_kind: kind,
            frame: rect,
            total_count: window_ids.len(),
            selected_index,
            window_ids,
        }
    }

    fn child_frames_in_axis<'a>(
        &'a self,
        parent_rect: CGRect,
        siblings: &'a [NodeId],
        horizontal: bool,
        gaps: &crate::common::config::GapSettings,
    ) -> impl Iterator<Item = CGRect> + 'a {
        let total: f32 = siblings.iter().map(|&child| self.tree.map[child].layout.size).sum();
        let (offset, usable_axis, inner_gap) =
            axis_span(parent_rect, horizontal, gaps, siblings.len());

        siblings.iter().scan(offset, move |offset, &child| {
            let ratio = f64::from(self.tree.map[child].layout.size) / f64::from(total);
            let seg_len = usable_axis * ratio;
            let frame = axis_slot(parent_rect, horizontal, *offset, seg_len);
            *offset += seg_len;
            *offset += inner_gap;
            Some(frame)
        })
    }

    pub(super) fn collect_group_containers(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
        selection_path_only: bool,
    ) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
        use crate::layout_engine::LayoutKind::*;

        let mut out = Vec::new();
        let map = &self.tree.map;
        let active = self.participating_nodes(self.root(layout));

        let tiling_area = compute_tiling_area(screen, gaps);

        let mut stack: Vec<(NodeId, CGRect)> = vec![(self.root(layout), tiling_area)];

        while let Some((node, rect)) = stack.pop() {
            if self.tree.map.is_effectively_fullscreen(node) {
                if selection_path_only {
                    out.clear();
                    break;
                }
                continue;
            }

            let kind = self.tree.map.kind(node);
            let children: Vec<_> = node
                .children(map)
                .filter(|c| self.inactive.is_empty() || active.contains(c))
                .collect();

            let selected = self
                .tree
                .map
                .local_selection(node)
                .filter(|c| self.inactive.is_empty() || active.contains(c))
                .or_else(|| children.first().copied());

            if matches!(kind, HorizontalStack | VerticalStack) {
                if children.is_empty() {
                    continue;
                }

                let local_sel = selected.unwrap();
                if selection_path_only && self.tree.map.is_effectively_fullscreen(local_sel) {
                    out.clear();
                    break;
                }
                let selected_index = children.iter().position(|&c| c == local_sel).unwrap_or(0);

                let is_horizontal = matches!(kind, HorizontalStack);
                out.push(self.stack_group_container_info(
                    node,
                    kind,
                    rect,
                    &children,
                    selected_index,
                ));

                let layout_res = stack_layout_result(
                    rect,
                    children.len(),
                    stack_offset,
                    is_horizontal,
                    stack_line,
                );

                for (i, &child) in children.iter().enumerate().rev() {
                    if (selection_path_only && Some(child) != selected)
                        || self.tree.map.is_effectively_fullscreen(child)
                    {
                        continue;
                    }
                    let child_rect = layout_res.get_frame_for_index(i);
                    stack.push((child, child_rect));
                }

                continue;
            }

            // Calculate sibling frames once in forward order, preserving the exact
            // accumulation used by selection-path geometry. Reverse only the work
            // queue so the depth-first traversal still visits children in order.
            let start = stack.len();
            stack.extend(
                children
                    .iter()
                    .copied()
                    .zip(self.child_frames_in_axis(rect, &children, kind == Horizontal, gaps))
                    .filter(|(child, _)| !selection_path_only || Some(*child) == selected),
            );
            stack[start..].reverse();
        }

        out
    }

    pub(super) fn stack_members(&self, layout: LayoutId, window: WindowId) -> Vec<WindowId> {
        let Some(parent) =
            self.window_node(layout, window).and_then(|node| node.parent(self.map()))
        else {
            return Vec::new();
        };
        if !self.layout(parent).is_group() {
            return Vec::new();
        }
        parent.children(self.map()).filter_map(|node| self.window_at(node)).collect()
    }

    pub(super) fn descend_selection(&mut self, layout: LayoutId) -> bool {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return false;
        }
        if let Some(child) = self.tree.map.last_selection(self.selection(layout)) {
            self.select(child);
            return true;
        }
        false
    }

    pub(super) fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        let selection = self.selection(layout);
        if let Some(new_node) = self.traverse_internal(selection, direction) {
            let focus_target = self.find_best_focus_target(new_node);
            let Some((focus_node, focus_window)) = focus_target else {
                return (None, vec![]);
            };
            let mut highest_revealed = focus_node;
            let mut node = focus_node;
            while let Some(parent) = node.parent(&self.tree.map) {
                let is_group = self.layout(parent).is_group();
                if self.tree.map.select_locally(node) && is_group {
                    highest_revealed = node;
                }
                node = parent;
            }
            let raise_windows = self.visible_windows_under_internal(highest_revealed);
            (Some(focus_window), raise_windows)
        } else {
            (None, vec![])
        }
    }

    pub(super) fn apply_stacking_to_parent_of_selection(
        &mut self,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return Vec::new();
        }
        let selection = self.selection(layout);
        let container = if self.window_at(selection).is_some() {
            selection.parent(self.map())
        } else {
            Some(selection)
        };
        let Some(container) = container else { return Vec::new() };
        let kind = self.layout(container);
        let orientation = if kind.is_stacked() {
            perpendicular(kind.orientation())
        } else {
            group_orientation(kind.orientation(), default_orientation)
        };
        self.set_layout(container, match orientation {
            Orientation::Horizontal => LayoutKind::HorizontalStack,
            Orientation::Vertical => LayoutKind::VerticalStack,
        });
        if let Some(first) = container.first_child(self.map()) {
            self.select(first);
        }
        self.visible_windows_under_internal(container)
    }

    pub(super) fn stacked_selection_container(&self, layout: LayoutId) -> Option<NodeId> {
        let selection = self.selection(layout);
        let stacked = |&node: &NodeId| self.layout(node).is_stacked();
        if self.window_at(selection).is_some() {
            selection.ancestors(self.map()).skip(1).find(stacked)
        } else if stacked(&selection) {
            Some(selection)
        } else {
            selection.children(self.map()).find(stacked)
        }
    }

    pub(super) fn unstack_parent_of_selection(
        &mut self,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return Vec::new();
        }
        let Some(container) = self.stacked_selection_container(layout) else {
            return Vec::new();
        };
        let orientation =
            group_orientation(self.layout(container).orientation(), default_orientation);
        self.set_layout(container, match orientation {
            Orientation::Horizontal => LayoutKind::Horizontal,
            Orientation::Vertical => LayoutKind::Vertical,
        });
        self.visible_windows_under_internal(container)
    }

    pub(super) fn move_stack_tab(
        &mut self,
        layout: LayoutId,
        window: WindowId,
        target: WindowId,
    ) -> bool {
        if !self.window_participates(window) || !self.window_participates(target) {
            return false;
        }
        let map = &self.tree.map;
        let (Some(source), Some(target)) = (
            self.tree.window.node_for(layout, window),
            self.tree.window.node_for(layout, target),
        ) else {
            return false;
        };
        // A tab can hold a nested container. Find the two children directly under their stack.
        let Some(stack) = source.common_ancestor(target, map) else {
            return false;
        };
        if !self.layout(stack).is_stacked() {
            return false;
        }
        let tab = |node: NodeId| node.ancestors(map).find(|node| node.parent(map) == Some(stack));
        let (Some(moving), Some(anchor)) = (tab(source), tab(target)) else {
            return false;
        };
        if moving == anchor {
            return false;
        }
        let forward = stack.children(map).position(|c| c == moving)
            < stack.children(map).position(|c| c == anchor);
        let place = if forward {
            Placement::After(anchor)
        } else {
            Placement::Before(anchor)
        };
        self.tree.move_node(moving, place);
        true
    }

    pub(super) fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        if !self.window_participates(a) || !self.window_participates(b) {
            return false;
        }
        let node_a = match self.tree.window.node_for(layout, a) {
            Some(n) => n,
            None => return false,
        };
        let node_b = match self.tree.window.node_for(layout, b) {
            Some(n) => n,
            None => return false,
        };

        if node_a == node_b {
            return false;
        }

        let selection = self.selection(layout);
        // Exchange the occupants of the slots without restructuring the tree.
        // Reparenting a leaf can collapse a single-child container mid-swap.
        let index = &mut self.tree.window;
        self.tree.map[node_a].set_window(b);
        self.tree.map[node_b].set_window(a);
        for (wid, from, to) in [(a, node_a, node_b), (b, node_b, node_a)] {
            let info = index
                .window_nodes
                .get_mut(&wid)
                .unwrap()
                .0
                .iter_mut()
                .find(|info| info.layout == layout && info.node == from)
                .unwrap();
            info.node = to;
        }
        self.select(if selection == node_a {
            node_b
        } else if selection == node_b {
            node_a
        } else {
            selection
        });

        true
    }

    pub(super) fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.tree
            .window
            .window_nodes
            .range(WindowId::new(pid, 1)..=WindowId::new(pid, u32::MAX))
            .any(|(_, nodes)| nodes.0.iter().any(|info| info.layout == layout))
    }

    pub(super) fn window_participates(&self, window: WindowId) -> bool {
        !self.inactive.contains(&window)
    }

    pub(super) fn set_window_participation(
        &mut self,
        window: WindowId,
        participates: bool,
    ) -> bool {
        super::update_participation(&mut self.inactive, window, participates)
    }

    pub(super) fn create_layout(&mut self) -> LayoutId {
        let root = self.tree.create_root();
        self.layout_roots.insert(LayoutRoot(root))
    }

    pub(super) fn contains_layout(&self, layout: LayoutId) -> bool {
        self.layout_roots.contains_key(layout)
    }

    pub(super) fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let root = self.root(layout);
        self.layout_roots
            .insert_with_key(|destination| LayoutRoot(self.tree.clone_branch(root, destination)))
    }

    pub(super) fn remove_layout(&mut self, layout: LayoutId) {
        self.tree.remove(self.layout_roots.remove(layout).unwrap().0)
    }

    pub(super) fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        let selection = self.tree.map.current_selection(self.root(layout));
        if let Some(window) = self.window_at(selection).filter(|w| !self.inactive.contains(w)) {
            return Some(window);
        }
        self.find_best_focus_target(selection)
            .or_else(|| self.find_best_focus_target(self.root(layout)))
            .map(|(_, window)| window)
    }

    pub(super) fn selection_contains_window(&self, layout: LayoutId, window: WindowId) -> bool {
        let selection = self.selection(layout);
        self.window_node(layout, window)
            .is_some_and(|node| node.ancestors(self.map()).any(|ancestor| ancestor == selection))
    }

    pub(super) fn window_in_direction(
        &self,
        layout: LayoutId,
        direction: Direction,
    ) -> Option<WindowId> {
        self.window_in_direction_from(self.root(layout), direction)
    }

    pub(super) fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.tree.window.node_for(layout, wid).is_some()
    }

    pub(super) fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        let root = self.root(layout);
        let active = self.participating_nodes(root);
        root.traverse_preorder(&self.tree.map).any(|node| {
            (self.inactive.is_empty() || active.contains(&node))
                && self.tree.map.is_effectively_fullscreen(node)
        })
    }

    pub(super) fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        self.stacked_selection_container(layout).is_some()
    }

    pub(super) fn windows(&self, layout: LayoutId, query: WindowQuery) -> Vec<WindowId> {
        match query {
            WindowQuery::All => self.windows_with_nodes(layout).map(|(_, window)| window).collect(),
            WindowQuery::Visible => {
                let root = self.root(layout);
                self.visible_windows_under_internal(root)
            }
            WindowQuery::Selection => {
                let selection = self.selection(layout);
                self.visible_windows_under_internal(selection)
            }
            WindowQuery::Stack(window) => self.stack_members(layout, window),
            WindowQuery::App(pid) => self
                .windows_with_nodes(layout)
                .filter_map(|(_, window)| (window.pid == pid).then_some(window))
                .collect(),
        }
    }

    pub(super) fn focus(&mut self, layout: LayoutId, focus: Focus) -> FocusResult {
        match focus {
            Focus::Select(window) => FocusResult::selection({
                let wid = window;

                if self.inactive.contains(&wid) {
                    return FocusResult::selection(false);
                }
                if let Some(node) = self.tree.window.node_for(layout, wid) {
                    self.select(node);
                    true
                } else {
                    false
                }
            }),
            Focus::Ascend => FocusResult::selection({
                if !self.inactive.is_empty()
                    && self.find_best_focus_target(self.root(layout)).is_none()
                {
                    return FocusResult::selection(false);
                }
                if let Some(parent) = self.selection(layout).parent(self.map()) {
                    self.select(parent);
                    return FocusResult::selection(true);
                }
                false
            }),
            Focus::Descend => FocusResult::selection(self.descend_selection(layout)),
            Focus::Direction(direction) => {
                FocusResult::movement(self.move_focus(layout, direction))
            }
        }
    }

    pub(super) fn participating_nodes(&self, root: NodeId) -> HashSet<NodeId> {
        let mut active = HashSet::default();
        if self.inactive.is_empty() {
            return active;
        }
        for node in root.traverse_postorder(self.map()) {
            if self.window_at(node).is_some_and(|w| !self.inactive.contains(&w))
                || node.children(self.map()).any(|child| active.contains(&child))
            {
                active.insert(node);
            }
        }
        active
    }

    pub(super) fn find_best_focus_target(&self, node: NodeId) -> Option<(NodeId, WindowId)> {
        self.tree.map.focus_target(node, &|window| self.window_participates(window))
    }

    pub(super) fn root(&self, layout: LayoutId) -> NodeId { self.layout_roots[layout].0 }

    pub(super) fn selection(&self, layout: LayoutId) -> NodeId {
        let selection = self.tree.map.current_selection(self.root(layout));
        if !self.inactive.is_empty() && self.find_best_focus_target(selection).is_none() {
            return self
                .find_best_focus_target(self.root(layout))
                .map_or(self.root(layout), |(node, _)| node);
        }
        selection
    }

    pub(super) fn map(&self) -> &NodeMap { &self.tree.map }

    pub(super) fn local_selection(&self, node: NodeId) -> Option<NodeId> {
        self.tree
            .map
            .local_selection(node)
            .or_else(|| self.tree.map.last_selection(node))
    }

    pub(super) fn layout(&self, node: NodeId) -> LayoutKind { self.tree.map.kind(node) }

    pub(super) fn layouts_for_window(&self, wid: WindowId) -> Vec<LayoutId> {
        self.tree.window.layouts_for(wid)
    }

    pub(super) fn window_node(&self, layout: LayoutId, wid: WindowId) -> Option<NodeId> {
        self.tree.window.node_for(layout, wid)
    }

    pub(super) fn windows_with_nodes(
        &self,
        layout: LayoutId,
    ) -> impl Iterator<Item = (NodeId, WindowId)> + '_ {
        self.root(layout)
            .traverse_preorder(self.map())
            .filter_map(|node| self.window_at(node).map(|window| (node, window)))
    }

    pub(super) fn windows_from(
        &self,
        region: NodeId,
        start: Option<NodeId>,
        forward: bool,
    ) -> impl Iterator<Item = WindowId> + '_ {
        let map = self.map();
        std::iter::successors(start, move |&node| {
            if forward {
                node.next_in_subtree(region, map)
            } else {
                node.prev_in_subtree(region, map)
            }
        })
        .filter_map(|node| self.window_at(node))
    }

    pub(super) fn contains_any_window(&self, wid: WindowId) -> bool {
        self.tree.window.window_nodes.contains_key(&wid)
    }

    pub(super) fn window_is_visible(&self, layout: LayoutId, wid: WindowId) -> bool {
        let Some(node) = self.window_node(layout, wid) else {
            return false;
        };
        node.ancestors_with_parent(self.map()).all(|(child, parent)| {
            parent.is_none_or(|parent| {
                !self.layout(parent).is_stacked()
                    || self.tree.map.local_selection(parent) == Some(child)
            })
        })
    }

    pub(super) fn reconcile_windows(
        &mut self,
        layout: LayoutId,
        pid: pid_t,
        desired: Vec<WindowId>,
    ) -> Vec<WindowId> {
        let delta = reconcile_app_membership(
            pid,
            self.windows(layout, WindowQuery::App(pid)),
            desired,
            |window| self.window_participates(window),
        );
        for window in delta.removals {
            if let Some(node) = self.window_node(layout, window)
                && !self.tree.map[node].layout.is_fullscreen
            {
                self.tree.remove(node);
            }
        }
        delta.additions
    }

    pub(super) fn fullscreen_frame(
        &self,
        node: NodeId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) -> Option<CGRect> {
        node.ancestors(self.map()).find_map(|node| {
            let info = &self.tree.map[node].layout;
            if info.is_fullscreen {
                Some(screen)
            } else if info.is_fullscreen_within_gaps {
                Some(compute_tiling_area(screen, gaps))
            } else {
                None
            }
        })
    }

    pub(super) fn set_layout(&mut self, node: NodeId, kind: LayoutKind) {
        self.tree.map.set_kind(node, kind);
    }

    pub(super) fn apply_explicit_window_drop(
        &mut self,
        layout: LayoutId,
        source: WindowId,
        target: WindowId,
        action: crate::layout_engine::WindowDropAction,
    ) -> bool {
        if source == target {
            return false;
        }
        if action == crate::layout_engine::WindowDropAction::Swap {
            return self.swap_windows(layout, source, target);
        }
        let Some(source_node) = self.window_node(layout, source) else {
            return false;
        };
        let Some(target_node) = self.window_node(layout, target) else {
            return false;
        };

        if action == crate::layout_engine::WindowDropAction::Stack {
            if let Some(parent) = target_node.parent(self.map())
                && self.layout(parent).is_group()
            {
                self.tree.move_node(source_node, Placement::After(target_node));
                self.select(source_node);
                return true;
            }
            let parent_kind = target_node
                .parent(self.map())
                .map(|parent| self.layout(parent))
                .unwrap_or(LayoutKind::Horizontal);
            let stack_kind = match parent_kind.orientation() {
                Orientation::Horizontal => LayoutKind::HorizontalStack,
                Orientation::Vertical => LayoutKind::VerticalStack,
            };
            let container = self.tree.insert(Placement::Before(target_node));
            self.tree.map.assume_size_of(container, target_node);
            self.tree.move_node(target_node, Placement::Back(container));
            self.tree.move_node(source_node, Placement::Back(container));
            self.set_layout(container, stack_kind);
            self.select(source_node);
            return true;
        }

        let crate::layout_engine::WindowDropAction::Insert(direction) = action else {
            unreachable!()
        };
        let target_anchor = target_node
            .parent(self.map())
            .filter(|parent| self.layout(*parent).is_group())
            .unwrap_or(target_node);
        if target_anchor == source_node
            || target_anchor.ancestors(self.map()).any(|node| node == source_node)
        {
            return false;
        }
        let before = matches!(direction, Direction::Left | Direction::Up);
        if let Some(parent) = target_anchor.parent(self.map())
            && !self.layout(parent).is_group()
            && self.layout(parent).orientation() == direction.orientation()
        {
            if source_node.parent(self.map()) == Some(parent) {
                source_node.reorder(target_anchor, before, &mut self.tree.map);
            } else {
                if before {
                    self.tree.move_node(source_node, Placement::Before(target_anchor));
                } else {
                    self.tree.move_node(source_node, Placement::After(target_anchor));
                }
            }
        } else {
            let kind = match direction.orientation() {
                Orientation::Horizontal => LayoutKind::Horizontal,
                Orientation::Vertical => LayoutKind::Vertical,
            };
            let container = if target_anchor.parent(self.map()).is_none() {
                // A root stack needs a new layout root, rather than a sibling.
                self.nest_in_container_internal(layout, target_anchor, kind)
            } else {
                let container = self.tree.insert(Placement::Before(target_anchor));
                self.tree.map.assume_size_of(container, target_anchor);
                self.set_layout(container, kind);
                container
            };
            // Attach the stack first: extracting the source can collapse a
            // two-window stack and invalidate target_anchor after reattachment.
            self.tree.move_node(target_anchor, Placement::Back(container));
            if before {
                self.tree.move_node(source_node, Placement::Before(target_anchor));
            } else {
                self.tree.move_node(source_node, Placement::Back(container));
            }
        }
        self.select(source_node);
        true
    }

    pub(super) fn project(
        &self,
        node: NodeId,
        regions: Option<(NodeId, NodeId)>,
        screen: CGRect,
        stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        let map = &self.tree.map;
        let mut prepared = ConstraintCache::default();
        for child in node.traverse_postorder(map) {
            if map.window(child).is_some_and(|w| !self.inactive.contains(&w))
                || child.children(map).any(|c| prepared.contains_key(&c))
            {
                let axes = [false, true].map(|horizontal| {
                    self.tree.map.node_axis_constraints(
                        &prepared,
                        child,
                        constraints,
                        stack_offset,
                        stack_line,
                        gaps,
                        horizontal,
                    )
                });
                prepared.insert(child, axes);
            }
        }
        // An empty secondary region gives the primary region the whole screen slot.
        let node = regions
            .filter(|(_, secondary)| !prepared.contains_key(secondary))
            .map_or(node, |(primary, _)| primary);
        let mut sizes = Vec::new();
        Projection {
            prepared: &prepared,
            map,
            screen,
            stack_offset,
            constraints,
            gaps,
            stack_line,
        }
        .apply(node, compute_tiling_area(screen, gaps), &mut sizes);
        sizes
    }
}
