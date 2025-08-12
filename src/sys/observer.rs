use std::borrow::Cow;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ptr;

use accessibility::AXUIElement;
use accessibility_sys::{
    AXError, AXObserverAddNotification, AXObserverCreate, AXObserverGetRunLoopSource,
    AXObserverGetTypeID, AXObserverRef, AXObserverRemoveNotification, AXUIElementRef,
    kAXErrorSuccess, pid_t,
};
use core_foundation::base::TCFType;
use core_foundation::runloop::{CFRunLoopAddSource, CFRunLoopGetCurrent, kCFRunLoopCommonModes};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::{declare_TCFType, impl_TCFType};

declare_TCFType!(AXObserver, AXObserverRef);
impl_TCFType!(AXObserver, AXObserverRef, AXObserverGetTypeID);

/// An observer for accessibility events.
pub struct Observer {
    callback: *mut (),
    dtor: unsafe fn(*mut ()),
    observer: ManuallyDrop<AXObserver>,
}

static_assertions::assert_not_impl_any!(Observer: Send);

/// Helper type for building an [`Observer`].
//
// This type exists to carry type information about our callback `F` to the call
// to `new` from the call to `install`. It exists because of the following
// constraints:
//
// * Creating the observer object can fail, e.g. if the app in question is no
//   longer running.
// * The `Observer` often needs to go inside an object that is also referenced
//   by the callback. This necessitates the use of APIs like
//   [`std::rc::Rc::make_cyclic`], which unfortunately is not fallible.
// * `Observer` should not know about the type of its callback, both because
//   that type usually cannot be named and for convenience.
// * We want to avoid double indirection on calls to the callback, which
//   necessitates knowing the type of `F` when creating the system observer
//   object during the call to `new`.
//
// This means we make creation of the Observer a two-step process. `new` can
// fail and can be called before the call to `make_cyclic`. `install` is
// infallible and can be called inside, meaning the callback passed to it can
// capture a weak pointer to our object.
pub struct ObserverBuilder<F>(AXObserver, PhantomData<F>);

impl Observer {
    /// Creates a new observer for an app, given its `pid`.
    ///
    /// Note that you must call [`ObserverBuilder::install`] on the result of
    /// this function and supply a callback for the observer to have any effect.
    pub fn new<F: Fn(AXUIElement, &str) + 'static>(
        pid: pid_t,
    ) -> Result<ObserverBuilder<F>, accessibility::Error> {
        // SAFETY: We just create an observer here, and check the return code.
        // The callback cannot be called yet. The API guarantees that F will be
        // supplied as the callback in the call to install (and the 'static
        // bound on F means we don't need to worry about variance).
        let mut observer: AXObserverRef = ptr::null_mut();
        unsafe {
            make_result(AXObserverCreate(pid, internal_callback::<F>, &mut observer))?;
        }
        Ok(ObserverBuilder(
            unsafe { AXObserver::wrap_under_create_rule(observer) },
            PhantomData,
        ))
    }
}

impl<F: Fn(AXUIElement, &str) + 'static> ObserverBuilder<F> {
    /// Installs the observer with the supplied callback into the current
    /// thread's run loop.
    pub fn install(self, callback: F) -> Observer {
        // SAFETY: We know from typestate that the observer will call
        // internal_callback::<F>. F is 'static, so even if our destructor is
        // not run it will remain valid to call.
        unsafe {
            let source = AXObserverGetRunLoopSource(self.0.as_concrete_TypeRef());
            CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
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

impl Observer {
    pub fn add_notification(
        &self,
        elem: &AXUIElement,
        notification: &'static str,
    ) -> Result<(), accessibility::Error> {
        make_result(unsafe {
            AXObserverAddNotification(
                self.observer.as_concrete_TypeRef(),
                elem.as_concrete_TypeRef(),
                CFString::from_static_string(notification).as_concrete_TypeRef(),
                self.callback as *mut c_void,
            )
        })
    }

    pub fn remove_notification(
        &self,
        elem: &AXUIElement,
        notification: &'static str,
    ) -> Result<(), accessibility::Error> {
        make_result(unsafe {
            AXObserverRemoveNotification(
                self.observer.as_concrete_TypeRef(),
                elem.as_concrete_TypeRef(),
                CFString::from_static_string(notification).as_concrete_TypeRef(),
            )
        })
    }
}

unsafe extern "C" fn internal_callback<F: Fn(AXUIElement, &str) + 'static>(
    _observer: AXObserverRef,
    elem: AXUIElementRef,
    notif: CFStringRef,
    data: *mut c_void,
) {
    let callback = unsafe { &*(data as *const F) };
    let elem = unsafe { AXUIElement::wrap_under_get_rule(elem) };
    let notif = unsafe { CFString::wrap_under_get_rule(notif) };
    let notif = Cow::<str>::from(&notif);
    callback(elem, &*notif);
}

fn make_result(err: AXError) -> Result<(), accessibility::Error> {
    if err != kAXErrorSuccess {
        return Err(accessibility::Error::Ax(err));
    }
    Ok(())
}
