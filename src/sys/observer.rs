use std::ffi::c_void;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ptr::{self, NonNull};

use dispatchr::queue;
use dispatchr::time::Time;
use objc2_application_services::{AXError, AXObserver, AXUIElement as RawAXUIElement};
use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopMode, CFString, kCFRunLoopCommonModes,
};

use crate::sys::app::pid_t;
use crate::sys::axuielement::{AXUIElement, Error as AxError};
use crate::sys::dispatch::DispatchExt;

/// An observer for accessibility events.
pub struct Observer {
    callback: *mut (),
    dtor: unsafe fn(*mut ()),
    observer: ManuallyDrop<CFRetained<AXObserver>>,
}

static_assertions::assert_not_impl_any!(Observer: Send);

/// Helper type for building an [`Observer`].
pub struct ObserverBuilder<F>(CFRetained<AXObserver>, PhantomData<F>);

impl Observer {
    /// Creates a new observer for an app, given its `pid`.
    ///
    /// Note that you must call [`ObserverBuilder::install`] on the result of
    /// this function and supply a callback for the observer to have any effect.
    pub fn new<F: Fn(AXUIElement, &str) + 'static>(
        pid: pid_t,
    ) -> Result<ObserverBuilder<F>, AxError> {
        let mut observer_ptr: *mut AXObserver = ptr::null_mut();
        let status = unsafe {
            AXObserver::create(
                pid,
                Some(internal_callback::<F>),
                NonNull::new(&mut observer_ptr as *mut *mut AXObserver).expect("nonnull pointer"),
            )
        };
        make_result(status)?;
        let observer = unsafe {
            CFRetained::from_raw(NonNull::new(observer_ptr).expect("observer must be non-null"))
        };
        Ok(ObserverBuilder(observer, PhantomData))
    }
}

impl<F: Fn(AXUIElement, &str) + 'static> ObserverBuilder<F> {
    /// Installs the observer with the supplied callback into the current
    /// thread's run loop.
    pub fn install(self, callback: F) -> Observer {
        let run_loop_source = unsafe { self.0.run_loop_source() };
        if let Some(run_loop) = CFRunLoop::current() {
            let mode: &CFRunLoopMode =
                unsafe { kCFRunLoopCommonModes.expect("kCFRunLoopCommonModes") };
            run_loop.add_source(Some(run_loop_source.as_ref()), Some(mode));
        }
        Observer {
            callback: Box::into_raw(Box::new(callback)) as *mut (),
            dtor: destruct::<F>,
            observer: ManuallyDrop::new(self.0),
        }
    }
}

unsafe fn destruct<T>(ptr: *mut ()) { let _ = unsafe { Box::from_raw(ptr as *mut T) }; }

impl Drop for Observer {
    fn drop(&mut self) {
        unsafe {
            ManuallyDrop::drop(&mut self.observer);
            (self.dtor)(self.callback);
        }
    }
}

struct AddNotifRetryCtx {
    observer: CFRetained<AXObserver>,
    elem: AXUIElement,
    notification: &'static str,
    callback: *mut c_void,
}

extern "C" fn add_notif_retry(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { Box::from_raw(ctx as *mut AddNotifRetryCtx) };
    let notification_cf = CFString::from_static_str(ctx.notification);
    let _ = unsafe {
        ctx.observer.add_notification(
            ctx.elem.as_concrete_TypeRef(),
            notification_cf.as_ref(),
            ctx.callback,
        )
    };
}

impl Observer {
    pub fn add_notification(
        &self,
        elem: &AXUIElement,
        notification: &'static str,
    ) -> Result<(), AxError> {
        let notification_cf = CFString::from_static_str(notification);
        let observer: &AXObserver = &self.observer;
        let first = unsafe {
            observer.add_notification(
                elem.as_concrete_TypeRef(),
                notification_cf.as_ref(),
                self.callback as *mut c_void,
            )
        };
        if make_result(first).is_ok() {
            return Ok(());
        }
        if first == AXError::CannotComplete {
            let retained_observer =
                unsafe { CFRetained::retain(CFRetained::as_ptr(&*self.observer)) };
            let ctx = Box::new(AddNotifRetryCtx {
                observer: retained_observer,
                elem: elem.clone(),
                notification,
                callback: self.callback as *mut c_void,
            });
            queue::main().after_f(
                Time::NOW.new_after(10_000_000),
                Box::into_raw(ctx) as *mut c_void,
                add_notif_retry,
            );
            return Ok(());
        }
        make_result(first)
    }

    pub fn remove_notification(
        &self,
        elem: &AXUIElement,
        notification: &'static str,
    ) -> Result<(), AxError> {
        let notification_cf = CFString::from_static_str(notification);
        let observer: &AXObserver = &self.observer;
        make_result(unsafe {
            observer.remove_notification(elem.as_concrete_TypeRef(), notification_cf.as_ref())
        })
    }
}

unsafe extern "C-unwind" fn internal_callback<F: Fn(AXUIElement, &str) + 'static>(
    _observer: NonNull<AXObserver>,
    elem: NonNull<RawAXUIElement>,
    notif: NonNull<CFString>,
    data: *mut c_void,
) {
    let callback = unsafe { &*(data as *const F) };
    let elem = unsafe { AXUIElement::from_get_rule(elem.as_ptr()) };
    let notif = unsafe { CFRetained::retain(notif) };
    let notif = notif.to_string();
    callback(elem, &notif);
}

fn make_result(err: AXError) -> Result<(), AxError> {
    if err == AXError::Success {
        Ok(())
    } else {
        Err(AxError::Ax(err))
    }
}
