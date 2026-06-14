use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ptr::{self, NonNull};
use std::sync::Arc;

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
    subscription_ctx: RefCell<HashMap<NotificationKey, Arc<SubscriptionContext>>>,
}

static_assertions::assert_not_impl_any!(Observer: Send);

/// Helper type for building an [`Observer`].
pub struct ObserverBuilder<F>(CFRetained<AXObserver>, PhantomData<F>);

type NotificationKey = (NonNull<RawAXUIElement>, &'static str);

struct SubscriptionContext {
    callback: *mut c_void,
    data: Cell<usize>,
}

impl Observer {
    /// Creates a new observer for an app, given its `pid`.
    ///
    /// Note that you must call [`ObserverBuilder::install`] on the result of
    /// this function and supply a callback for the observer to have any effect.
    pub fn new<F: Fn(AXUIElement, usize) + 'static>(
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

impl<F: Fn(AXUIElement, usize) + 'static> ObserverBuilder<F> {
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
            subscription_ctx: RefCell::new(HashMap::new()),
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
    callback_ctx: Arc<SubscriptionContext>,
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
            Arc::as_ptr(&ctx.callback_ctx).cast_mut().cast(),
        )
    };
}

impl Observer {
    pub fn add_notification(
        &self,
        elem: &AXUIElement,
        notification: &'static str,
    ) -> Result<(), AxError> {
        self.add_notification_with_data(elem, notification, 0)
    }

    pub fn add_notification_with_data(
        &self,
        elem: &AXUIElement,
        notification: &'static str,
        data: usize,
    ) -> Result<(), AxError> {
        let callback_ctx = self.subscription_context(elem, notification, data);
        self.add_notification_inner(elem, notification, callback_ctx)
    }

    fn add_notification_inner(
        &self,
        elem: &AXUIElement,
        notification: &'static str,
        callback_ctx: Arc<SubscriptionContext>,
    ) -> Result<(), AxError> {
        let notification_cf = CFString::from_static_str(notification);
        let observer: &AXObserver = &self.observer;
        let callback_data = Arc::as_ptr(&callback_ctx).cast_mut().cast();
        let first = unsafe {
            observer.add_notification(
                elem.as_concrete_TypeRef(),
                notification_cf.as_ref(),
                callback_data,
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
                callback_ctx,
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

    fn subscription_context(
        &self,
        elem: &AXUIElement,
        notification: &'static str,
        data: usize,
    ) -> Arc<SubscriptionContext> {
        let key = (elem.raw_ptr(), notification);
        let mut subscription_ctx = self.subscription_ctx.borrow_mut();
        let ctx = subscription_ctx.entry(key).or_insert_with(|| {
            Arc::new(SubscriptionContext {
                callback: self.callback as *mut c_void,
                data: Cell::new(data),
            })
        });
        ctx.data.set(data);
        Arc::clone(ctx)
    }

    pub fn remove_notification(
        &self,
        elem: &AXUIElement,
        notification: &'static str,
    ) -> Result<(), AxError> {
        let notification_cf = CFString::from_static_str(notification);
        let observer: &AXObserver = &self.observer;
        let result = make_result(unsafe {
            observer.remove_notification(elem.as_concrete_TypeRef(), notification_cf.as_ref())
        });
        if result.is_ok() {
            self.subscription_ctx.borrow_mut().remove(&(elem.raw_ptr(), notification));
        }
        result
    }
}

unsafe extern "C-unwind" fn internal_callback<F: Fn(AXUIElement, usize) + 'static>(
    _observer: NonNull<AXObserver>,
    elem: NonNull<RawAXUIElement>,
    _notif: NonNull<CFString>,
    data: *mut c_void,
) {
    let ctx = unsafe { &*(data as *const SubscriptionContext) };
    let callback = unsafe { &*(ctx.callback as *const F) };
    let elem = unsafe { AXUIElement::from_get_rule(elem.as_ptr()) };
    callback(elem, ctx.data.get());
}

fn make_result(err: AXError) -> Result<(), AxError> {
    if err == AXError::Success {
        Ok(())
    } else {
        Err(AxError::Ax(err))
    }
}
