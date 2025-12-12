//! This actor manages the global notification queue, which tells us when an
//! application is launched or focused or the screen state changes.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::{future, mem};

use dispatchr::queue;
use dispatchr::time::Time;
use objc2::rc::{Allocated, Retained};
use objc2::{AnyThread, ClassType, DeclaredClass, Encode, Encoding, define_class, msg_send, sel};
use objc2_app_kit::{self, NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey};
use objc2_core_graphics::CGDisplayBounds;
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSObject, NSProcessInfo, NSString,
};
use tracing::{debug, info_span, trace, warn};

use super::wm_controller::{self, WmEvent};
use crate::sys::app::NSRunningApplicationExt;
use crate::sys::dispatch::DispatchExt;
use crate::sys::power::{init_power_state, set_low_power_mode_state};
use crate::sys::screen::{CoordinateConverter, ScreenCache, ScreenDescriptor, SpaceId};
use crate::sys::skylight::{CGDisplayRegisterReconfigurationCallback, DisplayReconfigFlags};

const REFRESH_DEFAULT_DELAY_NS: i64 = 150_000_000;
const REFRESH_RETRY_DELAY_NS: i64 = 150_000_000;
const REFRESH_MAX_RETRIES: u8 = 10;

#[repr(C)]
struct Instance {
    screen_cache: RefCell<ScreenCache>,
    events_tx: wm_controller::Sender,
    refresh_pending: Cell<bool>,
    reconfig_in_progress: Cell<bool>,
    pending_reconfig_flags: Cell<DisplayReconfigFlags>,
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
            {
                let mut cache = self.ivars().screen_cache.borrow_mut();
                cache.mark_sleeping(false);
                cache.mark_dirty();
            }
            // On wake, refresh state immediately so display swaps while asleep
            // are reflected as soon as possible.
            self.send_event(WmEvent::SystemWoke);
            self.send_screen_parameters();
        }

        #[unsafe(method(recvSleepEvent:))]
        fn recv_sleep_event(&self, notif: &NSNotification) {
            trace!("{notif:#?}");
            let mut cache = self.ivars().screen_cache.borrow_mut();
            cache.mark_sleeping(true);
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
    }
}

impl NotificationCenterInner {
    fn new(events_tx: wm_controller::Sender) -> Retained<Self> {
        let instance = Instance {
            screen_cache: RefCell::new(ScreenCache::new(MainThreadMarker::new().unwrap())),
            events_tx,
            refresh_pending: Cell::new(false),
            reconfig_in_progress: Cell::new(false),
            pending_reconfig_flags: Cell::new(DisplayReconfigFlags::empty()),
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
        if unsafe { NSWorkspaceActiveSpaceDidChangeNotification } == name
            || name.to_string() == "NSWorkspaceActiveDisplayDidChangeNotification"
        {
            self.send_current_space();
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

    fn collect_state(
        &self,
    ) -> Option<(Vec<ScreenDescriptor>, CoordinateConverter, Vec<Option<SpaceId>>)> {
        let mut screen_cache = self.ivars().screen_cache.borrow_mut();
        screen_cache.refresh().or_else(|| {
            warn!("Unable to refresh screen configuration; skipping update");
            None
        })
    }

    fn send_screen_parameters(&self) {
        let span = info_span!("notification_center::send_screen_parameters");
        let _s = span.enter();
        self.process_screen_refresh(0, true);
    }

    fn process_screen_refresh(&self, attempt: u8, allow_retry: bool) {
        let span = info_span!("notification_center::process_screen_refresh", attempt);
        let _s = span.enter();
        let ivars = self.ivars();

        let Some((descriptors, converter, spaces)) = self.collect_state() else {
            warn!("Unable to refresh screen configuration; skipping update");
            ivars.refresh_pending.set(false);
            return;
        };

        if descriptors.is_empty() {
            trace!("Skipping screen parameter update: no active displays reported");
            ivars.refresh_pending.set(false);
            return;
        }

        if spaces.iter().any(|space| space.is_none()) {
            if allow_retry && attempt < REFRESH_MAX_RETRIES {
                trace!(attempt, "Spaces not yet available; retrying refresh");
                self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, attempt + 1);
                return;
            }
            warn!(
                attempt,
                "Spaces missing after retries; proceeding with partial info"
            );
        }

        self.send_event(WmEvent::ScreenParametersChanged(descriptors, converter, spaces));
        ivars.refresh_pending.set(false);
    }

    fn send_current_space(&self) {
        let span = info_span!("notification_center::send_current_space");
        let _s = span.enter();
        // Avoid emitting space changes while a display reconfiguration is in-flight or a
        // screen refresh is pending; these can interleave and cause window thrash between
        // displays/spaces. The refresh will emit a consistent SpaceChanged afterward.
        let ivars = self.ivars();
        if ivars.refresh_pending.get() || ivars.reconfig_in_progress.get() {
            trace!("Skipping current space update during display reconfig/refresh");
            return;
        }
        if let Some((_, _, spaces)) = self.collect_state() {
            self.send_event(WmEvent::SpaceChanged(spaces));
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

    fn handle_display_reconfig(&self, flags: DisplayReconfigFlags) {
        let ivars = self.ivars();

        if flags.contains(DisplayReconfigFlags::BEGIN_CONFIGURATION) {
            trace!("Display reconfig begin; aggregating changes");
            ivars.reconfig_in_progress.set(true);
            ivars.pending_reconfig_flags.set(DisplayReconfigFlags::empty());
            return;
        }

        let aggregated = ivars.pending_reconfig_flags.get() | flags;
        ivars.pending_reconfig_flags.set(aggregated);

        if !Self::needs_refresh_for_flags(aggregated) {
            trace!(?aggregated, "Display reconfig ignored (no impactful flags)");
            return;
        }

        // We got a post-begin callback or a standalone change; prefer to refresh immediately
        // for add/remove so we capture the new topology as soon as it stabilises.
        let saw_begin = ivars.reconfig_in_progress.replace(false);
        ivars.pending_reconfig_flags.set(DisplayReconfigFlags::empty());

        let immediate = aggregated
            .intersects(DisplayReconfigFlags::ADD | DisplayReconfigFlags::REMOVE)
            || saw_begin;

        trace!(
            ?aggregated,
            immediate, "Display reconfig detected; scheduling refresh"
        );
        {
            let mut cache = ivars.screen_cache.borrow_mut();
            cache.mark_dirty();
        }
        if immediate {
            self.schedule_screen_refresh_after(0, 0);
        } else {
            self.schedule_screen_refresh();
        }
    }

    fn handle_dock_pref_changed(&self) {
        trace!("Dock preferences changed; scheduling refresh");
        self.schedule_screen_refresh();
    }

    fn handle_menu_bar_pref_changed(&self) {
        trace!("Menu bar autohide changed; scheduling refresh");
        self.schedule_screen_refresh();
    }

    fn schedule_screen_refresh(&self) {
        self.schedule_screen_refresh_after(REFRESH_DEFAULT_DELAY_NS, 0);
    }

    fn schedule_screen_refresh_after(&self, delay_ns: i64, attempt: u8) {
        let ivars = self.ivars();
        if attempt == 0 {
            if ivars.refresh_pending.replace(true) {
                return;
            }
        } else if !ivars.refresh_pending.get() {
            ivars.refresh_pending.set(true);
        }

        let handler_ptr = self as *const _ as *mut Self;
        queue::main().after_f_s(
            Time::new_after(Time::NOW, delay_ns),
            (handler_ptr, attempt),
            |(handler_ptr, attempt)| unsafe {
                let handler = &*handler_ptr;
                handler.process_screen_refresh(attempt, true);
            },
        );
    }

    fn needs_refresh_for_flags(flags: DisplayReconfigFlags) -> bool {
        flags.intersects(
            DisplayReconfigFlags::ADD
                | DisplayReconfigFlags::REMOVE
                | DisplayReconfigFlags::MOVED
                | DisplayReconfigFlags::SET_MAIN
                | DisplayReconfigFlags::SET_MODE
                | DisplayReconfigFlags::ENABLED
                | DisplayReconfigFlags::DISABLED
                | DisplayReconfigFlags::MIRROR
                | DisplayReconfigFlags::UNMIRROR
                | DisplayReconfigFlags::DESKTOP_SHAPE_CHANGED,
        )
    }

    unsafe extern "C" fn display_reconfig_callback(
        _display: u32,
        flags: u32,
        user_info: *mut c_void,
    ) {
        if user_info.is_null() {
            return;
        }
        let handler_ptr = user_info as *mut NotificationCenterInner;
        let parsed = DisplayReconfigFlags::from_bits_truncate(flags);
        let normalized = NotificationCenterInner::normalize_display_flags(parsed, _display);
        queue::main().after_f_s(
            Time::NOW,
            (handler_ptr, normalized),
            |(handler_ptr, flags)| unsafe {
                let handler = &*handler_ptr;
                handler.handle_display_reconfig(flags);
            },
        );
    }

    /// Normalize conflicting CGDisplay flags to a single add/remove decision, following
    /// observed macOS behavior where add/remove/enable/disable can all be set together.
    fn normalize_display_flags(flags: DisplayReconfigFlags, display: u32) -> DisplayReconfigFlags {
        let mut flags = flags;

        // Map enable/disable into add/remove.
        if flags.contains(DisplayReconfigFlags::DISABLED) {
            flags.insert(DisplayReconfigFlags::REMOVE);
        }
        if flags.contains(DisplayReconfigFlags::ENABLED) {
            flags.insert(DisplayReconfigFlags::ADD);
        }

        // Mirroring treats the external display as removed; unmirror as added.
        if flags.contains(DisplayReconfigFlags::MIRROR) {
            flags.insert(DisplayReconfigFlags::REMOVE);
        }
        if flags.contains(DisplayReconfigFlags::UNMIRROR) {
            flags.insert(DisplayReconfigFlags::ADD);
        }

        if flags.contains(DisplayReconfigFlags::ADD) && flags.contains(DisplayReconfigFlags::REMOVE)
        {
            // Heuristic informed by SDL issue: a valid non-zero mode implies add wins unless mirroring.
            let bounds = CGDisplayBounds(display);
            let size_valid = bounds.size.width > 1.0 && bounds.size.height > 1.0;
            if !flags.contains(DisplayReconfigFlags::MIRROR) && size_valid {
                flags.remove(DisplayReconfigFlags::REMOVE);
            } else {
                flags.remove(DisplayReconfigFlags::ADD);
            }
        }

        flags
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
