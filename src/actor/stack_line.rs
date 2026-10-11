use objc2::MainThreadMarker;
use objc2_core_foundation::{CGRect, CGSize};
use tracing::instrument;

use crate::actor::app::WindowId;
use crate::actor::reactor::{Command, ReactorCommand};
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::{self, reactor};
use crate::common::collections::HashMap;
use crate::common::config::Config;
use crate::model::tree::NodeId;
use crate::sys::screen::{CoordinateConverter, SpaceId};
#[derive(Debug, Clone, PartialEq)]
pub struct GroupInfo {
    pub node_id: NodeId,
    pub space_id: SpaceId,
    pub frame: CGRect,
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
struct NativeGroup {
    bar: cgs::TabBar,
    info: GroupInfo,
}

#[derive(Debug)]
pub enum Event {
    TabAction(NodeId, cgs::TabBarAction),
    TitleChanged(WindowId, String),
    GroupsUpdated {
        active_space_ids: Vec<SpaceId>,
        space_id: SpaceId,
        groups: Vec<GroupInfo>,
        active_workspace_for_space_has_fullscreen: bool,
    },
    SpaceStateUpdated(CoordinateConverter, ForwardedSpaceState),
    ConfigUpdated(Config),
}

pub struct StackLine {
    config: Config,
    rx: Receiver,
    mtm: MainThreadMarker,
    native_groups: HashMap<NodeId, NativeGroup>,
    icons: HashMap<i32, (String, Option<cgs::Image>)>,
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
            icons: HashMap::default(),
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

    #[instrument(name = "stack_line::handle_event", skip(self))]
    fn handle_event(&mut self, event: Event) {
        if !self.is_enabled()
            && !matches!(event, Event::ConfigUpdated(_) | Event::SpaceStateUpdated(..))
        {
            return;
        }
        match event {
            Event::TabAction(node, action) => self.handle_tab_action(node, action),
            Event::TitleChanged(wid, title) => {
                let nodes: Vec<_> = self
                    .native_groups
                    .iter_mut()
                    .filter_map(|(&node, group)| {
                        let tab = group.info.tabs.iter_mut().find(|t| t.window_id == wid)?;
                        tab.title = title.clone();
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
            } => {
                self.handle_groups_updated(
                    active_space_ids,
                    space_id,
                    groups,
                    active_workspace_for_space_has_fullscreen,
                );
            }
            Event::SpaceStateUpdated(converter, space_state) => {
                if space_state.authoritative {
                    self.native_groups.retain(|_, group| {
                        space_state.active_spaces.contains(&group.info.space_id)
                            && !space_state.fullscreen_spaces.contains(&group.info.space_id)
                    });
                    self.icons.retain(|pid, _| {
                        self.native_groups
                            .values()
                            .any(|g| g.info.tabs.iter().any(|t| t.window_id.pid == *pid))
                    });
                }
                self.handle_space_state_updated(converter);
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
    ) {
        let active: crate::common::collections::HashSet<SpaceId> =
            active_space_ids.iter().copied().collect();

        self.native_groups.retain(|_, g| {
            active.contains(&g.info.space_id)
                && (g.info.space_id != space_id
                    || groups.iter().any(|info| info.node_id == g.info.node_id))
        });
        for group in groups {
            self.update_native_group(group);
        }

        for group in self.native_groups.values() {
            if group.info.space_id == space_id {
                if space_has_fullscreen {
                    group.bar.hide();
                } else if self.coordinate_converter.screen_height().is_some() {
                    if let Some(anchor) = group
                        .info
                        .tabs
                        .get(group.info.selected_index)
                        .and_then(|tab| tab.window_server_id)
                    {
                        group.bar.show_above(anchor.as_u32() as isize);
                    } else {
                        group.bar.hide();
                    }
                } else {
                    group.bar.hide();
                }
            }
        }
        self.icons.retain(|pid, _| {
            self.native_groups
                .values()
                .any(|g| g.info.tabs.iter().any(|t| t.window_id.pid == *pid))
        });
    }

    fn handle_space_state_updated(&mut self, converter: CoordinateConverter) {
        self.coordinate_converter = converter;
        for group in self.native_groups.values() {
            if let Some(frame) = converter.convert_rect(CGRect::new(
                group.info.frame.origin,
                CGSize::new(group.info.frame.size.width, 28.0),
            )) {
                group.bar.set_frame(frame);
            } else {
                group.bar.hide();
            }
        }
        tracing::debug!("Updated coordinate converter for group indicators");
    }

    fn handle_config_updated(&mut self, config: Config) {
        self.config = config;
        if !self.is_enabled() {
            self.native_groups.clear();
            self.icons.clear();
        }
    }

    fn handle_tab_action(&self, node: NodeId, action: cgs::TabBarAction) {
        let Some(group) = self.native_groups.get(&node) else {
            return;
        };
        if !group.bar.is_visible() {
            return;
        }
        let Some(command) = tab_command(&group.info.tabs, action) else {
            return;
        };
        self.reactor_tx.send(reactor::Event::Command(Command::Reactor(command)));
    }

    fn refresh_native_tabs(&mut self, node: NodeId) {
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
        group.bar.set_tabs(tabs, group.info.selected_index);
    }

    fn update_native_group(&mut self, info: GroupInfo) {
        if self.native_groups.get(&info.node_id).is_some_and(|g| g.info == info) {
            return;
        }
        let node = info.node_id;
        if let Some(group) = self.native_groups.get_mut(&node) {
            group.info = info;
        } else {
            let ui = cgs::Ui::new(self.mtm);
            let tx = self.tx.clone();
            let bar = cgs::TabBar::new(&ui)
                .on_action(move |action| tx.send(Event::TabAction(node, action)));
            self.native_groups.insert(node, NativeGroup { bar, info });
        }
        self.refresh_native_tabs(node);
        let group = &self.native_groups[&node];
        if let Some(frame) = self.coordinate_converter.convert_rect(CGRect::new(
            group.info.frame.origin,
            CGSize::new(group.info.frame.size.width, 28.0),
        )) {
            group.bar.set_frame(frame);
        } else {
            group.bar.hide();
        }
    }
}

fn tab_command(tabs: &[TabInfo], action: cgs::TabBarAction) -> Option<ReactorCommand> {
    let index = match action {
        cgs::TabBarAction::Select(i) | cgs::TabBarAction::Close(i) => i,
    };
    let tab = tabs.get(index)?;
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
    };
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_actions_resolve_latest_ids_and_never_fall_back_to_frontmost_close() {
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
            tab_command(&tabs, cgs::TabBarAction::Close(1)),
            Some(ReactorCommand::CloseWindow { window_server_id: Some(20) })
        ));
        tabs.remove(0);
        assert!(
            matches!(tab_command(&tabs, cgs::TabBarAction::Select(0)), Some(ReactorCommand::FocusWindow { window_id, window_server_id: Some(20) }) if window_id == second.into())
        );
        tabs[0].window_server_id = None;
        assert!(tab_command(&tabs, cgs::TabBarAction::Close(0)).is_none());
        assert!(tab_command(&tabs, cgs::TabBarAction::Select(usize::MAX)).is_none());
    }
}
