use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct WindowId {
    pub pid: i32,
    pub idx: u32,
}

impl WindowId {
    pub const fn new(pid: i32, idx: u32) -> Option<Self> {
        if idx == 0 {
            None
        } else {
            Some(Self { pid, idx })
        }
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceData {
    pub id: String,
    pub index: usize,
    pub name: String,
    pub layout_mode: String,
    pub is_active: bool,
    pub window_count: usize,
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
    pub mode: String,
    pub floating_windows: Vec<WindowId>,
    pub tiled_windows: Vec<WindowId>,
    pub focused_window: Option<WindowId>,
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
