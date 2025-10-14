use std::ffi::c_void;

use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopMode, CFRunLoopSource, kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGEvent, CGEventMask, CGEventTapLocation as CGTapLoc, CGEventTapOptions as CGTapOpt,
    CGEventTapPlacement as CGTapPlace, CGEventTapProxy, CGEventType,
};
use tracing::debug;

pub type TapCallback = Option<
    unsafe extern "C-unwind" fn(
        CGEventTapProxy,
        CGEventType,
        core::ptr::NonNull<CGEvent>,
        *mut c_void,
    ) -> *mut CGEvent,
>;

struct TrampolineCtx {
    callback: TapCallback,
    original_user_info: *mut c_void,
    original_drop: Option<unsafe fn(*mut c_void)>,
    port_ptr: Option<core::ptr::NonNull<CFMachPort>>,
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
            unsafe { CGEvent::tap_enable(port_ptr.as_ref(), true) };
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
    pub unsafe fn new_with_options(
        options: CGTapOpt,
        mask: CGEventMask,
        callback: TapCallback,
        user_info: *mut c_void,
        drop_ctx: Option<unsafe fn(*mut c_void)>,
    ) -> Option<Self> {
        let tramp = Box::new(TrampolineCtx {
            callback,
            original_user_info: user_info,
            original_drop: drop_ctx,
            port_ptr: None,
        });
        let tramp_ptr = Box::into_raw(tramp) as *mut c_void;

        let port = unsafe {
            CGEvent::tap_create(
                CGTapLoc::SessionEventTap,
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
                "EventTap::new_with_options: CFRunLoop::current() returned a run loop; adding source to common modes"
            );
            let mode: &CFRunLoopMode = unsafe {
                kCFRunLoopCommonModes.expect("kCFRunLoopCommonModes should be available on macOS")
            };
            rl.add_source(Some(&source), Some(mode));
        } else {
            debug!(
                "EventTap::new_with_options: CFRunLoop::current() returned None; run loop not present"
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
        }

        Some(event_tap)
    }

    pub unsafe fn new_listen_only(
        mask: CGEventMask,
        callback: TapCallback,
        user_info: *mut c_void,
        drop_ctx: Option<unsafe fn(*mut c_void)>,
    ) -> Option<Self> {
        unsafe { Self::new_with_options(CGTapOpt::ListenOnly, mask, callback, user_info, drop_ctx) }
    }

    pub fn set_enabled(&self, enabled: bool) { CGEvent::tap_enable(&self.port, enabled); }
}

impl Drop for EventTap {
    fn drop(&mut self) {
        CGEvent::tap_enable(&self.port, false);
        if let Some(rl) = CFRunLoop::current() {
            rl.remove_source(Some(&self.source), unsafe { kCFRunLoopCommonModes });
        }
        if let Some(dropper) = self.drop_ctx {
            unsafe { dropper(self.user_info) };
        }
    }
}
