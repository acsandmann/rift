use super::*;

pub(super) const CURRENT_SCHEMA_VERSION: u32 = 2;

fn legacy_schema_version() -> u32 { 0 }

/// Owned, versioned representation of the layout file.
///
/// Keep this type independent from runtime-only `LayoutEngine` fields. Adding an engine cache,
/// service, or transient index must never silently alter the persistence schema again.
#[derive(Deserialize)]
pub(super) struct PersistedLayout {
    #[serde(default = "legacy_schema_version")]
    pub(super) schema_version: u32,
    pub(super) workspace_layouts: WorkspaceLayouts,
    pub(super) floating: FloatingManager,
    pub(super) floating_positions: FloatingPositionStore,
    pub(super) virtual_workspace_manager: WorkspaceStore,
    #[serde(default)]
    pub(super) space_display_map: HashMap<SpaceId, Option<String>>,
    #[serde(default)]
    pub(super) display_last_space: HashMap<String, SpaceId>,
    #[serde(flatten)]
    pub(super) persistence: PersistenceState,
}

/// Borrowed serialization view, avoiding a deep clone of every layout tree during save.
#[derive(Serialize)]
struct PersistedLayoutRef<'a> {
    schema_version: u32,
    workspace_layouts: &'a WorkspaceLayouts,
    floating: &'a FloatingManager,
    floating_positions: &'a FloatingPositionStore,
    virtual_workspace_manager: &'a WorkspaceStore,
    space_display_map: &'a HashMap<SpaceId, Option<String>>,
    display_last_space: &'a HashMap<String, SpaceId>,
    #[serde(flatten)]
    persistence: &'a PersistenceState,
}

impl PersistedLayout {
    pub(super) fn deserialize(buf: &str) -> Result<Self, ron::error::SpannedError> {
        ron::from_str(buf)
    }

    pub(super) fn serialize_engine(engine: &LayoutEngine) -> String {
        ron::ser::to_string(&PersistedLayoutRef {
            schema_version: CURRENT_SCHEMA_VERSION,
            workspace_layouts: &engine.workspace_layouts,
            floating: &engine.floating,
            floating_positions: &engine.floating_positions,
            virtual_workspace_manager: &engine.virtual_workspace_manager,
            space_display_map: &engine.space_display_map,
            display_last_space: &engine.display_last_space,
            persistence: &engine.persistence,
        })
        .expect("persisted layout serialization must support all engine layout state")
    }

    pub(super) fn into_engine(self) -> LayoutEngine {
        LayoutEngine {
            workspace_layouts: self.workspace_layouts,
            floating: self.floating,
            floating_positions: self.floating_positions,
            app_rules: AppRuleEngine::default(),
            focused_window: None,
            window_layout_constraints: HashMap::default(),
            virtual_workspace_manager: self.virtual_workspace_manager,
            layout_settings: LayoutSettings::default(),
            broadcast_tx: None,
            space_display_map: self.space_display_map,
            display_last_space: self.display_last_space,
            persistence: self.persistence,
            startup_restore_pending: false,
        }
    }
}
