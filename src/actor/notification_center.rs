//! This actor manages the global notification queue, which tells us when an
//! application is launched or focused or the screen state changes.

use std::cell::Cell;
use std::ffi::c_void;
use std::{future, mem};

use dispatchr::queue;
use dispatchr::time::Time;
use objc2::rc::{Allocated, Retained};
use objc2::{AnyThread, ClassType, DeclaredClass, Encode, Encoding, define_class, msg_send, sel};
use objc2_app_kit::{self, NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationCenter,
    NSNotificationSuspensionBehavior, NSObject, NSProcessInfo, NSString,
};
use tracing::{debug, info_span, trace, warn};

use super::spaces;
use super::wm_controller::{self, WmEvent};
use crate::sys::app::NSRunningApplicationExt;
use crate::sys::dispatch::DispatchExt;
use crate::sys::power::{init_power_state, set_low_power_mode_state};
use crate::sys::skylight::{CGDisplayRegisterReconfigurationCallback, DisplayReconfigFlags};

#[repr(C)]
struct Instance {
    events_tx: wm_controller::Sender,
    spaces_tx: spaces::Sender,
    session_inactive_hint: Cell<bool>,
}

unsafe impl Encode for Instance {
    const ENCODING: Encoding = Encoding::Object;
}

define_class! {
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `NotificationHandler` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[ivars = Box<Instance>]
    struct NotificationCenterInner;

    // SAFETY: Each of these method signatures must match their invocations.
    impl NotificationCenterInner {
        #[unsafe(method_id(initWith:))]
        fn init(this: Allocated<Self>, instance: Instance) -> Option<Retained<Self>> {
            let this = this.set_ivars(Box::new(instance));
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(recvScreenChangedEvent:))]
        fn recv_screen_changed_event(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            self.handle_screen_changed_event(notif);
        }

        #[unsafe(method(recvAppEvent:))]
        fn recv_app_event(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            self.handle_app_event(notif);
        }

        #[unsafe(method(recvWakeEvent:))]
        fn recv_wake_event(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            self.send_space_event(spaces::Event::SystemDidWake);
        }

        #[unsafe(method(recvSleepEvent:))]
        fn recv_sleep_event(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            self.send_space_event(spaces::Event::SystemWillSleep);
        }

        #[unsafe(method(recvSessionEvent:))]
        fn recv_session_event(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            self.handle_session_event(notif);
        }

        #[unsafe(method(recvPowerEvent:))]
        fn recv_power_event(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            self.handle_power_event(notif);
        }

        #[unsafe(method(recvMenuBarPrefChanged:))]
        fn recv_menu_bar_pref_changed(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            self.handle_menu_bar_pref_changed();
        }

        #[unsafe(method(recvDockPrefChanged:))]
        fn recv_dock_pref_changed(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            self.handle_dock_pref_changed();
        }

        #[unsafe(method(recvKeyboardLayoutChanged:))]
        fn recv_keyboard_layout_changed(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            self.send_event(WmEvent::KeyboardLayoutChanged);
        }
    }
}

impl NotificationCenterInner {
    fn new(events_tx: wm_controller::Sender, spaces_tx: spaces::Sender) -> Retained<Self> {
        let instance = Instance {
            events_tx,
            spaces_tx,
            session_inactive_hint: Cell::new(false),
        };
        let handler: Retained<Self> = unsafe { msg_send![Self::alloc(), initWith: instance] };
        unsafe {
            CGDisplayRegisterReconfigurationCallback(
                Some(Self::display_reconfig_callback),
                Retained::<NotificationCenterInner>::as_ptr(&handler) as *mut c_void,
            );
        }
        handler
    }

    fn enter_session_inactive(&self) {
        if !self.ivars().session_inactive_hint.replace(true) {
            self.send_space_event(spaces::Event::SessionDidResignActive);
        }
    }

    fn leave_session_inactive(&self) {
        if self.ivars().session_inactive_hint.replace(false) {
            self.send_space_event(spaces::Event::SessionDidBecomeActive);
        }
    }

    fn handle_screen_changed_event(&self, notif: &NSNotification) {
        use objc2_app_kit::*;
        let name = &*notif.name();
        let span = info_span!("notification_center::handle_screen_changed_event", ?name);
        let _s = span.enter();
        if name.to_string() == "NSWorkspaceActiveDisplayDidChangeNotification" {
            self.send_space_event(spaces::Event::ActiveDisplayChanged);
        } else if unsafe { NSWorkspaceActiveSpaceDidChangeNotification } == name {
            self.send_space_event(spaces::Event::ActiveSpaceChanged);
        } else if unsafe { NSApplicationDidChangeScreenParametersNotification } == name {
            self.send_space_event(spaces::Event::ScreenRefreshRequested);
        } else {
            warn!("Unexpected screen changed event: {notif:?}");
        }
    }

    fn handle_session_event(&self, notif: &NSNotification) {
        use objc2_app_kit::*;
        let name = &*notif.name();
        let span = info_span!("notification_center::handle_session_event", ?name);
        let _guard = span.enter();

        if unsafe { NSWorkspaceSessionDidResignActiveNotification } == name
            || name.to_string() == "com.apple.screenIsLocked"
        {
            self.enter_session_inactive();
        } else if unsafe { NSWorkspaceSessionDidBecomeActiveNotification } == name
            || name.to_string() == "com.apple.screenIsUnlocked"
        {
            self.leave_session_inactive();
        } else {
            warn!("Unexpected session event: {notif:?}");
        }
    }

    fn handle_power_event(&self, _notif: &NSNotification) {
        let span = info_span!("notification_center::handle_power_event");
        let _s = span.enter();

        let process_info = NSProcessInfo::processInfo();
        let current_state = process_info.isLowPowerModeEnabled();
        let old_state = set_low_power_mode_state(current_state);

        if old_state != current_state {
            debug!("Low power mode changed: {} -> {}", old_state, current_state);
            self.send_event(WmEvent::PowerStateChanged(current_state));
        }
    }

    fn handle_app_event(&self, notif: &NSNotification) {
        use objc2_app_kit::*;
        let Some(app) = self.running_application(notif) else {
            return;
        };
        let pid = app.pid();
        let name = &*notif.name();
        let span = info_span!("notification_center::handle_app_event", ?name);
        let _guard = span.enter();
        if unsafe { NSWorkspaceDidDeactivateApplicationNotification } == name {
            self.send_event(WmEvent::AppGloballyDeactivated(pid));
        } else if unsafe { NSWorkspaceDidActivateApplicationNotification } == name {
            // Do not forward AppGloballyActivated from NSWorkspace here.
            //
            // Rift intentionally treats workspace app-activation notifications as a
            // lock/login hint channel only. The authoritative global activation
            // stream comes from the Carbon process actor (`K_EVENT_APP_FRONT_SWITCHED`),
            // which still drives the reactor's activation-time visible-window refresh
            // and workspace-switch behavior. Re-emitting activation here would create
            // duplicate front-app events; the only notification-center-specific job is
            // to notice loginwindow transitions that Carbon does not model as a
            // session-lock boundary.
            let bundle_id = app.bundle_id().as_deref().map(ToString::to_string);
            if bundle_id.as_deref() == Some("com.apple.loginwindow") {
                // OmniWM found loginwindow activation to be a more reliable lock
                // boundary than the distributed lock notification alone. Route it
                // through the same session event stream so the spaces actor
                // buffers topology while macOS swaps in the lock-screen spaces.
                self.enter_session_inactive();
            } else if self.should_leave_session_inactive_for_activation(pid, bundle_id.as_deref()) {
                self.leave_session_inactive();
            }
        }
    }

    fn should_leave_session_inactive_for_activation(
        &self,
        activated_pid: i32,
        activated_bundle_id: Option<&str>,
    ) -> bool {
        let frontmost = NSWorkspace::sharedWorkspace().frontmostApplication();
        let frontmost_pid = frontmost.as_ref().map(|app| app.pid());
        let frontmost_bundle_id = frontmost
            .as_ref()
            .and_then(|app| app.bundle_id().as_deref().map(ToString::to_string));

        should_leave_session_inactive_after_non_login_activation(
            self.ivars().session_inactive_hint.get(),
            activated_pid,
            activated_bundle_id,
            frontmost_pid,
            frontmost_bundle_id.as_deref(),
        )
    }

    fn send_event(&self, event: WmEvent) { _ = self.ivars().events_tx.send(event); }

    fn send_space_event(&self, event: spaces::Event) { self.ivars().spaces_tx.send(event); }

    fn running_application(
        &self,
        notif: &NSNotification,
    ) -> Option<Retained<NSRunningApplication>> {
        let info = notif.userInfo();
        let Some(info) = info else {
            warn!("Got app notification without user info: {notif:?}");
            return None;
        };
        let app = unsafe { info.valueForKey(NSWorkspaceApplicationKey) };
        let Some(app) = app else {
            warn!("Got app notification without app object: {notif:?}");
            return None;
        };
        assert!(app.class() == NSRunningApplication::class());
        let app: Retained<NSRunningApplication> = unsafe { mem::transmute(app) };
        Some(app)
    }

    fn handle_dock_pref_changed(&self) {
        trace!("Dock preferences changed; scheduling refresh");
        self.send_space_event(spaces::Event::ScreenRefreshRequested);
    }

    fn handle_menu_bar_pref_changed(&self) {
        trace!("Menu bar autohide changed; scheduling refresh");
        self.send_space_event(spaces::Event::ScreenRefreshRequested);
    }

    unsafe extern "C" fn display_reconfig_callback(
        display_id: u32,
        flags: u32,
        user_info: *mut c_void,
    ) {
        if user_info.is_null() {
            return;
        }
        let handler_ptr = user_info as *mut NotificationCenterInner;
        let parsed = DisplayReconfigFlags::from_bits_truncate(flags);
        queue::main().after_f_s(
            Time::NOW,
            (handler_ptr, display_id, parsed),
            |(handler_ptr, display_id, flags)| unsafe {
                let handler = &*handler_ptr;
                handler.send_space_event(spaces::Event::DisplayReconfigured { display_id, flags });
            },
        );
    }
}

fn should_leave_session_inactive_after_non_login_activation(
    session_inactive_hint: bool,
    activated_pid: i32,
    activated_bundle_id: Option<&str>,
    frontmost_pid: Option<i32>,
    frontmost_bundle_id: Option<&str>,
) -> bool {
    session_inactive_hint
        && activated_bundle_id != Some("com.apple.loginwindow")
        && frontmost_bundle_id != Some("com.apple.loginwindow")
        && frontmost_pid == Some(activated_pid)
}

pub struct NotificationCenter {
    inner: Retained<NotificationCenterInner>,
}

impl NotificationCenter {
    pub fn new(events_tx: wm_controller::Sender, spaces_tx: spaces::Sender) -> Self {
        let handler = NotificationCenterInner::new(events_tx.clone(), spaces_tx);

        // SAFETY: Selector must have signature fn(&self, &NSNotification)
        let register_unsafe =
            |selector, notif_name, center: &Retained<NSNotificationCenter>, object| unsafe {
                center.addObserver_selector_name_object(
                    &handler,
                    selector,
                    Some(notif_name),
                    Some(object),
                );
            };

        let workspace = &NSWorkspace::sharedWorkspace();
        let workspace_center = &workspace.notificationCenter();
        let default_center = &NSNotificationCenter::defaultCenter();
        let distributed_center = &NSDistributedNotificationCenter::defaultCenter();
        unsafe {
            use objc2_app_kit::*;
            workspace_center.addObserver_selector_name_object(
                &handler,
                sel!(recvScreenChangedEvent:),
                Some(&NSString::from_str(
                    "NSWorkspaceActiveDisplayDidChangeNotification",
                )),
                Some(workspace),
            );
            register_unsafe(
                sel!(recvScreenChangedEvent:),
                NSWorkspaceActiveSpaceDidChangeNotification,
                workspace_center,
                workspace,
            );
            default_center.addObserver_selector_name_object(
                &handler,
                sel!(recvScreenChangedEvent:),
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
            );
            register_unsafe(
                sel!(recvWakeEvent:),
                NSWorkspaceDidWakeNotification,
                workspace_center,
                workspace,
            );
            register_unsafe(
                sel!(recvSleepEvent:),
                NSWorkspaceWillSleepNotification,
                workspace_center,
                workspace,
            );
            register_unsafe(
                sel!(recvAppEvent:),
                NSWorkspaceDidActivateApplicationNotification,
                workspace_center,
                workspace,
            );
            register_unsafe(
                sel!(recvAppEvent:),
                NSWorkspaceDidDeactivateApplicationNotification,
                workspace_center,
                workspace,
            );
            register_unsafe(
                sel!(recvSessionEvent:),
                NSWorkspaceSessionDidResignActiveNotification,
                workspace_center,
                workspace,
            );
            register_unsafe(
                sel!(recvSessionEvent:),
                NSWorkspaceSessionDidBecomeActiveNotification,
                workspace_center,
                workspace,
            );
            default_center.addObserver_selector_name_object(
                &handler,
                sel!(recvDockPrefChanged:),
                Some(&NSString::from_str("com.apple.dock.prefchanged")),
                None,
            );
            default_center.addObserver_selector_name_object(
                &handler,
                sel!(recvMenuBarPrefChanged:),
                Some(&NSString::from_str(
                    "AppleInterfaceMenuBarHidingChangedNotification",
                )),
                None,
            );
            default_center.addObserver_selector_name_object(
                &handler,
                sel!(recvPowerEvent:),
                Some(&NSString::from_str(
                    "NSProcessInfoPowerStateDidChangeNotification",
                )),
                None,
            );
            distributed_center.addObserver_selector_name_object_suspensionBehavior(
                &handler,
                sel!(recvKeyboardLayoutChanged:),
                Some(&NSString::from_str(
                    "com.apple.Carbon.TISNotifySelectedKeyboardInputSourceChanged",
                )),
                None,
                NSNotificationSuspensionBehavior::DeliverImmediately,
            );
            distributed_center.addObserver_selector_name_object_suspensionBehavior(
                &handler,
                sel!(recvSessionEvent:),
                Some(&NSString::from_str("com.apple.screenIsLocked")),
                None,
                NSNotificationSuspensionBehavior::DeliverImmediately,
            );
            distributed_center.addObserver_selector_name_object_suspensionBehavior(
                &handler,
                sel!(recvSessionEvent:),
                Some(&NSString::from_str("com.apple.screenIsUnlocked")),
                None,
                NSNotificationSuspensionBehavior::DeliverImmediately,
            );
        };

        init_power_state();

        NotificationCenter { inner: handler }
    }

    pub async fn watch_for_notifications(self) {
        let workspace = &NSWorkspace::sharedWorkspace();

        self.inner.send_space_event(spaces::Event::ScreenRefreshRequested);
        self.inner.send_event(WmEvent::AppEventsRegistered);
        if let Some(app) = workspace.frontmostApplication() {
            if app.bundle_id().as_deref().map(ToString::to_string).as_deref()
                == Some("com.apple.loginwindow")
            {
                self.inner.enter_session_inactive();
            }
            self.inner.send_event(WmEvent::AppGloballyActivated(app.pid()));
        }

        future::pending().await
    }
}

#[cfg(test)]
mod tests {
    use super::should_leave_session_inactive_after_non_login_activation;

    #[test]
    fn unlock_fallback_requires_session_to_be_marked_inactive() {
        assert!(!should_leave_session_inactive_after_non_login_activation(
            false,
            42,
            Some("com.example.app"),
            Some(42),
            Some("com.example.app"),
        ));
    }

    #[test]
    fn unlock_fallback_rejects_loginwindow_or_non_frontmost_activations() {
        assert!(!should_leave_session_inactive_after_non_login_activation(
            true,
            42,
            Some("com.example.app"),
            Some(7),
            Some("com.example.app"),
        ));
        assert!(!should_leave_session_inactive_after_non_login_activation(
            true,
            42,
            Some("com.example.app"),
            Some(42),
            Some("com.apple.loginwindow"),
        ));
        assert!(!should_leave_session_inactive_after_non_login_activation(
            true,
            42,
            Some("com.apple.loginwindow"),
            Some(42),
            Some("com.apple.loginwindow"),
        ));
    }

    #[test]
    fn unlock_fallback_accepts_matching_frontmost_non_login_activation() {
        assert!(should_leave_session_inactive_after_non_login_activation(
            true,
            42,
            Some("com.example.app"),
            Some(42),
            Some("com.example.app"),
        ));
    }
}
