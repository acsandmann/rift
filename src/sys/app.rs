use std::path::PathBuf;

pub use nix::libc::pid_t;
use objc2::rc::Retained;
use objc2::{class, msg_send};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_core_foundation::CGRect;
use objc2_foundation::NSString;
use serde::{Deserialize, Serialize};

use super::geometry::CGRectDef;
use super::window_server::{WindowServerId, WindowServerInfo};
use crate::sys::axuielement::{
    AX_STANDARD_WINDOW_SUBROLE, AX_WINDOW_ROLE, AXUIElement, Error as AxError,
};

pub fn running_apps(bundle: Option<String>) -> impl Iterator<Item = (pid_t, AppInfo)> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .into_iter()
        .flat_map(move |app| {
            let bundle_id = app.bundle_id()?.to_string();
            if let Some(filter) = &bundle {
                if !bundle_id.contains(filter) {
                    return None;
                }
            }
            Some((app.pid(), AppInfo::from(&*app)))
        })
}

pub trait NSRunningApplicationExt {
    fn with_process_id(pid: pid_t) -> Option<Retained<Self>>;
    fn pid(&self) -> pid_t;
    fn bundle_id(&self) -> Option<Retained<NSString>>;
    fn localized_name(&self) -> Option<Retained<NSString>>;
}

impl NSRunningApplicationExt for NSRunningApplication {
    fn with_process_id(pid: pid_t) -> Option<Retained<Self>> {
        unsafe {
            // For some reason this binding isn't generated in icrate.
            msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier:pid]
        }
    }

    fn pid(&self) -> pid_t { unsafe { msg_send![self, processIdentifier] } }

    fn bundle_id(&self) -> Option<Retained<NSString>> { self.bundleIdentifier() }

    fn localized_name(&self) -> Option<Retained<NSString>> { self.localizedName() }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppInfo {
    pub bundle_id: Option<String>,
    pub localized_name: Option<String>,
}

impl From<&NSRunningApplication> for AppInfo {
    fn from(app: &NSRunningApplication) -> Self {
        AppInfo {
            bundle_id: app.bundle_id().as_deref().map(ToString::to_string),
            localized_name: app.localized_name().as_deref().map(ToString::to_string),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WindowInfo {
    pub is_standard: bool,
    #[serde(default)]
    pub is_root: bool,
    #[serde(default)]
    pub is_minimized: bool,
    pub title: String,
    #[serde(with = "CGRectDef")]
    pub frame: CGRect,
    pub sys_id: Option<WindowServerId>,
    pub bundle_id: Option<String>,
    pub path: Option<PathBuf>,
    pub ax_role: Option<String>,
    pub ax_subrole: Option<String>,
}

impl WindowInfo {
    pub fn from_ax_element(
        element: &AXUIElement,
        server_info_hint: Option<WindowServerInfo>,
    ) -> Result<(Self, Option<WindowServerInfo>), AxError> {
        let frame = element.frame()?;
        let role = element.role()?;
        let subrole = element.subrole()?;
        let is_standard = role == AX_WINDOW_ROLE && subrole == AX_STANDARD_WINDOW_SUBROLE;

        let ax_role = Some(role.clone());
        let ax_subrole = Some(subrole.clone());

        let mut server_info = server_info_hint;
        let id = server_info
            .map(|info| info.id)
            .or_else(|| WindowServerId::try_from(element).ok());
        let is_minimized = element.minimized().unwrap_or_default();

        let (bundle_id, path) = if !is_standard {
            (None, None)
        } else if let Some(info) = server_info {
            bundle_info_for_pid(info.pid)
        } else if let Some(window_id) = id {
            server_info = crate::sys::window_server::get_window(window_id);
            server_info.map(|info| bundle_info_for_pid(info.pid)).unwrap_or((None, None))
        } else {
            (None, None)
        };

        let info = WindowInfo {
            is_standard,
            is_root: true,
            is_minimized,
            title: element.title().unwrap_or_default(),
            frame,
            sys_id: id,
            bundle_id,
            path,
            ax_role,
            ax_subrole,
        };

        Ok((info, server_info))
    }
}

impl TryFrom<&AXUIElement> for WindowInfo {
    type Error = AxError;

    fn try_from(element: &AXUIElement) -> Result<Self, AxError> {
        WindowInfo::from_ax_element(element, None).map(|(info, _)| info)
    }
}

fn bundle_info_for_pid(pid: pid_t) -> (Option<String>, Option<PathBuf>) {
    NSRunningApplication::with_process_id(pid)
        .map(|app| {
            let bundle_id = app.bundle_id().as_deref().map(|b| b.to_string());
            let path = app.bundleURL().as_ref().and_then(|url| {
                let abs_str = url.absoluteString();
                abs_str.as_deref().map(|s| PathBuf::from(s.to_string()))
            });
            (bundle_id, path)
        })
        .unwrap_or((None, None))
}
