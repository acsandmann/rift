use nix::libc::pid_t;
use objc2_core_foundation::CGRect;
use serde::{Deserialize, Deserializer, Serialize};
use slotmap::{Key, KeyData, SlotMap};

use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{
    GapSettings, StackDefaultOrientation, StackLineReserve, WindowInsertionPoint,
    default_stack_orientation,
};
use crate::layout_engine::systems::{
    LayoutSystem, WindowLayoutConstraints, reconcile_app_membership,
};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{
    Direction, LayoutId, LayoutKind, ResizeOrientation, TraditionalLayoutSystem,
};
use crate::model::tree::NodeId;

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

    fn remove(&mut self, window: WindowId) {
        while let Some(index) = self.members.iter().position(|m| m.window == window) {
            let member = self.members.remove(index);
            if Some(member.id) == self.active {
                self.active = self.members.get(index).or_else(|| self.members.last()).map(|m| m.id);
                self.root_selected = false;
            }
        }
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

impl<'de> Deserialize<'de> for StackLayoutSystem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
            d: D,
        ) -> Result<Option<T>, D::Error> {
            T::deserialize(d).map(Some)
        }
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            layouts: SlotMap<LayoutId, Stack>,
            #[serde(default = "first_node")]
            next_node: u32,
            #[serde(default = "default_stack_orientation")]
            default_orientation: StackDefaultOrientation,
            #[serde(default, deserialize_with = "present")]
            inner: Option<TraditionalLayoutSystem>,
        }
        fn first_node() -> u32 { 1 }
        let Wire {
            layouts,
            next_node,
            default_orientation,
            inner,
        } = Wire::deserialize(deserializer)?;
        let mut system = match inner {
            None => Self {
                layouts,
                next_node,
                default_orientation,
                inactive: Default::default(),
                window_insertion_point: Default::default(),
            },
            Some(mut inner) => {
                // SlotMap's serialized slots carry the generations of saved LayoutIds.
                // Rewrite only their values, retaining holes and generations verbatim.
                let mut slots =
                    serde_json::to_value(&inner.layout_roots).map_err(serde::de::Error::custom)?;
                let mut next_node = 1;
                for (layout, _) in &inner.layout_roots {
                    let snapshot = inner.container_tree(layout);
                    let mut members = Vec::new();
                    fn read(
                        node: &rift_protocol::ContainerTreeNode,
                        out: &mut Vec<Member>,
                        next: &mut u32,
                    ) {
                        *next = (*next).max((node.node_id as u32).saturating_add(1));
                        if let Some(window) = node.window_id {
                            out.push(Member {
                                id: NodeId::from(KeyData::from_ffi(node.node_id)),
                                window: WindowId::new(window.pid, window.idx),
                                weight: node.weight.unwrap_or(1.0),
                                fullscreen: node.is_fullscreen,
                                within_gaps: node.is_fullscreen_within_gaps,
                            });
                        }
                        for child in &node.children {
                            read(child, out, next);
                        }
                    }
                    read(&snapshot, &mut members, &mut next_node);
                    let selected = inner.selected_window(layout);
                    let active = members.iter().find(|m| Some(m.window) == selected).map(|m| m.id);
                    let stack = Stack {
                        id: NodeId::from(KeyData::from_ffi(snapshot.node_id)),
                        members,
                        active,
                        root_selected: snapshot.is_selected,
                        kind: Self::stack_kind(
                            snapshot.layout_kind.unwrap_or(LayoutKind::VerticalStack),
                        ),
                        fullscreen: snapshot.is_fullscreen,
                        within_gaps: snapshot.is_fullscreen_within_gaps,
                    };
                    slots[layout.data().as_ffi() as u32 as usize]["value"] =
                        serde_json::to_value(stack).map_err(serde::de::Error::custom)?;
                }
                let layouts = serde_json::from_value(slots).map_err(serde::de::Error::custom)?;
                let old: Vec<_> = inner.layout_roots.keys().collect();
                for layout in old {
                    inner.remove_layout(layout);
                }
                Self {
                    layouts,
                    next_node,
                    default_orientation,
                    inactive: Default::default(),
                    window_insertion_point: Default::default(),
                }
            }
        };
        // Transient participation is re-established by the engine after restoration.
        system.inactive.clear();
        Ok(system)
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
        let enabled = if within {
            *gaps = !*gaps;
            if *gaps {
                *full = false;
            }
            *gaps
        } else {
            *full = !*full;
            if *full {
                *gaps = false;
            }
            *full
        };
        if enabled {
            self.visible_windows_under_selection(layout)
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
    fn window_participates(&self, window: WindowId) -> bool { !self.inactive.contains(&window) }

    fn set_window_participation(&mut self, window: WindowId, participates: bool) -> bool {
        if participates {
            self.inactive.remove(&window)
        } else {
            self.inactive.insert(window)
        }
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

    fn draw_tree(&self, layout: LayoutId) -> String { format!("{:#?}", self.layouts[layout]) }

    fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
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
        let axis = if horizontal {
            rect.size.width
        } else {
            rect.size.height
        };
        let required = constraints
            .get(&stack.members[active].window)
            .copied()
            .map(|c| {
                let c = c.normalized();
                c.fixed_for_axis(horizontal).unwrap_or(if c.is_resizable {
                    0.0
                } else {
                    c.min_for_axis(horizontal)
                })
            })
            .unwrap_or(0.0)
            .clamp(0.0, axis.max(0.0));
        let offset = if count > 1 {
            offset.min((axis - required).max(0.0) / (count - 1) as f64)
        } else {
            offset
        };
        let geometry =
            super::traditional::stack_layout_result(rect, count, offset, horizontal, line);
        stack
            .members
            .iter()
            .filter(|m| self.window_participates(m.window))
            .enumerate()
            .map(|(i, m)| {
                let mut frame = if m.fullscreen {
                    screen
                } else if m.within_gaps {
                    tiling
                } else {
                    geometry.get_frame_for_index(i)
                };
                if let Some(c) = constraints.get(&m.window).copied() {
                    let c = c.normalized();
                    for horizontal in [true, false] {
                        let size = if horizontal {
                            &mut frame.size.width
                        } else {
                            &mut frame.size.height
                        };
                        let min = if c.is_resizable && c.fixed_for_axis(horizontal).is_none() {
                            0.0
                        } else {
                            c.min_for_axis(horizontal)
                        };
                        let desired = c.fixed_for_axis(horizontal).unwrap_or(*size).max(min);
                        let max = c.max_for_axis(horizontal);
                        *size = (if max > 0.0 { desired.min(max) } else { desired })
                            .min(*size)
                            .max(0.0);
                    }
                }
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

    fn all_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.layouts[layout].members.iter().map(|m| m.window).collect()
    }

    fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        self.layouts[layout]
            .members
            .iter()
            .position(|m| m.window == window)
            .map(|i| vec![i])
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        let stack = &self.layouts[layout];
        stack
            .visible_index(&self.inactive)
            .map(|i| stack.members[i].window)
            .into_iter()
            .collect()
    }

    fn stack_members(&self, layout: LayoutId, window: WindowId) -> Vec<WindowId> {
        if self.contains_window(layout, window) {
            self.all_windows_in_layout(layout)
        } else {
            Vec::new()
        }
    }

    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        self.visible_windows_in_layout(layout)
    }

    fn ascend_selection(&mut self, layout: LayoutId) -> bool {
        if self.selected_window(layout).is_none() {
            return false;
        }
        let s = &mut self.layouts[layout];
        let changed = !s.root_selected;
        s.root_selected = true;
        changed
    }

    fn descend_selection(&mut self, layout: LayoutId) -> bool {
        if self.selected_window(layout).is_none() {
            return false;
        }
        let s = &mut self.layouts[layout];
        let changed = s.root_selected;
        s.root_selected = false;
        changed
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

    fn add_window_after_selection(&mut self, layout: LayoutId, window: WindowId) {
        let id = self.node_id();
        let s = &mut self.layouts[layout];
        let index = if self.window_insertion_point == WindowInsertionPoint::NextToSelection {
            s.members
                .iter()
                .position(|m| Some(m.id) == s.active)
                .map(|i| i + 1)
                .unwrap_or(s.members.len())
        } else {
            s.members.len()
        };
        s.members.insert(index, Member {
            id,
            window,
            weight: 1.0,
            fullscreen: false,
            within_gaps: false,
        });
        s.active = Some(id);
        s.root_selected = false;
    }

    fn replace_window(&mut self, from: WindowId, to: WindowId) {
        if self.inactive.remove(&from) {
            self.inactive.insert(to);
        }
        for s in self.layouts.values_mut() {
            for m in &mut s.members {
                if m.window == from {
                    m.window = to;
                }
            }
        }
    }

    fn remove_window(&mut self, window: WindowId) {
        self.inactive.remove(&window);
        for s in self.layouts.values_mut() {
            s.remove(window);
        }
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        self.inactive.retain(|w| w.pid != pid);
        for s in self.layouts.values_mut() {
            let removed: Vec<_> =
                s.members.iter().filter(|m| m.window.pid == pid).map(|m| m.window).collect();
            for w in removed {
                s.remove(w);
            }
        }
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        let current = self.layouts[layout]
            .members
            .iter()
            .filter(|m| m.window.pid == pid)
            .map(|m| m.window)
            .collect();
        let delta =
            reconcile_app_membership(pid, current, desired, |w| self.window_participates(w));
        for w in delta.removals {
            if !self.layouts[layout].members.iter().any(|m| m.window == w && m.fullscreen) {
                self.layouts[layout].remove(w);
            }
        }
        for w in delta.additions {
            self.add_window_after_selection(layout, w);
        }
    }

    fn contains_window(&self, layout: LayoutId, window: WindowId) -> bool {
        self.layouts[layout].members.iter().any(|m| m.window == window)
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
        let tiling = compute_tiling_area(screen, gaps);
        if new == screen {
            m.fullscreen = true;
            m.within_gaps = false;
        } else if old == screen {
            m.fullscreen = false;
        } else if new == tiling {
            m.within_gaps = true;
            m.fullscreen = false;
        } else if old == tiling {
            m.within_gaps = false;
        }
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

    fn apply_target_drop(
        &mut self,
        layout: LayoutId,
        source: WindowId,
        target: WindowId,
        _action: crate::layout_engine::WindowDropAction,
    ) -> bool {
        source != target
            && self.swap_windows(layout, source, target)
            && self.select_window(layout, source)
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let s = &mut self.layouts[layout];
        if s.root_selected {
            return false;
        }
        let Some(i) = s.active_index(&self.inactive) else {
            return false;
        };
        let target = if let Some(target) = s.neighbor(direction, &self.inactive) {
            target
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
    }

    fn move_selection_to_layout_after_selection(&mut self, from: LayoutId, to: LayoutId) {
        if from == to || self.layouts[from].root_selected {
            return;
        }
        let Some(i) = self.layouts[from].active_index(&self.inactive) else {
            return;
        };
        let m = self.layouts[from].members[i].clone();
        self.layouts[from].remove(m.window);
        let s = &mut self.layouts[to];
        let index = s
            .members
            .iter()
            .position(|m| Some(m.id) == s.active)
            .map(|i| i + 1)
            .unwrap_or(s.members.len());
        s.active = Some(m.id);
        s.root_selected = false;
        s.members.insert(index, m);
    }

    fn toggle_fullscreen_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        self.toggle_fullscreen(layout, false)
    }

    fn toggle_fullscreen_within_gaps_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        self.toggle_fullscreen(layout, true)
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

    fn apply_stacking_to_parent_of_selection(
        &mut self,
        layout: LayoutId,
        _orientation: StackDefaultOrientation,
    ) -> Vec<WindowId> {
        self.toggle_orientation(layout);
        self.visible_windows_in_layout(layout)
    }

    fn parent_of_selection_is_stacked(&self, _layout: LayoutId) -> bool { true }

    fn resize_selection_by(
        &mut self,
        _layout: LayoutId,
        _amount: f64,
        _orientation: ResizeOrientation,
    ) {
    }

    fn toggle_tile_orientation(&mut self, layout: LayoutId) { self.toggle_orientation(layout); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_engine::ResizeOrientation;

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

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
        inner.set_layout(inner.root(layout), LayoutKind::HorizontalStack);
        for window in [w(1), w(2), w(3)] {
            inner.add_window_after_selection(layout, window);
        }
        inner.select_window(layout, w(2));
        inner.toggle_fullscreen_within_gaps_of_selection(layout);
        let expected = inner.container_tree(layout);
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
            restored.remove_window(w(2));
            assert_eq!(restored.selected_window(layout), Some(w(3)));
            let new_layout = restored.create_layout();
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
        assert!(system.ascend_selection(layout));
        assert_eq!(system.selected_window(layout), Some(w(3)));
        assert_eq!(system.visible_windows_in_layout(layout), vec![w(1)]);
        assert_eq!(system.window_in_direction(layout, Direction::Left), Some(w(3)));
        assert_eq!(system.window_in_direction(layout, Direction::Right), Some(w(1)));
        assert!(system.descend_selection(layout));
        assert_eq!(system.visible_windows_in_layout(layout), vec![w(3)]);
        assert!(system.move_selection(layout, Direction::Left));
        assert_eq!(system.all_windows_in_layout(layout), vec![w(3), w(1), w(2)]);
        assert_eq!(
            system.container_tree(layout).layout_kind,
            Some(LayoutKind::HorizontalStack)
        );
    }

    #[test]
    fn create_layout_starts_as_stack() {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Perpendicular);
        let layout = system.create_layout();
        assert_eq!(
            system.container_tree(layout).layout_kind,
            Some(LayoutKind::VerticalStack)
        );
    }

    #[test]
    fn create_layout_honors_stack_default_orientation_setting() {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Horizontal);
        let layout = system.create_layout();
        assert_eq!(
            system.container_tree(layout).layout_kind,
            Some(LayoutKind::HorizontalStack)
        );
    }

    #[test]
    fn toggle_stack_command_path_keeps_layout_stacked() {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Perpendicular);
        let layout = system.create_layout();

        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));

        let _ = system.unstack_parent_of_selection(
            layout,
            crate::common::config::StackDefaultOrientation::Perpendicular,
        );
        let _ = system.apply_stacking_to_parent_of_selection(
            layout,
            crate::common::config::StackDefaultOrientation::Perpendicular,
        );

        assert!(system.container_tree(layout).layout_kind.unwrap().is_stacked());
    }

    #[test]
    fn set_windows_for_app_noop_keeps_fullscreen_state() {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Perpendicular);
        let layout = system.create_layout();

        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        let _ = system.toggle_fullscreen_of_selection(layout);
        assert!(system.has_any_fullscreen_node(layout));

        system.set_windows_for_app(layout, 1, vec![w(1), w(2)]);

        assert!(system.has_any_fullscreen_node(layout));
    }

    #[test]
    fn set_windows_for_app_honors_updated_end_of_tree_policy() {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Perpendicular);
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        system.select_window(layout, w(1));

        system.update_settings(
            StackDefaultOrientation::Perpendicular,
            WindowInsertionPoint::EndOfTree,
        );
        system.set_windows_for_app(layout, 1, vec![w(1), w(2), w(3)]);

        assert_eq!(system.all_windows_in_layout(layout), vec![w(1), w(2), w(3)]);
    }

    fn setup_fullscreen_stack_system() -> (StackLayoutSystem, LayoutId) {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Perpendicular);
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        let _ = system.toggle_fullscreen_of_selection(layout);
        assert!(system.has_any_fullscreen_node(layout));
        (system, layout)
    }

    #[test]
    fn resize_selection_noop_keeps_fullscreen_state() {
        let (mut system, layout) = setup_fullscreen_stack_system();
        system.resize_selection_by(layout, 0.1, ResizeOrientation::Horizontal);
        assert!(system.has_any_fullscreen_node(layout));
    }

    #[test]
    fn rebalance_noop_keeps_fullscreen_state() {
        let (mut system, layout) = setup_fullscreen_stack_system();
        system.rebalance(layout);
        assert!(system.has_any_fullscreen_node(layout));
    }

    #[test]
    fn split_selection_noop_keeps_fullscreen_state() {
        let (mut system, layout) = setup_fullscreen_stack_system();
        system.split_selection(layout, LayoutKind::Horizontal);
        assert!(system.has_any_fullscreen_node(layout));
    }

    #[test]
    fn join_selection_noop_keeps_fullscreen_state() {
        let (mut system, layout) = setup_fullscreen_stack_system();
        system.join_selection_with_direction(layout, Direction::Right);
        assert!(system.has_any_fullscreen_node(layout));
    }

    #[test]
    fn unjoin_selection_noop_keeps_fullscreen_state() {
        let (mut system, layout) = setup_fullscreen_stack_system();
        system.unjoin_selection(layout);
        assert!(system.has_any_fullscreen_node(layout));
    }
}
