use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;

use dispatchr::queue;
use dispatchr::time::Time;
pub use nix::libc::pid_t;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};
use objc2_core_foundation::CGRect;
use objc2_foundation::{NSCopying, NSObject, NSObjectProtocol, NSString, ns_string};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::geometry::CGRectDef;
use super::window_server::{WindowServerId, WindowServerInfo};
use crate::sys::axuielement::{
    AX_STANDARD_WINDOW_SUBROLE, AX_WINDOW_ROLE, AXUIElement, Error as AxError,
};
use crate::sys::dispatch::DispatchExt;

const NS_KEY_VALUE_OBSERVING_OPTION_NEW: usize = 1 << 0;
const NS_KEY_VALUE_OBSERVING_OPTION_INITIAL: usize = 1 << 2;

type ActivationPolicyCallback = Arc<dyn Fn(pid_t, AppInfo) + Send + Sync + 'static>;

struct ActivationPolicyObserverIvars {
    app: Retained<NSRunningApplication>,
    key_path: Retained<NSString>,
    handler: ActivationPolicyCallback,
    info: AppInfo,
    pid: pid_t,
    notified: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = ActivationPolicyObserverIvars]
    struct ActivationPolicyObserver;

    impl ActivationPolicyObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&AnyObject>,
            _context: *mut c_void,
        ) {
            self.handle_activation_policy();
        }
    }

    unsafe impl NSObjectProtocol for ActivationPolicyObserver {}
);

impl ActivationPolicyObserver {
    fn new(
        app: Retained<NSRunningApplication>,
        info: AppInfo,
        handler: ActivationPolicyCallback,
    ) -> Retained<Self> {
        let key_path = ns_string!("activationPolicy");
        let pid = app.pid();
        let observer = Self::alloc().set_ivars(ActivationPolicyObserverIvars {
            app,
            key_path: key_path.copy(),
            handler,
            info,
            pid,
            notified: Cell::new(false),
        });
        let observer: Retained<Self> = unsafe { msg_send![super(observer), init] };
        unsafe {
            let ivars = observer.ivars();
            let _: () = msg_send![
                &*ivars.app,
                addObserver: &*observer,
                forKeyPath: &*ivars.key_path,
                options: (NS_KEY_VALUE_OBSERVING_OPTION_NEW | NS_KEY_VALUE_OBSERVING_OPTION_INITIAL),
                context: std::ptr::null_mut::<c_void>()
            ];
        }
        observer
    }

    fn handle_activation_policy(&self) {
        let (callback, info, pid) = {
            let ivars = self.ivars();
            if ivars.notified.get() {
                return;
            }
            if ivars.app.activationPolicy() != NSApplicationActivationPolicy::Regular {
                return;
            }
            ivars.notified.set(true);
            (ivars.handler.clone(), ivars.info.clone(), ivars.pid)
        };
        callback(pid, info);
        schedule_observer_cleanup(pid);
    }
}

impl Drop for ActivationPolicyObserver {
    fn drop(&mut self) {
        unsafe {
            let ivars = self.ivars();
            let _: () = msg_send![
                &*ivars.app,
                removeObserver: &*self,
                forKeyPath: &*ivars.key_path
            ];
        }
    }
}

struct CleanupCtx(pid_t);

extern "C" fn cleanup_observer(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let pid = unsafe { Box::from_raw(ctx as *mut CleanupCtx).0 };
    if let Some(observer) = ACTIVATION_POLICY_OBSERVERS.lock().remove(&pid) {
        unsafe {
            let ptr = observer as *mut ActivationPolicyObserver;
            let _ = Retained::from_raw(ptr);
        }
    }
}

fn schedule_observer_cleanup(pid: pid_t) {
    let ctx = Box::new(CleanupCtx(pid));
    queue::main().after_f(Time::NOW, Box::into_raw(ctx) as *mut c_void, cleanup_observer);
}

static ACTIVATION_POLICY_CALLBACK: Lazy<Mutex<Option<ActivationPolicyCallback>>> =
    Lazy::new(|| Mutex::new(None));

static ACTIVATION_POLICY_OBSERVERS: Lazy<Mutex<HashMap<pid_t, usize>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn set_activation_policy_callback<F>(callback: F)
where F: Fn(pid_t, AppInfo) + Send + Sync + 'static {
    *ACTIVATION_POLICY_CALLBACK.lock() = Some(Arc::new(callback));
}

pub fn clear_activation_policy_callback() { *ACTIVATION_POLICY_CALLBACK.lock() = None; }

pub fn ensure_activation_policy_observer(pid: pid_t, info: AppInfo) {
    let callback = ACTIVATION_POLICY_CALLBACK.lock().clone();
    let Some(callback) = callback else {
        return;
    };
    if ACTIVATION_POLICY_OBSERVERS.lock().contains_key(&pid) {
        return;
    }
    let Some(app) = NSRunningApplication::with_process_id(pid) else {
        return;
    };
    observe_activation_policy(app, info, callback);
}

pub fn remove_activation_policy_observer(pid: pid_t) {
    if let Some(observer) = ACTIVATION_POLICY_OBSERVERS.lock().remove(&pid) {
        unsafe {
            let ptr = observer as *mut ActivationPolicyObserver;
            let _ = Retained::from_raw(ptr);
        }
    }
}

fn observe_activation_policy(
    app: Retained<NSRunningApplication>,
    info: AppInfo,
    callback: ActivationPolicyCallback,
) {
    let pid = app.pid();
    {
        if ACTIVATION_POLICY_OBSERVERS.lock().contains_key(&pid) {
            return;
        }
    }
    let observer = ActivationPolicyObserver::new(app, info, callback);
    let raw = Retained::into_raw(observer) as *mut ActivationPolicyObserver;
    ACTIVATION_POLICY_OBSERVERS.lock().insert(pid, raw as usize);
}

pub fn running_apps(bundle: Option<String>) -> impl Iterator<Item = (pid_t, AppInfo)> {
    let callback = ACTIVATION_POLICY_CALLBACK.lock().clone();
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .into_iter()
        .filter_map(move |app| {
            let bundle_id = app.bundle_id()?.to_string();
            if let Some(filter) = &bundle {
                if !bundle_id.contains(filter) {
                    return None;
                }
            }

            let info = AppInfo::from(&*app);
            let pid = app.pid();

            if app.activationPolicy() != NSApplicationActivationPolicy::Regular
                && bundle_id != "com.apple.loginwindow"
            {
                if let Some(cb) = callback.clone() {
                    observe_activation_policy(app, info, cb);
                }
                return None;
            }

            Some((pid, info))
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
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
    }

    fn pid(&self) -> pid_t { self.processIdentifier() }

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
