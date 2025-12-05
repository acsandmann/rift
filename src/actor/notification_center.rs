//! This actor manages the global notification queue, which tells us when an
//! application is launched or focused or the screen state changes.

use std::cell::RefCell;
use std::{future, mem};

use objc2::rc::{Allocated, Retained};
use objc2::{AnyThread, ClassType, DeclaredClass, Encode, Encoding, define_class, msg_send, sel};
use objc2_app_kit::{
    self, NSApplication, NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSObject, NSProcessInfo, NSString,
};
use tracing::{debug, info_span, trace, warn};

use super::wm_controller::{self, WmEvent};
use crate::sys::app::NSRunningApplicationExt;
use crate::sys::power::{init_power_state, set_low_power_mode_state};
use crate::sys::screen::{ScreenCache, ScreenDescriptor};

#[repr(C)]
struct Instance {
    screen_cache: RefCell<ScreenCache>,
    events_tx: wm_controller::Sender,
    last_screen_state: RefCell<Option<Vec<ScreenDescriptor>>>,
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
            // On wake, macOS may briefly report zero displays which would
            // cause us to clear screen state and lose track of windows.
            // Avoid pushing an immediate screen/space update here; instead,
            // rely on the subsequent system notifications
            // (NSApplicationDidChangeScreenParametersNotification and
            // NSWorkspaceActiveSpaceDidChangeNotification) to deliver the
            // real, stable configuration. We still notify the system-woke
            // event so subsystems can re-subscribe OS callbacks.
            self.send_event(WmEvent::SystemWoke);
        }

        #[unsafe(method(recvPowerEvent:))]
        fn recv_power_event(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            self.handle_power_event(notif);
        }
    }
}

impl NotificationCenterInner {
    fn new(events_tx: wm_controller::Sender) -> Retained<Self> {
        let instance = Instance {
            screen_cache: RefCell::new(ScreenCache::new(MainThreadMarker::new().unwrap())),
            events_tx,
            last_screen_state: RefCell::new(None),
        };
        unsafe { msg_send![Self::alloc(), initWith: instance] }
    }

    fn handle_screen_changed_event(&self, notif: &NSNotification) {
        use objc2_app_kit::*;
        let name = &*notif.name();
        let span = info_span!("notification_center::handle_screen_changed_event", ?name);
        let _s = span.enter();
        if unsafe { NSWorkspaceActiveSpaceDidChangeNotification } == name {
            self.send_current_space();
        } else if unsafe { NSApplicationDidChangeScreenParametersNotification } == name {
            self.send_screen_parameters();
        } else {
            panic!("Unexpected screen changed event: {notif:?}");
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

    fn send_screen_parameters(&self) {
        let mut screen_cache = self.ivars().screen_cache.borrow_mut();
        let Some((descriptors, converter)) = screen_cache.update_screen_config() else {
            return;
        };
        let spaces = screen_cache.get_screen_spaces();

        let mut last_state = self.ivars().last_screen_state.borrow_mut();
        let is_unchanged = match &*last_state {
            Some(prev) => *prev == descriptors,
            None => false,
        };

        if is_unchanged {
            trace!("Screen parameters unchanged; ignoring duplicate notification");
            return;
        }

        *last_state = Some(descriptors.clone());
        self.send_event(WmEvent::ScreenParametersChanged(descriptors, converter, spaces));
    }

    fn send_current_space(&self) {
        let mut screen_cache = self.ivars().screen_cache.borrow_mut();
        if let Some((descriptors, converter)) = screen_cache.update_screen_config() {
            let mut last_state = self.ivars().last_screen_state.borrow_mut();
            let is_unchanged = match &*last_state {
                Some(prev) => *prev == descriptors,
                None => false,
            };

            if !is_unchanged {
                *last_state = Some(descriptors.clone());
                drop(last_state);
                let spaces = screen_cache.get_screen_spaces();
                drop(screen_cache);
                self.send_event(WmEvent::ScreenParametersChanged(
                    descriptors,
                    converter,
                    spaces.clone(),
                ));
                self.send_event(WmEvent::SpaceChanged(spaces));
                return;
            }
        }
        let spaces = screen_cache.get_screen_spaces();
        drop(screen_cache);
        self.send_event(WmEvent::SpaceChanged(spaces));
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
}

pub struct NotificationCenter {
    inner: Retained<NotificationCenterInner>,
}

impl NotificationCenter {
    pub fn new(events_tx: wm_controller::Sender) -> Self {
        let handler = NotificationCenterInner::new(events_tx.clone());

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
        let shared_app = &NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
        unsafe {
            use objc2_app_kit::*;
            register_unsafe(
                sel!(recvScreenChangedEvent:),
                NSApplicationDidChangeScreenParametersNotification,
                default_center,
                shared_app,
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
                sel!(recvAppEvent:),
                NSWorkspaceDidDeactivateApplicationNotification,
                workspace_center,
                workspace,
            );
        };

        unsafe {
            default_center.addObserver_selector_name_object(
                &handler,
                sel!(recvPowerEvent:),
                Some(&NSString::from_str(
                    "NSProcessInfoPowerStateDidChangeNotification",
                )),
                None,
            );
        };

        init_power_state();

        NotificationCenter { inner: handler }
    }

    pub async fn watch_for_notifications(self) {
        let workspace = &NSWorkspace::sharedWorkspace();

        self.inner.send_screen_parameters();
        self.inner.send_event(WmEvent::AppEventsRegistered);
        if let Some(app) = workspace.frontmostApplication() {
            self.inner.send_event(WmEvent::AppGloballyActivated(app.pid()));
        }

        future::pending().await
    }
}
