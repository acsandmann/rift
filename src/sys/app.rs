use std::cell::Cell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

pub use nix::libc::pid_t;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, define_class, exception, msg_send};
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};
use objc2_core_foundation::{CGRect, CGSize};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString, ns_string};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::geometry::CGRectDef;
use super::window_server::{WindowServerId, WindowServerInfo};
use crate::sys::axuielement::{
    AX_STANDARD_WINDOW_SUBROLE, AX_WINDOW_ROLE, AXUIElement, Error as AxError,
};

const NS_KEY_VALUE_OBSERVING_OPTION_NEW: usize = 1 << 0;
const NS_KEY_VALUE_OBSERVING_OPTION_INITIAL: usize = 1 << 2;

type ApplicationCallback = Arc<dyn Fn(pid_t, AppInfo) + Send + Sync + 'static>;

struct ApplicationObserverIvars {
    app: Retained<NSRunningApplication>,
    handler: ApplicationCallback,
    info: AppInfo,
    pid: pid_t,
    observing_activation_policy: Cell<bool>,
    observing_finished_launching: Cell<bool>,
    activation_policy_notified: Cell<bool>,
    finished_launching_notified: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = ApplicationObserverIvars]
    struct ApplicationObserver;

    impl ApplicationObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(
            &self,
            key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&AnyObject>,
            _context: *mut c_void,
        ) {
            let Some(key_path) = key_path else {
                return;
            };
            if key_path.isEqualToString(ns_string!("activationPolicy")) {
                self.handle_activation_policy();
            } else if key_path.isEqualToString(ns_string!("finishedLaunching")) {
                self.handle_finished_launching();
            }
        }
    }

    unsafe impl NSObjectProtocol for ApplicationObserver {}
);

impl ApplicationObserver {
    fn new(
        app: Retained<NSRunningApplication>,
        info: AppInfo,
        handler: ApplicationCallback,
    ) -> Retained<Self> {
        let pid = app.pid();
        let observer = Self::alloc().set_ivars(ApplicationObserverIvars {
            app,
            handler,
            info,
            pid,
            observing_activation_policy: Cell::new(false),
            observing_finished_launching: Cell::new(false),
            activation_policy_notified: Cell::new(false),
            finished_launching_notified: Cell::new(false),
        });
        unsafe { msg_send![super(observer), init] }
    }

    fn observe_activation_policy(&self) {
        let ivars = self.ivars();
        if ivars.observing_activation_policy.get() || ivars.activation_policy_notified.get() {
            return;
        }
        ivars.observing_activation_policy.set(true);
        unsafe {
            let _: () = msg_send![
                &*ivars.app,
                addObserver: self,
                forKeyPath: ns_string!("activationPolicy"),
                options: (NS_KEY_VALUE_OBSERVING_OPTION_NEW | NS_KEY_VALUE_OBSERVING_OPTION_INITIAL),
                context: std::ptr::null_mut::<c_void>()
            ];
        }
    }

    fn observe_finished_launching(&self) {
        let ivars = self.ivars();
        if ivars.observing_finished_launching.get() || ivars.finished_launching_notified.get() {
            return;
        }
        ivars.observing_finished_launching.set(true);
        unsafe {
            let _: () = msg_send![
                &*ivars.app,
                addObserver: self,
                forKeyPath: ns_string!("finishedLaunching"),
                options: (NS_KEY_VALUE_OBSERVING_OPTION_NEW | NS_KEY_VALUE_OBSERVING_OPTION_INITIAL),
                context: std::ptr::null_mut::<c_void>()
            ];
        }
    }

    fn handle_activation_policy(&self) {
        let (callback, info, pid) = {
            let ivars = self.ivars();
            if ivars.activation_policy_notified.get() {
                return;
            }
            if ivars.app.activationPolicy() != NSApplicationActivationPolicy::Regular {
                return;
            }
            ivars.activation_policy_notified.set(true);
            (ivars.handler.clone(), ivars.info.clone(), ivars.pid)
        };

        self.unobserve_activation_policy();
        callback(pid, info);
    }

    fn handle_finished_launching(&self) {
        let (callback, info, pid) = {
            let ivars = self.ivars();
            if ivars.finished_launching_notified.get() {
                return;
            }
            if !ivars.app.isFinishedLaunching() {
                return;
            }
            ivars.finished_launching_notified.set(true);
            (ivars.handler.clone(), ivars.info.clone(), ivars.pid)
        };

        self.unobserve_finished_launching();
        callback(pid, info);
    }

    fn unobserve_activation_policy(&self) {
        let ivars = self.ivars();
        if !ivars.observing_activation_policy.replace(false) {
            return;
        }
        let _ = exception::catch(AssertUnwindSafe(|| unsafe {
            let _: () = msg_send![
                &*ivars.app,
                removeObserver: self,
                forKeyPath: ns_string!("activationPolicy"),
                context: std::ptr::null_mut::<c_void>()
            ];
        }));
    }

    fn unobserve_finished_launching(&self) {
        let ivars = self.ivars();
        if !ivars.observing_finished_launching.replace(false) {
            return;
        }
        let _ = exception::catch(AssertUnwindSafe(|| unsafe {
            let _: () = msg_send![
                &*ivars.app,
                removeObserver: self,
                forKeyPath: ns_string!("finishedLaunching"),
                context: std::ptr::null_mut::<c_void>()
            ];
        }));
    }
}

impl Drop for ApplicationObserver {
    fn drop(&mut self) {
        self.unobserve_activation_policy();
        self.unobserve_finished_launching();
    }
}

static APPLICATION_CALLBACK: Lazy<Mutex<Option<ApplicationCallback>>> =
    Lazy::new(|| Mutex::new(None));

static APPLICATION_OBSERVERS: Lazy<Mutex<HashMap<pid_t, usize>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn set_application_callback<F>(callback: F)
where F: Fn(pid_t, AppInfo) + Send + Sync + 'static {
    *APPLICATION_CALLBACK.lock() = Some(Arc::new(callback));
}

pub fn ensure_activation_policy_observer(pid: pid_t, info: AppInfo) {
    let callback = APPLICATION_CALLBACK.lock().clone();
    let Some(callback) = callback else {
        return;
    };
    let Some(app) = NSRunningApplication::with_process_id(pid) else {
        callback(pid, info);
        return;
    };
    observe_application(app, info, callback, |observer| {
        observer.observe_activation_policy()
    });
}

pub fn ensure_finished_launching_observer(pid: pid_t, info: AppInfo) {
    let callback = APPLICATION_CALLBACK.lock().clone();
    let Some(callback) = callback else {
        return;
    };
    let Some(app) = NSRunningApplication::with_process_id(pid) else {
        return;
    };
    if app.isFinishedLaunching() {
        callback(pid, info);
        return;
    };
    observe_application(app, info, callback, |observer| {
        observer.observe_finished_launching()
    });
}

pub fn remove_activation_policy_observer(pid: pid_t) {
    with_application_observer(pid, |observer| observer.unobserve_activation_policy());
}

pub fn remove_finished_launching_observer(pid: pid_t) {
    with_application_observer(pid, |observer| observer.unobserve_finished_launching());
}

pub fn remove_application_observer(pid: pid_t) {
    let observer = APPLICATION_OBSERVERS.lock().remove(&pid);
    if let Some(observer) = observer {
        unsafe {
            let _ = Retained::from_raw(observer as *mut ApplicationObserver);
        }
    }
}

fn with_application_observer(pid: pid_t, f: impl FnOnce(&ApplicationObserver)) {
    let observers = APPLICATION_OBSERVERS.lock();
    if let Some(&observer) = observers.get(&pid) {
        f(unsafe { &*(observer as *const ApplicationObserver) });
    }
}

fn observe_application(
    app: Retained<NSRunningApplication>,
    info: AppInfo,
    callback: ApplicationCallback,
    observe: impl FnOnce(&ApplicationObserver),
) {
    let pid = app.pid();
    let mut observers = APPLICATION_OBSERVERS.lock();
    let raw = match observers.entry(pid) {
        Entry::Occupied(entry) => *entry.get(),
        Entry::Vacant(entry) => {
            let observer = ApplicationObserver::new(app, info, callback);
            *entry.insert(Retained::into_raw(observer) as usize)
        }
    };
    observe(unsafe { &*(raw as *const ApplicationObserver) });
}

pub fn running_apps(bundle: Option<String>) -> impl Iterator<Item = (pid_t, AppInfo)> {
    let callback = APPLICATION_CALLBACK.lock().clone();
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .into_iter()
        .filter_map(move |app| {
            let bundle_id_opt = app.bundle_id();

            let bundle_id = bundle_id_opt.as_ref().map(|b| b.to_string());
            if let Some(filter) = &bundle {
                if let Some(ref bid) = bundle_id {
                    if !bid.contains(filter) {
                        return None;
                    }
                } else {
                    return None;
                }
            }

            let info = AppInfo::from(&*app);
            let pid = app.pid();

            if app.activationPolicy() != NSApplicationActivationPolicy::Regular
                && bundle_id.as_deref() != Some("com.apple.loginwindow")
            {
                if let Some(cb) = callback.clone() {
                    observe_application(app, info, cb, |observer| {
                        observer.observe_activation_policy()
                    });
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WindowInfo {
    pub is_standard: bool,
    #[serde(default)]
    pub is_root: bool,
    #[serde(default)]
    pub is_minimized: bool,
    #[serde(default)]
    pub is_resizable: bool,
    pub title: String,
    #[serde(with = "CGRectDef")]
    pub frame: CGRect,
    #[serde(skip)]
    pub min_size: Option<CGSize>,
    #[serde(skip)]
    pub max_size: Option<CGSize>,
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
            .filter(|id| id.as_nonzero().is_some())
            .or_else(|| WindowServerId::try_from(element).ok());
        let is_minimized = element.minimized().unwrap_or_default();
        let is_resizable = element.can_resize().unwrap_or(true);

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

        let min_size = server_info.map(|info| info.min_frame).or_else(|| None);
        let max_size = server_info.map(|info| info.max_frame).or_else(|| None);
        let info = WindowInfo {
            is_standard,
            is_root: true,
            is_minimized,
            is_resizable,
            min_size,
            max_size,
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
