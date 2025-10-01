use std::path::PathBuf;

use accessibility::{AXUIElement, AXUIElementAttributes};
pub use accessibility_sys::pid_t;
use accessibility_sys::{kAXStandardWindowSubrole, kAXWindowRole};
use objc2::rc::Retained;
use objc2::{class, msg_send};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_core_foundation::CGRect;
use objc2_foundation::NSString;
use serde::{Deserialize, Serialize};

use super::geometry::{CGRectDef, ToICrate};
use super::window_server::WindowServerId;

pub fn running_apps(bundle: Option<String>) -> impl Iterator<Item = (pid_t, AppInfo)> {
    unsafe { NSWorkspace::sharedWorkspace().runningApplications() }
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

    fn bundle_id(&self) -> Option<Retained<NSString>> { unsafe { self.bundleIdentifier() } }

    fn localized_name(&self) -> Option<Retained<NSString>> { unsafe { self.localizedName() } }
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

impl TryFrom<&AXUIElement> for WindowInfo {
    type Error = accessibility::Error;

    fn try_from(element: &AXUIElement) -> Result<Self, accessibility::Error> {
        // TODO: make this use fframe
        let frame = element.frame()?;
        let is_standard =
            element.role()? == kAXWindowRole && element.subrole()? == kAXStandardWindowSubrole;

        let ax_role = element.role().ok().map(|r| r.to_string());
        let ax_subrole = element.subrole().ok().map(|s| s.to_string());

        let id = WindowServerId::try_from(element).ok();
        let is_minimized = element.minimized().map(|b| bool::from(b)).unwrap_or_default();

        let (bundle_id, path) = if !is_standard {
            (None, None)
        } else if let Some(window_id) = &id {
            crate::sys::window_server::get_window(*window_id)
                .and_then(|window_info| {
                    NSRunningApplication::with_process_id(window_info.pid).map(|app| {
                        let bundle_id = app.bundle_id().as_deref().map(|b| b.to_string());
                        let path = unsafe { app.bundleURL() }.as_ref().and_then(|url| {
                            let abs_str = unsafe { url.absoluteString() };
                            abs_str.as_deref().map(|s| PathBuf::from(s.to_string()))
                        });
                        (bundle_id, path)
                    })
                })
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        Ok(WindowInfo {
            is_standard,
            is_root: true,
            is_minimized,
            title: element.title().map(|t| t.to_string()).unwrap_or_default(),
            frame: frame.to_icrate(),
            sys_id: id,
            bundle_id,
            path,
            ax_role,
            ax_subrole,
        })
    }
}
