// credits
// https://github.com/asmagill/hs._asm.undocumented.spaces/blob/master/CGSSpace.h.
// https://github.com/koekeishiya/yabai/blob/d55a647913ab72d8d8b348bee2d3e59e52ce4a5d/src/misc/extern.h.

use std::ffi::{c_int, c_uint, c_void};
use std::fmt::Display;

use accessibility_sys::{AXError, AXUIElementRef};
use bitflags::bitflags;
use core_foundation::base::CFTypeRef;
use core_foundation::string::CFStringRef;
use core_graphics::base::CGError;
use core_graphics::display::{CFArrayRef, CGWindowID};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::NSArray;
use once_cell::sync::Lazy;

use super::process::ProcessSerialNumber;

pub static G_CONNECTION: Lazy<cid_t> = Lazy::new(|| unsafe { SLSMainConnectionID() });

#[allow(non_camel_case_types)]
pub type cid_t = i32;

#[repr(u32)]
#[derive(Hash, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CGSEventType {
    WindowDestroyed = 804,
    WindowMoved = 806,
    WindowResized = 807,
    WindowCreated = 811,
    // All = 0xFFFF_FFFF,
}

impl From<CGSEventType> for u32 {
    fn from(e: CGSEventType) -> Self { e as u32 }
}

impl std::convert::TryFrom<u32> for CGSEventType {
    type Error = u32;

    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            804 => Ok(CGSEventType::WindowDestroyed),
            806 => Ok(CGSEventType::WindowMoved),
            807 => Ok(CGSEventType::WindowResized),
            811 => Ok(CGSEventType::WindowCreated),
            other => Err(other),
        }
    }
}

impl Display for CGSEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CGSEventType::WindowDestroyed => write!(f, "WindowDestroyed"),
            CGSEventType::WindowMoved => write!(f, "WindowMoved"),
            CGSEventType::WindowResized => write!(f, "WindowResized"),
            CGSEventType::WindowCreated => write!(f, "WindowCreated"),
        }
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    #[repr(transparent)]
    pub struct CGSSpaceMask: c_int {
        const INCLUDE_CURRENT = 1 << 0;
        const INCLUDE_OTHERS  = 1 << 1;

        const INCLUDE_USER    = 1 << 2;
        const INCLUDE_OS      = 1 << 3;

        const VISIBLE         = 1 << 16;

        const CURRENT_SPACES = Self::INCLUDE_USER.bits() | Self::INCLUDE_CURRENT.bits();
        const OTHER_SPACES = Self::INCLUDE_USER.bits() | Self::INCLUDE_OTHERS.bits();
        const ALL_SPACES =
            Self::INCLUDE_USER.bits() | Self::INCLUDE_OTHERS.bits() | Self::INCLUDE_CURRENT.bits();

        const ALL_VISIBLE_SPACES = Self::ALL_SPACES.bits() | Self::VISIBLE.bits();

        const CURRENT_OS_SPACES = Self::INCLUDE_OS.bits() | Self::INCLUDE_CURRENT.bits();
        const OTHER_OS_SPACES = Self::INCLUDE_OS.bits() | Self::INCLUDE_OTHERS.bits();
        const ALL_OS_SPACES =
            Self::INCLUDE_OS.bits() | Self::INCLUDE_OTHERS.bits() | Self::INCLUDE_CURRENT.bits();
    }
}

unsafe extern "C" {
    pub fn _AXUIElementGetWindow(elem: AXUIElementRef, wid: *mut CGWindowID) -> AXError;

    pub fn CGSGetWindowBounds(cid: cid_t, wid: u32, frame: *mut CGRect) -> i32;
    pub fn CGSMainConnectionID() -> cid_t;
    pub fn CGSSetConnectionProperty(
        cid: cid_t,
        target_cid: cid_t,
        key: CFStringRef,
        value: CFTypeRef,
    ) -> CGError;
    pub fn CGSGetActiveSpace(cid: c_int) -> u64;
    pub fn CGSCopySpaces(cid: c_int, mask: CGSSpaceMask) -> CFArrayRef;
    pub fn CGSCopyManagedDisplays(cid: c_int) -> CFArrayRef;
    pub fn CGSCopyManagedDisplaySpaces(cid: c_int) -> *mut NSArray;
    pub fn CGSManagedDisplayGetCurrentSpace(cid: c_int, uuid: CFStringRef) -> u64;
    pub fn CGSCopyBestManagedDisplayForRect(cid: c_int, rect: CGRect) -> CFStringRef;

    pub fn SLSMainConnectionID() -> cid_t;
    pub fn SLSDisableUpdate(cid: cid_t) -> i32;
    pub fn SLSReenableUpdate(cid: cid_t) -> i32;
    pub fn _SLPSSetFrontProcessWithOptions(
        psn: *const ProcessSerialNumber,
        wid: u32,
        mode: u32,
    ) -> CGError;
    pub fn SLPSPostEventRecordTo(psn: *const ProcessSerialNumber, bytes: *const u8) -> CGError;
    pub fn SLSFindWindowAndOwner(
        cid: c_int,
        zero: c_int,
        one: c_int,
        zero_again: c_int,
        screen_point: *mut CGPoint,
        window_point: *mut CGPoint,
        wid: *mut u32,
        wcid: *mut c_int,
    ) -> i32;
    pub fn SLSRegisterConnectionNotifyProc(
        cid: cid_t,
        callback: extern "C" fn(CGSEventType, *mut c_void, usize, *mut c_void, cid_t),
        event: CGSEventType,
        data: *mut c_void,
    ) -> i32;
    pub fn SLSRequestNotificationsForWindows(
        cid: cid_t,
        window_list: *const u32,
        window_count: i32,
    ) -> i32;
    pub fn SLSCopyWindowsWithOptionsAndTags(
        cid: c_int,
        owner: c_uint,
        spaces: CFArrayRef,
        options: c_uint,
        set_tags: *mut u64,
        clear_tags: *mut u64,
    ) -> CFArrayRef;

    pub fn SLSWindowQueryWindows(cid: c_int, windows: CFArrayRef, count: c_int) -> CFTypeRef;
    pub fn SLSWindowQueryResultCopyWindows(query: CFTypeRef) -> CFTypeRef;

    pub fn SLSWindowIteratorAdvance(iterator: CFTypeRef) -> bool;
    pub fn SLSWindowIteratorGetParentID(iterator: CFTypeRef) -> u32;
    pub fn SLSWindowIteratorGetWindowID(iterator: CFTypeRef) -> u32;
    pub fn SLSWindowIteratorGetTags(iterator: CFTypeRef) -> u64;
    pub fn SLSWindowIteratorGetAttributes(iterator: CFTypeRef) -> u64;
    pub fn SLSWindowIteratorGetLevel(iterator: CFTypeRef) -> c_int;
}
