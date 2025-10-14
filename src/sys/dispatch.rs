use std::ffi::c_void;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

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

    pub fn dispatch_after_f(
        when: Time,
        queue: *const Unmanaged,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
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
}

impl DispatchExt for Unmanaged {
    fn after_f(&self, when: Time, context: *mut c_void, work: extern "C" fn(*mut c_void)) {
        unsafe { dispatch_after_f(when, self, context, work) }
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

    extern "C" fn proc_event_handler(_ctx: *mut c_void) {
        loop {
            match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::StillAlive) => break,
                Ok(WaitStatus::Exited(p, _)) | Ok(WaitStatus::Signaled(p, _, _)) => {
                    let raw = p.as_raw();
                    if let Some(_src) = sources_map().lock().remove(&raw) {
                        // drop -> dispatch_release; source is gone
                    }
                    continue;
                }
                Ok(_) | Err(Errno::ECHILD) | Err(_) => break,
            }
        }
    }

    src.set_event_handler_f(proc_event_handler);
    src.resume();

    sources_map().lock().insert(pid, src);
}
