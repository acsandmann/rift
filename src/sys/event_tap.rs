use std::ffi::c_void;

use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopMode, CFRunLoopSource, kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGEvent, CGEventMask, CGEventTapLocation as CGTapLoc, CGEventTapOptions as CGTapOpt,
    CGEventTapPlacement as CGTapPlace, CGEventTapProxy, CGEventType,
};
use tracing::{error, warn};

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
    /// Creates a tap on the current run loop (Rift input uses active HID).
    /// On failure the caller retains ownership of `user_info`.
    pub unsafe fn new(
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
            )
        };
        let Some(port) = port else {
            unsafe {
                drop(Box::from_raw(tramp_ptr as *mut TrampolineCtx));
            }
            return None;
        };
        let Some(source) = CFMachPort::new_run_loop_source(None, Some(&port), 0) else {
            port.invalidate();
            unsafe {
                drop(Box::from_raw(tramp_ptr as *mut TrampolineCtx));
            }
            return None;
        };
        if let Some(rl) = CFRunLoop::current() {
            let mode: &CFRunLoopMode = unsafe {
                kCFRunLoopCommonModes.expect("kCFRunLoopCommonModes should be available on macOS")
            };
            rl.add_source(Some(&source), Some(mode));
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
        self.port.invalidate();
        if let Some(dropper) = self.drop_ctx {
            unsafe { dropper(self.user_info) };
        }
    }
}
