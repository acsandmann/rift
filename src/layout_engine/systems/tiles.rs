use objc2_core_foundation::CGRect;

use super::groups::GroupLayouts;
use super::{EditResult, WindowLayoutConstraints};
use crate::actor::app::WindowId;
use crate::common::collections::HashMap;
use crate::layout_engine::systems::constraints::{
    AxisConstraints, ConstraintCache, axis_slot, constrain_frame, container_axis_constraints,
    solve_axis_lengths, traditional_leaf_axis_constraints,
};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation, ResizeOrientation};
use crate::model::tree::{NodeId, NodeMap, Placement};
use crate::sys::geometry::Round;

pub(super) struct StackLayoutResult {
    frame: CGRect,
    stack_offset: f64,
    max_origin: f64,
    horizontal: bool,
}

impl StackLayoutResult {
    fn new(container: CGRect, count: usize, stack_offset: f64, horizontal: bool) -> Self {
        let offset_space = if count == 0 {
            0.0
        } else {
            (count - 1) as f64 * stack_offset
        };
        let mut frame = container;
        let (start, extent, length) = if horizontal {
            (container.origin.x, container.size.width, &mut frame.size.width)
        } else {
            (container.origin.y, container.size.height, &mut frame.size.height)
        };
        *length = (extent - offset_space).max(100.0).min(extent);
        let max_origin = (start + extent - *length).max(start);
        Self {
            frame,
            stack_offset,
            max_origin,
            horizontal,
        }
    }

    pub(super) fn get_frame_for_index(&self, index: usize) -> CGRect {
        let mut frame = self.frame;
        let origin = if self.horizontal {
            &mut frame.origin.x
        } else {
            &mut frame.origin.y
        };
        *origin = (*origin + index as f64 * self.stack_offset).clamp(*origin, self.max_origin);
        frame.round()
    }
}

impl NodeMap {
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
        self.normalize_weights(node.parent(self).unwrap());
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

pub(super) fn remove_and_rebalance(tiles: &mut GroupLayouts, window: WindowId) {
    tiles.inactive.remove(&window);
    for info in tiles.tree.window.take_window(window) {
        let node = info.node;
        let parent = node.parent(tiles.map());
        tiles.tree.remove(node);
        if let Some(parent) = parent.filter(|&node| tiles.map().contains(node)) {
            rebalance(&mut tiles.tree.map, parent);
        }
    }
}

pub(super) fn rebalance(map: &mut NodeMap, root: NodeId) -> bool {
    let mut changed = false;
    let mut child = root.first_child(map);
    while let Some(node) = child {
        changed |= std::mem::replace(&mut map[node].layout.size, 1.0) != 1.0;
        changed |= rebalance(map, node);
        child = node.next_sibling(map);
    }
    changed
}

pub(super) fn resize_window(
    tiles: &mut GroupLayouts,
    layout: LayoutId,
    window: WindowId,
    old: CGRect,
    new: CGRect,
    screen: CGRect,
    gaps: &crate::common::config::GapSettings,
) {
    if !tiles.window_participates(window) {
        return;
    }
    let Some(node) = tiles.window_node(layout, window) else {
        return;
    };
    let info = &mut tiles.tree.map[node].layout;
    if !super::track_fullscreen_resize(
        &mut info.is_fullscreen,
        &mut info.is_fullscreen_within_gaps,
        old,
        new,
        screen,
        gaps,
    ) {
        set_frame_from_resize(tiles, node, old, new, screen);
    }
}

fn child_frames_in_axis<'a>(
    tiles: &'a GroupLayouts,
    parent_rect: CGRect,
    siblings: &'a [NodeId],
    horizontal: bool,
    gaps: &crate::common::config::GapSettings,
) -> impl Iterator<Item = CGRect> + 'a {
    let total: f32 = siblings.iter().map(|&child| tiles.tree.map[child].layout.size).sum();
    let (offset, usable_axis, inner_gap) = axis_span(parent_rect, horizontal, gaps, siblings.len());

    siblings.iter().scan(offset, move |offset, &child| {
        let ratio = f64::from(tiles.tree.map[child].layout.size) / f64::from(total);
        let seg_len = usable_axis * ratio;
        let frame = axis_slot(parent_rect, horizontal, *offset, seg_len);
        *offset += seg_len;
        *offset += inner_gap;
        Some(frame)
    })
}

pub(super) fn collect_group_containers(
    tiles: &GroupLayouts,
    layout: LayoutId,
    screen: CGRect,
    stack_offset: f64,
    gaps: &crate::common::config::GapSettings,
    stack_line: crate::common::config::StackLineReserve,
    selection_path_only: bool,
) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
    use crate::layout_engine::LayoutKind::*;

    let mut out = Vec::new();
    let map = &tiles.tree.map;
    let active = tiles.participating_nodes(tiles.root(layout));

    let tiling_area = compute_tiling_area(screen, gaps);

    let mut stack: Vec<(NodeId, CGRect)> = vec![(tiles.root(layout), tiling_area)];

    while let Some((node, rect)) = stack.pop() {
        if tiles.tree.map.is_effectively_fullscreen(node) {
            if selection_path_only {
                out.clear();
                break;
            }
            continue;
        }

        let kind = tiles.tree.map.kind(node);
        let children: Vec<_> = node
            .children(map)
            .filter(|c| tiles.inactive.is_empty() || active.contains(c))
            .collect();

        let selected = tiles
            .tree
            .map
            .local_selection(node)
            .filter(|c| tiles.inactive.is_empty() || active.contains(c))
            .or_else(|| children.first().copied());

        if matches!(kind, HorizontalStack | VerticalStack) {
            if children.is_empty() {
                continue;
            }

            let local_sel = selected.unwrap();
            if selection_path_only && tiles.tree.map.is_effectively_fullscreen(local_sel) {
                out.clear();
                break;
            }
            let selected_index = children.iter().position(|&c| c == local_sel).unwrap_or(0);

            let is_horizontal = matches!(kind, HorizontalStack);
            out.push(tiles.stack_group_container_info(node, kind, rect, &children, selected_index));

            let layout_res =
                stack_layout_result(rect, children.len(), stack_offset, is_horizontal, stack_line);

            for (i, &child) in children.iter().enumerate().rev() {
                if (selection_path_only && Some(child) != selected)
                    || tiles.tree.map.is_effectively_fullscreen(child)
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
                .zip(child_frames_in_axis(
                    tiles,
                    rect,
                    &children,
                    kind == Horizontal,
                    gaps,
                ))
                .filter(|(child, _)| !selection_path_only || Some(*child) == selected),
        );
        stack[start..].reverse();
    }

    out
}

pub(super) fn resize_selection(
    tiles: &mut GroupLayouts,
    layout: LayoutId,
    amount: f64,
    orientation: ResizeOrientation,
) -> EditResult {
    if !tiles.inactive.is_empty() && tiles.find_best_focus_target(tiles.root(layout)).is_none() {
        return EditResult::Unchanged;
    }
    if amount == 0.0 {
        return EditResult::Unchanged;
    }
    let selection = tiles.selection(layout);
    if tiles.window_at(selection).is_none() {
        return EditResult::Unchanged;
    }
    let changed = match orientation {
        ResizeOrientation::Horizontal => [Direction::Right, Direction::Left]
            .into_iter()
            .any(|direction| resize_internal(tiles, selection, amount, direction)),
        ResizeOrientation::Vertical => [Direction::Down, Direction::Up]
            .into_iter()
            .any(|direction| resize_internal(tiles, selection, amount, direction)),
        ResizeOrientation::Smart => {
            let mut node = selection;
            while let Some(parent) = node.parent(tiles.map()) {
                let kind = tiles.layout(parent);
                if !kind.is_group() {
                    let directions = match kind.orientation() {
                        Orientation::Horizontal => [Direction::Right, Direction::Left],
                        Orientation::Vertical => [Direction::Down, Direction::Up],
                    };
                    if directions
                        .into_iter()
                        .any(|direction| resize_internal(tiles, node, amount, direction))
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

pub(super) fn resize_internal(
    tiles: &mut GroupLayouts,
    node: NodeId,
    screen_ratio: f64,
    direction: Direction,
) -> bool {
    let Some((resizing_node, sibling)) = resize_pair(tiles, node, direction) else {
        return false;
    };
    let exchange_rate = resizing_node
        .ancestors(&tiles.tree.map)
        .skip(1)
        .try_fold(1.0, |r, node| match node.parent(&tiles.tree.map) {
            Some(parent)
                if tiles.tree.map.kind(parent).orientation() == direction.orientation()
                    && !tiles.tree.map.kind(parent).is_group() =>
            {
                tiles.tree.map.proportion(node).map(|p| r * p)
            }
            _ => Some(r),
        })
        .unwrap_or(1.0);
    let local_ratio = f64::from(screen_ratio)
        * f64::from(tiles.tree.map.children_total(resizing_node.parent(&tiles.tree.map).unwrap()))
        / exchange_rate;
    tiles.tree.map.take_share(resizing_node, sibling, local_ratio as f32);
    true
}

fn resize_pair(
    tiles: &GroupLayouts,
    node: NodeId,
    direction: Direction,
) -> Option<(NodeId, NodeId)> {
    let map = tiles.map();
    node.ancestors_with_parent(map).find_map(|(candidate, parent)| {
        if map.kind(parent?).is_group() {
            return None;
        }
        tiles.move_over(candidate, direction).map(|sibling| (candidate, sibling))
    })
}

pub(super) fn set_frame_from_resize(
    tiles: &mut GroupLayouts,
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
            delta != 0.0 && resize_pair(tiles, node, direction).is_some()
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
            resize_internal(tiles, node, delta / extent, direction);
        }
    }
}

pub(super) fn apply_explicit_window_drop(
    tiles: &mut GroupLayouts,
    layout: LayoutId,
    source: WindowId,
    target: WindowId,
    action: crate::layout_engine::WindowDropAction,
) -> bool {
    if source == target || !tiles.window_participates(source) || !tiles.window_participates(target)
    {
        return false;
    }
    if action == crate::layout_engine::WindowDropAction::Swap {
        return tiles.swap_windows(layout, source, target);
    }
    let Some(source_node) = tiles.window_node(layout, source) else {
        return false;
    };
    let Some(target_node) = tiles.window_node(layout, target) else {
        return false;
    };

    if action == crate::layout_engine::WindowDropAction::Stack {
        if let Some(parent) = target_node.parent(tiles.map())
            && tiles.layout(parent).is_group()
        {
            tiles.tree.move_node(source_node, Placement::After(target_node));
            tiles.select(source_node);
            return true;
        }
        let parent_kind = target_node
            .parent(tiles.map())
            .map(|parent| tiles.layout(parent))
            .unwrap_or(LayoutKind::Horizontal);
        let stack_kind = match parent_kind.orientation() {
            Orientation::Horizontal => LayoutKind::HorizontalStack,
            Orientation::Vertical => LayoutKind::VerticalStack,
        };
        let container = tiles.tree.insert(Placement::Before(target_node));
        tiles.tree.map.assume_size_of(container, target_node);
        tiles.tree.move_node(target_node, Placement::Back(container));
        tiles.tree.move_node(source_node, Placement::Back(container));
        tiles.set_layout(container, stack_kind);
        tiles.select(source_node);
        return true;
    }

    let crate::layout_engine::WindowDropAction::Insert(direction) = action else {
        unreachable!()
    };
    let target_anchor = target_node
        .parent(tiles.map())
        .filter(|parent| tiles.layout(*parent).is_group())
        .unwrap_or(target_node);
    if target_anchor == source_node
        || target_anchor.ancestors(tiles.map()).any(|node| node == source_node)
    {
        return false;
    }
    let before = matches!(direction, Direction::Left | Direction::Up);
    if let Some(parent) = target_anchor.parent(tiles.map())
        && !tiles.layout(parent).is_group()
        && tiles.layout(parent).orientation() == direction.orientation()
    {
        let placement = if before {
            Placement::Before(target_anchor)
        } else {
            Placement::After(target_anchor)
        };
        tiles.tree.move_node(source_node, placement);
    } else {
        let kind = match direction.orientation() {
            Orientation::Horizontal => LayoutKind::Horizontal,
            Orientation::Vertical => LayoutKind::Vertical,
        };
        let container = if target_anchor.parent(tiles.map()).is_none() {
            // A root stack needs a new layout root, rather than a sibling.
            tiles.nest_in_container_internal(layout, target_anchor, kind)
        } else {
            let container = tiles.tree.insert(Placement::Before(target_anchor));
            tiles.tree.map.assume_size_of(container, target_anchor);
            tiles.set_layout(container, kind);
            container
        };
        // Attach the stack first: extracting the source can collapse a
        // two-window stack and invalidate target_anchor after reattachment.
        tiles.tree.move_node(target_anchor, Placement::Back(container));
        if before {
            tiles.tree.move_node(source_node, Placement::Before(target_anchor));
        } else {
            tiles.tree.move_node(source_node, Placement::Back(container));
        }
    }
    tiles.select(source_node);
    true
}

pub(super) fn project(
    tiles: &GroupLayouts,
    node: NodeId,
    root: impl FnOnce(&ConstraintCache) -> NodeId,
    screen: CGRect,
    stack_offset: f64,
    constraints: &HashMap<WindowId, WindowLayoutConstraints>,
    gaps: &crate::common::config::GapSettings,
    stack_line: crate::common::config::StackLineReserve,
) -> Vec<(WindowId, CGRect)> {
    let map = &tiles.tree.map;
    let mut prepared = ConstraintCache::default();
    for child in node.traverse_postorder(map) {
        if map.window(child).is_some_and(|w| !tiles.inactive.contains(&w))
            || child.children(map).any(|c| prepared.contains_key(&c))
        {
            let axes = [false, true].map(|horizontal| {
                tiles.tree.map.node_axis_constraints(
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
    .apply(root(&prepared), compute_tiling_area(screen, gaps), &mut sizes);
    sizes
}
