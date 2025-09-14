use std::ffi::c_void;

use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopSource, kCFRunLoopDefaultMode,
};
use objc2_core_graphics as ocg;
use ocg::{
    CGEventMask, CGEventTapLocation as CGTapLoc, CGEventTapOptions as CGTapOpt,
    CGEventTapPlacement as CGTapPlace,
};

pub type TapCallback = Option<
    unsafe extern "C-unwind" fn(
        ocg::CGEventTapProxy,
        ocg::CGEventType,
        core::ptr::NonNull<ocg::CGEvent>,
        *mut c_void,
    ) -> *mut ocg::CGEvent,
>;

pub struct EventTap {
    port: CFRetained<CFMachPort>,
    source: CFRetained<CFRunLoopSource>,
    user_info: *mut c_void,
    drop_ctx: Option<unsafe fn(*mut c_void)>,
}

impl EventTap {
    pub unsafe fn new_listen_only(
        mask: CGEventMask,
        callback: TapCallback,
        user_info: *mut c_void,
        drop_ctx: Option<unsafe fn(*mut c_void)>,
    ) -> Option<Self> {
        let port = unsafe {
            ocg::CGEvent::tap_create(
                CGTapLoc::SessionEventTap,
                CGTapPlace::HeadInsertEventTap,
                CGTapOpt::ListenOnly,
                mask,
                callback,
                user_info,
            )?
        };

        let source = CFMachPort::new_run_loop_source(None, Some(&port), 0)?;
        if let Some(rl) = CFRunLoop::current() {
            unsafe { rl.add_source(Some(&source), kCFRunLoopDefaultMode) };
        }
        unsafe { ocg::CGEvent::tap_enable(&port, true) };

        Some(Self {
            port,
            source,
            user_info,
            drop_ctx,
        })
    }

    pub fn set_enabled(&self, enabled: bool) {
        unsafe { ocg::CGEvent::tap_enable(&self.port, enabled) };
    }
}

impl Drop for EventTap {
    fn drop(&mut self) {
        unsafe { ocg::CGEvent::tap_enable(&self.port, false) };
        if let Some(rl) = CFRunLoop::current() {
            unsafe { rl.remove_source(Some(&self.source), kCFRunLoopDefaultMode) };
        }
        if let Some(dropper) = self.drop_ctx {
            unsafe { dropper(self.user_info) };
        }
    }
}
