use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::{self, NonNull};
use std::rc::Rc;

use block2::RcBlock;
use objc2_application_services::{AXError, AXObserver, AXUIElement as RawAXUIElement};
use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopMode, CFRunLoopTimer, CFString,
    kCFRunLoopCommonModes,
};

use crate::sys::app::pid_t;
use crate::sys::axuielement::{AXUIElement, Error as AxError};

/// An observer for accessibility events.
pub struct Observer {
    callback: Callback,
    observer: Rc<CFRetained<AXObserver>>,
    subscription_ctx: RefCell<HashMap<NotificationKey, Rc<SubscriptionContext>>>,
}

static_assertions::assert_not_impl_any!(Observer: Send);

/// Helper type for building an [`Observer`].
pub struct ObserverBuilder<F>(CFRetained<AXObserver>, PhantomData<F>);

type NotificationKey = (NonNull<RawAXUIElement>, &'static str);

struct SubscriptionContext {
    callback: Callback,
    data: Cell<usize>,
    notification: &'static str,
    generation: Cell<u64>,
}

enum Callback {
    Data(Rc<dyn Fn(AXUIElement, usize)>),
    Notification(Rc<dyn Fn(AXUIElement, &'static str)>),
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
            AXObserver::create(pid, Some(internal_callback), NonNull::from(&mut observer_ptr))
        };
        make_result(status)?;
        // SAFETY: AXObserverCreate transfers ownership of a non-null result.
        let observer =
            unsafe { CFRetained::from_raw(NonNull::new(observer_ptr).ok_or(AxError::NotFound)?) };
        Ok(ObserverBuilder(observer, PhantomData))
    }

    /// Creates an observer whose callback receives the notification name.
    ///
    /// This is useful for APIs such as Dock's expose notifications where the
    /// notification itself is the event payload rather than an opaque callback
    /// value supplied at registration time.
    pub fn new_with_notification<F: Fn(AXUIElement, &'static str) + 'static>(
        pid: pid_t,
    ) -> Result<ObserverBuilder<F>, AxError> {
        let mut observer_ptr: *mut AXObserver = ptr::null_mut();
        let status = unsafe {
            AXObserver::create(
                pid,
                Some(internal_callback_with_notification),
                NonNull::from(&mut observer_ptr),
            )
        };
        make_result(status)?;
        // SAFETY: AXObserverCreate transfers ownership of a non-null result.
        let observer =
            unsafe { CFRetained::from_raw(NonNull::new(observer_ptr).ok_or(AxError::NotFound)?) };
        Ok(ObserverBuilder(observer, PhantomData))
    }
}

impl<F: Fn(AXUIElement, usize) + 'static> ObserverBuilder<F> {
    /// Installs the observer with the supplied callback into the current
    /// thread's run loop.
    pub fn install(self, callback: F) -> Observer {
        self.install_inner(Callback::Data(Rc::new(callback)))
    }
}

impl<F: Fn(AXUIElement, &'static str) + 'static> ObserverBuilder<F> {
    /// Installs a callback that receives the registered notification name.
    pub fn install_with_notification(self, callback: F) -> Observer {
        self.install_inner(Callback::Notification(Rc::new(callback)))
    }
}

impl<F> ObserverBuilder<F> {
    fn install_inner(self, callback: Callback) -> Observer {
        let run_loop_source = unsafe { self.0.run_loop_source() };
        if let Some(run_loop) = CFRunLoop::current() {
            let mode: &CFRunLoopMode =
                unsafe { kCFRunLoopCommonModes.expect("kCFRunLoopCommonModes") };
            run_loop.add_source(Some(run_loop_source.as_ref()), Some(mode));
        }
        Observer {
            callback,
            observer: Rc::new(self.0),
            subscription_ctx: RefCell::new(HashMap::new()),
        }
    }
}

impl Clone for Callback {
    fn clone(&self) -> Self {
        match self {
            Self::Data(callback) => Self::Data(Rc::clone(callback)),
            Self::Notification(callback) => Self::Notification(Rc::clone(callback)),
        }
    }
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
        callback_ctx: Rc<SubscriptionContext>,
    ) -> Result<(), AxError> {
        let notification_cf = CFString::from_static_str(notification);
        let observer: &AXObserver = &self.observer;
        let callback_data = Rc::as_ptr(&callback_ctx).cast_mut().cast();
        let first = unsafe {
            observer.add_notification(
                elem.as_concrete_TypeRef(),
                notification_cf.as_ref(),
                callback_data,
            )
        };
        if first == AXError::CannotComplete
            && let Some(run_loop) = CFRunLoop::current()
        {
            let observer = Rc::downgrade(&self.observer);
            let generation = callback_ctx.generation.get();
            let context = Rc::downgrade(&callback_ctx);
            let elem = elem.clone();
            let retry = RcBlock::new(move |_: *mut CFRunLoopTimer| {
                let (Some(observer), Some(context)) = (observer.upgrade(), context.upgrade())
                else {
                    return;
                };
                if context.generation.get() != generation {
                    return;
                }
                // SAFETY: Both objects belong to this run loop. The observer's map retains the
                // context until the observer is released.
                let status = unsafe {
                    observer.add_notification(
                        elem.as_concrete_TypeRef(),
                        notification_cf.as_ref(),
                        Rc::as_ptr(&context).cast_mut().cast(),
                    )
                };
                if status != AXError::Success && status != AXError::NotificationAlreadyRegistered {
                    tracing::debug!(?status, notification, "AX notification retry failed");
                }
            });
            // SAFETY: The one-shot timer retains the block and invokes it on the owner run loop.
            if let Some(timer) = unsafe {
                CFRunLoopTimer::with_handler(
                    None,
                    CFAbsoluteTimeGetCurrent() + 0.01,
                    0.,
                    0,
                    0,
                    Some(&retry),
                )
            } {
                // SAFETY: This is an immutable Core Foundation runtime constant.
                let mode = unsafe { kCFRunLoopCommonModes };
                run_loop.add_timer(Some(&timer), mode);
                return Ok(());
            }
        }
        make_result(first)
    }

    fn subscription_context(
        &self,
        elem: &AXUIElement,
        notification: &'static str,
        data: usize,
    ) -> Rc<SubscriptionContext> {
        let key = (elem.raw_ptr(), notification);
        let mut subscription_ctx = self.subscription_ctx.borrow_mut();
        let ctx = subscription_ctx.entry(key).or_insert_with(|| {
            Rc::new(SubscriptionContext {
                callback: self.callback.clone(),
                data: Cell::new(data),
                notification,
                generation: Cell::new(0),
            })
        });
        ctx.data.set(data);
        ctx.generation.set(ctx.generation.get().wrapping_add(1));
        Rc::clone(ctx)
    }

    pub fn remove_notification(
        &self,
        elem: &AXUIElement,
        notification: &'static str,
    ) -> Result<(), AxError> {
        let notification_cf = CFString::from_static_str(notification);
        let observer: &AXObserver = &self.observer;
        if let Some(ctx) = self.subscription_ctx.borrow().get(&(elem.raw_ptr(), notification)) {
            ctx.generation.set(ctx.generation.get().wrapping_add(1));
        }
        let result = make_result(unsafe {
            observer.remove_notification(elem.as_concrete_TypeRef(), notification_cf.as_ref())
        });
        // AXObserverRemoveNotification can succeed while a notification
        // carrying this refcon is already queued on the run loop. Keep the
        // context alive until the observer is dropped instead of freeing
        // memory that CoreFoundation may still pass back to us.
        result
    }
}

unsafe extern "C-unwind" fn internal_callback(
    _observer: NonNull<AXObserver>,
    elem: NonNull<RawAXUIElement>,
    _notif: NonNull<CFString>,
    data: *mut c_void,
) {
    let Some(ctx) = (unsafe { data.cast::<SubscriptionContext>().as_ref() }) else {
        return;
    };
    let elem = unsafe { AXUIElement::from_get_rule(elem.as_ptr()) };
    if let Callback::Data(callback) = &ctx.callback {
        callback(elem, ctx.data.get());
    }
}

unsafe extern "C-unwind" fn internal_callback_with_notification(
    _observer: NonNull<AXObserver>,
    elem: NonNull<RawAXUIElement>,
    _notif: NonNull<CFString>,
    data: *mut c_void,
) {
    let Some(ctx) = (unsafe { data.cast::<SubscriptionContext>().as_ref() }) else {
        return;
    };
    let elem = unsafe { AXUIElement::from_get_rule(elem.as_ptr()) };
    if let Callback::Notification(callback) = &ctx.callback {
        callback(elem, ctx.notification);
    }
}

fn make_result(err: AXError) -> Result<(), AxError> {
    if err == AXError::Success {
        Ok(())
    } else {
        Err(AxError::Ax(err))
    }
}
