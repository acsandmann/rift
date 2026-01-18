//! Helpers for managing run loops.

use std::ffi::c_void;
use std::mem;

use objc2_core_foundation::{
    CFIndex, CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, kCFRunLoopCommonModes,
};

/// A core foundation run loop source.
///
/// This type primarily exists for the purpose of managing manual sources, which
/// can be used for signaling code that blocks on a run loop.
///
/// More information is available in the Apple documentation at
/// https://developer.apple.com/documentation/corefoundation/cfrunloopsource-rhr.
#[derive(Clone, PartialEq)]
pub struct WakeupHandle(CFRetained<CFRunLoopSource>, CFRetained<CFRunLoop>);

// SAFETY:
// - CFRunLoopSource and CFRunLoop are CoreFoundation/ObjC objects which are allowed to be used
//   from multiple threads.
// - This handle only exposes `wake()` (signal + wake_up). It does not expose the underlying
//   handler or allow mutation of the run loop/source beyond signaling.
// - Therefore it is safe to treat this as Send + Sync for the purposes of a Waker hot path.
unsafe impl Send for WakeupHandle {}
unsafe impl Sync for WakeupHandle {}

struct Handler<F> {
    ref_count: isize,
    func: F,
}

impl WakeupHandle {
    /// Creates and adds a manual source for the current [`CFRunLoop`].
    ///
    /// The supplied function `handler` is called inside the run loop when this
    /// handle has been woken and the run loop is running.
    ///
    /// The handler is run in all common modes. `order` controls the order it is
    /// run in relative to other run loop sources, and should normally be set to
    /// 0.
    pub fn for_current_thread<F: Fn() + 'static>(order: CFIndex, handler: F) -> WakeupHandle {
        let handler_ptr = Box::into_raw(Box::new(Handler { ref_count: 0, func: handler }));

        // Use the C-unwind ABI and the exact pointer types expected by
        // CFRunLoopSourceContext.
        //
        // The callbacks are unsafe and may be called from C code. Each callback
        // receives the `info` pointer we stored (a *mut Handler<F>). We cast it
        // back and operate on it. The retain/release callbacks mutate the
        // `ref_count` and free the box when it reaches zero.
        unsafe extern "C-unwind" fn perform<F: Fn() + 'static>(info: *mut c_void) {
            // SAFETY: `info` was created from a Box<Handler<F>> and is valid.
            let handler = unsafe { &mut *(info as *mut Handler<F>) };
            (handler.func)();
        }
        unsafe extern "C-unwind" fn retain<F>(info: *const c_void) -> *const c_void {
            // SAFETY: `info` was created from a Box<Handler<F>> and is valid.
            let handler = unsafe { &mut *(info as *mut Handler<F>) };
            handler.ref_count += 1;
            info
        }
        unsafe extern "C-unwind" fn release<F>(info: *const c_void) {
            // SAFETY: `info` was created from a Box<Handler<F>> and is valid.
            let handler = unsafe { &mut *(info as *mut Handler<F>) };
            handler.ref_count -= 1;
            if handler.ref_count == 0 {
                // Recreate the Box to drop it.
                mem::drop(unsafe { Box::from_raw(info as *mut Handler<F>) });
            }
        }

        let mut context = CFRunLoopSourceContext {
            version: 0,
            info: handler_ptr as *mut c_void,
            retain: Some(retain::<F>),
            release: Some(release::<F>),
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform: Some(perform::<F>),
        };

        let source = unsafe { CFRunLoopSource::new(None, order, &mut context as *mut _) };

        let run_loop = CFRunLoop::current().unwrap();
        run_loop.add_source(source.as_deref(), unsafe { kCFRunLoopCommonModes });

        WakeupHandle(source.unwrap(), run_loop)
    }

    /// Wakes the run loop that owns the target of this handle and schedules its
    /// handler to be called.
    ///
    /// Multiple signals may be collapsed into a single call of the handler.
    pub fn wake(&self) {
        self.0.signal();
        self.1.wake_up();
    }
}
