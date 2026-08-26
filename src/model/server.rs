use rift_protocol as protocol;
use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::actor::app::WindowId;
use crate::sys::app::WindowInfo;
use crate::sys::geometry::CGRectDef;
use crate::sys::screen::{ScreenId, ScreenInfo, SpaceId};
use crate::sys::window_server::WindowServerId;

/// Runtime-only workspace projection. Its windows retain the macOS
/// accessibility metadata needed by the UI; IPC uses the protocol-owned
/// `rift_protocol::WorkspaceData` representation.
#[derive(Debug, Clone)]
pub struct RuntimeWorkspaceData {
    pub id: String,
    pub index: usize,
    pub name: String,
    pub layout_mode: String,
    pub is_active: bool,
    pub window_count: usize,
    pub windows: Vec<RuntimeWindowData>,
}

#[derive(Debug, Clone)]
pub struct RuntimeWindowData {
    pub id: WindowId,
    pub is_floating: bool,
    pub is_focused: bool,
    pub layout_position: Option<protocol::WindowLayoutPosition>,
    pub app_name: Option<String>,
    pub info: WindowInfo,
}

#[derive(Debug, Clone)]
pub struct RuntimeDisplayData {
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

impl From<WindowId> for protocol::WindowId {
    fn from(value: WindowId) -> Self {
        Self {
            pid: value.pid,
            idx: value.idx.get(),
        }
    }
}

impl From<RuntimeWindowData> for protocol::WindowData {
    fn from(value: RuntimeWindowData) -> Self {
        Self {
            id: value.id.into(),
            title: value.info.title,
            frame: protocol::Rect {
                origin: protocol::Point {
                    x: value.info.frame.origin.x,
                    y: value.info.frame.origin.y,
                },
                size: protocol::Size {
                    width: value.info.frame.size.width,
                    height: value.info.frame.size.height,
                },
            },
            is_floating: value.is_floating,
            is_focused: value.is_focused,
            bundle_id: value.info.bundle_id,
            app_name: value.app_name,
            window_server_id: value.info.sys_id.map(|id| id.as_u32()),
            layout_position: value.layout_position,
        }
    }
}

impl From<RuntimeWorkspaceData> for protocol::WorkspaceData {
    fn from(value: RuntimeWorkspaceData) -> Self {
        Self {
            id: value.id,
            index: value.index,
            name: value.name,
            layout_mode: value.layout_mode,
            is_active: value.is_active,
            window_count: value.window_count,
            windows: value.windows.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<RuntimeDisplayData> for protocol::DisplayData {
    fn from(value: RuntimeDisplayData) -> Self {
        Self {
            uuid: value.info.display_uuid,
            name: value.info.name,
            screen_id: value.info.id.as_u32(),
            frame: protocol::Rect {
                origin: protocol::Point {
                    x: value.info.frame.origin.x,
                    y: value.info.frame.origin.y,
                },
                size: protocol::Size {
                    width: value.info.frame.size.width,
                    height: value.info.frame.size.height,
                },
            },
            space: value.info.space.map(|space| space.get()),
            is_active_space: value.is_active_space,
            is_active_context: value.is_active_context,
            active_space_ids: value.active_space_ids,
            inactive_space_ids: value.inactive_space_ids,
        }
    }
}

impl Serialize for RuntimeWindowData {
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
            app_name: Option<&'a String>,
            window_server_id: Option<u32>,
            layout_position: Option<&'a protocol::WindowLayoutPosition>,
        }

        let helper = WindowDataSer {
            id: self.id,
            title: &self.info.title,
            frame: &self.info.frame,
            is_floating: self.is_floating,
            is_focused: self.is_focused,
            bundle_id: self.info.bundle_id.as_ref(),
            app_name: self.app_name.as_ref(),
            window_server_id: self.info.sys_id.map(|id| id.as_u32()),
            layout_position: self.layout_position.as_ref(),
        };

        helper.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RuntimeWindowData {
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
            app_name: Option<String>,
            window_server_id: Option<u32>,
            layout_position: Option<protocol::WindowLayoutPosition>,
        }

        let helper = WindowDataDe::deserialize(deserializer)?;
        let info = WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: helper.title,
            frame: helper.frame,
            sys_id: helper.window_server_id.map(WindowServerId::new),
            bundle_id: helper.bundle_id,
            path: None,
            ax_role: None,
            ax_subrole: None,
        };

        Ok(RuntimeWindowData {
            id: helper.id,
            is_floating: helper.is_floating,
            is_focused: helper.is_focused,
            layout_position: helper.layout_position,
            app_name: helper.app_name,
            info,
        })
    }
}

impl Serialize for RuntimeDisplayData {
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

impl<'de> Deserialize<'de> for RuntimeDisplayData {
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

        Ok(RuntimeDisplayData {
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
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Test".to_string(),
            frame: CGRect::new(CGPoint::new(1.0, 2.0), CGSize::new(3.0, 4.0)),
            sys_id: Some(WindowServerId::new(99)),
            bundle_id: Some("com.example.test".to_string()),
            path: None,
            ax_role: None,
            ax_subrole: None,
        };
        let data = RuntimeWindowData {
            id: WindowId::new(123, 7),
            is_floating: true,
            is_focused: false,
            layout_position: Some(protocol::WindowLayoutPosition { column: 2, row: 1 }),
            app_name: Some("Test App".to_string()),
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
            "app_name": "Test App",
            "window_server_id": 99,
            "layout_position": { "column": 2, "row": 1 },
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
        let data = RuntimeDisplayData {
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
