use crate::common::collections::{HashMap, HashSet};
use crate::sys::screen::{ScreenInfo, SpaceId};
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

#[derive(Debug, Clone)]
pub struct WindowSnapshot {
    pub info: WindowServerInfo,
}

#[derive(Debug, Clone)]
pub struct DisplaySnapshot {
    pub ordered_screens: Vec<ScreenInfo>,
    pub active_spaces: HashSet<SpaceId>,
    pub inactive_spaces: HashSet<SpaceId>,
    pub windows: HashMap<WindowServerId, WindowSnapshot>,
}
