//! Helpers for managing run loops.

use std::ffi::c_void;
use std::{mem, ptr};

use core_foundation::base::TCFType;
use core_foundation::mach_port::CFIndex;
use core_foundation::runloop::{
    CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, CFRunLoopSourceCreate,
    CFRunLoopSourceSignal, CFRunLoopWakeUp, kCFRunLoopCommonModes,
};

/// A core foundation run loop source.
///
/// This type primarily exists for the purpose of managing manual sources, which
/// can be used for signaling code that blocks on a run loop.
///
/// More information is available in the Apple documentation at
/// https://developer.apple.com/documentation/corefoundation/cfrunloopsource-rhr.
#[derive(Clone, PartialEq)]
pub struct WakeupHandle(CFRunLoopSource, CFRunLoop);

// SAFETY:
// - CFRunLoopSource and CFRunLoop are ObjC objects which are allowed to be used
//   from multiple threads.
// - We only allow signaling the source from this handle. No access to the
//   underlying handler is given, so it does not need to be Send or Sync.
unsafe impl Send for WakeupHandle {}

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
        let handler = Box::into_raw(Box::new(Handler { ref_count: 0, func: handler }));

        extern "C-unwind" fn perform<F: Fn() + 'static>(info: *const c_void) {
            // SAFETY: Only one thread may call these functions, and the mutable
            // reference lives only during the function call. No other code has
            // access to the handler.
            let handler = unsafe { &mut *(info as *mut Handler<F>) };
            (handler.func)();
        }
        extern "C" fn retain<F>(info: *const c_void) -> *const c_void {
            // SAFETY: As above.
            let handler = unsafe { &mut *(info as *mut Handler<F>) };
            handler.ref_count += 1;
            info
        }
        extern "C-unwind" fn release<F>(info: *const c_void) {
            // SAFETY: As above.
            let handler = unsafe { &mut *(info as *mut Handler<F>) };
            handler.ref_count -= 1;
            if handler.ref_count == 0 {
                mem::drop(unsafe { Box::from_raw(info as *mut Handler<F>) });
            }
        }

        // SAFETY: Strip the C-unwind ABI from the function pointer types since
        // the core-foundation crate hasn't been updated with this ABI yet. This
        // should be sound as long as we don't call the transmuted function
        // pointer from Rust.
        let release = unsafe {
            mem::transmute::<extern "C-unwind" fn(*const c_void), extern "C" fn(*const c_void)>(
                release::<F>,
            )
        };
        let perform = unsafe {
            mem::transmute::<extern "C-unwind" fn(*const c_void), extern "C" fn(*const c_void)>(
                perform::<F>,
            )
        };

        let mut context = CFRunLoopSourceContext {
            version: 0,
            info: handler as *mut c_void,
            retain: Some(retain::<F>),
            release: Some(release),
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform,
        };

        let source = unsafe {
            let source = CFRunLoopSourceCreate(ptr::null(), order, &mut context as *mut _);
            CFRunLoopSource::wrap_under_create_rule(source)
        };
        let run_loop = CFRunLoop::get_current();
        run_loop.add_source(&source, unsafe { kCFRunLoopCommonModes });

        WakeupHandle(source, run_loop)
    }

    /// Wakes the run loop that owns the target of this handle and schedules its
    /// handler to be called.
    ///
    /// Multiple signals may be collapsed into a single call of the handler.
    pub fn wake(&self) {
        unsafe {
            CFRunLoopSourceSignal(self.0.as_concrete_TypeRef());
            CFRunLoopWakeUp(self.1.as_concrete_TypeRef());
        }
    }
}
