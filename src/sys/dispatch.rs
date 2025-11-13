use std::ffi::{CString, c_void};
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use dispatchr::data::dispatch_release;
use dispatchr::qos::QoS;
use dispatchr::queue;
use dispatchr::queue::Unmanaged;
use dispatchr::semaphore::Managed;
use dispatchr::source::{Managed as DSource, dispatch_source_type_t as DSrcTy};
use dispatchr::time::Time;
use futures_task::{ArcWake, waker};
use nix::errno::Errno;
use nix::libc::pid_t;
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;

use crate::common::collections::HashMap;

const DISPATCH_PROC_EXIT: usize = 0x8000_0000;

struct NamedQueueHandle {
    queue: *mut Unmanaged,
    _label: CString,
}

unsafe impl Send for NamedQueueHandle {}
unsafe impl Sync for NamedQueueHandle {}

impl Drop for NamedQueueHandle {
    fn drop(&mut self) { unsafe { dispatch_release(self.queue as *const c_void) }; }
}

static NAMED_QUEUES: OnceCell<Mutex<Vec<Box<NamedQueueHandle>>>> = OnceCell::new();
fn named_queue_registry() -> &'static Mutex<Vec<Box<NamedQueueHandle>>> {
    NAMED_QUEUES.get_or_init(|| Mutex::new(Vec::new()))
}

pub trait NamedQueueExt {
    fn named(label: &str) -> Option<&'static Unmanaged>;
}

impl NamedQueueExt for Unmanaged {
    fn named(label: &str) -> Option<&'static Unmanaged> {
        let cname = CString::new(label).ok()?;
        let queue = unsafe { dispatch_queue_create(cname.as_ptr(), std::ptr::null_mut()) };
        if queue.is_null() {
            return None;
        }

        let queue_ref = unsafe { &*queue };
        named_queue_registry()
            .lock()
            .push(Box::new(NamedQueueHandle { queue, _label: cname }));
        Some(queue_ref)
    }
}

static Q_REAPER: OnceCell<&'static queue::Unmanaged> = OnceCell::new();
fn reaper_queue() -> &'static queue::Unmanaged {
    Q_REAPER.get_or_init(|| queue::global(QoS::Utility).unwrap_or_else(|| queue::main()))
}

static SOURCES: OnceCell<Mutex<HashMap<pid_t, DSource>>> = OnceCell::new();
fn sources_map() -> &'static Mutex<HashMap<pid_t, DSource>> {
    SOURCES.get_or_init(|| Mutex::new(HashMap::default()))
}

unsafe extern "C" {
    static _dispatch_source_type_proc: c_void;
    static _dispatch_source_type_timer: c_void;

    fn dispatch_after_f(
        when: Time,
        queue: *const Unmanaged,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );

    fn dispatch_set_context(object: *mut c_void, context: *mut c_void);

    fn dispatch_source_set_timer(source: *mut c_void, start: Time, interval: i64, leeway: i64);

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
    fn set_context(&self, context: *mut c_void);
    fn set_timer(&self, start: Time, interval: i64, leeway: i64);
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

    fn set_context(&self, context: *mut c_void) {
        unsafe { dispatch_set_context(self as *const _ as *mut c_void, context) }
    }

    fn set_timer(&self, start: Time, interval: i64, leeway: i64) {
        unsafe {
            dispatch_source_set_timer(self as *const _ as *mut c_void, start, interval, leeway)
        }
    }
}

impl DispatchExt for DSource {
    fn after_f(&self, when: Time, context: *mut c_void, work: extern "C" fn(*mut c_void)) {
        unsafe {
            dispatch_after_f(when, self.deref() as *const _ as *const Unmanaged, context, work)
        }
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

    fn set_context(&self, context: *mut c_void) {
        unsafe { dispatch_set_context(self.deref() as *const _ as *mut c_void, context) }
    }

    fn set_timer(&self, start: Time, interval: i64, leeway: i64) {
        unsafe {
            dispatch_source_set_timer(
                self.deref() as *const _ as *mut c_void,
                start,
                interval,
                leeway,
            )
        }
    }
}

pub fn block_on<T: 'static>(
    mut fut: r#continue::Future<T>,
    timeout: Duration,
) -> Result<T, String> {
    struct GcdWaker {
        sem: Managed,
    }
    impl ArcWake for GcdWaker {
        fn wake_by_ref(this: &Arc<Self>) { this.sem.signal(); }
    }

    let sem = Managed::new(0);
    let waker: Waker = waker(Arc::new(GcdWaker { sem: sem.clone() }));
    let mut cx = Context::from_waker(&waker);

    let deadline = Instant::now() + timeout;

    loop {
        match Pin::new(&mut fut).poll(&mut cx) {
            Poll::Ready(v) => return Ok(v),
            Poll::Pending => {
                let now = Instant::now();
                if now >= deadline {
                    return Err("Timeout".into());
                }

                let remaining = deadline - now;
                let ns = i64::try_from(remaining.as_nanos()).unwrap_or(i64::MAX);
                let t = Time::NOW.new_after(ns);

                if sem.wait(t) != 0 {
                    return Err("Timeout".into());
                }
            }
        }
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
        match waitpid(Pid::from_raw(pid), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::StillAlive) => {}
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

    src.set_context(ctx);
    src.set_event_handler_f(proc_event_handler);
    src.resume();
    sources_map().lock().insert(pid, src);
}
