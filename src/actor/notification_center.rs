//! This actor manages the global notification queue, which tells us when an
//! application is launched or focused or the screen state changes.

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
            self.send_event(WmEvent::SystemWoke);
        }

        #[unsafe(method(recvSleepEvent:))]
        fn recv_sleep_event(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            self.send_space_event(spaces::Event::SystemWillSleep);
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

    fn handle_screen_changed_event(&self, notif: &NSNotification) {
        use objc2_app_kit::*;
        let name = &*notif.name();
        let span = info_span!("notification_center::handle_screen_changed_event", ?name);
        let _s = span.enter();
        if name.to_string() == "NSWorkspaceActiveDisplayDidChangeNotification" {
            self.send_space_event(spaces::Event::ActiveDisplayChanged);
        } else if unsafe { NSWorkspaceActiveSpaceDidChangeNotification } == name {
            self.send_space_event(spaces::Event::ActiveSpaceChanged);
        } else {
            warn!("Unexpected screen changed event: {notif:?}");
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
        }
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
                NSWorkspaceDidDeactivateApplicationNotification,
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
        };

        init_power_state();

        NotificationCenter { inner: handler }
    }

    pub async fn watch_for_notifications(self) {
        let workspace = &NSWorkspace::sharedWorkspace();

        self.inner.send_space_event(spaces::Event::ScreenRefreshRequested);
        self.inner.send_event(WmEvent::AppEventsRegistered);
        if let Some(app) = workspace.frontmostApplication() {
            self.inner.send_event(WmEvent::AppGloballyActivated(app.pid()));
        }

        future::pending().await
    }
}
