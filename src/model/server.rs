use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::actor::app::{WindowId, pid_t};
use crate::sys::app::WindowInfo;
use crate::sys::geometry::CGRectDef;
use crate::sys::screen::{ScreenId, ScreenInfo, SpaceId};
use crate::sys::window_server::WindowServerId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceData {
    pub id: String,
    pub index: usize,
    pub name: String,
    pub layout_mode: String,
    pub is_active: bool,
    pub window_count: usize,
    pub windows: Vec<WindowData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceLayoutData {
    pub id: String,
    pub index: usize,
    pub name: String,
    pub layout_mode: String,
    pub is_active: bool,
}

#[derive(Debug, Clone)]
pub struct WindowData {
    pub id: WindowId,
    pub is_floating: bool,
    pub is_focused: bool,
    pub info: WindowInfo,
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

#[derive(Debug, Clone)]
pub struct DisplayData {
    pub info: ScreenInfo,
    /// True if this display's space is active per the activation policy.
    pub is_active_space: bool,
    /// True if this display corresponds to the context Rift uses when no space_id is provided
    pub is_active_context: bool,
    /// Active space ids for this display (empty if none).
    pub active_space_ids: Vec<u64>,
    /// Inactive space ids for this display (empty if none).
    pub inactive_space_ids: Vec<u64>,
}

impl Serialize for WindowData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        #[serde_as]
        #[derive(Serialize)]
        struct WindowDataSer<'a> {
            id: WindowId,
            title: &'a str,
            #[serde_as(as = "CGRectDef")]
            frame: &'a objc2_core_foundation::CGRect,
            is_floating: bool,
            is_focused: bool,
            bundle_id: Option<&'a String>,
            window_server_id: Option<u32>,
        }

        let helper = WindowDataSer {
            id: self.id,
            title: &self.info.title,
            frame: &self.info.frame,
            is_floating: self.is_floating,
            is_focused: self.is_focused,
            bundle_id: self.info.bundle_id.as_ref(),
            window_server_id: self.info.sys_id.map(|id| id.as_u32()),
        };

        helper.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for WindowData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: Deserializer<'de> {
        #[serde_as]
        #[derive(Deserialize)]
        struct WindowDataDe {
            id: WindowId,
            title: String,
            #[serde_as(as = "CGRectDef")]
            frame: objc2_core_foundation::CGRect,
            is_floating: bool,
            is_focused: bool,
            bundle_id: Option<String>,
            window_server_id: Option<u32>,
        }

        let helper = WindowDataDe::deserialize(deserializer)?;
        let info = WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            title: helper.title,
            frame: helper.frame,
            sys_id: helper.window_server_id.map(WindowServerId::new),
            bundle_id: helper.bundle_id,
            path: None,
            ax_role: None,
            ax_subrole: None,
        };

        Ok(WindowData {
            id: helper.id,
            is_floating: helper.is_floating,
            is_focused: helper.is_focused,
            info,
        })
    }
}

impl Serialize for DisplayData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        #[serde_as]
        #[derive(Serialize)]
        struct DisplayDataSer<'a> {
            uuid: &'a str,
            name: Option<&'a String>,
            screen_id: u32,
            #[serde_as(as = "CGRectDef")]
            frame: &'a objc2_core_foundation::CGRect,
            space: Option<u64>,
            is_active_space: bool,
            is_active_context: bool,
            active_space_ids: &'a [u64],
            inactive_space_ids: &'a [u64],
        }

        let helper = DisplayDataSer {
            uuid: &self.info.display_uuid,
            name: self.info.name.as_ref(),
            screen_id: self.info.id.as_u32(),
            frame: &self.info.frame,
            space: self.info.space.map(|s| s.get()),
            is_active_space: self.is_active_space,
            is_active_context: self.is_active_context,
            active_space_ids: &self.active_space_ids,
            inactive_space_ids: &self.inactive_space_ids,
        };

        helper.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DisplayData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: Deserializer<'de> {
        #[serde_as]
        #[derive(Deserialize)]
        struct DisplayDataDe {
            uuid: String,
            name: Option<String>,
            screen_id: u32,
            #[serde_as(as = "CGRectDef")]
            frame: objc2_core_foundation::CGRect,
            space: Option<u64>,
            is_active_space: bool,
            is_active_context: bool,
            active_space_ids: Vec<u64>,
            inactive_space_ids: Vec<u64>,
        }

        let helper = DisplayDataDe::deserialize(deserializer)?;
        let info = ScreenInfo {
            id: ScreenId::new(helper.screen_id),
            frame: helper.frame,
            display_uuid: helper.uuid,
            name: helper.name,
            space: helper.space.map(SpaceId::new),
        };

        Ok(DisplayData {
            info,
            is_active_space: helper.is_active_space,
            is_active_context: helper.is_active_context,
            active_space_ids: helper.active_space_ids,
            inactive_space_ids: helper.inactive_space_ids,
        })
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use serde_json::json;

    use super::*;

    #[test]
    fn window_data_serializes_with_legacy_shape() {
        let info = WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            title: "Test".to_string(),
            frame: CGRect::new(CGPoint::new(1.0, 2.0), CGSize::new(3.0, 4.0)),
            sys_id: Some(WindowServerId::new(99)),
            bundle_id: Some("com.example.test".to_string()),
            path: None,
            ax_role: None,
            ax_subrole: None,
        };
        let data = WindowData {
            id: WindowId::new(123, 7),
            is_floating: true,
            is_focused: false,
            info,
        };

        let value = serde_json::to_value(&data).expect("serialize WindowData");
        let expected = json!({
            "id": { "pid": 123, "idx": 7 },
            "title": "Test",
            "frame": { "origin": { "x": 1.0, "y": 2.0 }, "size": { "width": 3.0, "height": 4.0 } },
            "is_floating": true,
            "is_focused": false,
            "bundle_id": "com.example.test",
            "window_server_id": 99,
        });
        assert_eq!(value, expected);
    }

    #[test]
    fn display_data_serializes_with_legacy_shape() {
        let info = ScreenInfo {
            id: ScreenId::new(7),
            frame: CGRect::new(CGPoint::new(10.0, 20.0), CGSize::new(300.0, 400.0)),
            display_uuid: "display-uuid".to_string(),
            name: Some("Primary".to_string()),
            space: Some(SpaceId::new(42)),
        };
        let data = DisplayData {
            info,
            is_active_space: true,
            is_active_context: false,
            active_space_ids: vec![42],
            inactive_space_ids: vec![43, 44],
        };

        let value = serde_json::to_value(&data).expect("serialize DisplayData");
        let expected = json!({
            "uuid": "display-uuid",
            "name": "Primary",
            "screen_id": 7,
            "frame": { "origin": { "x": 10.0, "y": 20.0 }, "size": { "width": 300.0, "height": 400.0 } },
            "space": 42,
            "is_active_space": true,
            "is_active_context": false,
            "active_space_ids": [42],
            "inactive_space_ids": [43, 44],
        });
        assert_eq!(value, expected);
    }
}
