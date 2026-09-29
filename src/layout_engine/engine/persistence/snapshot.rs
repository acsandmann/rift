use super::*;

pub(super) const CURRENT_SCHEMA_VERSION: u32 = 3;

fn legacy_schema_version() -> u32 { 0 }

/// Pre-consolidation file representation. It is normalized before any runtime use.
#[derive(Deserialize, Default)]
pub(super) struct LegacyWorkspaceLayouts {
    map: HashMap<
        (SpaceId, VirtualWorkspaceId),
        crate::layout_engine::workspaces::WorkspaceLayoutState,
    >,
}

/// Owned, versioned representation of the layout file.
///
/// Keep this type independent from runtime-only `LayoutEngine` fields. Adding an engine cache,
/// service, or transient index must never silently alter the persistence schema again.
#[derive(Deserialize)]
pub(super) struct PersistedLayout {
    #[serde(default = "legacy_schema_version")]
    pub(super) schema_version: u32,
    #[serde(default)]
    pub(super) workspace_layouts: LegacyWorkspaceLayouts,
    pub(super) floating: FloatingManager,
    pub(super) floating_positions: FloatingPositionStore,
    #[serde(rename = "virtual_workspace_manager")]
    pub(super) workspaces: WorkspaceStore,
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
    floating: &'a FloatingManager,
    floating_positions: &'a FloatingPositionStore,
    #[serde(rename = "virtual_workspace_manager")]
    workspaces: &'a WorkspaceStore,
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
            floating: &engine.floating,
            floating_positions: &engine.floating_positions,
            workspaces: &engine.workspaces,
            space_display_map: &engine.space_display_map,
            display_last_space: &engine.display_last_space,
            persistence: &engine.persistence,
        })
        .expect("persisted layout serialization must support all engine layout state")
    }

    pub(super) fn normalize_workspace_layouts(&mut self) -> anyhow::Result<()> {
        for ((space, id), state) in std::mem::take(&mut self.workspace_layouts.map) {
            let workspace = self.workspaces.workspaces.get_mut(id).ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid workspace layouts: layout state references missing workspace {id:?}"
                )
            })?;
            if workspace.space != space {
                return Err(anyhow::anyhow!(
                    "invalid workspace layouts: workspace {id:?} is stored under the wrong native space"
                ));
            }
            if self.schema_version >= 3 {
                return Err(anyhow::anyhow!(
                    "invalid workspace layouts: legacy layout state in schema 3"
                ));
            }
            workspace.layout_state = state;
        }
        self.workspaces
            .validate_layouts()
            .map_err(|error| anyhow::anyhow!("invalid workspace layouts: {error}"))
    }

    pub(super) fn into_engine(self) -> LayoutEngine {
        LayoutEngine {
            scroll_boundary: None,
            floating: self.floating,
            floating_positions: self.floating_positions,
            app_rules: AppRuleEngine::default(),
            focused_window: None,
            window_layout_constraints: HashMap::default(),
            workspaces: self.workspaces,
            layout_settings: LayoutSettings::default(),
            broadcast_tx: None,
            space_display_map: self.space_display_map,
            display_last_space: self.display_last_space,
            space_frames: HashMap::default(),
            persistence: self.persistence,
            startup_restore_pending: false,
        }
    }
}
