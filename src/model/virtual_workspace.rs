use objc2_core_foundation::CGSize;
use serde::{Deserialize, Serialize};
use slotmap::{SlotMap, new_key_type};
use tracing::{error, warn};

use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet};
#[cfg(test)]
use crate::common::config::AppWorkspaceRule;
use crate::common::config::{
    DisplayMode, LayoutMode, LayoutSettings, MAX_WORKSPACES, VirtualWorkspaceSettings,
    WorkspaceSelector,
};
use crate::common::log::trace_misc;
use crate::layout_engine::systems::LayoutSystemKind;
use crate::layout_engine::workspaces::WorkspaceLayoutState;
use crate::layout_engine::{Direction, LayoutId, LayoutSystem};
use crate::model::WindowStore;
use crate::model::app_rules::{AppRuleDecision, AppRuleEffects, AppRuleRejection, AppRuleResult};
use crate::sys::screen::SpaceId;

new_key_type! {
    pub struct VirtualWorkspaceId;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NavigationScope {
    #[default]
    Local,
    SharedVisible,
}

/// Resolution consumes accepted Reactor topology, never the full native inventory.
#[derive(Clone, Copy)]
pub(crate) struct WorkspaceContext<'a> {
    pub space: SpaceId,
    pub visible: &'a [SpaceId],
    pub navigation: NavigationScope,
}

impl From<SpaceId> for WorkspaceContext<'_> {
    fn from(space: SpaceId) -> Self {
        Self {
            space,
            visible: &[],
            navigation: NavigationScope::Local,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkspaceTarget {
    pub id: VirtualWorkspaceId,
    pub space: SpaceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceTransferReason {
    Explicit,
    DisplayDeparture,
    Restoration,
}

impl std::fmt::Display for VirtualWorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dbg = format!("{:?}", self);
        let digits: String = dbg.chars().filter(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u64>() {
            write!(f, "{:08}", n)
        } else {
            write!(f, "{}", dbg)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    NoWorkspacesAvailable,
    AssignmentFailed,
    InvalidWorkspaceId(VirtualWorkspaceId),
    InvalidWorkspaceIndex(usize),
    InconsistentState(String),
}

/// Workspace-local configuration and layout state.
///
/// This intentionally does not own the set of member windows. Window-to-
/// workspace assignment is authoritative in `WindowStore`; the layout system
/// here is only the arrangement for the windows currently projected into this
/// workspace.
#[derive(Debug, Serialize, Deserialize)]
pub struct VirtualWorkspace {
    pub name: String,
    pub space: SpaceId,
    #[serde(default)]
    pub preferred_display: Option<String>,
    #[serde(default)]
    pub home_space: Option<SpaceId>,
    last_focused: Option<WindowId>,
    #[serde(default = "default_layout_system_kind")]
    pub layout_system: LayoutSystemKind,
    #[serde(default)]
    pub layout_mode: LayoutMode,
    #[serde(default)]
    pub(crate) layout_state: WorkspaceLayoutState,
}

fn default_layout_system_kind() -> LayoutSystemKind {
    VirtualWorkspace::create_layout_system(LayoutMode::default(), &LayoutSettings::default())
}

impl VirtualWorkspace {
    fn new(name: String, space: SpaceId, mode: LayoutMode, settings: &LayoutSettings) -> Self {
        let layout_system = Self::create_layout_system(mode, settings);
        Self {
            name,
            space,
            preferred_display: None,
            home_space: None,
            last_focused: None,
            layout_system,
            layout_mode: mode,
            layout_state: WorkspaceLayoutState::default(),
        }
    }

    pub(crate) fn active_layout(&self) -> Option<LayoutId> { self.layout_state.active() }

    pub(crate) fn ensure_layout_for_size(&mut self, size: CGSize) {
        self.layout_state.ensure_active(size, &mut self.layout_system);
    }

    pub(crate) fn replace_layout_mode(
        &mut self,
        mode: LayoutMode,
        settings: &LayoutSettings,
    ) -> LayoutId {
        self.layout_mode = mode;
        self.layout_system = Self::create_layout_system(mode, settings);
        let layout = self.layout_system.create_layout();
        self.layout_state.replace(layout);
        layout
    }

    pub fn layout_mode(&self) -> LayoutMode { self.layout_mode }

    pub fn create_layout_system(mode: LayoutMode, settings: &LayoutSettings) -> LayoutSystemKind {
        match mode {
            LayoutMode::Traditional => LayoutSystemKind::Traditional(
                crate::layout_engine::systems::TraditionalLayoutSystem::new(
                    settings.window_insertion_point_for(mode),
                    settings.traditional.equalize_nodes,
                ),
            ),
            LayoutMode::Floating => LayoutSystemKind::Floating(
                crate::layout_engine::systems::FloatingLayoutSystem::new(
                    settings.window_insertion_point_for(mode),
                ),
            ),
            LayoutMode::Bsp => {
                let mut system = crate::layout_engine::systems::BspLayoutSystem::new(
                    settings.window_insertion_point_for(mode),
                );
                system.set_single_window_aspect_ratio(settings.bsp.single_window_aspect_ratio);
                LayoutSystemKind::Bsp(system)
            }
            LayoutMode::Stack => LayoutSystemKind::Stack(
                crate::layout_engine::systems::StackLayoutSystem::new_with_insertion_point(
                    settings.stack.default_orientation,
                    settings.window_insertion_point_for(mode),
                ),
            ),
            LayoutMode::MasterStack => {
                let mut mode_settings = settings.master_stack.clone();
                mode_settings.base = settings.resolved_base_for(mode);
                LayoutSystemKind::MasterStack(
                    crate::layout_engine::systems::MasterStackLayoutSystem::new(mode_settings),
                )
            }
            LayoutMode::Scrolling => {
                let mut mode_settings = settings.scrolling.clone();
                mode_settings.base = settings.resolved_base_for(mode);
                LayoutSystemKind::Scrolling(
                    crate::layout_engine::systems::ScrollingLayoutSystem::new(&mode_settings),
                )
            }
        }
    }

    pub fn set_last_focused(&mut self, window_id: Option<WindowId>) {
        self.last_focused = window_id;
    }

    pub fn last_focused(&self) -> Option<WindowId> { self.last_focused }
}

/// Current ownership is never inferred from home intent. A departed display can
/// remember its presentation without keeping a foreign workspace in its active slot.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SpaceActivation {
    pub current: Option<VirtualWorkspaceId>,
    /// Navigation history; shared mode can return to a workspace on another display.
    pub previous: Option<VirtualWorkspaceId>,
    pub remembered: Option<VirtualWorkspaceId>,
}

impl SpaceActivation {
    fn showing(id: VirtualWorkspaceId) -> Self {
        Self {
            current: Some(id),
            ..Self::default()
        }
    }
}

fn deserialize_activations<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<SpaceId, SpaceActivation>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Snapshot {
        Current(SpaceActivation),
        Legacy(Option<VirtualWorkspaceId>, VirtualWorkspaceId),
    }
    Ok(HashMap::<SpaceId, Snapshot>::deserialize(deserializer)?
        .into_iter()
        .map(|(space, saved)| {
            let activation = match saved {
                Snapshot::Current(activation) => activation,
                Snapshot::Legacy(previous, current) => SpaceActivation {
                    current: Some(current),
                    previous,
                    remembered: None,
                },
            };
            (space, activation)
        })
        .collect())
}

pub(crate) struct WorkspaceSettingsPlan {
    names: Vec<(VirtualWorkspaceId, String)>,
    additions: Vec<(SpaceId, String)>,
}

/// Owns virtual workspace topology and workspace-scoped layout configurations.
///
/// Membership is single-source-of-truth in `WindowStore`. Any code that
/// needs to answer "which workspace owns this window?" or "which windows belong
/// to this workspace?" must go through the store-backed helpers on this
/// manager. Keeping the mapping out of `VirtualWorkspace` prevents stale
/// duplicated membership from surviving topology churn or discovery refreshes.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspaceStore {
    pub(crate) workspaces: SlotMap<VirtualWorkspaceId, VirtualWorkspace>,
    #[serde(
        alias = "workspaces_by_space",
        deserialize_with = "deserialize_workspace_order"
    )]
    order: Vec<VirtualWorkspaceId>,
    #[serde(deserialize_with = "deserialize_activations")]
    pub active_workspace_per_space: HashMap<SpaceId, SpaceActivation>,
    workspace_counter: usize,
    #[serde(skip)]
    pub(crate) settings: VirtualWorkspaceSettings,
    #[serde(skip)]
    pub layout_settings: LayoutSettings,
}

fn selector_matches(selector: &WorkspaceSelector, index: usize, name: &str) -> bool {
    match selector {
        WorkspaceSelector::Index(i) => *i == index,
        WorkspaceSelector::Name(n) => n == name,
    }
}

// Schema 4 uses one ordered catalog. Older snapshots group IDs by native Space;
// migrate deterministically, preserving the legacy ordinal contract.
fn deserialize_workspace_order<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<VirtualWorkspaceId>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Vec<VirtualWorkspaceId>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("workspace catalog or legacy Space index")
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
            Vec::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            let spaces =
                std::collections::BTreeMap::<SpaceId, Vec<VirtualWorkspaceId>>::deserialize(
                    serde::de::value::MapAccessDeserializer::new(map),
                )?;
            Ok(spaces.into_values().flatten().collect())
        }
    }
    deserializer.deserialize_any(Visitor)
}

impl std::ops::Index<VirtualWorkspaceId> for WorkspaceStore {
    type Output = VirtualWorkspace;

    fn index(&self, id: VirtualWorkspaceId) -> &Self::Output { &self.workspaces[id] }
}

impl std::ops::IndexMut<VirtualWorkspaceId> for WorkspaceStore {
    fn index_mut(&mut self, id: VirtualWorkspaceId) -> &mut Self::Output {
        &mut self.workspaces[id]
    }
}

impl Default for WorkspaceStore {
    fn default() -> Self { Self::new() }
}

impl WorkspaceStore {
    pub(crate) fn workspace_space(&self, id: VirtualWorkspaceId) -> Option<SpaceId> {
        self.workspaces.get(id).map(|workspace| workspace.space)
    }

    pub(crate) fn active_layout(
        &self,
        space: SpaceId,
        workspace: VirtualWorkspaceId,
    ) -> Option<LayoutId> {
        self.workspace_info(space, workspace)?.active_layout()
    }

    pub(crate) fn active_layout_for_space(
        &self,
        space: SpaceId,
    ) -> Option<(VirtualWorkspaceId, LayoutId)> {
        let id = self.active_workspace(space)?;
        Some((id, self.workspaces.get(id)?.active_layout()?))
    }

    pub(crate) fn workspace_id_at(
        &mut self,
        space: SpaceId,
        index: Option<usize>,
    ) -> Option<VirtualWorkspaceId> {
        if let Some(index) = index {
            self.ensure_space_initialized(space);
            self.workspaces_on(space).nth(index)
        } else {
            self.active_workspace(space)
        }
    }

    fn scope_ids(&self, space: SpaceId) -> impl Iterator<Item = VirtualWorkspaceId> + '_ {
        let shared = self.settings.display_mode == DisplayMode::Shared;
        self.order.iter().copied().filter(move |id| shared || self[*id].space == space)
    }

    fn eligible(&self, context: WorkspaceContext<'_>, id: VirtualWorkspaceId) -> bool {
        self.settings.display_mode != DisplayMode::Shared
            || context.visible.contains(&self[id].space)
    }

    pub(crate) fn resolve_workspace<'a>(
        &self,
        context: impl Into<WorkspaceContext<'a>>,
        selector: &WorkspaceSelector,
    ) -> Option<WorkspaceTarget> {
        let context = context.into();
        let mut ids = self.scope_ids(context.space);
        let id = match selector {
            WorkspaceSelector::Index(index) => ids.nth(*index)?,
            WorkspaceSelector::Name(name) => {
                let mut matches =
                    ids.filter(|id| self[*id].name == *name && self.eligible(context, *id));
                let id = matches.next()?;
                if matches.next().is_some() {
                    return None;
                }
                id
            }
        };
        self.eligible(context, id)
            .then_some(WorkspaceTarget { id, space: self[id].space })
    }

    pub(crate) fn workspace_index(&self, space: SpaceId, id: VirtualWorkspaceId) -> Option<usize> {
        self.scope_ids(space).position(|candidate| candidate == id)
    }

    pub(crate) fn workspace_view(
        &self,
        context: WorkspaceContext<'_>,
    ) -> Vec<(usize, VirtualWorkspaceId, String)> {
        self.scope_ids(context.space)
            .enumerate()
            .filter(|(_, id)| self.eligible(context, *id))
            .map(|(index, id)| (index, id, self[id].name.clone()))
            .collect()
    }

    pub(crate) fn catalog(&self) -> impl Iterator<Item = VirtualWorkspaceId> + '_ {
        self.order.iter().copied()
    }

    /// Ownership changes leave the workspace object, layout and focus history intact.
    pub(crate) fn transfer_workspace(
        &mut self,
        id: VirtualWorkspaceId,
        target: SpaceId,
        display: Option<&str>,
        reason: WorkspaceTransferReason,
    ) -> bool {
        let Some(workspace) = self.workspaces.get(id) else {
            return false;
        };
        let source = workspace.space;
        if reason == WorkspaceTransferReason::Explicit
            && let Some(home) = workspace.home_space
            && let Some(activation) = self.active_workspace_per_space.get_mut(&home)
            && activation.remembered == Some(id)
        {
            activation.remembered = None;
        }
        if source == target {
            if reason != WorkspaceTransferReason::Explicit {
                return false;
            }
            let workspace = &mut self[id];
            let changed = workspace.home_space != Some(target)
                || workspace.preferred_display.as_deref() != display;
            workspace.home_space = Some(target);
            workspace.preferred_display = display.map(str::to_owned);
            return changed;
        }
        if reason == WorkspaceTransferReason::DisplayDeparture {
            let activation = self.active_workspace_per_space.entry(source).or_default();
            if activation.current == Some(id) && activation.remembered.is_none() {
                activation.remembered = Some(id);
            }
        }
        let workspace = &mut self[id];
        if reason == WorkspaceTransferReason::Explicit {
            workspace.preferred_display = display.map(str::to_owned);
            workspace.home_space = Some(target);
        }
        workspace.space = target;
        // Remembered presentation becomes active again only when ownership returns.
        if reason == WorkspaceTransferReason::Explicit {
            self.set_active_workspace(target, id);
        } else if self.active_workspace(target).is_none()
            || self
                .active_workspace_per_space
                .get(&target)
                .is_some_and(|activation| activation.remembered == Some(id))
        {
            self.set_active_workspace(target, id);
            self.active_workspace_per_space.get_mut(&target).unwrap().remembered = None;
        }
        self.repair_activation(source);
        self.repair_activation(target);
        if reason == WorkspaceTransferReason::Explicit && self.active_workspace(source).is_none() {
            self.ensure_space_initialized(source);
        }
        true
    }

    pub(crate) fn migrate_legacy_activation(&mut self) {
        for (&space, activation) in &mut self.active_workspace_per_space {
            if let Some(id) = activation.current
                && self.workspaces.get(id).is_some_and(|workspace| {
                    workspace.space != space && workspace.home_space == Some(space)
                })
            {
                activation.remembered = Some(id);
                activation.current =
                    self.order.iter().copied().find(|id| self.workspaces[*id].space == space);
            }
            if activation.previous == activation.current {
                activation.previous = None;
            }
        }
    }

    fn repair_activation(&mut self, space: SpaceId) {
        let owned = |id| self.workspaces.get(id).is_some_and(|workspace| workspace.space == space);
        let mut activation =
            self.active_workspace_per_space.get(&space).copied().unwrap_or_default();
        if !activation.current.is_some_and(owned) {
            activation.current = activation
                .previous
                .filter(|&id| owned(id))
                .or_else(|| self.workspaces_on(space).next());
        }
        activation.previous = activation.previous.filter(|&id| {
            self.workspaces.contains_key(id)
                && Some(id) != activation.current
                && (self.settings.display_mode == DisplayMode::Shared || owned(id))
        });
        self.active_workspace_per_space.insert(space, activation);
    }

    pub(crate) fn ensure_layouts_for_size(&mut self, space: SpaceId, size: CGSize) {
        self.ensure_space_initialized(space);
        for workspace in self.workspaces.values_mut().filter(|workspace| workspace.space == space) {
            workspace.ensure_layout_for_size(size);
        }
    }

    pub(crate) fn layout_spaces(&self) -> crate::common::collections::BTreeSet<SpaceId> {
        self.workspaces
            .values()
            .filter(|workspace| workspace.active_layout().is_some())
            .map(|workspace| workspace.space)
            .collect()
    }

    pub(crate) fn all_layouts(&self) -> Vec<(SpaceId, VirtualWorkspaceId, LayoutId)> {
        self.catalog()
            .flat_map(|id| {
                let workspace = &self[id];
                workspace
                    .layout_state
                    .all_layouts()
                    .map(move |layout| (workspace.space, id, layout))
            })
            .collect()
    }

    pub(crate) fn validate_layouts(&self) -> Result<(), String> {
        for (id, workspace) in &self.workspaces {
            workspace
                .layout_state
                .validate(&workspace.layout_system)
                .map_err(|error| format!("workspace {id:?} {error}"))?;
        }
        Ok(())
    }

    pub fn new() -> Self {
        Self::new_with_config(&VirtualWorkspaceSettings::default(), &LayoutSettings::default())
    }

    pub fn new_with_config(
        config: &VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
    ) -> Self {
        Self {
            workspaces: SlotMap::default(),
            order: Vec::new(),
            active_workspace_per_space: HashMap::default(),
            workspace_counter: 1,
            settings: config.clone(),
            layout_settings: layout_settings.clone(),
        }
    }

    /// Validate the projected namespace before mutating configuration or layout trees.
    pub(crate) fn plan_settings(
        &self,
        config: &VirtualWorkspaceSettings,
    ) -> Result<WorkspaceSettingsPlan, String> {
        self.validate_persisted_topology()?;
        if !(1..=MAX_WORKSPACES).contains(&config.default_workspace_count) {
            return Err("invalid default workspace count".into());
        }
        if config.display_mode == DisplayMode::Shared && self.order.len() > MAX_WORKSPACES {
            return Err(format!("shared catalog exceeds {MAX_WORKSPACES} workspaces"));
        }
        let mode_changed = self.settings.display_mode != config.display_mode;
        let mut plan = WorkspaceSettingsPlan {
            names: Vec::new(),
            additions: Vec::new(),
        };
        let mut seen = HashSet::default();
        for (global, id) in self.catalog().enumerate() {
            let workspace = &self[id];
            let index = if config.display_mode == DisplayMode::Shared {
                global
            } else {
                self.workspaces_on(workspace.space)
                    .position(|candidate| candidate == id)
                    .unwrap()
            };
            let mut name = if mode_changed {
                workspace.name.clone()
            } else {
                config.workspace_names.get(index).unwrap_or(&workspace.name).clone()
            };
            if mode_changed && config.display_mode == DisplayMode::Shared {
                let base = name.clone();
                let mut suffix = 2;
                while !seen.insert(name.clone()) {
                    name = format!("{base} ({suffix})");
                    suffix += 1;
                }
            }
            if name != workspace.name {
                plan.names.push((id, name));
            }
        }
        let mut total = self.order.len();
        for space in self.initialized_spaces() {
            let count = self.workspaces_on(space).count();
            if count == 0 {
                continue;
            } // Absent presentation contexts allocate on exposure.
            let missing = if config.display_mode == DisplayMode::Shared {
                config.default_workspace_count.saturating_sub(total)
            } else {
                config.default_workspace_count.saturating_sub(count)
            };
            for local in count..count + missing {
                let index = if config.display_mode == DisplayMode::Shared {
                    total
                } else {
                    local
                };
                let name = config
                    .workspace_names
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("Workspace {}", index + 1));
                plan.additions.push((space, name));
                total += 1;
            }
        }
        Ok(plan)
    }

    pub fn update_settings(
        &mut self,
        config: &VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
    ) {
        let plan = self
            .plan_settings(config)
            .expect("workspace settings must be validated before commit");
        self.apply_settings(config, layout_settings, plan);
    }

    pub(crate) fn apply_settings(
        &mut self,
        config: &VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
        plan: WorkspaceSettingsPlan,
    ) {
        self.settings = config.clone();
        self.layout_settings = layout_settings.clone();
        for (id, name) in plan.names {
            self[id].name = name;
        }
        for (space, name) in plan.additions {
            let index = self.scope_ids(space).count();
            let mode = self.resolve_layout_mode_for_workspace(index, &name);
            self.insert_workspace(space, name, mode);
        }
        for space in self.initialized_spaces() {
            self.repair_activation(space);
        }
    }

    /// Adds a workspace to an already-initialized space. If the space has been
    /// shown, the new workspace gets a layout for the space's current size so
    /// it can accept windows immediately instead of waiting for the next
    /// `SpaceExposed`.
    fn insert_workspace(
        &mut self,
        space: SpaceId,
        name: String,
        mode: LayoutMode,
    ) -> VirtualWorkspaceId {
        let size = self
            .workspaces_on(space)
            .find_map(|id| self.workspaces[id].layout_state.active_size());
        let mut workspace = VirtualWorkspace::new(name, space, mode, &self.layout_settings);
        workspace.preferred_display = self
            .workspaces
            .values()
            .find(|workspace| {
                workspace.home_space == Some(space) && workspace.preferred_display.is_some()
            })
            .and_then(|workspace| workspace.preferred_display.clone());
        workspace.home_space = Some(space);
        if let Some(size) = size {
            workspace.ensure_layout_for_size(size);
        }
        let id = self.workspaces.insert(workspace);
        self.order.push(id);
        id
    }

    fn default_workspace_name(&self, index: usize) -> String {
        self.settings
            .workspace_names
            .get(index)
            .cloned()
            .unwrap_or_else(|| format!("Workspace {}", index + 1))
    }

    fn append_default_workspace(&mut self, space: SpaceId) -> VirtualWorkspaceId {
        let index = self.scope_ids(space).count();
        let name = self.default_workspace_name(index);
        let mode = self.resolve_layout_mode_for_workspace(index, &name);
        self.insert_workspace(space, name, mode)
    }

    /// Allocate the initial shared target once, distributed over visible managed
    /// desktops. Subsequent native desktop arrivals get only their minimum state.
    /// `rule_space` resolves a workspace rule's display to one of `spaces`.
    pub(crate) fn ensure_visible_spaces(
        &mut self,
        spaces: &[SpaceId],
        rule_space: impl Fn(&str) -> Option<SpaceId>,
    ) {
        if spaces.is_empty() {
            return;
        }
        if self.settings.display_mode == DisplayMode::Shared && self.order.is_empty() {
            let count = self.settings.default_workspace_count.max(spaces.len()).min(MAX_WORKSPACES);
            for index in 0..count {
                let name = self.default_workspace_name(index);
                let space = self
                    .settings
                    .workspace_rules
                    .iter()
                    .rev()
                    .filter(|rule| selector_matches(&rule.workspace, index, &name))
                    .find_map(|rule| rule.display.as_deref())
                    .and_then(&rule_space)
                    .filter(|space| spaces.contains(space))
                    .unwrap_or(spaces[index % spaces.len()]);
                let id = self.append_default_workspace(space);
                if index == self.settings.default_workspace
                    || !self.active_workspace_per_space.contains_key(&space)
                {
                    self.active_workspace_per_space.insert(space, SpaceActivation::showing(id));
                }
            }
        } else {
            for &space in spaces {
                self.ensure_space_initialized(space);
            }
        }
    }

    pub(crate) fn ensure_space_initialized(&mut self, space: SpaceId) {
        if self.active_workspace(space).is_some() {
            return;
        }
        let first = self.workspaces_on(space).next();
        if let Some(id) = first {
            self.set_active_workspace(space, id);
            return;
        }
        let target = self.settings.default_workspace_count.max(1).min(MAX_WORKSPACES);
        let count = if self.settings.display_mode == DisplayMode::Shared {
            target.saturating_sub(self.order.len()).max(1)
        } else {
            target
        };
        let ids: Vec<_> = (0..count).map(|_| self.append_default_workspace(space)).collect();
        if let Some(&id) = ids.get(self.settings.default_workspace.min(ids.len() - 1)) {
            self.active_workspace_per_space.insert(space, SpaceActivation::showing(id));
        }
    }

    pub(crate) fn resolve_layout_mode_for_workspace(&self, index: usize, name: &str) -> LayoutMode {
        // Check workspace_rules (last matching rule wins, like app_rules)
        self.settings
            .workspace_rules
            .iter()
            .rev()
            .filter(|rule| selector_matches(&rule.workspace, index, name))
            .find_map(|rule| rule.layout)
            .unwrap_or(self.layout_settings.mode)
    }

    /// Shared-mode display assignments from workspace rules (last matching rule wins).
    /// Empty unless shared mode is active and some rule names a display.
    pub(crate) fn rule_displays(&self) -> Vec<(VirtualWorkspaceId, &str)> {
        if self.settings.display_mode != DisplayMode::Shared
            || !self.settings.workspace_rules.iter().any(|rule| rule.display.is_some())
        {
            return Vec::new();
        }
        self.order
            .iter()
            .enumerate()
            .filter_map(|(index, &id)| {
                let name = &self[id].name;
                let display = self
                    .settings
                    .workspace_rules
                    .iter()
                    .rev()
                    .filter(|rule| selector_matches(&rule.workspace, index, name))
                    .find_map(|rule| rule.display.as_deref())?;
                Some((id, display))
            })
            .collect()
    }

    pub fn initialized_spaces(&self) -> Vec<SpaceId> {
        let mut spaces = self.active_workspace_per_space.keys().copied().collect::<Vec<_>>();
        spaces.sort_unstable();
        spaces
    }

    /// Validate the serialized workspace graph before layout code indexes into slotmaps.
    ///
    /// Persistence files are user-visible and may be old, truncated, or manually edited. Loading
    /// malformed topology must return a useful error instead of panicking later through indexing.
    pub(crate) fn validate_persisted_topology(&self) -> Result<(), String> {
        let mut indexed = HashSet::default();
        for &workspace in &self.order {
            if !self.workspaces.contains_key(workspace) || !indexed.insert(workspace) {
                return Err(format!(
                    "invalid or duplicate workspace {workspace:?} in catalog"
                ));
            }
        }
        for (&space, activation) in &self.active_workspace_per_space {
            for (kind, id) in [
                ("active", activation.current),
                ("previous", activation.previous),
            ] {
                if id.is_some_and(|id| {
                    !self.workspaces.contains_key(id)
                        || (kind == "active" && self.workspace_space(id) != Some(space))
                        || (kind == "previous" && Some(id) == activation.current)
                }) {
                    return Err(format!(
                        "native space {} has an invalid {kind} workspace",
                        space.get()
                    ));
                }
            }
            if activation.remembered.is_some_and(|id| !self.workspaces.contains_key(id)) {
                return Err(format!(
                    "native space {} remembers a missing workspace",
                    space.get()
                ));
            }
        }

        for (workspace, entry) in &self.workspaces {
            if !indexed.contains(&workspace)
                || !self.active_workspace_per_space.contains_key(&entry.space)
            {
                return Err(format!(
                    "workspace {workspace:?} for native space {} is not indexed",
                    entry.space.get()
                ));
            }
        }
        Ok(())
    }

    /// Moves `old_space`'s workspaces onto `new_space` and returns the workspaces that were
    /// auto-created there and deleted to make room, so callers can drop state keyed by them.
    pub fn remap_space(
        &mut self,
        window_store: &mut WindowStore,
        old_space: SpaceId,
        new_space: SpaceId,
    ) -> Vec<VirtualWorkspaceId> {
        for workspace in self.workspaces.values_mut() {
            if workspace.home_space == Some(old_space) {
                workspace.home_space = Some(new_space);
            }
        }
        if old_space == new_space || !self.active_workspace_per_space.contains_key(&old_space) {
            return Vec::new();
        }

        let deleted_target_workspace_ids = if self.settings.display_mode == DisplayMode::Independent
        {
            self.workspace_ids(new_space)
        } else {
            Vec::new()
        };
        for &id in &deleted_target_workspace_ids {
            self.workspaces.remove(id);
        }
        if !deleted_target_workspace_ids.is_empty() {
            self.order.retain(|id| !deleted_target_workspace_ids.contains(id));
        }
        for workspace in
            self.workspaces.values_mut().filter(|workspace| workspace.space == old_space)
        {
            workspace.space = new_space;
        }
        if let Some(activation) = self.active_workspace_per_space.remove(&old_space) {
            self.active_workspace_per_space.insert(new_space, activation);
        }
        window_store.remap_space(old_space, new_space);

        // The window server puts windows on a desktop before rift is told it replaced
        // another, so some are already assigned to the workspaces just deleted. Dropping
        // those assignments leaves the window on no workspace, which also loses which
        // workspace of the desktop it was on. Each moves to the migrated workspace in the
        // same position instead.
        if !deleted_target_workspace_ids.is_empty() {
            let ids = self.workspace_ids(new_space);
            for (ordinal, &deleted) in deleted_target_workspace_ids.iter().enumerate() {
                window_store
                    .remap_workspace(deleted, ids.get(ordinal).or_else(|| ids.last()).copied());
            }
        }

        deleted_target_workspace_ids
    }

    pub fn create_workspace(
        &mut self,
        space: SpaceId,
        name: Option<String>,
    ) -> Result<VirtualWorkspaceId, WorkspaceError> {
        if self.settings.display_mode == DisplayMode::Shared && self.order.len() >= MAX_WORKSPACES {
            return Err(WorkspaceError::InconsistentState(
                "Maximum global workspace limit reached".into(),
            ));
        }
        self.ensure_space_initialized(space);
        let count = self.scope_ids(space).count();
        if count >= MAX_WORKSPACES {
            return Err(WorkspaceError::InconsistentState(format!(
                "Maximum workspace limit ({}) reached for space {:?}",
                MAX_WORKSPACES, space
            )));
        }

        let name = name.unwrap_or_else(|| {
            let name = format!("Workspace {}", self.workspace_counter);
            self.workspace_counter += 1;
            name
        });

        let idx = self.scope_ids(space).count();
        let mode = self.resolve_layout_mode_for_workspace(idx, &name);

        Ok(self.insert_workspace(space, name, mode))
    }

    pub fn last_workspace(&self, space: SpaceId) -> Option<VirtualWorkspaceId> {
        self.active_workspace_per_space.get(&space)?.previous
    }

    pub fn active_workspace(&self, space: SpaceId) -> Option<VirtualWorkspaceId> {
        self.active_workspace_per_space.get(&space)?.current
    }

    pub fn active_workspace_idx(&self, space: SpaceId) -> Option<u64> {
        self.workspace_index(space, self.active_workspace(space)?)
            .map(|index| index as u64)
    }

    /// Select a workspace and remember where navigation started, even when the
    /// destination is already visible on another display.
    pub fn set_active_workspace_from(
        &mut self,
        source: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> bool {
        let Some(target) = self.workspace_space(workspace_id) else {
            return false;
        };
        let previous = self.active_workspace(source);
        if !self.set_active_workspace(target, workspace_id) {
            return false;
        }
        if self.settings.display_mode == DisplayMode::Shared && source != target {
            self.active_workspace_per_space.get_mut(&target).unwrap().previous = previous;
        }
        true
    }

    pub fn set_active_workspace(
        &mut self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> bool {
        trace_misc("set_active_workspace", || {
            if self.workspace_space(workspace_id) != Some(space) {
                error!(
                    ?workspace_id,
                    ?space,
                    "Attempted to activate a missing or foreign workspace"
                );
                return false;
            }
            let activation = self.active_workspace_per_space.entry(space).or_default();
            if activation.current != Some(workspace_id) {
                activation.previous = activation.current.replace(workspace_id);
            }
            true
        })
    }

    /// Independent mode steps through `space`'s workspaces. Shared mode steps through
    /// the whole visible catalog, so the target may live on another display.
    pub(crate) fn step_workspace<'a>(
        &self,
        window_store: &WindowStore,
        context: impl Into<WorkspaceContext<'a>>,
        current: VirtualWorkspaceId,
        skip_empty: Option<bool>,
        dir: Direction,
    ) -> Option<VirtualWorkspaceId> {
        let context = context.into();
        let own = [context.space];
        let visible = if context.visible.is_empty() {
            &own[..]
        } else {
            context.visible
        };
        let context = WorkspaceContext { visible, ..context };
        let eligible = |id| {
            self.eligible(context, id)
                && ((self.settings.display_mode == DisplayMode::Shared
                    && context.navigation == NavigationScope::SharedVisible)
                    || self[id].space == context.space)
        };
        if !self.workspaces.contains_key(current) || !eligible(current) {
            return None;
        }
        let non_empty =
            |id| skip_empty != Some(true) || window_store.workspace_window_count(id) != 0;
        let mut crossed = false;
        let mut wrapped = None;
        let mut remaining = self.order.iter();
        loop {
            let id = match dir {
                Direction::Right => remaining.next(),
                Direction::Left => remaining.next_back(),
                _ => return None,
            };
            let Some(&id) = id else { break };
            if !eligible(id) {
                continue;
            }
            if id == current {
                crossed = true;
                if wrapped.is_none() && non_empty(id) {
                    wrapped = Some(id);
                }
            } else if non_empty(id) {
                if crossed {
                    return Some(id);
                }
                wrapped.get_or_insert(id);
            }
        }
        (!self.settings.prevent_wrapping).then_some(wrapped).flatten()
    }

    pub fn assign_window_to_workspace(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        window_id: WindowId,
        workspace_id: VirtualWorkspaceId,
    ) -> bool {
        trace_misc("assign_window_to_workspace", || {
            if !self.workspaces.contains_key(workspace_id)
                || self.workspaces.get(workspace_id).map(|w| w.space) != Some(space)
            {
                error!(
                    "Attempted to assign window to non-existent/foreign workspace {:?} for space {:?}",
                    workspace_id, space
                );
                return false;
            }

            let previous_assignment = window_store.workspace_for_window(window_id);
            window_store.assign_window_to_workspace(window_id, workspace_id);
            if let Some(previous_assignment) = previous_assignment
                && previous_assignment != workspace_id
                && let Some(previous_workspace) = self.workspaces.get_mut(previous_assignment)
                && previous_workspace.last_focused() == Some(window_id)
            {
                previous_workspace.set_last_focused(None);
            }
            true
        })
    }

    /// Moves a window to `space` while retaining the ordinal of its current
    /// virtual workspace. This is used for native-space identity churn, where
    /// WindowServer can briefly report a different space without a user moving
    /// the window to the destination's active workspace.
    pub fn assign_window_to_workspace_preserving_ordinal(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        window_id: WindowId,
    ) -> Option<VirtualWorkspaceId> {
        self.ensure_space_initialized(space);

        let existing_assignment = window_store.workspace_for_window(window_id)?;
        let existing_space = self.workspace_space(existing_assignment)?;
        if existing_space == space {
            return Some(existing_assignment);
        }
        let source_index =
            self.workspaces_on(existing_space).position(|id| id == existing_assignment)?;
        let target_workspace_id = self.workspaces_on(space).nth(source_index)?;

        self.assign_window_to_workspace(window_store, space, window_id, target_workspace_id)
            .then_some(target_workspace_id)
    }

    pub fn remove_window(&mut self, window_store: &mut WindowStore, window_id: WindowId) {
        let _ = window_store.remove_window_assignment(window_id);
        window_store.clear_rule_floating(window_id);
    }

    /// Gets all windows in the active virtual workspace for a given native space.
    pub fn windows_in_active_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
    ) -> Vec<WindowId> {
        if let Some(workspace_id) = self.active_workspace(space) {
            return window_store.workspace_windows(workspace_id);
        }
        Vec::new()
    }

    pub fn is_window_in_active_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        window_id: WindowId,
    ) -> bool {
        if let Some(active_workspace_id) = self.active_workspace(space) {
            if let Some(window_workspace_id) =
                self.workspace_for_window(window_store, space, window_id)
            {
                return window_workspace_id == active_workspace_id;
            }
        }
        true
    }

    pub fn windows_in_inactive_workspaces(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
    ) -> Vec<WindowId> {
        let active_workspace_id = self.active_workspace(space);

        self.workspaces
            .iter()
            .filter(|(id, workspace)| workspace.space == space && Some(*id) != active_workspace_id)
            .flat_map(|(id, _)| window_store.workspace_windows(id))
            .collect()
    }

    pub fn find_window_by_idx(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        idx: u32,
    ) -> Option<WindowId> {
        window_store.iter_workspace_assignments().find_map(|(wid, info)| {
            (self.workspaces.get(info).is_some_and(|workspace| workspace.space == space)
                && wid.idx.get() == idx)
                .then_some(wid)
        })
    }

    pub fn find_window_in_workspace_by_idx(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        idx: u32,
    ) -> Option<WindowId> {
        if self.workspaces.get(workspace_id).map(|w| w.space) != Some(space) {
            return None;
        }

        self.workspaces.get(workspace_id).and_then(|_| {
            window_store
                .workspace_windows(workspace_id)
                .into_iter()
                .find(|wid| wid.idx.get() == idx)
        })
    }

    pub fn set_last_focused_window(
        &mut self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        window_id: Option<WindowId>,
    ) {
        if self.workspaces.get(workspace_id).map(|w| w.space) == Some(space) {
            if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                workspace.set_last_focused(window_id);
            }
        }
    }

    pub fn last_focused_window(
        &self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> Option<WindowId> {
        if self.workspaces.get(workspace_id).map(|w| w.space) == Some(space) {
            self.workspaces.get(workspace_id)?.last_focused()
        } else {
            None
        }
    }

    pub fn workspace_info(
        &self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> Option<&VirtualWorkspace> {
        self.workspaces.get(workspace_id).filter(|workspace| workspace.space == space)
    }

    pub fn transfer_window_identity(&mut self, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }
        for workspace in self.workspaces.values_mut() {
            let contained_from = workspace.last_focused() == Some(from);
            if workspace.last_focused() == Some(to) {
                workspace.set_last_focused(None);
            }
            if contained_from {
                workspace.set_last_focused(Some(to));
            }
        }
    }

    pub(crate) fn forget_window_identity(&mut self, window: WindowId) {
        for workspace in self.workspaces.values_mut() {
            if workspace.last_focused() == Some(window) {
                workspace.set_last_focused(None);
            }
        }
    }

    pub(crate) fn persisted_focus_locations(&self) -> Vec<(SpaceId, VirtualWorkspaceId, WindowId)> {
        self.workspaces
            .iter()
            .filter_map(|(workspace, info)| {
                info.last_focused().map(|window| (info.space, workspace, window))
            })
            .collect()
    }

    pub(crate) fn retain_window_focus_location(
        &mut self,
        window: WindowId,
        keep: VirtualWorkspaceId,
    ) {
        for (workspace_id, workspace) in self.workspaces.iter_mut() {
            if workspace_id != keep && workspace.last_focused() == Some(window) {
                workspace.set_last_focused(None);
            }
        }
    }

    pub fn list_workspaces(&mut self, space: SpaceId) -> Vec<(VirtualWorkspaceId, String)> {
        self.ensure_space_initialized(space);
        self.existing_workspaces(space)
    }

    /// Read workspace topology without creating missing state. Validation and restore planning
    /// must use this accessor so a failed transaction cannot initialize part of the live engine.
    pub(crate) fn existing_workspaces(&self, space: SpaceId) -> Vec<(VirtualWorkspaceId, String)> {
        self.workspaces_on(space)
            .filter_map(|id| self.workspaces.get(id).map(|ws| (id, ws.name.clone())))
            .collect()
    }

    /// Workspace IDs are kept in canonical ordinal order, including when loading older files.
    pub(crate) fn workspace_ids(&self, space: SpaceId) -> Vec<VirtualWorkspaceId> {
        self.workspaces_on(space).collect()
    }

    pub(crate) fn workspaces_on(
        &self,
        space: SpaceId,
    ) -> impl Iterator<Item = VirtualWorkspaceId> + '_ {
        self.order.iter().copied().filter(move |id| self[*id].space == space)
    }

    pub fn rename_workspace(
        &mut self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        new_name: String,
    ) -> bool {
        if self.workspaces.get(workspace_id).map(|w| w.space) != Some(space) {
            return false;
        }
        if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
            workspace.name = new_name;

            true
        } else {
            false
        }
    }

    pub fn workspace_for_window(
        &self,
        windows: &WindowStore,
        space: SpaceId,
        window: WindowId,
    ) -> Option<VirtualWorkspaceId> {
        windows.workspace_for_window(window).filter(|assignment| {
            self.workspaces
                .get(*assignment)
                .is_some_and(|workspace| workspace.space == space)
        })
    }

    pub fn auto_assign_window(
        &mut self,
        window_store: &mut WindowStore,
        window_id: WindowId,
        space: SpaceId,
    ) -> Result<VirtualWorkspaceId, WorkspaceError> {
        let default_workspace_id = self.get_default_workspace(space)?;
        if self.assign_window_to_workspace(window_store, space, window_id, default_workspace_id) {
            window_store.clear_rule_floating(window_id);
            Ok(default_workspace_id)
        } else {
            Err(WorkspaceError::AssignmentFailed)
        }
    }

    pub(crate) fn apply_app_rule_decision<'a>(
        &mut self,
        window_store: &mut WindowStore,
        window_id: WindowId,
        context: impl Into<WorkspaceContext<'a>>,
        rule_decision: Option<AppRuleDecision>,
        preserve_existing: bool,
    ) -> Result<AppRuleResult, WorkspaceError> {
        let context = context.into();
        let space = context.space;
        self.ensure_space_initialized(space);
        if self.workspaces_on(space).next().is_none() {
            return Err(WorkspaceError::NoWorkspacesAvailable);
        }

        let existing_assignment = self.workspace_for_window(window_store, space, window_id);

        let rule_override = rule_decision.as_ref().and_then(AppRuleDecision::management_override);
        let admitted = window_store
            .window(window_id)
            .map(|window| window.is_admitted_with_override(rule_override))
            // Assignment tests and restore paths may not yet have an AX
            // snapshot. In that case only an explicit rejection can deny it.
            .unwrap_or(rule_override != Some(false));
        if let Some(window) = window_store.window_mut(window_id) {
            window.manage_override = rule_override;
        }
        if !admitted {
            window_store.clear_rule_floating(window_id);
            return Ok(AppRuleResult::Rejected(if rule_override == Some(false) {
                AppRuleRejection::ExplicitRule
            } else {
                AppRuleRejection::Heuristic
            }));
        }

        let (workspace, floating, position, size, focus) =
            rule_decision.map_or((None, false, None, None, false), |decision| {
                (
                    decision.workspace,
                    decision.floating,
                    decision.position,
                    decision.size,
                    decision.focus,
                )
            });
        let selected = workspace
            .as_ref()
            .and_then(|selector| self.resolve_workspace(context, selector))
            .map(|target| target.id);
        if workspace.is_some() && selected.is_none() {
            warn!(
                ?space,
                ?workspace,
                "App rule workspace was not found; preserving assignment"
            );
        }
        let workspace_id = if preserve_existing {
            existing_assignment.or(selected)
        } else {
            selected.or(existing_assignment)
        }
        .map(Ok)
        .unwrap_or_else(|| self.get_default_workspace(space))?;
        let prior_assignment = window_store.workspace_for_window(window_id);
        let workspace_changed = prior_assignment != Some(workspace_id);
        if !self.assign_window_to_workspace(
            window_store,
            self[workspace_id].space,
            window_id,
            workspace_id,
        ) {
            error!("Failed to apply window workspace assignment");
            return Err(WorkspaceError::AssignmentFailed);
        }
        let was_rule_floating = window_store.replace_rule_floating(window_id, floating);
        // Focus is a placement side effect, not persistent state. Rediscovery may
        // reapply the same rule repeatedly, so only emit it for a real transition.
        let focus = focus && (workspace_changed || was_rule_floating != floating);
        Ok(AppRuleResult::Managed(AppRuleEffects {
            source_workspace: prior_assignment,
            workspace_id,
            floating,
            position,
            size,
            focus,
            was_rule_floating,
        }))
    }

    #[cfg(test)]
    fn assign_window_with_app_info(
        &mut self,
        window_store: &mut WindowStore,
        window_id: WindowId,
        space: SpaceId,
        app_bundle_id: Option<&str>,
        app_name: Option<&str>,
        window_title: Option<&str>,
        ax_role: Option<&str>,
        ax_subrole: Option<&str>,
    ) -> Result<AppRuleResult, WorkspaceError> {
        let decision = crate::model::AppRuleEngine::new(&self.settings.app_rules).evaluate(
            crate::model::WindowRuleContext {
                app_bundle_id,
                app_name,
                window_title,
                ax_role,
                ax_subrole,
            },
        );
        self.apply_app_rule_decision(window_store, window_id, space, decision, false)
    }

    fn get_default_workspace(
        &mut self,
        space: SpaceId,
    ) -> Result<VirtualWorkspaceId, WorkspaceError> {
        self.ensure_space_initialized(space);
        self.active_workspace(space).ok_or(WorkspaceError::NoWorkspacesAvailable)
    }

    pub(crate) fn window_counts<'a>(
        &'a self,
        windows: &'a WindowStore,
    ) -> impl Iterator<Item = (VirtualWorkspaceId, usize)> + 'a {
        self.catalog().map(|id| (id, windows.workspace_window_count(id)))
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::*;
    use crate::actor::app::WindowId;
    use crate::model::{HiddenWindowPlacement, HideCorner};
    use crate::sys::screen::SpaceId;

    fn expect_managed(result: Result<AppRuleResult, WorkspaceError>) -> AppRuleEffects {
        match result {
            Ok(AppRuleResult::Managed(decision)) => decision,
            Ok(AppRuleResult::Rejected(reason)) => {
                panic!("Window was unexpectedly rejected: {reason:?}")
            }
            Err(e) => panic!("assign_window_with_app_info failed: {:?}", e),
        }
    }

    fn assign(
        manager: &mut WorkspaceStore,
        window_store: &mut WindowStore,
        window_id: WindowId,
        space: SpaceId,
        app_id: Option<&str>,
        app_name: Option<&str>,
        window_title: Option<&str>,
        ax_role: Option<&str>,
        ax_subrole: Option<&str>,
    ) -> AppRuleEffects {
        expect_managed(manager.assign_window_with_app_info(
            window_store,
            window_id,
            space,
            app_id,
            app_name,
            window_title,
            ax_role,
            ax_subrole,
        ))
    }

    #[test]
    fn navigation_scope_wrap_empty_and_dormant_policy_matrix() {
        let (a, b, dormant) = (SpaceId::new(101), SpaceId::new(201), SpaceId::new(301));
        for mode in [DisplayMode::Independent, DisplayMode::Shared] {
            for prevent_wrapping in [false, true] {
                let settings = VirtualWorkspaceSettings {
                    display_mode: mode,
                    default_workspace_count: 6,
                    prevent_wrapping,
                    ..Default::default()
                };
                let mut store =
                    WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
                store.ensure_visible_spaces(&[a, b], |_| None);
                store.ensure_space_initialized(dormant);
                let mut windows = WindowStore::default();
                for scope in [NavigationScope::Local, NavigationScope::SharedVisible] {
                    for visible in [&[a][..], &[a, b][..]] {
                        let context = WorkspaceContext {
                            space: a,
                            visible,
                            navigation: scope,
                        };
                        let expected: Vec<_> = store
                            .catalog()
                            .filter(|&id| {
                                store[id].space == a
                                    || (mode == DisplayMode::Shared
                                        && scope == NavigationScope::SharedVisible
                                        && visible.contains(&store[id].space))
                            })
                            .collect();
                        for (position, &current) in expected.iter().enumerate() {
                            let next = expected
                                .get(position + 1)
                                .copied()
                                .or_else(|| (!prevent_wrapping).then_some(expected[0]));
                            let previous =
                                position.checked_sub(1).map(|index| expected[index]).or_else(
                                    || (!prevent_wrapping).then_some(*expected.last().unwrap()),
                                );
                            assert_eq!(
                                store.step_workspace(
                                    &windows,
                                    context,
                                    current,
                                    Some(false),
                                    Direction::Right
                                ),
                                next
                            );
                            assert_eq!(
                                store.step_workspace(
                                    &windows,
                                    context,
                                    current,
                                    None,
                                    Direction::Left
                                ),
                                previous
                            );
                            // Every workspace is empty: skipping empties cannot invent a target.
                            assert_eq!(
                                store.step_workspace(
                                    &windows,
                                    context,
                                    current,
                                    Some(true),
                                    Direction::Right
                                ),
                                None
                            );
                            assert_eq!(
                                store.step_workspace(
                                    &windows,
                                    context,
                                    current,
                                    Some(true),
                                    Direction::Left
                                ),
                                None
                            );
                        }
                        let occupied = *expected.last().unwrap();
                        let window = WindowId::new(90, 1);
                        assert!(store.assign_window_to_workspace(
                            &mut windows,
                            store[occupied].space,
                            window,
                            occupied
                        ));
                        assert_eq!(
                            store.step_workspace(
                                &windows,
                                context,
                                expected[0],
                                Some(true),
                                Direction::Right
                            ),
                            (expected.len() > 1 || !prevent_wrapping).then_some(occupied)
                        );
                        assert_eq!(
                            store.step_workspace(
                                &windows,
                                context,
                                expected[0],
                                Some(true),
                                Direction::Left
                            ),
                            (!prevent_wrapping).then_some(occupied)
                        );
                        windows.debug_assert_invariants();
                        store.remove_window(&mut windows, window);
                    }
                }
            }
        }
    }

    #[test]
    fn shared_initial_count_is_global_and_later_desktops_append_minimal_state() {
        let (a, b, c) = (SpaceId::new(51), SpaceId::new(52), SpaceId::new(53));
        for count in [1, 6] {
            let settings = VirtualWorkspaceSettings {
                display_mode: DisplayMode::Shared,
                default_workspace_count: count,
                ..Default::default()
            };
            let mut store = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
            store.ensure_visible_spaces(&[a, b], |_| None);
            let catalog: Vec<_> = store.catalog().collect();
            assert_eq!(catalog.len(), count.max(2));
            for (index, &id) in catalog.iter().enumerate() {
                assert_eq!(store[id].space, if index % 2 == 0 { a } else { b });
            }
            store.ensure_visible_spaces(&[a, b, c], |_| None);
            assert_eq!(store.workspace_ids(c).len(), 1);
            assert_eq!(store.catalog().take(catalog.len()).collect::<Vec<_>>(), catalog);
            store.ensure_visible_spaces(&[a, b, c], |_| None);
            assert_eq!(store.catalog().count(), catalog.len() + 1);
            store.validate_persisted_topology().unwrap();
        }
    }

    #[test]
    fn native_remap_preserves_shared_catalog_and_remaps_a_temporarily_away_home() {
        let mut settings = VirtualWorkspaceSettings {
            default_workspace_count: 1,
            display_mode: DisplayMode::Shared,
            ..Default::default()
        };
        let mut store = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let mut windows = WindowStore::default();
        let old = SpaceId::new(41);
        let fallback = SpaceId::new(42);
        let new = SpaceId::new(43);
        let id = store.list_workspaces(old)[0].0;
        store[id].home_space = Some(old);
        store[id].preferred_display = Some("home".into());
        let fallback_id = store.list_workspaces(fallback)[0].0;
        assert!(store.transfer_workspace(
            id,
            fallback,
            Some("fallback"),
            WorkspaceTransferReason::DisplayDeparture
        ));
        let before: Vec<_> = store.catalog().collect();
        assert!(store.remap_space(&mut windows, old, new).is_empty());
        assert_eq!(store[id].home_space, Some(new));
        assert_eq!(store[id].space, fallback);
        settings.display_mode = DisplayMode::Independent;
        store.update_settings(&settings, &LayoutSettings::default());
        assert!(
            store.workspaces_on(new).next().is_none(),
            "an absent home needs no replacement catalog on reload"
        );
        settings.display_mode = DisplayMode::Shared;
        store.update_settings(&settings, &LayoutSettings::default());
        assert_eq!(store.catalog().collect::<Vec<_>>(), before);
        assert!(store.transfer_workspace(
            id,
            new,
            Some("home"),
            WorkspaceTransferReason::Restoration
        ));
        assert_eq!(store.catalog().collect::<Vec<_>>(), before);
        assert_eq!(store.active_workspace(new), Some(id));
        assert_eq!(store.active_workspace(fallback), Some(fallback_id));
        // A proven replacement can already have independently customized shared workspaces.
        let destination = store.list_workspaces(old)[0].0;
        store[destination].name = "Customized".into();
        let before: Vec<_> = store.catalog().collect();
        store.remap_space(&mut windows, new, old);
        assert_eq!(store.catalog().collect::<Vec<_>>(), before);
        assert_eq!(store[destination].name, "Customized");
        assert_eq!(store[id].space, old);
        settings.display_mode = DisplayMode::Independent;
        store.update_settings(&settings, &LayoutSettings::default());
        assert!(store.workspace_ids(old).contains(&id));
        assert!(store.workspace_ids(old).contains(&destination));
        store.validate_persisted_topology().unwrap();
    }

    #[test]
    fn shared_catalog_resolution_retains_dormant_positions_and_rejects_ambiguous_names() {
        let a1 = SpaceId::new(11);
        let a2 = SpaceId::new(12);
        let b1 = SpaceId::new(21);
        let mut settings = VirtualWorkspaceSettings {
            default_workspace_count: 2,
            ..Default::default()
        };
        let mut store = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let a = store.list_workspaces(a1);
        let b = store.list_workspaces(b1);
        settings.display_mode = DisplayMode::Shared;
        store.update_settings(&settings, &LayoutSettings::default());
        assert_ne!(
            store[a[0].0].name, store[b[0].0].name,
            "migration disambiguates duplicate names"
        );
        store[b[0].0].name = store[a[0].0].name.clone();
        let context = WorkspaceContext {
            space: a1,
            visible: &[a1, b1],
            navigation: NavigationScope::SharedVisible,
        };
        assert_eq!(
            store.resolve_workspace(context, &WorkspaceSelector::Index(2)),
            Some(WorkspaceTarget { id: b[0].0, space: b1 })
        );
        assert_eq!(
            store.resolve_workspace(context, &WorkspaceSelector::Name(a[0].1.clone())),
            None
        );
        store.list_workspaces(a2);
        let switched = WorkspaceContext {
            space: a2,
            visible: &[a2, b1],
            navigation: Default::default(),
        };
        assert_eq!(
            store.resolve_workspace(switched, &WorkspaceSelector::Index(0)),
            None
        );
        assert_eq!(
            store.resolve_workspace(switched, &WorkspaceSelector::Index(2)).unwrap().id,
            b[0].0
        );
        assert_eq!(
            store
                .workspace_view(switched)
                .iter()
                .map(|(index, _, _)| *index)
                .collect::<Vec<_>>(),
            [2, 3, 4]
        );
        assert_eq!(store.active_workspace(a1), Some(a[0].0));
        assert_eq!(
            store.resolve_workspace(context, &WorkspaceSelector::Index(0)).unwrap().id,
            a[0].0
        );
        settings.display_mode = DisplayMode::Independent;
        store.update_settings(&settings, &LayoutSettings::default());
        assert_eq!(
            store.workspace_ids(a1),
            a.iter().map(|(id, _)| *id).collect::<Vec<_>>()
        );
        assert!(store.workspace_ids(a2).len() >= 2);
        assert_eq!(
            store.workspace_ids(b1),
            b.iter().map(|(id, _)| *id).collect::<Vec<_>>()
        );
        store.validate_persisted_topology().unwrap();
    }

    #[test]
    fn workspace_transfer_preserves_identity_membership_layout_and_explicit_home() {
        let source = SpaceId::new(31);
        let target = SpaceId::new(32);
        let mut store = WorkspaceStore::new();
        let mut windows = WindowStore::default();
        let id = store.list_workspaces(source)[0].0;
        store[id].ensure_layout_for_size(CGSize::new(1200.0, 800.0));
        let window = WindowId::new(5, 1);
        store.assign_window_to_workspace(&mut windows, source, window, id);
        store[id].set_last_focused(Some(window));
        store[id].preferred_display = Some("external".into());
        store[id].home_space = Some(source);
        let layout = store[id].active_layout();
        assert!(store.transfer_workspace(
            id,
            target,
            Some("builtin"),
            WorkspaceTransferReason::DisplayDeparture
        ));
        assert_eq!(windows.workspace_for_window(window).unwrap(), id);
        assert_eq!(store[id].active_layout(), layout);
        assert_eq!(store[id].last_focused(), Some(window));
        assert_eq!(store[id].preferred_display.as_deref(), Some("external"));
        assert_eq!(store[id].home_space, Some(source));
        assert!(store.transfer_workspace(
            id,
            source,
            Some("external"),
            WorkspaceTransferReason::Restoration
        ));
        assert!(store.transfer_workspace(
            id,
            target,
            Some("builtin"),
            WorkspaceTransferReason::Explicit
        ));
        assert_eq!(store[id].home_space, Some(target));
        assert_eq!(store[id].preferred_display.as_deref(), Some("builtin"));
        assert_eq!(store.active_workspace(target), Some(id));
        store.validate_persisted_topology().unwrap();
        let restored: WorkspaceStore =
            ron::from_str(&ron::ser::to_string(&store).unwrap()).unwrap();
        assert_eq!(restored[id].home_space, Some(target));
        assert_eq!(restored[id].active_layout(), layout);
    }

    #[test]
    fn test_virtual_workspace_creation() {
        let mut manager = WorkspaceStore::new();

        let space = SpaceId::new(1);
        assert_eq!(
            manager.list_workspaces(space).len(),
            manager.workspace_ids(space).len()
        );

        let ws_id = manager.create_workspace(space, Some("Test Workspace".to_string())).unwrap();
        assert!(
            manager
                .list_workspaces(space)
                .iter()
                .any(|(id, name)| *id == ws_id && name == "Test Workspace")
        );

        let workspace = manager.workspace_info(space, ws_id).unwrap();
        assert_eq!(workspace.name, "Test Workspace");
    }

    #[test]
    fn test_window_assignment() {
        let mut window_store = WindowStore::default();
        let mut manager = WorkspaceStore::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();

        let window1 = WindowId::new(1, 1);
        let window2 = WindowId::new(1, 2);

        assert!(manager.assign_window_to_workspace(&mut window_store, space, window1, ws1_id));
        assert!(manager.assign_window_to_workspace(&mut window_store, space, window2, ws2_id));

        assert_eq!(
            manager.workspace_for_window(&window_store, space, window1),
            Some(ws1_id)
        );
        assert_eq!(
            manager.workspace_for_window(&window_store, space, window2),
            Some(ws2_id)
        );

        assert_eq!(window_store.workspace_windows(ws1_id), vec![window1]);
        assert_eq!(window_store.workspace_windows(ws2_id), vec![window2]);
    }

    #[test]
    fn reassignment_updates_authoritative_workspace_index() {
        let mut window_store = WindowStore::default();
        let mut manager = WorkspaceStore::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();
        let window = WindowId::new(9, 1);

        assert!(manager.assign_window_to_workspace(&mut window_store, space, window, ws1_id));
        assert_eq!(
            manager.workspace_for_window(&window_store, space, window),
            Some(ws1_id)
        );
        assert_eq!(window_store.workspace_windows(ws1_id), vec![window]);

        assert!(manager.assign_window_to_workspace(&mut window_store, space, window, ws2_id));
        assert_eq!(
            manager.workspace_for_window(&window_store, space, window),
            Some(ws2_id)
        );
        assert!(window_store.workspace_windows(ws1_id).is_empty());
        assert_eq!(window_store.workspace_windows(ws2_id), vec![window]);
    }

    #[test]
    fn reassignment_clears_stale_last_focused_on_source_workspace() {
        let mut window_store = WindowStore::default();
        let mut manager = WorkspaceStore::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();
        let window = WindowId::new(9, 1);

        assert!(manager.assign_window_to_workspace(&mut window_store, space, window, ws1_id));
        manager.set_last_focused_window(space, ws1_id, Some(window));

        assert!(manager.assign_window_to_workspace(&mut window_store, space, window, ws2_id));

        assert_eq!(manager.last_focused_window(space, ws1_id), None);
        assert_eq!(
            manager.workspace_for_window(&window_store, space, window),
            Some(ws2_id)
        );
    }

    #[test]
    fn remap_space_migrates_workspaces_and_rehomes_deleted_target_windows() {
        let mut window_store = WindowStore::default();
        let mut manager = WorkspaceStore::new();
        let old_space = SpaceId::new(1);
        let new_space = SpaceId::new(2);
        let migrated_ws = manager.create_workspace(old_space, Some("Old".to_string())).unwrap();
        let transient_ws =
            manager.create_workspace(new_space, Some("Transient".to_string())).unwrap();
        let migrated_window = WindowId::new(10, 1);
        let transient_window = WindowId::new(11, 1);

        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            old_space,
            migrated_window,
            migrated_ws
        ));
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            new_space,
            transient_window,
            transient_ws
        ));

        manager[migrated_ws].replace_layout_mode(LayoutMode::Stack, &LayoutSettings::default());
        manager[migrated_ws].ensure_layout_for_size(CGSize::new(1200.0, 800.0));
        let first = manager[migrated_ws].active_layout();
        manager[migrated_ws].ensure_layout_for_size(CGSize::new(1600.0, 1000.0));
        manager[migrated_ws].set_last_focused(Some(migrated_window));
        let configurations = ron::ser::to_string(&manager[migrated_ws].layout_state).unwrap();
        manager.remap_space(&mut window_store, old_space, new_space);

        assert_eq!(manager[migrated_ws].space, new_space);
        assert_eq!(manager[migrated_ws].layout_mode, LayoutMode::Stack);
        assert_eq!(manager[migrated_ws].last_focused(), Some(migrated_window));
        assert_eq!(
            ron::ser::to_string(&manager[migrated_ws].layout_state).unwrap(),
            configurations
        );
        manager[migrated_ws].ensure_layout_for_size(CGSize::new(1200.0, 800.0));
        assert_eq!(manager[migrated_ws].active_layout(), first);
        assert!(!manager.layout_spaces().contains(&old_space));

        assert_eq!(
            manager.workspace_for_window(&window_store, new_space, migrated_window),
            Some(migrated_ws)
        );
        let mut on_migrated = window_store.workspace_windows(migrated_ws);
        on_migrated.sort();
        assert_eq!(on_migrated, vec![migrated_window, transient_window]);
        // The window the window server had already placed keeps a workspace.
        assert_eq!(
            manager.workspace_for_window(&window_store, new_space, transient_window),
            Some(migrated_ws)
        );
        assert!(window_store.workspace_windows(transient_ws).is_empty());
        assert!(manager.workspace_info(new_space, transient_ws).is_none());
    }

    #[test]
    fn remap_space_rehomes_windows_of_deleted_target_workspaces_by_ordinal() {
        let mut window_store = WindowStore::default();
        let mut manager = WorkspaceStore::new();
        let old_space = SpaceId::new(1);
        let new_space = SpaceId::new(2);
        let migrated: Vec<_> =
            manager.list_workspaces(old_space).into_iter().map(|(id, _)| id).collect();
        // One more than the migrated space has, so its last position has no counterpart.
        manager.create_workspace(new_space, Some("Extra".to_string())).unwrap();
        let transient: Vec<_> =
            manager.list_workspaces(new_space).into_iter().map(|(id, _)| id).collect();
        assert_eq!(transient.len(), migrated.len() + 1);
        let positions = [0, 1, transient.len() - 1];
        let windows: Vec<_> = (1..=3).map(|idx| WindowId::new(20, idx)).collect();
        for (window, &position) in windows.iter().zip(&positions) {
            assert!(manager.assign_window_to_workspace(
                &mut window_store,
                new_space,
                *window,
                transient[position]
            ));
        }

        let fullscreen = WindowId::new(21, 1);
        manager.assign_window_to_workspace(&mut window_store, new_space, fullscreen, transient[1]);
        window_store.suspend_window_to_native_fullscreen(
            fullscreen,
            None,
            Some(new_space),
            SpaceId::new(99),
            crate::model::window_store::NativeFullscreenTransition::Suspended,
        );
        window_store.remove_window(fullscreen);
        let deleted = manager.remap_space(&mut window_store, old_space, new_space);

        assert_eq!(
            window_store.native_fullscreen_record_for_window(fullscreen).unwrap().workspace,
            Some(migrated[1])
        );
        assert_eq!(deleted, transient, "callers are told every workspace that went");
        let landed: Vec<_> = windows
            .iter()
            .map(|w| manager.workspace_for_window(&window_store, new_space, *w))
            .collect();
        assert_eq!(landed, vec![
            Some(migrated[0]),
            Some(migrated[1]),
            Some(*migrated.last().unwrap())
        ]);
    }

    #[test]
    fn generic_assignment_does_not_infer_space_id_churn_from_empty_target() {
        let mut window_store = WindowStore::default();
        let mut settings = VirtualWorkspaceSettings::default();
        settings.default_workspace_count = 3;
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let old_space = SpaceId::new(1);
        let new_space = SpaceId::new(2);
        let window = WindowId::new(12, 1);

        let old_workspaces = manager.list_workspaces(old_space);
        let new_workspaces = manager.list_workspaces(new_space);
        let preserved_workspace = old_workspaces[2].0;
        let expected_target_workspace = new_workspaces[0].0;

        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            old_space,
            window,
            preserved_workspace
        ));

        let assignment = assign(
            &mut manager,
            &mut window_store,
            window,
            new_space,
            None,
            None,
            None,
            None,
            None,
        );

        assert_eq!(assignment.workspace_id, expected_target_workspace);
        assert_eq!(
            window_store.workspace_for_window(window),
            Some(expected_target_workspace)
        );
    }

    #[test]
    fn does_not_preserve_workspace_ordinal_when_target_space_already_has_assignments() {
        let mut window_store = WindowStore::default();
        let mut settings = VirtualWorkspaceSettings::default();
        settings.default_workspace_count = 3;
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let old_space = SpaceId::new(1);
        let new_space = SpaceId::new(2);
        let moved_window = WindowId::new(13, 1);
        let existing_window = WindowId::new(14, 1);

        let old_workspaces = manager.list_workspaces(old_space);
        let new_workspaces = manager.list_workspaces(new_space);

        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            old_space,
            moved_window,
            old_workspaces[2].0
        ));
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            new_space,
            existing_window,
            new_workspaces[1].0
        ));

        let assignment = assign(
            &mut manager,
            &mut window_store,
            moved_window,
            new_space,
            None,
            None,
            None,
            None,
            None,
        );

        assert_eq!(assignment.workspace_id, new_workspaces[0].0);
        assert_eq!(
            window_store.workspace_for_window(moved_window),
            Some(new_workspaces[0].0)
        );
    }

    #[test]
    fn topology_reassignment_preserves_workspace_ordinal_with_destination_assignments() {
        let mut window_store = WindowStore::default();
        let mut settings = VirtualWorkspaceSettings::default();
        settings.default_workspace_count = 3;
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let source_space = SpaceId::new(1);
        let destination_space = SpaceId::new(2);
        let moved_window = WindowId::new(15, 1);
        let destination_window = WindowId::new(16, 1);

        let source_workspaces = manager.list_workspaces(source_space);
        let destination_workspaces = manager.list_workspaces(destination_space);
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            source_space,
            moved_window,
            source_workspaces[2].0
        ));
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            destination_space,
            destination_window,
            destination_workspaces[0].0
        ));

        assert_eq!(
            manager.assign_window_to_workspace_preserving_ordinal(
                &mut window_store,
                destination_space,
                moved_window
            ),
            Some(destination_workspaces[2].0)
        );
        assert_eq!(
            window_store.workspace_for_window(moved_window),
            Some(destination_workspaces[2].0)
        );
    }

    #[test]
    fn unmanaged_rule_does_not_reassign_window_during_transient_space_id_churn() {
        let mut window_store = WindowStore::default();
        let mut settings = VirtualWorkspaceSettings::default();
        settings.default_workspace_count = 3;
        settings.app_rules = vec![AppWorkspaceRule {
            app_id: Some("com.example.unmanaged".into()),
            workspace: None,
            floating: false,
            position: None,
            size: None,
            focus: false,
            manage: Some(false),
            app_name: None,
            title_regex: None,
            title_substring: None,
            ax_role: None,
            ax_subrole: None,
        }];
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let old_space = SpaceId::new(1);
        let new_space = SpaceId::new(2);
        let window = WindowId::new(15, 1);

        let old_workspaces = manager.list_workspaces(old_space);
        let old_assignment = old_workspaces[2].0;
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            old_space,
            window,
            old_assignment
        ));

        let result = manager.assign_window_with_app_info(
            &mut window_store,
            window,
            new_space,
            Some("com.example.unmanaged"),
            None,
            None,
            None,
            None,
        );

        assert!(matches!(
            result,
            Ok(AppRuleResult::Rejected(AppRuleRejection::ExplicitRule))
        ));
        assert_eq!(
            manager.workspace_for_window(&window_store, new_space, window),
            None
        );
        assert_eq!(window_store.workspace_for_window(window), Some(old_assignment));
    }

    #[test]
    fn test_active_workspace_switching() {
        let mut manager = WorkspaceStore::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();

        assert!(manager.set_active_workspace(space, ws1_id));
        assert_eq!(manager.active_workspace(space), Some(ws1_id));

        assert!(manager.set_active_workspace(space, ws2_id));
        assert_eq!(manager.active_workspace(space), Some(ws2_id));
    }

    #[test]
    fn test_window_visibility() {
        let mut window_store = WindowStore::default();
        fn is_window_visible(
            wm: &WorkspaceStore,
            window_store: &WindowStore,
            window_id: WindowId,
            space: SpaceId,
        ) -> bool {
            let window_workspace = wm.workspace_for_window(window_store, space, window_id);
            let active_workspace = wm.active_workspace(space);

            match (window_workspace, active_workspace) {
                (Some(window_ws), Some(active_ws)) => window_ws == active_ws,
                _ => true,
            }
        }
        let mut manager = WorkspaceStore::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();
        let window1 = WindowId::new(1, 1);
        let window2 = WindowId::new(1, 2);

        manager.set_active_workspace(space, ws1_id);
        manager.assign_window_to_workspace(&mut window_store, space, window1, ws1_id);
        manager.assign_window_to_workspace(&mut window_store, space, window2, ws2_id);

        assert!(is_window_visible(&manager, &window_store, window1, space));
        assert!(!is_window_visible(&manager, &window_store, window2, space));

        manager.set_active_workspace(space, ws2_id);
        assert!(!is_window_visible(&manager, &window_store, window1, space));
        assert!(is_window_visible(&manager, &window_store, window2, space));
    }

    #[test]
    fn default_workspace_setting_applied() {
        let mut settings = VirtualWorkspaceSettings::default();
        settings.default_workspace_count = 5;
        settings.default_workspace = 3;

        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());

        let space = SpaceId::new(42);
        let workspaces = manager.list_workspaces(space);
        let expected_ws = workspaces.get(settings.default_workspace).unwrap().0;

        assert_eq!(manager.active_workspace(space), Some(expected_ws));
    }

    #[test]
    fn test_workspace_navigation() {
        let window_store = WindowStore::default();
        let mut manager = WorkspaceStore::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();
        let ws3_id = manager.create_workspace(space, Some("WS3".to_string())).unwrap();

        assert_eq!(
            manager.step_workspace(&window_store, space, ws1_id, None, Direction::Right),
            Some(ws2_id)
        );
        assert_eq!(
            manager.step_workspace(&window_store, space, ws2_id, None, Direction::Right),
            Some(ws3_id)
        );

        assert_eq!(
            manager.step_workspace(&window_store, space, ws2_id, None, Direction::Left),
            Some(ws1_id)
        );
        assert_eq!(
            manager.step_workspace(&window_store, space, ws3_id, None, Direction::Left),
            Some(ws2_id)
        );
    }

    #[test]
    fn legacy_catalog_preserves_custom_ordinal_order() {
        let window_store = WindowStore::default();
        let settings = VirtualWorkspaceSettings {
            default_workspace_count: 3,
            workspace_names: vec!["A".into(), "B".into(), "C".into()],
            ..VirtualWorkspaceSettings::default()
        };
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let space = SpaceId::new(2);
        let indexed = manager.list_workspaces(space);
        let mut reversed = manager.workspace_ids(space);
        reversed.reverse();
        let serialized = ron::ser::to_string(&manager).unwrap();
        let current = format!("order:{}", ron::ser::to_string(&manager.order).unwrap());
        let legacy = format!(
            "workspaces_by_space:{}",
            ron::ser::to_string(&HashMap::from_iter([(space, reversed)])).unwrap()
        );
        manager = ron::from_str(&serialized.replace(&current, &legacy)).unwrap();

        assert_eq!(
            manager
                .list_workspaces(space)
                .iter()
                .map(|(_, name)| name.as_str())
                .collect::<Vec<_>>(),
            ["C", "B", "A"],
        );
        assert_eq!(
            manager.step_workspace(&window_store, space, indexed[2].0, None, Direction::Right),
            Some(indexed[1].0),
        );
        assert_eq!(
            manager.step_workspace(&window_store, space, indexed[0].0, None, Direction::Left),
            Some(indexed[1].0),
        );
    }

    #[test]
    fn workspace_navigation_wraps_by_default() {
        let window_store = WindowStore::default();
        let settings = VirtualWorkspaceSettings {
            default_workspace_count: 3,
            ..VirtualWorkspaceSettings::default()
        };
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let space = SpaceId::new(1);
        let workspaces = manager.list_workspaces(space).to_vec();

        assert_eq!(
            manager.step_workspace(&window_store, space, workspaces[2].0, None, Direction::Right),
            Some(workspaces[0].0)
        );
        assert_eq!(
            manager.step_workspace(&window_store, space, workspaces[0].0, None, Direction::Left),
            Some(workspaces[2].0)
        );
    }

    #[test]
    fn prevent_wrapping_stops_workspace_navigation_at_boundaries() {
        let mut window_store = WindowStore::default();
        let settings = VirtualWorkspaceSettings {
            default_workspace_count: 4,
            prevent_wrapping: true,
            ..VirtualWorkspaceSettings::default()
        };
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let space = SpaceId::new(1);
        let workspaces = manager.list_workspaces(space).to_vec();

        assert_eq!(
            manager.step_workspace(&window_store, space, workspaces[0].0, None, Direction::Left),
            None
        );
        assert_eq!(
            manager.step_workspace(&window_store, space, workspaces[3].0, None, Direction::Right),
            None
        );

        let window = WindowId::new(1, 1);
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            space,
            window,
            workspaces[3].0
        ));
        assert_eq!(
            manager.step_workspace(
                &window_store,
                space,
                workspaces[1].0,
                Some(true),
                Direction::Right
            ),
            Some(workspaces[3].0)
        );
        assert_eq!(
            manager.step_workspace(
                &window_store,
                space,
                workspaces[3].0,
                Some(true),
                Direction::Right
            ),
            None
        );
    }

    #[test]
    fn prevent_wrapping_updates_on_config_reload() {
        let window_store = WindowStore::default();
        let mut settings = VirtualWorkspaceSettings {
            default_workspace_count: 2,
            ..VirtualWorkspaceSettings::default()
        };
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let space = SpaceId::new(1);
        let workspaces = manager.list_workspaces(space).to_vec();

        assert_eq!(
            manager.step_workspace(&window_store, space, workspaces[1].0, None, Direction::Right),
            Some(workspaces[0].0)
        );

        settings.prevent_wrapping = true;
        manager.update_settings(&settings, &LayoutSettings::default());
        assert_eq!(
            manager.step_workspace(&window_store, space, workspaces[1].0, None, Direction::Right),
            None
        );
    }

    #[test]
    fn app_rules() {
        let mut window_store = WindowStore::default();
        let space1 = SpaceId::new(1);
        let space2 = SpaceId::new(2);

        let mut settings = VirtualWorkspaceSettings::default();

        if settings.workspace_names.len() < 4 {
            while settings.workspace_names.len() < 4 {
                settings
                    .workspace_names
                    .push(format!("Workspace {}", settings.workspace_names.len() + 1));
            }
        }
        settings.workspace_names[1] = "coding".to_string();

        settings.app_rules = vec![
            // Floating by app_id
            AppWorkspaceRule {
                app_id: Some("com.example.test".into()),
                floating: true,
                manage: Some(true),
                ..Default::default()
            },
            // Match by app_name -> workspace 1
            AppWorkspaceRule {
                workspace: Some(WorkspaceSelector::Index(1)),
                manage: Some(true),
                app_name: Some("Calendar".into()),
                ..Default::default()
            },
            // Title substring -> workspace 0
            AppWorkspaceRule {
                app_id: Some("com.example.foo".into()),
                workspace: Some(WorkspaceSelector::Index(0)),
                manage: Some(true),
                title_substring: Some("Preferences".into()),
                ..Default::default()
            },
            // Title regex -> workspace 2
            AppWorkspaceRule {
                app_id: Some("com.example.foo".into()),
                workspace: Some(WorkspaceSelector::Index(2)),
                manage: Some(true),
                title_regex: Some(r"Dialog\s+\d+".into()),
                ..Default::default()
            },
            // AX role + subrole floating
            AppWorkspaceRule {
                app_id: Some("com.example.special".into()),
                floating: true,
                manage: Some(true),
                ax_role: Some("AXWindow".into()),
                ax_subrole: Some("AXDialog".into()),
                ..Default::default()
            },
            // Workspace by name
            AppWorkspaceRule {
                app_id: Some("com.example.name".into()),
                workspace: Some(WorkspaceSelector::Name("coding".into())),
                manage: Some(true),
                ..Default::default()
            },
            // A title-specific rule can replace an existing assignment.
            AppWorkspaceRule {
                app_id: Some("app.zen-browser.zen".into()),
                workspace: Some(WorkspaceSelector::Index(2)),
                manage: Some(true),
                ..Default::default()
            },
            AppWorkspaceRule {
                app_id: Some("app.zen-browser.zen".into()),
                workspace: Some(WorkspaceSelector::Index(3)),
                floating: true,
                manage: Some(true),
                title_substring: Some("bitwarden".into()),
                ..Default::default()
            },
        ];

        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());

        // 1. Floating persistence via app_id (case-insensitive)
        let w_float = WindowId::new(10, 1);
        let assignment = assign(
            &mut manager,
            &mut window_store,
            w_float,
            space1,
            Some("COM.EXAMPLE.Test"),
            None,
            None,
            None,
            None,
        );
        assert!(assignment.floating);

        manager.remove_window(&mut window_store, w_float);

        // After removal, reassign should still float.
        let assignment_again = assign(
            &mut manager,
            &mut window_store,
            w_float,
            space1,
            Some("com.example.test"),
            None,
            None,
            None,
            None,
        );
        assert!(assignment_again.floating);

        // 2. Match by app_name
        let w_name = WindowId::new(20, 2);
        let ws_name = assign(
            &mut manager,
            &mut window_store,
            w_name,
            space1,
            None,
            Some("MyCalendarApp"),
            None,
            None,
            None,
        )
        .workspace_id;
        let coding_idx = 1; // Calendar rule points to workspace index 1
        let expected_ws_name = manager.list_workspaces(space1).get(coding_idx).unwrap().0;
        assert_eq!(ws_name, expected_ws_name);

        // 3. Title substring and regex for same app
        let w_pref = WindowId::new(30, 3);
        let w_dialog = WindowId::new(30, 4);
        let ws_pref = assign(
            &mut manager,
            &mut window_store,
            w_pref,
            space1,
            Some("com.example.foo"),
            None,
            Some("App Preferences"),
            None,
            None,
        )
        .workspace_id;
        let ws_dialog = assign(
            &mut manager,
            &mut window_store,
            w_dialog,
            space1,
            Some("com.example.foo"),
            None,
            Some("Dialog 42"),
            None,
            None,
        )
        .workspace_id;
        let expected_pref = manager.list_workspaces(space1).get(0).unwrap().0;
        let expected_dialog = manager.list_workspaces(space1).get(2).unwrap().0;
        assert_eq!(ws_pref, expected_pref);
        assert_eq!(ws_dialog, expected_dialog);

        // 4. AX role + subrole floating
        let w_ax = WindowId::new(40, 5);
        let ax_assignment = assign(
            &mut manager,
            &mut window_store,
            w_ax,
            space1,
            Some("com.example.special"),
            None,
            None,
            Some("AXWindow"),
            Some("AXDialog"),
        );
        assert!(ax_assignment.floating);

        // 5. Workspace name resolution
        let w_named = WindowId::new(50, 6);
        let ws_named = assign(
            &mut manager,
            &mut window_store,
            w_named,
            space1,
            Some("com.example.name"),
            None,
            None,
            None,
            None,
        )
        .workspace_id;
        let coding_ws =
            manager.list_workspaces(space1).iter().find(|(_, n)| n == "coding").unwrap().0;
        assert_eq!(ws_named, coding_ws);

        // 6. Reapplication updates both workspace and floating state.
        let w_bw2 = WindowId::new(80, 9);
        let bw2_initial_assignment = assign(
            &mut manager,
            &mut window_store,
            w_bw2,
            space2,
            Some("app.zen-browser.zen"),
            None,
            None,
            None,
            None,
        );
        assert!(!bw2_initial_assignment.floating);
        let bw2_updated_assignment = assign(
            &mut manager,
            &mut window_store,
            w_bw2,
            space2,
            Some("app.zen-browser.zen"),
            None,
            Some("Bitwarden Vault"),
            None,
            None,
        );
        let expected_initial = manager.list_workspaces(space2).get(2).unwrap().0; // workspace index 1
        let expected_updated = manager.list_workspaces(space2).get(3).unwrap().0;
        assert_eq!(bw2_initial_assignment.workspace_id, expected_initial);
        assert_eq!(bw2_updated_assignment.workspace_id, expected_updated);
        assert!(bw2_updated_assignment.floating);
    }

    #[test]
    fn hidden_position_uses_corner_anchor_while_hiding_offscreen() {
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(100.0, 100.0));
        let frame = CGRect::new(CGPoint::new(20.0, 37.0), CGSize::new(30.0, 20.0));

        let hidden = HiddenWindowPlacement::calculate(screen, frame, HideCorner::BottomRight, &[]);

        assert_eq!(hidden.origin.y, screen.max().y - 1.0);
        assert_eq!(hidden.origin.x, screen.max().x - 1.0);
    }

    #[test]
    fn hidden_position_flips_sides_to_avoid_neighboring_monitor_overlap() {
        let primary = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(100.0, 100.0));
        let right_neighbor = CGRect::new(CGPoint::new(100.0, 0.0), CGSize::new(100.0, 100.0));
        let frame = CGRect::new(CGPoint::new(20.0, 25.0), CGSize::new(30.0, 20.0));

        let hidden = HiddenWindowPlacement::calculate(primary, frame, HideCorner::BottomRight, &[
            right_neighbor,
        ]);

        assert_eq!(hidden.origin.y, primary.max().y - 1.0);
        assert_eq!(hidden.origin.x, primary.origin.x - frame.size.width + 1.0);
    }
}
