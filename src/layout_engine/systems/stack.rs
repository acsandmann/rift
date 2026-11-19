use nix::libc::pid_t;
use objc2_core_foundation::CGRect;
use serde::{Deserialize, Deserializer, Serialize};
use slotmap::{Key, KeyData, SlotMap};

use super::constraints::{constrain_frame, traditional_leaf_axis_constraints};
use super::groups::LayoutRoot;
use super::{EditResult, Focus, FocusResult, GroupEdit, LayoutEdit, WindowChange, WindowQuery};
use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{
    GapSettings, StackDefaultOrientation, StackLineReserve, WindowInsertionPoint,
    default_stack_orientation,
};
use crate::layout_engine::systems::{LayoutSystem, WindowLayoutConstraints};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind};
use crate::model::tree::{NodeId, Tree};

#[derive(Clone, Serialize, Deserialize, Debug)]
struct Member {
    id: NodeId,
    window: WindowId,
    #[serde(default = "unit_weight")]
    weight: f64,
    #[serde(default)]
    fullscreen: bool,
    #[serde(default)]
    within_gaps: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct Stack {
    id: NodeId,
    members: Vec<Member>,
    active: Option<NodeId>,
    root_selected: bool,
    kind: LayoutKind,
    fullscreen: bool,
    within_gaps: bool,
}

fn unit_weight() -> f64 { 1.0 }

impl Stack {
    fn active_index(&self, inactive: &HashSet<WindowId>) -> Option<usize> {
        self.members
            .iter()
            .position(|m| Some(m.id) == self.active && !inactive.contains(&m.window))
            .or_else(|| self.members.iter().position(|m| !inactive.contains(&m.window)))
    }

    fn visible_index(&self, inactive: &HashSet<WindowId>) -> Option<usize> {
        if self.root_selected {
            self.members.iter().position(|m| !inactive.contains(&m.window))
        } else {
            self.active_index(inactive)
        }
    }

    fn retain(&mut self, keep: impl Fn(&Member) -> bool) {
        if let Some(index) = self.members.iter().position(|m| Some(m.id) == self.active)
            && !keep(&self.members[index])
        {
            self.active = self.members[index + 1..]
                .iter()
                .find(|m| keep(m))
                .or_else(|| self.members[..index].iter().rev().find(|m| keep(m)))
                .map(|m| m.id);
            self.root_selected = false;
        }
        self.members.retain(keep);
    }

    fn remove(&mut self, window: WindowId) { self.retain(|member| member.window != window); }

    fn insert(&mut self, member: Member, after_selection: bool) {
        let index = if after_selection {
            self.members.iter().position(|m| Some(m.id) == self.active).map(|i| i + 1)
        } else {
            None
        }
        .unwrap_or(self.members.len());
        self.active = Some(member.id);
        self.root_selected = false;
        self.members.insert(index, member);
    }

    fn neighbor(&self, direction: Direction, inactive: &HashSet<WindowId>) -> Option<usize> {
        if self.root_selected || self.kind.orientation() != direction.orientation() {
            return None;
        }
        let index = self.active_index(inactive)?;
        match direction {
            Direction::Left | Direction::Up => {
                (0..index).rev().find(|&i| !inactive.contains(&self.members[i].window))
            }
            Direction::Right | Direction::Down => (index + 1..self.members.len())
                .find(|&i| !inactive.contains(&self.members[i].window)),
        }
    }
}

/// A stack owns an ordered list; selection and fullscreen never require rebuilding it.
#[derive(Clone, Serialize, Debug)]
pub struct StackLayoutSystem {
    layouts: SlotMap<LayoutId, Stack>,
    next_node: u32,
    default_orientation: StackDefaultOrientation,
    #[serde(skip)]
    inactive: HashSet<WindowId>,
    #[serde(skip)]
    window_insertion_point: WindowInsertionPoint,
}

#[derive(Deserialize)]
struct LegacyStackForest {
    #[serde(deserialize_with = "super::groups::deserialize_tile_tree")]
    tree: Tree,
    layout_roots: Vec<super::SavedSlot<LayoutRoot>>,
}

impl LegacyStackForest {
    fn migrate(self) -> Result<(SlotMap<LayoutId, Stack>, u32), ron::Error> {
        let map = self.tree.map;
        let mut next_node = 1;
        let slots: Vec<_> = self
            .layout_roots
            .into_iter()
            .map(|slot| {
                let value = slot.value.map(|root| {
                    let root = root.0;
                    let selected = map.current_selection(root);
                    let active = map
                        .focus_target(selected, &|_| true)
                        .or_else(|| map.focus_target(root, &|_| true))
                        .map(|(node, _)| node);
                    let members: Vec<_> = root
                        .traverse_preorder(&map)
                        .filter_map(|id| {
                            next_node =
                                next_node.max((id.data().as_ffi() as u32).saturating_add(1));
                            let window = map.window(id)?;
                            let info = &map[id].layout;
                            Some(Member {
                                id,
                                window,
                                weight: f64::from(info.size),
                                fullscreen: info.is_fullscreen,
                                within_gaps: info.is_fullscreen_within_gaps,
                            })
                        })
                        .collect();
                    let info = &map[root].layout;
                    Stack {
                        id: root,
                        active,
                        members,
                        root_selected: selected == root,
                        kind: StackLayoutSystem::stack_kind(info.kind),
                        fullscreen: info.is_fullscreen,
                        within_gaps: info.is_fullscreen_within_gaps,
                    }
                });
                super::SavedSlot { value, version: slot.version }
            })
            .collect();
        let layouts = ron::from_str(&ron::to_string(&slots)?).map_err(|error| error.code)?;
        Ok((layouts, next_node))
    }
}

impl<'de> Deserialize<'de> for StackLayoutSystem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            layouts: SlotMap<LayoutId, Stack>,
            #[serde(default = "first_node")]
            next_node: u32,
            #[serde(default = "default_stack_orientation")]
            default_orientation: StackDefaultOrientation,
            #[serde(default, deserialize_with = "super::present")]
            inner: Option<LegacyStackForest>,
        }
        fn first_node() -> u32 { 1 }
        let Wire {
            layouts,
            next_node,
            default_orientation,
            inner,
        } = Wire::deserialize(deserializer)?;
        let (layouts, next_node) = if let Some(inner) = inner {
            inner.migrate().map_err(serde::de::Error::custom)?
        } else {
            (layouts, next_node)
        };
        Ok(Self {
            layouts,
            next_node,
            default_orientation,
            inactive: Default::default(),
            window_insertion_point: Default::default(),
        })
    }
}

impl Default for StackLayoutSystem {
    fn default() -> Self { Self::new(default_stack_orientation()) }
}

impl StackLayoutSystem {
    pub fn new(orientation: StackDefaultOrientation) -> Self {
        Self::new_with_insertion_point(orientation, WindowInsertionPoint::default())
    }

    pub fn new_with_insertion_point(
        default_orientation: StackDefaultOrientation,
        window_insertion_point: WindowInsertionPoint,
    ) -> Self {
        Self {
            layouts: SlotMap::with_key(),
            next_node: 1,
            default_orientation,
            window_insertion_point,
            inactive: Default::default(),
        }
    }

    pub fn update_settings(
        &mut self,
        orientation: StackDefaultOrientation,
        insertion: WindowInsertionPoint,
    ) {
        self.default_orientation = orientation;
        self.window_insertion_point = insertion;
    }

    fn node_id(&mut self) -> NodeId {
        let index = self.next_node;
        self.next_node = index.checked_add(1).expect("stack node identity exhausted");
        NodeId::from(KeyData::from_ffi((1u64 << 32) | u64::from(index)))
    }

    fn stack_kind(kind: LayoutKind) -> LayoutKind {
        match kind {
            LayoutKind::Horizontal | LayoutKind::HorizontalStack => LayoutKind::HorizontalStack,
            _ => LayoutKind::VerticalStack,
        }
    }

    fn toggle_orientation(&mut self, layout: LayoutId) {
        let stack = &mut self.layouts[layout];
        stack.kind = if stack.kind == LayoutKind::HorizontalStack {
            LayoutKind::VerticalStack
        } else {
            LayoutKind::HorizontalStack
        };
    }

    fn toggle_fullscreen(&mut self, layout: LayoutId, within: bool) -> Vec<WindowId> {
        if self.selected_window(layout).is_none() {
            return Vec::new();
        }
        let stack = &mut self.layouts[layout];
        let (full, gaps) = if stack.root_selected {
            (&mut stack.fullscreen, &mut stack.within_gaps)
        } else {
            let index = stack.active_index(&self.inactive).unwrap();
            let m = &mut stack.members[index];
            (&mut m.fullscreen, &mut m.within_gaps)
        };
        if super::toggle_fullscreen_mode(full, gaps, within) {
            self.visible_windows_in_layout(layout)
        } else {
            Vec::new()
        }
    }

    pub(crate) fn collect_group_containers(
        &self,
        layout: LayoutId,
        screen: CGRect,
        _offset: f64,
        gaps: &GapSettings,
        _line: StackLineReserve,
        selection_path_only: bool,
    ) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
        let stack = &self.layouts[layout];
        let Some(active) = stack.visible_index(&self.inactive) else {
            return Vec::new();
        };
        if stack.fullscreen
            || stack.within_gaps
            || (selection_path_only
                && (stack.members[active].fullscreen || stack.members[active].within_gaps))
        {
            return Vec::new();
        }
        let mut selected_index = 0;
        let window_ids: Vec<_> = stack
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| !self.inactive.contains(&m.window))
            .map(|(i, m)| {
                if i < active {
                    selected_index += 1;
                }
                m.window
            })
            .collect();
        vec![crate::layout_engine::engine::GroupContainerInfo {
            node_id: stack.id,
            container_kind: stack.kind,
            frame: compute_tiling_area(screen, gaps),
            total_count: window_ids.len(),
            selected_index,
            window_ids,
        }]
    }
}

impl LayoutSystem for StackLayoutSystem {
    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.layouts[layout].members.iter().any(|member| member.window.pid == pid)
    }

    fn window_participates(&self, window: WindowId) -> bool { !self.inactive.contains(&window) }

    fn set_window_participation(&mut self, window: WindowId, participates: bool) -> bool {
        super::update_participation(&mut self.inactive, window, participates)
    }

    fn create_layout(&mut self) -> LayoutId {
        let id = self.node_id();
        let kind = match self.default_orientation {
            StackDefaultOrientation::Horizontal | StackDefaultOrientation::Same => {
                LayoutKind::HorizontalStack
            }
            _ => LayoutKind::VerticalStack,
        };
        self.layouts.insert(Stack {
            id,
            members: Vec::new(),
            active: None,
            root_selected: false,
            kind,
            fullscreen: false,
            within_gaps: false,
        })
    }

    fn contains_layout(&self, layout: LayoutId) -> bool { self.layouts.contains_key(layout) }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let mut stack = self.layouts[layout].clone();
        stack.id = self.node_id();
        for member in &mut stack.members {
            let id = self.node_id();
            if stack.active == Some(member.id) {
                stack.active = Some(id);
            }
            member.id = id;
        }
        self.layouts.insert(stack)
    }

    fn remove_layout(&mut self, layout: LayoutId) { self.layouts.remove(layout); }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &GapSettings,
        line: StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        let stack = &self.layouts[layout];
        let count = stack.members.iter().filter(|m| self.window_participates(m.window)).count();
        let Some(active) = stack.visible_index(&self.inactive) else {
            return Vec::new();
        };
        let horizontal = stack.kind == LayoutKind::HorizontalStack;
        let tiling = compute_tiling_area(screen, gaps);
        let rect = if stack.fullscreen { screen } else { tiling };
        let required = constraints
            .get(&stack.members[active].window)
            .copied()
            .map(|c| {
                let (min, fixed, _, _) = traditional_leaf_axis_constraints(c, horizontal);
                fixed.unwrap_or(min)
            })
            .unwrap_or(0.0);
        let offset = super::tiles::stack_offset_with_required_size(
            rect, horizontal, count, offset, required,
        );
        let geometry = super::tiles::stack_layout_result(rect, count, offset, horizontal, line);
        stack
            .members
            .iter()
            .filter(|m| self.window_participates(m.window))
            .enumerate()
            .map(|(i, m)| {
                let frame = if m.fullscreen {
                    screen
                } else if m.within_gaps {
                    tiling
                } else {
                    geometry.get_frame_for_index(i)
                };
                let frame = constraints.get(&m.window).copied().map_or(frame, |c| {
                    constrain_frame(frame, |horizontal| {
                        traditional_leaf_axis_constraints(c, horizontal)
                    })
                });
                (m.window, frame)
            })
            .collect()
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        let s = &self.layouts[layout];
        s.active_index(&self.inactive).map(|i| s.members[i].window)
    }

    fn selection_contains_window(&self, layout: LayoutId, window: WindowId) -> bool {
        let s = &self.layouts[layout];
        if s.root_selected {
            s.members.iter().any(|m| m.window == window)
        } else {
            s.members.iter().any(|m| Some(m.id) == s.active && m.window == window)
        }
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        let stack = &self.layouts[layout];
        if !stack.root_selected
            && let Some(member) = stack
                .members
                .iter()
                .find(|m| Some(m.id) == stack.active && self.window_participates(m.window))
        {
            return Some(member.window);
        }
        match direction {
            Direction::Left | Direction::Up => {
                stack.members.iter().rev().find(|m| self.window_participates(m.window))
            }
            _ => stack.members.iter().find(|m| self.window_participates(m.window)),
        }
        .map(|m| m.window)
    }

    fn contains_window(&self, layout: LayoutId, window: WindowId) -> bool {
        self.layouts[layout].members.iter().any(|m| m.window == window)
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        let s = &self.layouts[layout];
        self.selected_window(layout).is_some()
            && (s.fullscreen
                || s.within_gaps
                || s.members
                    .iter()
                    .any(|m| self.window_participates(m.window) && (m.fullscreen || m.within_gaps)))
    }

    fn parent_of_selection_is_stacked(&self, _layout: LayoutId) -> bool { true }

    fn windows(&self, layout: LayoutId, query: WindowQuery) -> Vec<WindowId> {
        match query {
            WindowQuery::All => self.all_windows_in_layout(layout),
            WindowQuery::Visible | WindowQuery::Selection => self.visible_windows_in_layout(layout),
            WindowQuery::Stack(window) => {
                if self.contains_window(layout, window) {
                    self.all_windows_in_layout(layout)
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn focus(&mut self, layout: LayoutId, focus: Focus) -> FocusResult {
        match focus {
            Focus::Select(window) => FocusResult::selection(self.select_window(layout, window)),
            Focus::Ascend | Focus::Descend => FocusResult::selection({
                if self.selected_window(layout).is_none() {
                    return FocusResult::selection(false);
                }
                let select_root = matches!(focus, Focus::Ascend);
                std::mem::replace(&mut self.layouts[layout].root_selected, select_root)
                    != select_root
            }),
            Focus::Direction(direction) => {
                FocusResult::movement(self.move_focus(layout, direction))
            }
        }
    }

    fn update(&mut self, change: WindowChange<'_>) {
        match change {
            WindowChange::Add(layout, window, _) => self.add_window_after_selection(layout, window),
            WindowChange::Remove(window) | WindowChange::RemoveAndRebalance(window) => {
                self.inactive.remove(&window);
                for s in self.layouts.values_mut() {
                    s.remove(window);
                }
            }
            WindowChange::Replace(from, to) => {
                super::replace_participation(&mut self.inactive, from, to);
                for s in self.layouts.values_mut() {
                    for m in &mut s.members {
                        if m.window == from {
                            m.window = to;
                        }
                    }
                }
            }
            WindowChange::CloseApp(pid) => {
                self.inactive.retain(|w| w.pid != pid);
                for stack in self.layouts.values_mut() {
                    stack.retain(|member| member.window.pid != pid);
                }
            }
            WindowChange::Resize {
                layout,
                window,
                old,
                new,
                screen,
                gaps,
            } => self.on_window_resized(layout, window, old, new, screen, gaps),
        }
    }

    fn edit_layout(&mut self, layout: LayoutId, edit: LayoutEdit) -> EditResult {
        match edit {
            LayoutEdit::Move {
                direction,
                has_display_neighbor,
            } => EditResult::from({
                let s = &mut self.layouts[layout];
                if s.root_selected {
                    return EditResult::Unchanged;
                }
                let Some(i) = s.active_index(&self.inactive) else {
                    return EditResult::Unchanged;
                };
                let target = if let Some(target) = s.neighbor(direction, &self.inactive) {
                    target
                } else if has_display_neighbor || s.kind.orientation() == direction.orientation() {
                    return EditResult::Unchanged;
                } else {
                    // Leaving the stack axis places the member at the corresponding outer
                    // edge and turns the whole stack onto that axis.
                    s.kind = Self::stack_kind(LayoutKind::from(direction.orientation()));
                    match direction {
                        Direction::Left | Direction::Up => 0,
                        _ => s.members.len() - 1,
                    }
                };
                let member = s.members.remove(i);
                s.active = Some(member.id);
                s.members.insert(target, member);
                true
            }),

            LayoutEdit::Fullscreen { within_gaps } => {
                EditResult::Raise(self.toggle_fullscreen(layout, within_gaps))
            }
            LayoutEdit::Swap(a, b) => EditResult::from(self.swap_windows(layout, a, b)),
            LayoutEdit::ReorderTab { window, target } => {
                EditResult::from(self.move_stack_tab(layout, window, target))
            }
            LayoutEdit::Drop { source, target, .. } => EditResult::from({
                source != target
                    && self.swap_windows(layout, source, target)
                    && self.select_window(layout, source)
            }),
            LayoutEdit::Group(GroupEdit::Stack(_)) => EditResult::Raise({
                self.toggle_orientation(layout);
                self.visible_windows_in_layout(layout)
            }),
            LayoutEdit::ToggleOrientation => {
                self.toggle_orientation(layout);
                EditResult::Changed
            }
            _ => EditResult::Unsupported,
        }
    }
}

impl StackLayoutSystem {
    fn all_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.layouts[layout].members.iter().map(|m| m.window).collect()
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        let stack = &self.layouts[layout];
        stack
            .visible_index(&self.inactive)
            .map(|i| stack.members[i].window)
            .into_iter()
            .collect()
    }

    fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        let s = &mut self.layouts[layout];
        let Some(i) = s.neighbor(direction, &self.inactive) else {
            return (None, Vec::new());
        };
        s.active = Some(s.members[i].id);
        (Some(s.members[i].window), vec![s.members[i].window])
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, window: WindowId) {
        if self.contains_window(layout, window) {
            return;
        }
        let id = self.node_id();
        self.layouts[layout].insert(
            Member {
                id,
                window,
                weight: 1.0,
                fullscreen: false,
                within_gaps: false,
            },
            self.window_insertion_point == WindowInsertionPoint::NextToSelection,
        );
    }

    fn select_window(&mut self, layout: LayoutId, window: WindowId) -> bool {
        if !self.window_participates(window) {
            return false;
        }
        let s = &mut self.layouts[layout];
        let Some(m) = s.members.iter().find(|m| m.window == window) else {
            return false;
        };
        s.active = Some(m.id);
        s.root_selected = false;
        true
    }

    fn on_window_resized(
        &mut self,
        layout: LayoutId,
        window: WindowId,
        old: CGRect,
        new: CGRect,
        screen: CGRect,
        gaps: &GapSettings,
    ) {
        if !self.window_participates(window) {
            return;
        }
        let Some(m) = self.layouts[layout].members.iter_mut().find(|m| m.window == window) else {
            return;
        };
        super::track_fullscreen_resize(
            &mut m.fullscreen,
            &mut m.within_gaps,
            old,
            new,
            screen,
            gaps,
        );
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        if !self.window_participates(a) || !self.window_participates(b) {
            return false;
        }
        let s = &mut self.layouts[layout];
        let Some(i) = s.members.iter().position(|m| m.window == a) else {
            return false;
        };
        let Some(j) = s.members.iter().position(|m| m.window == b) else {
            return false;
        };
        if i == j {
            return false;
        }
        if !s.root_selected {
            if s.active == Some(s.members[i].id) {
                s.active = Some(s.members[j].id);
            } else if s.active == Some(s.members[j].id) {
                s.active = Some(s.members[i].id);
            }
        }
        s.members[i].window = b;
        s.members[j].window = a;
        true
    }

    fn move_stack_tab(&mut self, layout: LayoutId, window: WindowId, target: WindowId) -> bool {
        if !self.window_participates(window) || !self.window_participates(target) {
            return false;
        }
        let s = &mut self.layouts[layout];
        let Some(i) = s.members.iter().position(|m| m.window == window) else {
            return false;
        };
        let Some(j) = s.members.iter().position(|m| m.window == target) else {
            return false;
        };
        if i == j {
            return false;
        }
        let m = s.members.remove(i);
        s.members.insert(j, m);
        true
    }
}

impl StackLayoutSystem {
    pub(crate) fn draw_tree(&self, layout: LayoutId) -> String {
        format!("{:#?}", self.layouts[layout])
    }

    pub(crate) fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        use rift_protocol::{ContainerNodeType, ContainerTreeNode as Node};
        let stack = &self.layouts[layout];
        Node {
            node_id: stack.id.data().as_ffi(),
            layout_kind: Some(stack.kind),
            is_selected: stack.root_selected || stack.members.is_empty(),
            is_fullscreen: stack.fullscreen,
            is_fullscreen_within_gaps: stack.within_gaps,
            children: stack
                .members
                .iter()
                .map(|m| Node {
                    node_id: m.id.data().as_ffi(),
                    node_type: ContainerNodeType::Window,
                    window_id: Some(m.window.into()),
                    weight: Some(m.weight),
                    is_selected: !stack.root_selected && stack.active == Some(m.id),
                    is_fullscreen: m.fullscreen,
                    is_fullscreen_within_gaps: m.within_gaps,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    pub(crate) fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        self.layouts[layout]
            .members
            .iter()
            .position(|m| m.window == window)
            .map(|i| vec![i])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_engine::TraditionalLayoutSystem;
    use crate::layout_engine::systems::LayoutEdit;

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

    #[test]
    fn moving_past_the_stack_edge_leaves_the_move_to_the_engine() {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Horizontal);
        let layout = system.create_layout();
        let first = WindowId::new(1, 1);
        let last = WindowId::new(1, 2);
        for window in [first, last] {
            system.update(WindowChange::Add(layout, window, None));
        }
        for (direction, has_display_neighbor) in [(Direction::Right, false), (Direction::Up, true)]
        {
            assert!(
                !system
                    .edit(layout, LayoutEdit::Move {
                        direction,
                        has_display_neighbor
                    })
                    .changed()
            );
        }
        assert_eq!(system.windows(layout, WindowQuery::All), [first, last]);
        assert_eq!(system.selected_window(layout), Some(last));
    }

    #[test]
    fn legacy_stack_restoration_preserves_identity_selection_and_fullscreen() {
        #[derive(Serialize)]
        struct Legacy<'a> {
            inner: &'a TraditionalLayoutSystem,
            default_orientation: StackDefaultOrientation,
        }
        let mut inner = TraditionalLayoutSystem::default();
        let removed = inner.create_layout();
        let layout = inner.create_layout();
        inner.remove_layout(removed);
        inner.tiles.set_layout(inner.tiles.root(layout), LayoutKind::HorizontalStack);
        for window in [w(1), w(2), w(3)] {
            inner.update(WindowChange::Add(layout, window, None));
        }
        inner.focus(layout, Focus::Select(w(2))).handled;
        inner.edit(layout, LayoutEdit::Fullscreen { within_gaps: true }).into_windows();
        let expected = inner.tiles.container_tree(layout);
        let legacy = Legacy {
            inner: &inner,
            default_orientation: StackDefaultOrientation::Horizontal,
        };
        let mut restored =
            ron::from_str::<StackLayoutSystem>(&ron::to_string(&legacy).unwrap()).unwrap();
        {
            assert_eq!(restored.container_tree(layout), expected);
            assert_eq!(restored.selected_window(layout), Some(w(2)));
            let clone = restored.clone_layout(layout);
            assert_eq!(restored.selected_window(clone), Some(w(2)));
            assert!(restored.has_any_fullscreen_node(clone));
            restored.update(WindowChange::Remove(w(2)));
            assert_eq!(restored.selected_window(layout), Some(w(3)));
            let new_layout = restored.create_layout();
            assert!(!restored.contains_layout(removed));
            let reused = [clone, new_layout]
                .into_iter()
                .find(|layout| layout.data().as_ffi() as u32 == removed.data().as_ffi() as u32)
                .expect("a vacant saved slot must be reused");
            assert_ne!(reused, removed);
            assert_ne!(restored.container_tree(new_layout).node_id, expected.node_id);
        }
    }

    #[test]
    fn selecting_the_stack_retains_focus_history_and_presents_its_first_member() {
        let mut system = StackLayoutSystem::default();
        let layout = system.create_layout();
        for window in [w(1), w(2), w(3)] {
            system.add_window_after_selection(layout, window);
        }
        assert!(system.focus(layout, Focus::Ascend).handled);
        assert_eq!(system.selected_window(layout), Some(w(3)));
        assert_eq!(system.visible_windows_in_layout(layout), vec![w(1)]);
        assert_eq!(system.window_in_direction(layout, Direction::Left), Some(w(3)));
        assert_eq!(system.window_in_direction(layout, Direction::Right), Some(w(1)));
        assert!(system.focus(layout, Focus::Descend).handled);
        assert_eq!(system.visible_windows_in_layout(layout), vec![w(3)]);
        assert!(
            system
                .edit(layout, LayoutEdit::Move {
                    direction: Direction::Left,
                    has_display_neighbor: false
                })
                .changed()
        );
        assert_eq!(system.all_windows_in_layout(layout), vec![w(3), w(1), w(2)]);
        assert_eq!(
            system.container_tree(layout).layout_kind,
            Some(LayoutKind::HorizontalStack)
        );
        let survivor = WindowId::new(2, 1);
        system.add_window_after_selection(layout, survivor);
        system.select_window(layout, w(1));
        system.update(WindowChange::CloseApp(1));
        assert_eq!(system.all_windows_in_layout(layout), vec![survivor]);
        assert_eq!(system.selected_window(layout), Some(survivor));
    }

    #[test]
    fn fixed_window_caps_match_traditional_when_minimum_exceeds_locked_size() {
        use objc2_core_foundation::{CGPoint, CGSize};
        let mut system = StackLayoutSystem::default();
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        let constraints = HashMap::from_iter([(w(1), WindowLayoutConstraints {
            locked_width: 200.0,
            min_width: 300.0,
            ..Default::default()
        })]);
        let frames = system.calculate_layout(
            layout,
            CGRect::new(CGPoint::ZERO, CGSize::new(1000.0, 800.0)),
            12.0,
            &constraints,
            &GapSettings::default(),
            StackLineReserve::default(),
        );
        assert_eq!(frames[0].1.size.width, 200.0);
    }
}
