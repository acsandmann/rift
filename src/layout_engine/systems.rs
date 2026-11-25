use enum_dispatch::enum_dispatch;
use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{GapSettings, StackDefaultOrientation};
use crate::layout_engine::{Direction, LayoutKind, ResizeOrientation, WindowDropAction};

#[derive(Serialize, Deserialize)]
struct SavedSlot<T> {
    value: Option<T>,
    version: u32,
}

fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

slotmap::new_key_type! { pub struct LayoutId; }

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WindowLayoutConstraints {
    pub is_resizable: bool,
    pub locked_width: f64,
    pub locked_height: f64,
    pub min_width: f64,
    pub min_height: f64,
    pub max_width: f64,
    pub max_height: f64,
}

impl WindowLayoutConstraints {
    pub fn normalized(self) -> Self {
        let clean = |v: f64| if v.is_finite() { v.max(0.0) } else { 0.0 };
        let min_width = clean(self.min_width);
        let min_height = clean(self.min_height);
        let mut max_width = clean(self.max_width);
        let mut max_height = clean(self.max_height);
        if max_width > 0.0 && max_width < min_width {
            max_width = min_width;
        }
        if max_height > 0.0 && max_height < min_height {
            max_height = min_height;
        }
        Self {
            is_resizable: self.is_resizable,
            locked_width: clean(self.locked_width),
            locked_height: clean(self.locked_height),
            min_width,
            min_height,
            max_width,
            max_height,
        }
    }

    pub fn min_for_axis(self, horizontal: bool) -> f64 {
        if horizontal {
            self.min_width
        } else {
            self.min_height
        }
    }

    pub fn max_for_axis(self, horizontal: bool) -> f64 {
        if horizontal {
            self.max_width
        } else {
            self.max_height
        }
    }

    pub fn fixed_for_axis(self, horizontal: bool) -> Option<f64> {
        let locked = if horizontal {
            self.locked_width
        } else {
            self.locked_height
        };
        let min = self.min_for_axis(horizontal);
        let max = self.max_for_axis(horizontal);
        // Axis-specific lock: when min/max collapse to the same positive value,
        // treat that axis as fixed even if the window is generally resizable.
        if min > 0.0 && max > 0.0 && (min - max).abs() <= f64::EPSILON {
            return Some(max);
        }
        if !self.is_resizable {
            return (locked > 0.0).then_some(locked);
        }
        None
    }

    pub fn resizable_for_axis(self, horizontal: bool) -> bool {
        self.fixed_for_axis(horizontal).is_none()
    }

    pub fn resizable_any_axis(self) -> bool {
        self.resizable_for_axis(true) || self.resizable_for_axis(false)
    }
}

fn update_participation(
    inactive: &mut HashSet<WindowId>,
    window: WindowId,
    participates: bool,
) -> bool {
    if participates {
        inactive.remove(&window)
    } else {
        inactive.insert(window)
    }
}

fn replace_participation(inactive: &mut HashSet<WindowId>, from: WindowId, to: WindowId) {
    if inactive.remove(&from) {
        inactive.insert(to);
    }
}

#[derive(Clone, Copy, Debug)]
pub enum WindowQuery {
    All,
    Visible,
    Selection,
    Stack(WindowId),
}

pub enum WindowChange<'a> {
    Add(LayoutId, WindowId, Option<Direction>),
    Remove(WindowId),
    RemoveAndRebalance(WindowId),
    Replace(WindowId, WindowId),
    CloseApp(pid_t),
    Resize {
        layout: LayoutId,
        window: WindowId,
        old: CGRect,
        new: CGRect,
        screen: CGRect,
        gaps: &'a GapSettings,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum Focus {
    Select(WindowId),
    Ascend,
    Descend,
    Direction(Direction),
}

#[derive(Debug)]
pub struct FocusResult {
    pub handled: bool,
    pub window: Option<WindowId>,
    pub raise: Vec<WindowId>,
}

impl FocusResult {
    pub fn selection(handled: bool) -> Self {
        Self {
            handled,
            window: None,
            raise: Vec::new(),
        }
    }

    pub fn movement((window, raise): (Option<WindowId>, Vec<WindowId>)) -> Self {
        Self {
            handled: window.is_some(),
            window,
            raise,
        }
    }

    pub fn into_move(self) -> (Option<WindowId>, Vec<WindowId>) { (self.window, self.raise) }
}

#[derive(Clone, Copy, Debug)]
pub enum GroupEdit {
    Join(Direction),
    ConsumeOrExpel(Direction),
    Stack(StackDefaultOrientation),
    Unstack(StackDefaultOrientation),
    Unjoin,
}

#[derive(Clone, Copy, Debug)]
pub enum LayoutEdit {
    Move {
        direction: Direction,
        has_display_neighbor: bool,
    },
    Split(LayoutKind),
    Fullscreen {
        within_gaps: bool,
    },
    Swap(WindowId, WindowId),
    ReorderTab {
        window: WindowId,
        target: WindowId,
    },
    Drop {
        source: WindowId,
        target: WindowId,
        action: WindowDropAction,
    },
    Group(GroupEdit),
    Resize {
        amount: f64,
        orientation: ResizeOrientation,
    },
    Rebalance,
    ToggleOrientation,
}

#[derive(Debug)]
pub enum EditResult {
    Unsupported,
    Unchanged,
    Changed,
    Raise(Vec<WindowId>),
}

impl From<bool> for EditResult {
    fn from(changed: bool) -> Self {
        if changed {
            Self::Changed
        } else {
            Self::Unchanged
        }
    }
}

impl EditResult {
    pub fn changed(&self) -> bool { matches!(self, Self::Changed | Self::Raise(_)) }

    pub fn into_windows(self) -> Vec<WindowId> {
        if let Self::Raise(windows) = self {
            windows
        } else {
            Vec::new()
        }
    }

    pub fn discard(self) {}
}

/// Toggle one fullscreen mode and clear the other when enabling it.
fn toggle_fullscreen_mode(full: &mut bool, gaps: &mut bool, within_gaps: bool) -> bool {
    let (target, other) = if within_gaps {
        (gaps, full)
    } else {
        (full, gaps)
    };
    *target = !*target;
    if *target {
        *other = false;
    }
    *target
}

/// Return whether a native resize entered or left a fullscreen frame.
fn track_fullscreen_resize(
    full: &mut bool,
    within_gaps: &mut bool,
    old: CGRect,
    new: CGRect,
    screen: CGRect,
    gaps: &GapSettings,
) -> bool {
    if new == screen {
        (*full, *within_gaps) = (true, false);
    } else if old == screen {
        *full = false;
    } else {
        let tiling = super::utils::compute_tiling_area(screen, gaps);
        if new == tiling {
            (*full, *within_gaps) = (false, true);
        } else if old == tiling {
            *within_gaps = false;
        } else {
            return false;
        }
    }
    true
}

#[enum_dispatch]
pub trait LayoutSystem: Serialize + for<'de> Deserialize<'de> {
    fn window_participates(&self, _window: WindowId) -> bool { true }

    fn set_window_participation(&mut self, _window: WindowId, _participates: bool) -> bool { false }

    fn create_layout(&mut self) -> LayoutId;

    fn contains_layout(&self, layout: LayoutId) -> bool;

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId;

    fn remove_layout(&mut self, layout: LayoutId);

    fn prepare_layout(
        &mut self,
        _layout: LayoutId,
        _screen: CGRect,
        _constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        _gaps: &crate::common::config::GapSettings,
    ) {
    }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)>;

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId>;

    fn selection_contains_window(&self, layout: LayoutId, window: WindowId) -> bool {
        self.selected_window(layout) == Some(window)
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId>;

    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool;

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool;

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool;

    fn parent_of_selection_is_stacked(&self, _layout: LayoutId) -> bool { false }

    fn windows(&self, layout: LayoutId, query: WindowQuery) -> Vec<WindowId>;
    fn focus(&mut self, layout: LayoutId, focus: Focus) -> FocusResult;
    fn update(&mut self, change: WindowChange<'_>);
    fn edit_layout(&mut self, layout: LayoutId, edit: LayoutEdit) -> EditResult;

    fn edit(&mut self, layout: LayoutId, edit: LayoutEdit) -> EditResult {
        let LayoutEdit::Drop { source, target, action } = edit else {
            return self.edit_layout(layout, edit);
        };
        if !self.window_participates(source) || !self.window_participates(target) {
            return EditResult::Unchanged;
        }
        let translated = match action {
            WindowDropAction::Move(direction) => LayoutEdit::Move {
                direction,
                has_display_neighbor: false,
            },
            WindowDropAction::ConsumeOrExpel(direction) => {
                LayoutEdit::Group(GroupEdit::ConsumeOrExpel(direction))
            }
            _ => return self.edit_layout(layout, edit),
        };
        if !self.focus(layout, Focus::Select(source)).handled {
            return EditResult::Unchanged;
        }
        self.edit_layout(layout, translated)
    }
}

pub(super) fn node_slot(
    mut node: crate::model::tree::NodeId,
    root: crate::model::tree::NodeId,
    map: &crate::model::tree::NodeMap,
) -> Option<Vec<usize>> {
    let mut path = Vec::new();
    while node != root {
        let parent = node.parent(map)?;
        path.push(parent.children(map).position(|child| child == node)?);
        node = parent;
    }
    path.reverse();
    Some(path)
}

mod groups;
mod tiles;
mod traditional;
pub use traditional::TraditionalLayoutSystem;
mod bsp;
pub(crate) mod constraints;
pub use bsp::BspLayoutSystem;
mod master_stack;
pub use master_stack::MasterStackLayoutSystem;
pub(crate) mod scrolling;
pub use scrolling::{ScrollingLayoutSystem, ViewportRelease};

#[cfg(test)]
mod tests {
    use super::{
        BspLayoutSystem, LayoutSystem, MasterStackLayoutSystem, ScrollingLayoutSystem,
        TraditionalLayoutSystem,
    };
    use crate::actor::app::WindowId;
    use crate::common::config::{ScrollingLayoutSettings, WindowInsertionPoint};
    use crate::layout_engine::systems::{Focus, WindowChange, WindowQuery};

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

    #[test]
    fn common_insertion_point_controls_tree_and_linear_layouts() {
        let mut traditional = TraditionalLayoutSystem::new(WindowInsertionPoint::EndOfTree, false);
        let traditional_layout = traditional.create_layout();
        traditional.update(WindowChange::Add(traditional_layout, w(1), None));
        traditional.update(WindowChange::Add(traditional_layout, w(2), None));
        traditional.focus(traditional_layout, Focus::Select(w(1))).handled;
        traditional.update(WindowChange::Add(traditional_layout, w(3), None));
        assert_eq!(traditional.windows(traditional_layout, WindowQuery::All), vec![
            w(1),
            w(2),
            w(3)
        ]);

        let mut scrolling_settings = ScrollingLayoutSettings::default();
        scrolling_settings.base.window_insertion_point = Some(WindowInsertionPoint::EndOfTree);
        let mut scrolling = ScrollingLayoutSystem::new(&scrolling_settings);
        let scrolling_layout = scrolling.create_layout();
        scrolling.update(WindowChange::Add(scrolling_layout, w(1), None));
        scrolling.update(WindowChange::Add(scrolling_layout, w(2), None));
        scrolling.focus(scrolling_layout, Focus::Select(w(1))).handled;
        scrolling.update(WindowChange::Add(scrolling_layout, w(3), None));
        assert_eq!(scrolling.windows(scrolling_layout, WindowQuery::All), vec![
            w(1),
            w(2),
            w(3)
        ]);
    }

    fn window_nodes(
        tree: &rift_protocol::ContainerTreeNode,
    ) -> Vec<&rift_protocol::ContainerTreeNode> {
        let mut windows = Vec::new();
        if tree.node_type == rift_protocol::ContainerNodeType::Window {
            windows.push(tree);
        }
        for child in &tree.children {
            windows.extend(window_nodes(child));
        }
        windows
    }

    fn node_ids(tree: &rift_protocol::ContainerTreeNode) -> Vec<u64> {
        let mut ids = vec![tree.node_id];
        for child in &tree.children {
            ids.extend(node_ids(child));
        }
        ids
    }

    fn assert_stable_unique_ids(
        before: &rift_protocol::ContainerTreeNode,
        after: &rift_protocol::ContainerTreeNode,
    ) {
        let before_ids = node_ids(before);
        let after_ids = node_ids(after);
        assert_eq!(before_ids, after_ids);
        let unique: std::collections::HashSet<_> = after_ids.iter().copied().collect();
        assert_eq!(unique.len(), after_ids.len());
    }

    #[test]
    fn normalized_container_trees_expose_layout_topology() {
        let mut traditional = TraditionalLayoutSystem::default();
        let layout = traditional.create_layout();
        traditional.update(WindowChange::Add(layout, w(1), None));
        traditional.update(WindowChange::Add(layout, w(2), None));
        let tree = traditional.tiles.container_tree(layout);
        assert_eq!(tree.node_type, rift_protocol::ContainerNodeType::Container);
        assert_eq!(window_nodes(&tree).len(), 2);
        assert_eq!(
            window_nodes(&tree).iter().filter(|node| node.is_selected).count(),
            1
        );
        traditional.focus(layout, Focus::Select(w(1))).handled;
        assert_stable_unique_ids(&tree, &traditional.tiles.container_tree(layout));

        let mut bsp = BspLayoutSystem::default();
        let layout = bsp.create_layout();
        bsp.update(WindowChange::Add(layout, w(1), None));
        bsp.update(WindowChange::Add(layout, w(2), None));
        let tree = bsp.container_tree(layout);
        assert_eq!(tree.children.len(), 2);
        let total_weight: f64 = tree.children.iter().filter_map(|node| node.weight).sum();
        assert!((total_weight - 1.0).abs() < f64::EPSILON);
        bsp.focus(layout, Focus::Select(w(1))).handled;
        assert_stable_unique_ids(&tree, &bsp.container_tree(layout));

        let mut master_stack = MasterStackLayoutSystem::default();
        let layout = master_stack.create_layout();
        master_stack.update(WindowChange::Add(layout, w(1), None));
        master_stack.update(WindowChange::Add(layout, w(2), None));
        let tree = master_stack.container_tree(layout);
        let roles: Vec<_> = tree.children.iter().filter_map(|node| node.role.as_deref()).collect();
        assert!(roles.contains(&"master"), "{tree:#?}");
        assert!(roles.contains(&"stack"), "{tree:#?}");

        let mut scrolling = ScrollingLayoutSystem::default();
        let layout = scrolling.create_layout();
        scrolling.update(WindowChange::Add(layout, w(1), None));
        scrolling.update(WindowChange::Add(layout, w(2), None));
        let tree = scrolling.container_tree(layout);
        assert!(tree.children.iter().all(|node| node.role.as_deref() == Some("column")));
        assert_eq!(window_nodes(&tree).len(), 2);
        scrolling.focus(layout, Focus::Select(w(1))).handled;
        assert_stable_unique_ids(&tree, &scrolling.container_tree(layout));
    }
}
mod floating;
pub use floating::FloatingLayoutSystem;
mod stack;
pub use stack::StackLayoutSystem;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Debug)]
#[enum_dispatch(LayoutSystem)]
pub enum LayoutSystemKind {
    Traditional(TraditionalLayoutSystem),
    Bsp(BspLayoutSystem),
    MasterStack(MasterStackLayoutSystem),
    Scrolling(ScrollingLayoutSystem),
    Stack(StackLayoutSystem),
    Floating(FloatingLayoutSystem),
}

impl LayoutSystemKind {
    /// Seed a freshly loaded layout's participation without editing its saved viewport.
    pub(crate) fn initialize_dormant_windows(&mut self, windows: Vec<WindowId>) {
        if let Self::Scrolling(system) = self {
            system.initialize_dormant_windows(windows);
        } else {
            for window in windows {
                self.set_window_participation(window, false);
            }
        }
    }

    pub(crate) fn preview_clone(&self) -> Option<Self> {
        match self {
            Self::Traditional(system) => Some(Self::Traditional(system.clone())),
            Self::Bsp(system) => Some(Self::Bsp(system.clone())),
            Self::MasterStack(system) => Some(Self::MasterStack(system.clone())),
            Self::Scrolling(system) => Some(Self::Scrolling(system.clone())),
            Self::Stack(system) => Some(Self::Stack(system.clone())),
            Self::Floating(_) => None,
        }
    }
}

#[cfg(test)]
mod participation_tests {
    use objc2_core_foundation::{CGPoint, CGSize};

    use super::*;
    use crate::layout_engine::systems::{Focus, LayoutEdit, WindowChange, WindowQuery};

    #[test]
    fn suspended_members_preserve_slots_and_geometry_in_every_backend() {
        let mut systems = [
            LayoutSystemKind::Traditional(Default::default()),
            LayoutSystemKind::Bsp(Default::default()),
            LayoutSystemKind::Stack(Default::default()),
            LayoutSystemKind::MasterStack(Default::default()),
            LayoutSystemKind::Scrolling(Default::default()),
            LayoutSystemKind::Floating(Default::default()),
        ];
        let screen = CGRect::new(CGPoint::ZERO, CGSize::new(1200.0, 800.0));
        let constraints = HashMap::default();
        let mut gaps = crate::common::config::GapSettings::default();
        gaps.inner.horizontal = 16.0;
        gaps.inner.vertical = 12.0;
        for (system, mode) in systems.iter_mut().zip([
            "traditional",
            "bsp",
            "stack",
            "master_stack",
            "scrolling",
            "floating",
        ]) {
            let layout = system.create_layout();
            let frames = |system: &mut LayoutSystemKind| {
                system.prepare_layout(layout, screen, &constraints, &gaps);
                system.calculate_layout(
                    layout,
                    screen,
                    20.0,
                    &constraints,
                    &gaps,
                    Default::default(),
                )
            };
            let windows = [1, 2, 3, 4].map(|i| WindowId::new(1, i));
            for (index, &window) in windows.iter().enumerate() {
                if index == 2 {
                    system.edit(layout, LayoutEdit::Split(LayoutKind::Vertical)).discard();
                }
                system.update(WindowChange::Add(layout, window, None));
            }
            system.focus(layout, Focus::Select(windows[1])).handled;
            system
                .edit(layout, LayoutEdit::Resize {
                    amount: 0.13,
                    orientation: ResizeOrientation::Horizontal,
                })
                .discard();
            system.focus(layout, Focus::Select(windows[0])).handled;
            let slots: Vec<_> = windows.iter().map(|&w| system.window_slot(layout, w)).collect();
            let original = frames(system);
            for &window in &windows[1..] {
                system.set_window_participation(window, false);
            }
            let projected = frames(system);
            assert_eq!(
                projected.iter().map(|(w, _)| *w).collect::<Vec<_>>(),
                vec![windows[0]],
                "{mode}"
            );
            if mode != "scrolling" && mode != "floating" {
                assert_eq!(
                    projected[0].1,
                    crate::layout_engine::utils::compute_tiling_area(screen, &gaps),
                    "{mode}: an inactive subtree cannot reserve geometry"
                );
            }
            for index in [2, 1, 3] {
                system.set_window_participation(windows[index], true);
            }
            assert_eq!(frames(system), original, "{mode}");
            assert_eq!(
                windows.iter().map(|&w| system.window_slot(layout, w)).collect::<Vec<_>>(),
                slots
            );

            for &window in &windows {
                system.set_window_participation(window, false);
            }
            assert!(frames(system).is_empty());
            for &window in windows.iter().rev() {
                system.set_window_participation(window, true);
            }
            assert_eq!(
                windows.iter().map(|&w| system.window_slot(layout, w)).collect::<Vec<_>>(),
                slots
            );

            let returned = frames(system);
            assert_eq!(
                returned.iter().map(|(w, frame)| (*w, frame.size)).collect::<Vec<_>>(),
                original.iter().map(|(w, frame)| (*w, frame.size)).collect::<Vec<_>>(),
                "{mode}"
            );

            system.set_window_participation(windows[1], false);
            assert!(!system.focus(layout, Focus::Select(windows[1])).handled);
            assert!(!system.edit(layout, LayoutEdit::Swap(windows[0], windows[1])).changed());
            let replacement = WindowId::new(1, 20);
            system.update(WindowChange::Replace(windows[1], replacement));
            assert_eq!(system.window_slot(layout, replacement), slots[1]);
            assert!(!system.window_participates(replacement));
            system.update(WindowChange::Remove(windows[2]));
            let inserted = WindowId::new(1, 5);
            system.update(WindowChange::Add(layout, inserted, None));
            assert!(
                system.contains_window(layout, replacement),
                "inventory must retain suspended members"
            );
            assert!(!system.contains_window(layout, windows[2]));
            let serialized = ron::ser::to_string(&system).unwrap();
            let mut restarted: LayoutSystemKind = ron::de::from_str(&serialized).unwrap();
            assert!(restarted.contains_window(layout, replacement));
            assert!(!restarted.contains_window(layout, windows[1]));
            restarted.update(WindowChange::CloseApp(1));
            assert!(restarted.windows(layout, WindowQuery::All).is_empty());
            system.set_window_participation(replacement, true);
            system.update(WindowChange::Remove(replacement));
            assert!(!system.contains_window(layout, replacement));
            assert!(system.contains_window(layout, inserted));
            let retained = system.windows(layout, WindowQuery::All);
            for &window in &retained {
                system.set_window_participation(window, false);
            }
            let newcomer = WindowId::new(1, 6);
            system.update(WindowChange::Add(layout, newcomer, None));
            assert_eq!(frames(system).iter().map(|(w, _)| *w).collect::<Vec<_>>(), vec![
                newcomer
            ]);
            for &window in &retained {
                system.set_window_participation(window, true);
                assert!(system.contains_window(layout, window));
            }
        }
    }
}

// Inspection is deliberately outside the ordinary layout interface.
impl LayoutSystemKind {
    pub fn draw_tree(&self, layout: LayoutId) -> String {
        match self {
            Self::Traditional(system) => system.tiles.draw_tree(layout, &[]),
            Self::Bsp(system) => system.draw_tree(layout),
            Self::Stack(system) => system.draw_tree(layout),
            Self::MasterStack(system) => system.draw_tree(layout),
            Self::Scrolling(system) => system.draw_tree(layout),
            Self::Floating(system) => system.groups.draw_tree(layout, &[]),
        }
    }

    pub fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        match self {
            Self::Traditional(system) => system.tiles.container_tree(layout),
            Self::Bsp(system) => system.container_tree(layout),
            Self::Stack(system) => system.container_tree(layout),
            Self::MasterStack(system) => system.container_tree(layout),
            Self::Scrolling(system) => system.container_tree(layout),
            Self::Floating(system) => system.groups.container_tree(layout),
        }
    }

    pub fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        match self {
            Self::Traditional(system) => system.tiles.window_slot(layout, window),
            Self::Bsp(system) => system.window_slot(layout, window),
            Self::Stack(system) => system.window_slot(layout, window),
            Self::MasterStack(system) => system.tiles.window_slot(layout, window),
            Self::Scrolling(system) => system.window_slot(layout, window),
            Self::Floating(system) => system.groups.window_slot(layout, window),
        }
    }
}

impl LayoutSystemKind {
    pub(crate) fn project_groups(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &GapSettings,
        stack_line: crate::common::config::StackLineReserve,
        emit_frames: bool,
        groups: &mut Vec<crate::layout_engine::engine::GroupContainerInfo>,
        selection_path_only: bool,
    ) -> Option<Vec<(WindowId, CGRect)>> {
        let (forest, root, regions) = match self {
            Self::Traditional(system) => (&system.tiles, system.tiles.root(layout), None),
            Self::MasterStack(system) => {
                let (root, master, stack) = system.region_roots(layout);
                (&system.tiles, root, Some((master, stack)))
            }
            _ => return None,
        };
        Some(tiles::project(
            forest,
            root,
            |active| tiles::project_root(root, regions, active),
            screen,
            stack_offset,
            constraints,
            gaps,
            stack_line,
            emit_frames,
            Some(tiles::GroupOutput { groups, selection_path_only }),
        ))
    }

    pub(crate) fn collect_group_containers(
        &self,
        layout_id: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &GapSettings,
        stack_line: crate::common::config::StackLineReserve,
        selection_path_only: bool,
    ) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
        let mut groups = Vec::new();
        if self
            .project_groups(
                layout_id,
                screen,
                stack_offset,
                constraints,
                gaps,
                stack_line,
                false,
                &mut groups,
                selection_path_only,
            )
            .is_some()
        {
            return groups;
        }
        match self {
            LayoutSystemKind::Stack(s) => s.collect_group_containers(
                layout_id,
                screen,
                stack_offset,
                gaps,
                stack_line,
                selection_path_only,
            ),
            LayoutSystemKind::Floating(s) => {
                s.collect_group_containers(layout_id, screen, gaps, selection_path_only)
            }
            _ => Vec::new(),
        }
    }
}
