use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2_core_foundation::CGRect;
use tracing::instrument;

use crate::actor::app::WindowId;
use crate::actor::reactor::{Command, ReactorCommand};
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::{self, reactor};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{Config, HorizontalPlacement, VerticalPlacement};
use crate::layout_engine::LayoutCommand;
use crate::model::tree::NodeId;
use crate::sys::screen::{CoordinateConverter, SpaceId};
#[derive(Debug, Clone, PartialEq)]
pub struct GroupInfo {
    pub node_id: NodeId,
    pub space_id: SpaceId,
    pub frame: CGRect,
    /// Horizontal stacks take a bar above or below; vertical stacks one beside them.
    pub horizontal: bool,
    pub selected_index: usize,
    pub tabs: Vec<TabInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabInfo {
    pub window_id: WindowId,
    pub title: String,
    pub bundle_id: Option<String>,
    pub window_server_id: Option<crate::sys::window_server::WindowServerId>,
}

impl GroupInfo {
    /// The window the bar is ordered above: the selected window, or else another window
    /// of the same stack. Never a window outside the stack.
    fn anchor(&self) -> Option<crate::sys::window_server::WindowServerId> {
        self.tabs
            .get(self.selected_index)
            .into_iter()
            .chain(&self.tabs)
            .find_map(|tab| tab.window_server_id)
    }
}

type GroupId = (SpaceId, NodeId);

struct NativeGroup {
    bar: cgs::TabBar,
    info: GroupInfo,
    /// The windows the bar currently shows, so a click resolves against what was clicked.
    shown: Rc<RefCell<Vec<WindowId>>>,
    generation: u64,
    fullscreen: bool,
    ordered_anchor: Cell<Option<crate::sys::window_server::WindowServerId>>,
    placed_frame: Cell<Option<CGRect>>,
}

#[derive(Debug)]
pub enum Event {
    TabAction(GroupId, u64, WindowId, cgs::TabBarAction),
    /// A tab was dragged onto `target`'s slot. The bar already shows the new order.
    TabMoved(GroupId, u64, WindowId, WindowId),
    TitleChanged(WindowId, String),
    GroupsUpdated {
        active_space_ids: Vec<SpaceId>,
        space_id: SpaceId,
        groups: Vec<GroupInfo>,
        active_workspace_for_space_has_fullscreen: bool,
        /// Native focus can raise the anchor without changing selection or geometry.
        reorder: bool,
    },
    SpaceStateUpdated(CoordinateConverter, ForwardedSpaceState),
    ConfigUpdated(Config),
}

pub struct StackLine {
    config: Config,
    rx: Receiver,
    mtm: MainThreadMarker,
    native_groups: HashMap<GroupId, NativeGroup>,
    next_generation: u64,
    icons: HashMap<i32, (String, Option<cgs::Image>)>,
    live_groups: HashSet<GroupId>,
    live_pids: HashSet<i32>,
    tx: Sender,
    reactor_tx: reactor::Sender,
    coordinate_converter: CoordinateConverter,
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

impl StackLine {
    pub fn new(
        config: Config,
        rx: Receiver,
        tx: Sender,
        mtm: MainThreadMarker,
        reactor_tx: reactor::Sender,
        coordinate_converter: CoordinateConverter,
    ) -> Self {
        Self {
            config,
            rx,
            tx,
            native_groups: HashMap::default(),
            next_generation: 0,
            icons: HashMap::default(),
            live_groups: HashSet::default(),
            live_pids: HashSet::default(),
            mtm,
            reactor_tx,
            coordinate_converter,
        }
    }

    pub async fn run(mut self) {
        if !self.is_enabled() {
            tracing::debug!("stack line disabled at start; will listen for config changes");
        }

        while let Some((span, event)) = self.rx.recv().await {
            let _guard = span.enter();
            self.handle_event(event);
        }
    }

    fn is_enabled(&self) -> bool { self.config.settings.ui.stack_line.enabled }

    #[instrument(name = "stack_line::handle_event", skip_all)]
    fn handle_event(&mut self, event: Event) {
        if !self.is_enabled()
            && !matches!(event, Event::ConfigUpdated(_) | Event::SpaceStateUpdated(..))
        {
            return;
        }
        match event {
            Event::TabAction(id, generation, window, action) => {
                self.handle_tab_action(id, generation, window, action);
            }
            Event::TabMoved(id, generation, window, target) => {
                self.handle_tab_moved(id, generation, window, target);
            }
            Event::TitleChanged(wid, title) => {
                let nodes: Vec<_> = self
                    .native_groups
                    .iter_mut()
                    .filter_map(|(&node, group)| {
                        let tab = group.info.tabs.iter_mut().find(|t| t.window_id == wid)?;
                        if tab.title == title {
                            return None;
                        }
                        tab.title.clone_from(&title);
                        Some(node)
                    })
                    .collect();
                for node in nodes {
                    self.refresh_native_tabs(node);
                }
            }
            Event::GroupsUpdated {
                active_space_ids,
                space_id,
                groups,
                active_workspace_for_space_has_fullscreen,
                reorder,
            } => {
                self.handle_groups_updated(
                    active_space_ids,
                    space_id,
                    groups,
                    active_workspace_for_space_has_fullscreen,
                    reorder,
                );
            }
            Event::SpaceStateUpdated(converter, space_state) => {
                if space_state.authoritative {
                    self.native_groups.retain(|_, group| {
                        space_state.active_spaces.contains(&group.info.space_id)
                            && !space_state.fullscreen_spaces.contains(&group.info.space_id)
                    });
                    self.prune_icons();
                    self.handle_space_state_updated(
                        converter,
                        space_state.should_force_refresh_layout || space_state.display_set_changed,
                    );
                }
            }
            Event::ConfigUpdated(config) => {
                self.handle_config_updated(config);
            }
        }
    }

    fn handle_groups_updated(
        &mut self,
        active_space_ids: Vec<SpaceId>,
        space_id: SpaceId,
        groups: Vec<GroupInfo>,
        space_has_fullscreen: bool,
        reorder: bool,
    ) {
        self.live_groups.clear();
        self.live_groups.extend(groups.iter().map(|info| (info.space_id, info.node_id)));
        self.native_groups.retain(|&id, _| {
            active_space_ids.contains(&id.0) && (id.0 != space_id || self.live_groups.contains(&id))
        });
        if !active_space_ids.contains(&space_id) {
            self.prune_icons();
            return;
        }
        for info in groups {
            if info.space_id != space_id {
                continue;
            }
            let id = (space_id, info.node_id);
            self.update_native_group(info);
            let group = self.native_groups.get_mut(&id).unwrap();
            group.fullscreen = space_has_fullscreen;
            self.place(&self.native_groups[&id], reorder);
        }
        self.prune_icons();
    }

    fn prune_icons(&mut self) {
        // One pass over tabs, rather than searching every group for each cached PID.
        self.live_pids.clear();
        self.live_pids.extend(
            self.native_groups
                .values()
                .flat_map(|group| group.info.tabs.iter().map(|tab| tab.window_id.pid)),
        );
        self.icons.retain(|pid, _| self.live_pids.contains(pid));
    }

    fn handle_space_state_updated(&mut self, converter: CoordinateConverter, reorder: bool) {
        self.coordinate_converter = converter;
        for group in self.native_groups.values() {
            // AppKit can relocate panels when displays change. Recheck the native
            // frame even if the converted layout frame happens to be unchanged.
            group.placed_frame.set(None);
            self.place(group, reorder);
        }
    }

    fn handle_config_updated(&mut self, config: Config) {
        self.config = config;
        if !self.is_enabled() {
            self.native_groups.clear();
            self.icons.clear();
            self.live_groups.clear();
            self.live_pids.clear();
        }
        for group in self.native_groups.values() {
            self.place(group, false);
        }
    }

    fn place(&self, group: &NativeGroup, reorder: bool) {
        let settings = &self.config.settings.ui.stack_line;
        group.bar.set_select_on_hover(settings.hover.selects_on_hover());
        let edge = match (
            group.info.horizontal,
            settings.horiz_placement,
            settings.vert_placement,
        ) {
            (true, HorizontalPlacement::Top, _) => cgs::TabBarEdge::Top,
            (true, HorizontalPlacement::Bottom, _) => cgs::TabBarEdge::Bottom,
            (false, _, VerticalPlacement::Left) => cgs::TabBarEdge::Left,
            (false, _, VerticalPlacement::Right) => cgs::TabBarEdge::Right,
        };
        group.bar.set_edge(edge);
        let frame = self
            .coordinate_converter
            .convert_rect(settings.reserve().bar(group.info.frame, group.info.horizontal))
            .filter(|frame| {
                [group.info.frame, *frame].iter().all(|r| {
                    r.origin.x.is_finite()
                        && r.origin.y.is_finite()
                        && r.size.width.is_finite()
                        && r.size.height.is_finite()
                        && r.size.width > 0.0
                        && r.size.height > 0.0
                })
            });
        let anchor = group.info.anchor();
        if !self.is_enabled() || group.fullscreen || frame.is_none() || anchor.is_none() {
            if group.bar.is_visible() {
                group.bar.hide();
            }
            group.ordered_anchor.set(None);
            return;
        }
        let frame = frame.unwrap();
        if group.placed_frame.replace(Some(frame)) != Some(frame) {
            group.bar.set_frame(frame);
        }
        if reorder || group.ordered_anchor.get() != anchor || !group.bar.is_visible() {
            group.bar.show_above(anchor.unwrap().as_u32() as isize);
            group.ordered_anchor.set(anchor);
        }
    }

    fn handle_tab_action(
        &self,
        id: GroupId,
        generation: u64,
        window: WindowId,
        action: cgs::TabBarAction,
    ) {
        let Some(group) = self.native_groups.get(&id) else {
            return;
        };
        // A queued callback belongs to this panel lifetime, not a replacement which
        // happens to reuse the same layout key (even with the same windows).
        if generation != group.generation || !group.bar.is_visible() {
            return;
        }
        let Some(command) = tab_command(&group.info.tabs, window, action) else {
            return;
        };
        self.reactor_tx.send(reactor::Event::Command(Command::Reactor(command)));
    }

    fn handle_tab_moved(
        &mut self,
        id: GroupId,
        generation: u64,
        window: WindowId,
        target: WindowId,
    ) {
        let Some(group) = self.native_groups.get_mut(&id) else {
            return;
        };
        if generation != group.generation {
            return;
        }
        let tabs = &mut group.info.tabs;
        let (Some(from), Some(to)) = (
            tabs.iter().position(|tab| tab.window_id == window),
            tabs.iter().position(|tab| tab.window_id == target),
        ) else {
            return;
        };
        // Match what the bar shows until the layout confirms it, so the confirmation
        // is a no-op rather than a second shuffle.
        let tab = tabs.remove(from);
        tabs.insert(to, tab);
        group.info.selected_index = moved_index(group.info.selected_index, from, to);
        self.reactor_tx.send(reactor::Event::Command(Command::Layout(
            LayoutCommand::MoveStackTab {
                window: window.into(),
                target: target.into(),
            },
        )));
    }

    fn refresh_native_tabs(&mut self, node: GroupId) {
        let Some(group) = self.native_groups.get(&node) else {
            return;
        };
        let tabs = group.info.tabs.iter().map(|tab| {
            let (name, icon) = self.icons.entry(tab.window_id.pid).or_insert_with(|| {
                let app = objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(tab.window_id.pid);
                let name = app.as_ref().and_then(|a| a.localizedName()).map(|s| s.to_string()).unwrap_or_default();
                let icon = app.and_then(|a| a.icon()).or_else(|| tab.bundle_id.as_deref().and_then(cgs::Application::icon_for_bundle));
                (name, icon)
            });
            cgs::TabBarItem { title: if tab.title.is_empty() { name.clone() } else { tab.title.clone() }, icon: icon.clone() }
        }).collect();
        {
            let mut shown = group.shown.borrow_mut();
            shown.clear();
            shown.extend(group.info.tabs.iter().map(|tab| tab.window_id));
        }
        group.bar.set_tabs(tabs, group.info.selected_index);
    }

    fn update_native_group(&mut self, info: GroupInfo) {
        let node = (info.space_id, info.node_id);
        if self.native_groups.get(&node).is_some_and(|g| g.info == info) {
            return;
        }
        let mut tabs_changed = true;
        let mut selection_changed = false;
        if let Some(group) = self.native_groups.get_mut(&node) {
            selection_changed = group.info.selected_index != info.selected_index;
            tabs_changed = group.info.tabs.len() != info.tabs.len()
                || group.info.tabs.iter().zip(&info.tabs).any(|(old, new)| {
                    old.window_id != new.window_id
                        || old.title != new.title
                        || old.bundle_id != new.bundle_id
                });
            group.info = info;
        } else {
            let generation = self.next_generation;
            self.next_generation =
                self.next_generation.checked_add(1).expect("panel generation exhausted");
            let ui = cgs::Ui::new(self.mtm);
            let tx = self.tx.clone();
            let shown = Rc::new(RefCell::new(Vec::new()));
            let clicked = shown.clone();
            let bar = cgs::TabBar::new(&ui).on_action(move |action| match action {
                cgs::TabBarAction::Select(i) | cgs::TabBarAction::Close(i) => {
                    if let Some(&window) = clicked.borrow().get(i) {
                        tx.send(Event::TabAction(node, generation, window, action));
                    }
                }
                cgs::TabBarAction::Move { from, to } => {
                    let mut shown = clicked.borrow_mut();
                    let Some(&target) = shown.get(to).filter(|_| from < shown.len()) else {
                        return;
                    };
                    // Later clicks resolve against the order the bar now shows.
                    let window = shown.remove(from);
                    shown.insert(to, window);
                    tx.send(Event::TabMoved(node, generation, window, target));
                }
            });
            self.native_groups.insert(node, NativeGroup {
                bar,
                info,
                shown,
                generation,
                fullscreen: false,
                ordered_anchor: Cell::new(None),
                placed_frame: Cell::new(None),
            });
        }
        if tabs_changed {
            self.refresh_native_tabs(node);
        } else if selection_changed {
            let group = &self.native_groups[&node];
            group.bar.set_selected_index(group.info.selected_index);
        }
    }
}

/// Targets the clicked window by identity, with its latest known WindowServer ID.
fn tab_command(
    tabs: &[TabInfo],
    window: WindowId,
    action: cgs::TabBarAction,
) -> Option<ReactorCommand> {
    let tab = tabs.iter().find(|tab| tab.window_id == window)?;
    let command = match action {
        cgs::TabBarAction::Select(_) => ReactorCommand::FocusWindow {
            window_id: tab.window_id.into(),
            window_server_id: tab.window_server_id.map(|id| id.as_u32()),
        },
        cgs::TabBarAction::Close(_) => {
            let id = tab.window_server_id?;
            ReactorCommand::CloseWindow {
                window_server_id: Some(id.as_u32()),
            }
        }
        cgs::TabBarAction::Move { .. } => return None,
    };
    Some(command)
}

/// Where the tab at `index` ends up once the tab at `from` moves to `to`.
fn moved_index(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < to && (from + 1..=to).contains(&index) {
        index - 1
    } else if to < from && (to..from).contains(&index) {
        index + 1
    } else {
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_actions_target_the_clicked_window_and_never_fall_back_to_frontmost_close() {
        use std::num::NonZeroU32;

        use crate::sys::window_server::WindowServerId;
        let first = WindowId {
            pid: 111,
            idx: NonZeroU32::new(1).unwrap(),
        };
        let second = WindowId {
            pid: 222,
            idx: NonZeroU32::new(2).unwrap(),
        };
        let mut tabs = vec![
            TabInfo {
                window_id: first,
                title: String::new(),
                bundle_id: None,
                window_server_id: Some(WindowServerId::new(10)),
            },
            TabInfo {
                window_id: second,
                title: String::new(),
                bundle_id: None,
                window_server_id: Some(WindowServerId::new(20)),
            },
        ];
        assert!(matches!(
            tab_command(&tabs, second, cgs::TabBarAction::Close(1)),
            Some(ReactorCommand::CloseWindow { window_server_id: Some(20) })
        ));
        tabs.remove(0);
        assert!(
            matches!(tab_command(&tabs, second, cgs::TabBarAction::Select(0)), Some(ReactorCommand::FocusWindow { window_id, window_server_id: Some(20) }) if window_id == second.into())
        );
        tabs[0].window_server_id = None;
        assert!(tab_command(&tabs, second, cgs::TabBarAction::Close(0)).is_none());
        assert!(tab_command(&tabs, first, cgs::TabBarAction::Select(0)).is_none());
    }

    #[test]
    fn bars_anchor_only_to_windows_of_their_own_stack() {
        use crate::sys::window_server::WindowServerId;
        let group = |selected_index, ids: [Option<u32>; 3]| GroupInfo {
            node_id: NodeId::default(),
            space_id: SpaceId::new(1),
            frame: CGRect::default(),
            horizontal: true,
            selected_index,
            tabs: (1..)
                .zip(ids)
                .map(|(idx, id)| TabInfo {
                    window_id: WindowId::new(1, idx),
                    title: String::new(),
                    bundle_id: None,
                    window_server_id: id.map(WindowServerId::new),
                })
                .collect(),
        };
        // The selected window first, then any other window of the same stack, else none.
        assert_eq!(
            group(1, [Some(10), Some(20), Some(30)]).anchor(),
            Some(WindowServerId::new(20))
        );
        assert_eq!(
            group(1, [None, None, Some(30)]).anchor(),
            Some(WindowServerId::new(30))
        );
        assert_eq!(group(0, [None; 3]).anchor(), None);
    }
}
