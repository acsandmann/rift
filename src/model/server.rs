use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::actor::app::{WindowId, pid_t};
use crate::sys::geometry::CGRectDef;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceQueryResponse {
    pub workspaces: Vec<WorkspaceData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceData {
    pub id: String,
    pub index: usize,
    pub name: String,
    pub is_active: bool,
    pub window_count: usize,
    pub windows: Vec<WindowData>,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowData {
    pub id: WindowId,
    pub title: String,
    #[serde_as(as = "CGRectDef")]
    pub frame: CGRect,
    pub is_floating: bool,
    pub is_focused: bool,
    pub bundle_id: Option<String>,
    pub window_server_id: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationData {
    pub pid: pid_t,
    pub bundle_id: Option<String>,
    pub name: String,
    pub is_frontmost: bool,
    pub window_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutStateData {
    pub space_id: u64,
    pub mode: String,
    pub floating_windows: Vec<WindowId>,
    pub tiled_windows: Vec<WindowId>,
    pub focused_window: Option<WindowId>,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayData {
    pub uuid: String,
    pub name: Option<String>,
    pub screen_id: u32,
    #[serde_as(as = "CGRectDef")]
    pub frame: CGRect,
    pub space: Option<u64>,
}
