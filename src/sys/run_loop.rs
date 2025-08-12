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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::thread::JoinHandle;

    use core_foundation::runloop::CFRunLoop;

    use super::WakeupHandle;

    struct RunLoopThread {
        num_wakeups: Arc<AtomicI32>,
        shutdown: Arc<AtomicBool>,
        channel: Receiver<Option<WakeupHandle>>,
        drop_tracker: DropTracker,
        thread: JoinHandle<()>,
    }

    fn spawn_run_loop_thread(run: bool) -> RunLoopThread {
        let num_wakeups = Arc::new(AtomicI32::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (handler_wakeups, handler_shutdown) = (num_wakeups.clone(), shutdown.clone());
        let (tx, rx) = channel();
        let (drop_tracker, drop_signaler) = DropTracker::new();
        let thread = std::thread::spawn(move || {
            let handler_tx = tx.clone();
            let wakeup = WakeupHandle::for_current_thread(0, move || {
                println!("handler");
                let _signaler = &drop_signaler;
                handler_wakeups.fetch_add(1, Ordering::SeqCst);
                let _ = handler_tx.send(None);
                if handler_shutdown.load(Ordering::SeqCst) {
                    CFRunLoop::get_current().stop();
                }
                println!("done");
            });
            tx.send(Some(wakeup)).unwrap();
            if run {
                CFRunLoop::run_current();
            }
        });
        RunLoopThread {
            num_wakeups,
            shutdown,
            channel: rx,
            drop_tracker,
            thread,
        }
    }

    #[test]
    fn it_works_without_wakeups() {
        let RunLoopThread {
            num_wakeups,
            channel: rx,
            drop_tracker,
            thread,
            ..
        } = spawn_run_loop_thread(false);
        let wakeup = rx.recv().unwrap().expect("should receive a wakeup handle");
        thread.join().unwrap();
        assert_eq!(0, num_wakeups.load(Ordering::SeqCst));
        drop(wakeup);
        drop_tracker.wait_for_drop();
    }

    #[test]
    fn it_wakes() {
        let RunLoopThread {
            num_wakeups,
            shutdown,
            channel: rx,
            drop_tracker,
            thread,
        } = spawn_run_loop_thread(true);
        let wakeup = rx.recv().unwrap().expect("should receive a wakeup handle");
        assert_eq!(0, num_wakeups.load(Ordering::SeqCst));
        shutdown.store(true, Ordering::SeqCst);
        wakeup.wake();
        thread.join().unwrap();
        assert_eq!(1, num_wakeups.load(Ordering::SeqCst));
        drop(wakeup);
        drop_tracker.wait_for_drop();
    }

    #[test]
    fn it_can_wake_from_multiple_threads() {
        let RunLoopThread {
            num_wakeups,
            shutdown,
            channel: rx,
            drop_tracker,
            thread,
        } = spawn_run_loop_thread(true);
        let wakeup = rx.recv().unwrap().expect("should receive a wakeup handle");
        assert_eq!(0, num_wakeups.load(Ordering::SeqCst));
        let thread_wakeup = wakeup.clone();
        std::thread::spawn(move || thread_wakeup.wake()).join().unwrap();
        let _ = rx.recv().unwrap();
        assert_eq!(1, num_wakeups.load(Ordering::SeqCst));
        shutdown.store(true, Ordering::SeqCst);
        wakeup.wake();
        thread.join().unwrap();
        assert_eq!(2, num_wakeups.load(Ordering::SeqCst));
        drop(wakeup);
        drop_tracker.wait_for_drop();
    }

    struct DropTracker(Arc<AtomicUsize>, Receiver<()>);
    impl DropTracker {
        fn new() -> (DropTracker, DropSignaller) {
            let (tx, rx) = channel();
            let tracker = DropTracker(Default::default(), rx);
            let signaller = DropSignaller(tracker.0.clone(), tx);
            (tracker, signaller)
        }

        fn wait_for_drop(self) {
            self.1.recv().unwrap();
            assert_eq!(1, self.0.load(Ordering::SeqCst));
            assert!(
                Arc::into_inner(self.0).is_some(),
                "Another clone of our Arc exists somewhere!"
            );
            assert!(
                self.1.recv().is_err(),
                "Another clone of our sender exists somewhere!",
            );
        }
    }

    struct DropSignaller(Arc<AtomicUsize>, Sender<()>);
    impl Drop for DropSignaller {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
            self.1.send(()).unwrap();
        }
    }
}
