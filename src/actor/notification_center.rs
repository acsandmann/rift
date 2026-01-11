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
use crate::sys::window_server;

const REFRESH_DEFAULT_DELAY_NS: i64 = 150_000_000;
const REFRESH_RETRY_DELAY_NS: i64 = 150_000_000;
const REFRESH_MAX_RETRIES: u8 = 10;

const DISPLAY_CHURN_QUIET_NS: i64 = 300_000_000;
const DISPLAY_STABILIZE_RETRY_NS: i64 = 200_000_000;
const DISPLAY_STABILIZE_MAX_ATTEMPTS: u8 = 25;
const DISPLAY_STABLE_REQUIRED_HITS: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
struct DisplayTopologyFingerprint(Vec<(String, u64, u64, u64, u64)>);

#[derive(Debug, Clone)]
struct DisplayTopologyState {
    fingerprint: DisplayTopologyFingerprint,
    hits: u8,
}

#[repr(C)]
struct Instance {
    screen_cache: RefCell<ScreenCache>,
    events_tx: wm_controller::Sender,
    refresh_pending: Cell<bool>,

    display_churn_active: Cell<bool>,
    display_churn_epoch: Cell<u64>,
    display_churn_flags: Cell<DisplayReconfigFlags>,
    display_topology_state: RefCell<Option<DisplayTopologyState>>,
    refresh_deferred_until_stable: Cell<bool>,
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

            display_churn_active: Cell::new(false),
            display_churn_epoch: Cell::new(0),
            display_churn_flags: Cell::new(DisplayReconfigFlags::empty()),
            display_topology_state: RefCell::new(None),
            refresh_deferred_until_stable: Cell::new(false),
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

        if ivars.display_churn_active.get() {
            trace!("Deferring screen refresh until display churn ends");
            ivars.refresh_deferred_until_stable.set(true);
            ivars.refresh_pending.set(false);
            return;
        }

        let Some((descriptors, converter, spaces)) = self.collect_state() else {
            if allow_retry && attempt < REFRESH_MAX_RETRIES {
                trace!(attempt, "Screen state not ready; retrying refresh");
                self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, attempt + 1);
                return;
            }
            warn!("Unable to refresh screen configuration; skipping update");
            ivars.refresh_pending.set(false);
            return;
        };

        if descriptors.is_empty() {
            if allow_retry && attempt < REFRESH_MAX_RETRIES {
                trace!(attempt, "No displays yet; retrying refresh");
                self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, attempt + 1);
                return;
            }
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
        if ivars.refresh_pending.get() || ivars.display_churn_active.get() {
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

    fn handle_display_reconfig_event(&self, display_id: u32, flags: DisplayReconfigFlags) {
        let ivars = self.ivars();

        let was_active = ivars.display_churn_active.replace(true);
        ivars.display_churn_flags.set(ivars.display_churn_flags.get() | flags);
        ivars.display_churn_epoch.set(ivars.display_churn_epoch.get().wrapping_add(1));
        ivars.display_topology_state.borrow_mut().take();
        if !was_active {
            self.send_event(WmEvent::DisplayChurnBegin);
        }

        {
            let mut cache = ivars.screen_cache.borrow_mut();
            cache.mark_dirty();
        }

        let expected_epoch = ivars.display_churn_epoch.get();
        trace!(
            display_id,
            ?flags,
            expected_epoch,
            "Display reconfig event; debouncing"
        );

        self.schedule_display_stabilization_check(expected_epoch);
    }

    fn schedule_display_stabilization_check(&self, expected_epoch: u64) {
        self.schedule_display_stabilization(expected_epoch, 0, DISPLAY_CHURN_QUIET_NS);
    }

    fn attempt_finish_display_churn(&self, expected_epoch: u64, attempt: u8) {
        let ivars = self.ivars();
        if expected_epoch != ivars.display_churn_epoch.get() || !ivars.display_churn_active.get() {
            return;
        }

        let Some((descriptors, _, _)) = self.collect_state() else {
            self.retry_or_finish_display_churn(
                expected_epoch,
                attempt,
                "Unable to refresh displays after retries; forcing refresh",
            );
            return;
        };

        if descriptors.is_empty() {
            self.retry_or_finish_display_churn(
                expected_epoch,
                attempt,
                "No active displays reported after retries; forcing refresh",
            );
            return;
        }

        let fingerprint = Self::fingerprint_displays(&descriptors);
        let mut state = ivars.display_topology_state.borrow_mut();
        let hits = match state.as_mut() {
            Some(existing) if existing.fingerprint == fingerprint => {
                existing.hits = existing.hits.saturating_add(1);
                existing.hits
            }
            _ => {
                trace!(
                    "fingerprint_changed expected_epoch={} attempt={}",
                    expected_epoch, attempt
                );
                *state = Some(DisplayTopologyState { fingerprint, hits: 1 });
                drop(state);
                self.schedule_display_stabilization_retry(expected_epoch, attempt + 1);
                return;
            }
        };
        drop(state);

        if hits >= DISPLAY_STABLE_REQUIRED_HITS {
            if !window_server::windowserver_quiet_for_us(window_server::WINDOWSERVER_QUIET_US) {
                trace!(
                    hits,
                    expected_epoch, "WindowServer still churning; waiting before finalizing"
                );
                if !self.retry_display_stabilization(expected_epoch, attempt) {
                    warn!("WindowServer churn did not settle; forcing refresh");
                    self.finish_display_churn(expected_epoch);
                }
                return;
            }

            trace!(hits, expected_epoch, "Display churn settled; refreshing");
            self.finish_display_churn(expected_epoch);
            return;
        }

        if !self.retry_display_stabilization(expected_epoch, attempt) {
            warn!("Unable to confirm stable display topology; forcing refresh");
            self.finish_display_churn(expected_epoch);
        }
    }

    fn schedule_display_stabilization_retry(&self, expected_epoch: u64, attempt: u8) {
        self.schedule_display_stabilization(expected_epoch, attempt, DISPLAY_STABILIZE_RETRY_NS);
    }

    fn schedule_display_stabilization(&self, expected_epoch: u64, attempt: u8, delay_ns: i64) {
        let handler_ptr = self as *const _ as *mut Self;
        queue::main().after_f_s(
            Time::new_after(Time::NOW, delay_ns),
            (handler_ptr, expected_epoch, attempt),
            |(handler_ptr, expected_epoch, attempt)| unsafe {
                let handler = &*handler_ptr;
                handler.attempt_finish_display_churn(expected_epoch, attempt);
            },
        );
    }

    fn retry_display_stabilization(&self, expected_epoch: u64, attempt: u8) -> bool {
        if attempt < DISPLAY_STABILIZE_MAX_ATTEMPTS {
            self.schedule_display_stabilization_retry(expected_epoch, attempt + 1);
            return true;
        }
        false
    }

    fn retry_or_finish_display_churn(
        &self,
        expected_epoch: u64,
        attempt: u8,
        warn_msg: &'static str,
    ) {
        if !self.retry_display_stabilization(expected_epoch, attempt) {
            warn!("{}", warn_msg);
            self.finish_display_churn(expected_epoch);
        }
    }

    fn finish_display_churn(&self, expected_epoch: u64) {
        let ivars = self.ivars();
        if expected_epoch != ivars.display_churn_epoch.get() {
            return;
        }
        if !ivars.display_churn_active.get() {
            return;
        }

        trace!(
            expected_epoch,
            churn_flags = ?ivars.display_churn_flags.get(),
            "Finalizing display churn"
        );

        ivars.display_churn_active.set(false);
        ivars.display_churn_epoch.set(ivars.display_churn_epoch.get().wrapping_add(1));
        ivars.display_churn_flags.set(DisplayReconfigFlags::empty());
        ivars.display_topology_state.borrow_mut().take();
        self.send_event(WmEvent::DisplayChurnEnd);

        if ivars.refresh_deferred_until_stable.replace(false) {
            trace!("Running deferred refresh after display churn");
        }
        self.schedule_screen_refresh_after(0, 0);
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
        if attempt == 0 && ivars.display_churn_active.get() {
            trace!("Deferring refresh until display churn ends");
            ivars.refresh_deferred_until_stable.set(true);
            return;
        }

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
                handler.handle_display_reconfig_event(display_id, flags);
            },
        );
    }

    fn fingerprint_displays(descriptors: &[ScreenDescriptor]) -> DisplayTopologyFingerprint {
        DisplayTopologyFingerprint(
            descriptors
                .iter()
                .map(|d| {
                    (
                        d.display_uuid.clone(),
                        d.frame.origin.x.to_bits(),
                        d.frame.origin.y.to_bits(),
                        d.frame.size.width.to_bits(),
                        d.frame.size.height.to_bits(),
                    )
                })
                .collect(),
        )
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
