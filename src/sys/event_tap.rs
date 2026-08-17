use std::ffi::c_void;

use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopMode, CFRunLoopSource, kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventMask, CGEventTapLocation as CGTapLoc,
    CGEventTapOptions as CGTapOpt, CGEventTapPlacement as CGTapPlace, CGEventTapProxy, CGEventType,
};
use tracing::{debug, error, warn};

const K_CGS_EVENT_TYPE_FIELD: CGEventField = CGEventField(55);
const K_CGS_EVENT_DOCK_CONTROL: i64 = 30;
const K_GESTURE_HID_TYPE_FIELD: CGEventField = CGEventField(110);
const K_GESTURE_SWIPE_MOTION_FIELD: CGEventField = CGEventField(123);
const K_IOHID_EVENT_TYPE_DOCK_SWIPE: i64 = 23;
const K_CG_GESTURE_MOTION_HORIZONTAL: i64 = 1;

pub type TapCallback = Option<
    unsafe extern "C-unwind" fn(
        CGEventTapProxy,
        CGEventType,
        core::ptr::NonNull<CGEvent>,
        *mut c_void,
    ) -> *mut CGEvent,
>;

pub type TapReenabledCallback = Option<unsafe extern "C-unwind" fn(*mut c_void)>;
pub type TapInvalidatedCallback = Option<unsafe extern "C-unwind" fn(*mut c_void)>;

struct TrampolineCtx {
    callback: TapCallback,
    original_user_info: *mut c_void,
    original_drop: Option<unsafe fn(*mut c_void)>,
    reenabled_callback: TapReenabledCallback,
    invalidated_callback: TapInvalidatedCallback,
    port_ptr: Option<core::ptr::NonNull<CFMachPort>>,
}

extern "C-unwind" fn port_invalidated(_port: *mut CFMachPort, user_info: *mut c_void) {
    if user_info.is_null() {
        return;
    }

    let ctx = unsafe { &*(user_info as *const TrampolineCtx) };
    warn!("Event tap Mach port was invalidated; scheduling tap recreation");
    if let Some(callback) = ctx.invalidated_callback {
        unsafe { callback(ctx.original_user_info) };
    }
}

extern "C-unwind" fn trampoline_callback(
    proxy: CGEventTapProxy,
    etype: CGEventType,
    event_ref: core::ptr::NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    if user_info.is_null() {
        return event_ref.as_ptr();
    }

    let ctx = unsafe { &*(user_info as *const TrampolineCtx) };

    // kCGEventTapDisabledByTimeout (-2) & kCGEventTapDisabledByUserInput (-1)
    let ety = etype.0 as i32;
    if ety == -1 || ety == -2 {
        if let Some(port_ptr) = ctx.port_ptr {
            let port = unsafe { port_ptr.as_ref() };
            let reason = if ety == -2 { "timeout" } else { "user input" };
            warn!(reason, "Event tap was disabled; re-enabling it");
            CGEvent::tap_enable(port, true);
            if CGEvent::tap_is_enabled(port) {
                if let Some(callback) = ctx.reenabled_callback {
                    unsafe { callback(ctx.original_user_info) };
                }
            } else {
                error!(reason, "Event tap did not re-enable; scheduling tap recreation");
                if let Some(callback) = ctx.invalidated_callback {
                    unsafe { callback(ctx.original_user_info) };
                }
            }
        }

        return event_ref.as_ptr();
    }

    if let Some(orig_cb) = ctx.callback {
        return unsafe { orig_cb(proxy, etype, event_ref, ctx.original_user_info) };
    }

    event_ref.as_ptr()
}

unsafe fn trampoline_drop(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }

    let ctx: Box<TrampolineCtx> = unsafe { Box::from_raw(ptr as *mut TrampolineCtx) };
    if let Some(dropper) = ctx.original_drop {
        if !ctx.original_user_info.is_null() {
            unsafe { dropper(ctx.original_user_info) };
        }
    }
}

pub struct EventTap {
    port: CFRetained<CFMachPort>,
    source: CFRetained<CFRunLoopSource>,
    user_info: *mut c_void,
    drop_ctx: Option<unsafe fn(*mut c_void)>,
}

impl EventTap {
    unsafe fn create(
        location: CGTapLoc,
        options: CGTapOpt,
        mask: CGEventMask,
        callback: TapCallback,
        user_info: *mut c_void,
        drop_ctx: Option<unsafe fn(*mut c_void)>,
        reenabled_callback: TapReenabledCallback,
        invalidated_callback: TapInvalidatedCallback,
    ) -> Option<Self> {
        let tramp = Box::new(TrampolineCtx {
            callback,
            original_user_info: user_info,
            original_drop: drop_ctx,
            reenabled_callback,
            invalidated_callback,
            port_ptr: None,
        });
        let tramp_ptr = Box::into_raw(tramp) as *mut c_void;

        let port = unsafe {
            CGEvent::tap_create(
                location,
                CGTapPlace::HeadInsertEventTap,
                options,
                mask,
                Some(trampoline_callback),
                tramp_ptr,
            )?
        };

        let source = CFMachPort::new_run_loop_source(None, Some(&port), 0)?;
        if let Some(rl) = CFRunLoop::current() {
            debug!(
                "EventTap::new_at_location_with_options: CFRunLoop::current() returned a run loop; adding source to common modes"
            );
            let mode: &CFRunLoopMode = unsafe {
                kCFRunLoopCommonModes.expect("kCFRunLoopCommonModes should be available on macOS")
            };
            rl.add_source(Some(&source), Some(mode));
        } else {
            debug!(
                "EventTap::new_at_location_with_options: CFRunLoop::current() returned None; run loop not present"
            );
        }
        CGEvent::tap_enable(&port, true);

        let event_tap = Self {
            port,
            source,
            user_info: tramp_ptr,
            drop_ctx: Some(trampoline_drop),
        };

        unsafe {
            let tramp_ctx = &mut *(tramp_ptr as *mut TrampolineCtx);
            tramp_ctx.port_ptr = Some(core::ptr::NonNull::from(&*event_tap.port));
            event_tap.port.set_invalidation_call_back(Some(port_invalidated));
        }

        Some(event_tap)
    }

    pub unsafe fn new_at_location_with_options(
        location: CGTapLoc,
        options: CGTapOpt,
        mask: CGEventMask,
        callback: TapCallback,
        user_info: *mut c_void,
        drop_ctx: Option<unsafe fn(*mut c_void)>,
    ) -> Option<Self> {
        unsafe {
            Self::create(
                location, options, mask, callback, user_info, drop_ctx, None, None,
            )
        }
    }

    /// Creates an event tap at `location` and reports both successful
    /// re-enables and failures that require the owner to recreate the tap.
    pub unsafe fn new_at_location_with_options_and_recovery_callbacks(
        location: CGTapLoc,
        options: CGTapOpt,
        mask: CGEventMask,
        callback: TapCallback,
        user_info: *mut c_void,
        drop_ctx: Option<unsafe fn(*mut c_void)>,
        reenabled_callback: TapReenabledCallback,
        invalidated_callback: TapInvalidatedCallback,
    ) -> Option<Self> {
        unsafe {
            Self::create(
                location,
                options,
                mask,
                callback,
                user_info,
                drop_ctx,
                reenabled_callback,
                invalidated_callback,
            )
        }
    }

    pub unsafe fn new_with_options(
        options: CGTapOpt,
        mask: CGEventMask,
        callback: TapCallback,
        user_info: *mut c_void,
        drop_ctx: Option<unsafe fn(*mut c_void)>,
    ) -> Option<Self> {
        unsafe {
            Self::new_at_location_with_options(
                CGTapLoc::SessionEventTap,
                options,
                mask,
                callback,
                user_info,
                drop_ctx,
            )
        }
    }

    /// Creates a session event tap that invokes `reenabled_callback` immediately
    /// after Core Graphics reports and the trampoline recovers a disabled tap.
    pub unsafe fn new_with_options_and_recovery_callbacks(
        options: CGTapOpt,
        mask: CGEventMask,
        callback: TapCallback,
        user_info: *mut c_void,
        drop_ctx: Option<unsafe fn(*mut c_void)>,
        reenabled_callback: TapReenabledCallback,
        invalidated_callback: TapInvalidatedCallback,
    ) -> Option<Self> {
        unsafe {
            Self::new_at_location_with_options_and_recovery_callbacks(
                CGTapLoc::SessionEventTap,
                options,
                mask,
                callback,
                user_info,
                drop_ctx,
                reenabled_callback,
                invalidated_callback,
            )
        }
    }

    pub unsafe fn new_listen_only(
        mask: CGEventMask,
        callback: TapCallback,
        user_info: *mut c_void,
        drop_ctx: Option<unsafe fn(*mut c_void)>,
    ) -> Option<Self> {
        unsafe { Self::new_with_options(CGTapOpt::ListenOnly, mask, callback, user_info, drop_ctx) }
    }

    pub unsafe fn new_at_location_listen_only(
        location: CGTapLoc,
        mask: CGEventMask,
        callback: TapCallback,
        user_info: *mut c_void,
        drop_ctx: Option<unsafe fn(*mut c_void)>,
    ) -> Option<Self> {
        unsafe {
            Self::new_at_location_with_options(
                location,
                CGTapOpt::ListenOnly,
                mask,
                callback,
                user_info,
                drop_ctx,
            )
        }
    }

    pub fn set_enabled(&self, enabled: bool) { CGEvent::tap_enable(&self.port, enabled); }
}

impl Drop for EventTap {
    fn drop(&mut self) {
        if self.port.is_valid() {
            // Intentional teardown/replacement must not be mistaken for an
            // unexpected Mach-port failure by the event-driven recovery path.
            unsafe { self.port.set_invalidation_call_back(None) };
            CGEvent::tap_enable(&self.port, false);
        }
        if let Some(rl) = CFRunLoop::current() {
            rl.remove_source(Some(&self.source), unsafe { kCFRunLoopCommonModes });
        }
        if let Some(dropper) = self.drop_ctx {
            unsafe { dropper(self.user_info) };
        }
    }
}

/// Consumes only WindowServer's physical horizontal Dock swipe events. Gesture
/// recognition itself belongs to MultitouchSupport; this tap exists solely to
/// prevent macOS from acting on a gesture Rift owns.
pub struct DockSwipeSuppressor {
    _tap: EventTap,
}

impl DockSwipeSuppressor {
    pub unsafe fn new(
        user_info: *mut c_void,
        drop_ctx: Option<unsafe fn(*mut c_void)>,
        invalidated_callback: TapInvalidatedCallback,
    ) -> Option<Self> {
        let tap = unsafe {
            EventTap::new_at_location_with_options_and_recovery_callbacks(
                CGTapLoc::HIDEventTap,
                CGTapOpt::Default,
                1u64 << (K_CGS_EVENT_DOCK_CONTROL as u64),
                Some(suppress_dock_swipe),
                user_info,
                drop_ctx,
                None,
                invalidated_callback,
            )?
        };
        Some(Self { _tap: tap })
    }
}

unsafe extern "C-unwind" fn suppress_dock_swipe(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event_ref: core::ptr::NonNull<CGEvent>,
    _user_info: *mut c_void,
) -> *mut CGEvent {
    let event = unsafe { event_ref.as_ref() };
    let cgs_type = CGEvent::integer_value_field(Some(event), K_CGS_EVENT_TYPE_FIELD);
    let hid_type = CGEvent::integer_value_field(Some(event), K_GESTURE_HID_TYPE_FIELD);
    let motion = CGEvent::integer_value_field(Some(event), K_GESTURE_SWIPE_MOTION_FIELD);

    if (event_type.0 as i64 == K_CGS_EVENT_DOCK_CONTROL || cgs_type == K_CGS_EVENT_DOCK_CONTROL)
        && hid_type == K_IOHID_EVENT_TYPE_DOCK_SWIPE
        && motion == K_CG_GESTURE_MOTION_HORIZONTAL
    {
        std::ptr::null_mut()
    } else {
        event_ref.as_ptr()
    }
}
