use std::ffi::{CStr, c_void};
use std::ops::Deref;

use dispatchr::data::dispatch_release;
use dispatchr::queue::Unmanaged;
use dispatchr::source::{Managed as DSource, dispatch_source_type_t as DSrcTy};
use dispatchr::time::Time;
use nix::errno::Errno;
use nix::libc::pid_t;
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;

use crate::common::collections::HashMap;

const DISPATCH_PROC_EXIT: usize = 0x8000_0000;

struct SerialQueue(*mut Unmanaged);

// Dispatch queues are thread-safe; each static owns its queue for the process lifetime.
unsafe impl Send for SerialQueue {}
unsafe impl Sync for SerialQueue {}

impl SerialQueue {
    fn new(label: &CStr, qos: u32) -> Self {
        let queue = unsafe {
            let attr = dispatch_queue_attr_make_with_qos_class(std::ptr::null_mut(), qos, 0);
            dispatch_queue_create(label.as_ptr(), attr)
        };
        assert!(!queue.is_null(), "dispatch queue creation failed");
        Self(queue)
    }
}

impl Deref for SerialQueue {
    type Target = Unmanaged;

    fn deref(&self) -> &Unmanaged { unsafe { &*self.0 } }
}

impl Drop for SerialQueue {
    fn drop(&mut self) { unsafe { dispatch_release(self.0.cast()) }; }
}

/// Rare partial-lift deadlines, never per-frame recognition. Isolated from child reaping.
pub fn gesture_deadline_queue() -> &'static Unmanaged {
    static QUEUE: OnceCell<SerialQueue> = OnceCell::new();
    QUEUE.get_or_init(|| SerialQueue::new(c"rift.gesture-deadlines", 0x21)) // UserInteractive
}

fn reaper_queue() -> &'static Unmanaged {
    static QUEUE: OnceCell<SerialQueue> = OnceCell::new();
    QUEUE.get_or_init(|| SerialQueue::new(c"rift.child-reaper", 0x11)) // Utility
}

static SOURCES: OnceCell<Mutex<HashMap<pid_t, DSource>>> = OnceCell::new();
fn sources_map() -> &'static Mutex<HashMap<pid_t, DSource>> {
    SOURCES.get_or_init(|| Mutex::new(HashMap::default()))
}

unsafe extern "C" {
    static _dispatch_source_type_proc: c_void;

    fn dispatch_after_f(
        when: Time,
        queue: *const Unmanaged,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );

    fn dispatch_set_context(object: *mut c_void, context: *mut c_void);

    fn dispatch_queue_attr_make_with_qos_class(
        attr: *mut c_void,
        qos: u32,
        relative_priority: i32,
    ) -> *mut c_void;

    fn dispatch_queue_create(label: *const i8, attr: *mut c_void) -> *mut Unmanaged;
}

#[inline]
fn dispatch_source_type_proc() -> DSrcTy {
    // SAFETY: dispatchr::source::dispatch_source_type_t is repr(transparent) over a pointer
    unsafe {
        let p = &_dispatch_source_type_proc as *const _ as *const c_void;
        std::mem::transmute::<*const c_void, DSrcTy>(p)
    }
}

pub trait DispatchExt {
    fn after_f(&self, when: Time, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn after_f_s<T>(&self, when: Time, context: T, work: fn(T));
}

impl DispatchExt for Unmanaged {
    fn after_f(&self, when: Time, context: *mut c_void, work: extern "C" fn(*mut c_void)) {
        unsafe { dispatch_after_f(when, self, context, work) }
    }

    fn after_f_s<T>(&self, when: Time, context: T, work: fn(T)) {
        extern "C" fn trampoline<T>(ctx: *mut c_void) {
            let ctx = unsafe { Box::from_raw(ctx as *mut (T, fn(T))) };
            let (context, work) = *ctx;
            work(context);
        }
        let ctx = Box::into_raw(Box::new((context, work))) as *mut c_void;
        self.after_f(when, ctx, trampoline::<T>);
    }
}

pub fn reap_on_exit_proc(pid: pid_t) {
    if pid <= 0 {
        return;
    }
    let q = reaper_queue();
    let tipe = dispatch_source_type_proc();

    let src = DSource::create(tipe, pid as _, DISPATCH_PROC_EXIT as _, q);
    let ctx = Box::into_raw(Box::new(pid)) as *mut c_void;
    extern "C" fn proc_event_handler(ctx: *mut c_void) {
        let pid = unsafe { *(ctx as *mut pid_t) };
        let status = match waitpid(Pid::from_raw(pid), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::StillAlive) => waitpid(Pid::from_raw(pid), None),
            other => other,
        };
        match status {
            Ok(WaitStatus::Exited(p, _)) | Ok(WaitStatus::Signaled(p, _, _)) => {
                let raw = p.as_raw();
                if let Some(_src) = sources_map().lock().remove(&raw) {
                    // drop -> dispatch_release; source is gone
                }
                let _ = unsafe { Box::from_raw(ctx as *mut pid_t) };
            }
            Ok(_) | Err(Errno::ECHILD) | Err(_) => {}
        }
    }

    unsafe { dispatch_set_context(src.deref() as *const _ as *mut c_void, ctx) };
    src.set_event_handler_f(proc_event_handler);
    let mut sources = sources_map().lock();
    sources.insert(pid, src);
    sources[&pid].resume();
}
