use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{Direction, LayoutKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
pub struct WindowId {
    pub pid: i32,
    pub idx: u32,
}

impl<'de> Deserialize<'de> for WindowId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: Deserializer<'de> {
        struct WindowIdVisitor;

        impl<'de> Visitor<'de> for WindowIdVisitor {
            type Value = WindowId;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str(
                    "a window id object, tuple, or debug string like `WindowId { pid: 123, idx: 456 }`",
                )
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where E: de::Error {
                let value = value
                    .strip_prefix("WindowId { pid: ")
                    .and_then(|value| value.strip_suffix(" }"))
                    .ok_or_else(|| E::custom("invalid WindowId debug string"))?;
                let (pid, idx) = value
                    .split_once(", idx: ")
                    .ok_or_else(|| E::custom("invalid WindowId debug string"))?;
                let pid = pid.parse().map_err(|_| E::custom("invalid WindowId pid"))?;
                let idx = idx.parse().map_err(|_| E::custom("invalid WindowId idx"))?;
                WindowId::new(pid, idx).ok_or_else(|| E::custom("window id index must be non-zero"))
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where A: SeqAccess<'de> {
                let pid =
                    sequence.next_element()?.ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let idx =
                    sequence.next_element()?.ok_or_else(|| de::Error::invalid_length(1, &self))?;
                WindowId::new(pid, idx)
                    .ok_or_else(|| de::Error::custom("window id index must be non-zero"))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where A: MapAccess<'de> {
                let mut pid = None;
                let mut idx = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "pid" => pid = Some(map.next_value()?),
                        "idx" => idx = Some(map.next_value()?),
                        _ => {
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let pid = pid.ok_or_else(|| de::Error::missing_field("pid"))?;
                let idx = idx.ok_or_else(|| de::Error::missing_field("idx"))?;
                WindowId::new(pid, idx)
                    .ok_or_else(|| de::Error::custom("window id index must be non-zero"))
            }
        }

        deserializer.deserialize_any(WindowIdVisitor)
    }
}

impl WindowId {
    pub const fn new(pid: i32, idx: u32) -> Option<Self> {
        if idx == 0 {
            None
        } else {
            Some(Self { pid, idx })
        }
    }

    pub fn to_debug_string(self) -> String {
        format!("WindowId {{ pid: {}, idx: {} }}", self.pid, self.idx)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WindowLayoutPosition {
    /// Zero-based logical column in the workspace layout.
    pub column: usize,
    /// Zero-based logical row within `column`.
    pub row: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowData {
    pub id: WindowId,
    pub title: String,
    pub frame: Rect,
    pub is_floating: bool,
    pub is_focused: bool,
    pub bundle_id: Option<String>,
    pub app_name: Option<String>,
    pub window_server_id: Option<u32>,
    /// Stable topology-derived position in the workspace layout.
    ///
    /// This does not depend on the window's animated frame. It is `None` for floating windows,
    /// layout modes without column semantics, and queries without a workspace context.
    pub layout_position: Option<WindowLayoutPosition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceData {
    pub id: String,
    pub index: usize,
    pub name: String,
    pub layout_mode: String,
    pub is_active: bool,
    pub window_count: usize,
    /// Workspace windows in logical column-major order when the layout has column semantics.
    /// Windows without a logical position follow in their existing stable order.
    pub windows: Vec<WindowData>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceLayoutData {
    pub id: String,
    pub index: usize,
    pub name: String,
    pub layout_mode: String,
    pub is_active: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApplicationData {
    pub pid: i32,
    pub bundle_id: Option<String>,
    pub name: String,
    pub is_frontmost: bool,
    pub window_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutStateData {
    pub space_id: u64,
    pub workspace_id: usize,
    pub is_active_workspace: bool,
    pub mode: String,
    pub floating_windows: Vec<WindowId>,
    pub tiled_windows: Vec<WindowId>,
    pub focused_window: Option<WindowId>,
    /// The layout engine's selected window in the queried workspace.
    pub selected_window: Option<WindowId>,
    /// Normalized topology for the queried workspace's tiled layout.
    ///
    /// Internal node IDs are intentionally omitted because they are not stable across layout
    /// mutations. Consumers can identify leaves by `window_id` and other nodes by their path.
    pub container_tree: ContainerTreeNode,
}

/// The type of a node in Rift's normalized layout topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerNodeType {
    Container,
    Window,
    /// An empty slot retained by a layout engine, such as an empty BSP root.
    Placeholder,
}

/// A platform-neutral view of one node in a tiled layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContainerTreeNode {
    pub node_type: ContainerNodeType,
    /// Split/stack behavior for a container. Window and placeholder nodes use `None`.
    pub layout_kind: Option<LayoutKind>,
    /// This node's relative share within its parent, when the layout engine has one.
    pub weight: Option<f64>,
    pub window_id: Option<WindowId>,
    /// Layout-engine selection, which is distinct from OS window focus.
    pub is_selected: bool,
    pub is_fullscreen: bool,
    pub is_fullscreen_within_gaps: bool,
    /// Semantic role when the mode defines one, such as `master`, `stack`, or `column`.
    pub role: Option<String>,
    /// Pending BSP split direction, if this leaf is preselected for insertion.
    pub pending_split: Option<Direction>,
    pub children: Vec<ContainerTreeNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayData {
    pub uuid: String,
    pub name: Option<String>,
    pub screen_id: u32,
    pub frame: Rect,
    pub space: Option<u64>,
    pub is_active_space: bool,
    pub is_active_context: bool,
    pub active_space_ids: Vec<u64>,
    pub inactive_space_ids: Vec<u64>,
}
