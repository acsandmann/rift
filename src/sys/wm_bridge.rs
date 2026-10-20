//! SkyLight's bridged window management operations: moving windows between native
//! desktops and destroying desktops, without the scripting addition.
//!
//! The plain client calls (`SLSMoveWindowsToManagedSpace` and friends) only act on the
//! caller's own windows. The bridged operation objects do the same work through
//! `performWithWMBridgeDelegate`, which with no delegate set (see `rift.rs`) runs
//! SkyLight's fallback bridge, and that does act on other apps' windows provided the
//! caller is a running `NSApplication`. Measured on macOS 27 with SIP enabled: a window
//! of another process moved to a hidden desktop and back, and a desktop was created and
//! destroyed.
//!
//! The operations are asynchronous: the window server applies them a frame or two
//! later, so nothing should read the result back right away. Every function reports
//! whether the operation could be issued, not whether it took effect.

#[cfg(not(test))]
use objc2::rc::Retained;
#[cfg(not(test))]
use objc2::runtime::{AnyClass, AnyObject, Sel};
#[cfg(not(test))]
use objc2::{msg_send, sel};
#[cfg(not(test))]
use objc2_foundation::{NSArray, NSNumber};

use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

/// Moves `windows` onto `space`, off whichever desktops they are on.
pub fn move_windows_to_space(windows: &[WindowServerId], space: SpaceId) -> bool {
    if windows.is_empty() {
        return true;
    }
    #[cfg(test)]
    {
        test::record(test::Operation::Move(windows.to_vec(), space));
        true
    }
    #[cfg(not(test))]
    {
        let Some(class) = operation_class(
            c"SLSBridgedMoveWindowsToManagedSpaceOperation",
            sel!(initWithWindows:spaceID:),
        ) else {
            return false;
        };
        let numbers: Vec<Retained<NSNumber>> =
            windows.iter().map(|wsid| NSNumber::new_u32(wsid.as_u32())).collect();
        let windows = NSArray::from_retained_slice(&numbers);
        let operation: Option<Retained<AnyObject>> = unsafe {
            msg_send![msg_send![class, alloc], initWithWindows: &*windows, spaceID: space.get()]
        };
        operation.is_some_and(|operation| perform(&operation))
    }
}

/// Destroys the native desktop `space`. macOS moves any windows still on it to another
/// desktop of its display.
pub fn destroy_space(space: SpaceId) -> bool {
    #[cfg(test)]
    {
        test::record(test::Operation::Destroy(space));
        true
    }
    #[cfg(not(test))]
    {
        let Some(class) =
            operation_class(c"SLSBridgedSpaceDestroyOperation", sel!(initWithSpaceID:))
        else {
            return false;
        };
        let operation: Option<Retained<AnyObject>> =
            unsafe { msg_send![msg_send![class, alloc], initWithSpaceID: space.get()] };
        operation.is_some_and(|operation| perform(&operation))
    }
}

#[cfg(not(test))]
fn operation_class(name: &std::ffi::CStr, initializer: Sel) -> Option<&'static AnyClass> {
    let class = AnyClass::get(name)?;
    class.instance_method(initializer)?;
    Some(class)
}

#[cfg(not(test))]
fn perform(operation: &AnyObject) -> bool {
    if !operation.class().responds_to(sel!(performWithWMBridgeDelegate)) {
        return false;
    }
    let _: () = unsafe { msg_send![operation, performWithWMBridgeDelegate] };
    true
}

#[cfg(test)]
pub mod test {
    use std::cell::RefCell;

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Operation {
        Move(Vec<WindowServerId>, SpaceId),
        Destroy(SpaceId),
    }

    thread_local! {
        static OPERATIONS: RefCell<Vec<Operation>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn record(operation: Operation) {
        OPERATIONS.with(|operations| operations.borrow_mut().push(operation));
    }

    /// The operations issued on this thread since the last call.
    pub fn take() -> Vec<Operation> { OPERATIONS.with(|operations| operations.take()) }
}
