// credits
// https://github.com/asmagill/hs._asm.undocumented.spaces/blob/master/CGSSpace.h.
// https://github.com/koekeishiya/yabai/blob/d55a647913ab72d8d8b348bee2d3e59e52ce4a5d/src/misc/extern.h.

use std::ffi::{c_int, c_uint, c_void};
use std::fmt;
use std::ops::BitAnd;

use bitflags::bitflags;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{
    CFArray, CFData, CFDictionary, CFNumber, CFString, CFType, CGPoint, CGRect,
};
use objc2_core_graphics::{CGContext, CGError, CGImage, CGWindowID};
use objc2_foundation::NSArray;
use once_cell::sync::Lazy;

use super::process::ProcessSerialNumber;
use crate::sys::screen::SpaceId;

pub static G_CONNECTION: Lazy<cid_t> = Lazy::new(|| unsafe { SLSMainConnectionID() });

#[allow(non_camel_case_types)]
pub type cid_t = i32;

#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoPrimitive, TryFromPrimitive)]
pub enum SLSWindowTags {
    Document = 1 << 0,
    Floating = 1 << 1,
    Attached = 1 << 7,
    Sticky = 1 << 11,
    IgnoresCycle = 1 << 18,
    Modal = 1 << 31,
}

impl BitAnd for SLSWindowTags {
    type Output = u64;

    fn bitand(self, rhs: Self) -> Self::Output { self as u64 & rhs as u64 }
}

impl BitAnd<SLSWindowTags> for u64 {
    type Output = u64;

    fn bitand(self, rhs: SLSWindowTags) -> Self::Output { self & (rhs as u64) }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoPrimitive, TryFromPrimitive)]
pub enum KnownCGSEvent {
    DisplayWillSleep = 102,
    DisplayDidWake = 103,
    WindowUpdated = 723,
    // maybe loginwindow active? kCGSEventNotificationSystemDefined = 724,
    WindowClosed = 804,
    WindowMoved = 806,
    WindowResized = 807,
    WindowReordered = 808,
    WindowLevelChanged = 811,
    WindowUnhidden = 815,
    WindowHidden = 816,
    MissionControlEntered = 1204,
    WindowTitleChanged = 1322,
    SpaceWindowCreated = 1325,
    SpaceWindowDestroyed = 1326,
    SpaceCreated = 1327,
    SpaceDestroyed = 1328,
    WorkspaceWillChange = 1400,
    WorkspaceDidChange = 1401,
    WorkspaceWindowIsViewable = 1402,
    WorkspaceWindowIsNotViewable = 1403,
    WorkspaceWindowDidMove = 1404,
    WorkspacePrefsDidChange = 1405,
    WorkspacesWindowDragDidStart = 1411,
    WorkspacesWindowDragDidEnd = 1412,
    WorkspacesWindowDragWillEnd = 1413,
    WorkspacesShowSpaceForProcess = 1414,
    WorkspacesWindowDidOrderInOnNonCurrentManagedSpacesOnly = 1415,
    WorkspacesWindowDidOrderOutOnNonCurrentManagedSpaces = 1416,
    FrontmostApplicationChanged = 1508,
    TransitionDidFinish = 1700,
    All = 0xFFFF_FFFF,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CGSEventType {
    Known(KnownCGSEvent),
    Unknown(u32),
}

impl From<u32> for CGSEventType {
    fn from(v: u32) -> Self {
        match KnownCGSEvent::try_from(v) {
            Ok(k) => Self::Known(k),
            Err(_) => Self::Unknown(v),
        }
    }
}
impl From<CGSEventType> for u32 {
    fn from(k: CGSEventType) -> u32 {
        match k {
            CGSEventType::Known(k) => k as u32,
            CGSEventType::Unknown(v) => v,
        }
    }
}

impl fmt::Display for KnownCGSEvent {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { fmt::Debug::fmt(self, f) }
}

impl fmt::Display for CGSEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CGSEventType::Known(k) => write!(f, "{k}"),
            CGSEventType::Unknown(v) => write!(f, "Unknown({v})"),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub enum CGEventTapLocation {
    HID,
    Session,
    AnnotatedSession,
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

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    #[repr(transparent)]
    pub struct DisplayReconfigFlags: u32 {
        const BEGIN_CONFIGURATION        = 0x0000_0001;
        const MOVED                      = 0x0000_0002;
        const SET_MAIN                   = 0x0000_0004;
        const SET_MODE                   = 0x0000_0008;
        const ADD                        = 0x0000_0010;
        const REMOVE                     = 0x0000_0020;
        const ENABLED                    = 0x0000_0040;
        const DISABLED                   = 0x0000_0080;
        const MIRROR                     = 0x0000_0100;
        const UNMIRROR                   = 0x0000_0200;
        const DESKTOP_SHAPE_CHANGED      = 0x0000_1000;
    }
}

unsafe extern "C" {
    #[allow(clashing_extern_declarations)]
    pub fn CFRelease(cf: *mut CFType);
    pub fn CGRectMakeWithDictionaryRepresentation(
        dict: *mut CFDictionary,
        rect: *mut CGRect,
    ) -> bool;

    pub fn _AXUIElementGetWindow(elem: *mut AXUIElement, wid: *mut CGWindowID) -> AXError;
    pub fn _AXUIElementCreateWithRemoteToken(data: *mut CFData) -> *mut AXUIElement;

    pub fn CGEventCreate(source: *mut CFType) -> *mut CFType;
    pub fn CGEventSetIntegerValueField(event: *mut CFType, field: u32, value: i64);
    pub fn CGEventSetDoubleValueField(event: *mut CFType, field: u32, value: f64);
    pub fn CGEventPost(tapLocation: CGEventTapLocation, event: *mut CFType);
    pub fn CGWarpMouseCursorPosition(point: CGPoint) -> CGError;

    pub fn CGSGetWindowBounds(cid: cid_t, wid: u32, frame: *mut CGRect) -> i32;
    pub fn CGSSetConnectionProperty(
        cid: cid_t,
        target_cid: cid_t,
        key: *mut CFString,
        value: *mut CFType,
    ) -> CGError;
    pub fn CGSGetActiveSpace(cid: c_int) -> u64;
    pub fn CGSCopySpaces(cid: c_int, mask: CGSSpaceMask) -> *mut CFArray<SpaceId>;
    pub fn CGSCopyManagedDisplays(cid: c_int) -> *mut CFArray;
    pub fn CGSCopyManagedDisplaySpaces(cid: c_int) -> *mut NSArray;
    pub fn SLSGetSpaceManagementMode(cid: cid_t) -> c_int;
    pub fn CGSManagedDisplayGetCurrentSpace(cid: c_int, uuid: *mut CFString) -> u64;
    pub fn CGSCopyBestManagedDisplayForRect(cid: c_int, rect: CGRect) -> *mut CFString;
    pub fn CGDisplayCreateUUIDFromDisplayID(did: u32) -> *mut CFType;
    pub fn CFUUIDCreateString(allocator: *mut c_void, uuid: *mut CFType) -> *mut CFString;
    pub fn CGDisplayRegisterReconfigurationCallback(
        callback: Option<unsafe extern "C" fn(u32, u32, *mut c_void)>,
        user_info: *mut c_void,
    );
    pub fn CGDisplayRemoveReconfigurationCallback(
        callback: Option<unsafe extern "C" fn(u32, u32, *mut c_void)>,
        user_info: *mut c_void,
    );

    pub fn SLSMainConnectionID() -> cid_t;
    pub fn SLSDisableUpdate(cid: cid_t) -> i32;
    pub fn SLSReenableUpdate(cid: cid_t) -> i32;
    pub fn _SLPSSetFrontProcessWithOptions(
        psn: *const ProcessSerialNumber,
        wid: u32,
        mode: u32,
    ) -> CGError;
    pub fn _SLPSGetFrontProcess(psn: *mut ProcessSerialNumber) -> CGError;
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
    pub fn SLSGetCurrentCursorLocation(cid: cid_t, point: *mut CGPoint) -> CGError;
    pub fn SLSWindowIsOrderedIn(cid: cid_t, wid: u32, ordered: *mut u8) -> CGError;
    pub fn SLSRegisterConnectionNotifyProc(
        cid: cid_t,
        callback: extern "C" fn(u32, *mut c_void, usize, *mut c_void, cid_t),
        event: u32,
        data: *mut c_void,
    ) -> i32;
    pub fn SLSRegisterNotifyProc(
        callback: extern "C" fn(u32, *mut c_void, usize, *mut c_void, cid_t),
        event: u32,
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
        spaces: *mut CFArray<CFNumber>,
        options: c_uint,
        set_tags: *mut u64,
        clear_tags: *mut u64,
    ) -> *mut CFArray<CFNumber>;
    pub fn SLSCopyAssociatedWindows(cid: cid_t, wid: u32) -> *mut CFArray<CFNumber>;
    pub fn SLSManagedDisplayGetCurrentSpace(cid: cid_t, uuid: *mut CFString) -> u64;
    pub fn SLSCopyActiveMenuBarDisplayIdentifier(cid: cid_t) -> *mut CFString;
    pub fn SLSSpaceGetType(cid: cid_t, sid: u64) -> c_int;

    pub fn SLSWindowQueryWindows(
        cid: c_int,
        windows: *mut CFArray<CFNumber>,
        count: c_int,
    ) -> *mut CFType;
    pub fn SLSWindowQueryResultCopyWindows(query: *mut CFType) -> *mut CFType;
    pub fn SLSGetWindowLevel(cid: cid_t, wid: u32, level: *mut i32) -> CGError;

    pub fn SLSWindowIteratorAdvance(iterator: *mut CFType) -> bool;
    pub fn SLSWindowIteratorGetParentID(iterator: *mut CFType) -> u32;
    pub fn SLSWindowIteratorGetWindowID(iterator: *mut CFType) -> u32;
    pub fn SLSWindowIteratorGetTags(iterator: *mut CFType) -> u64;
    pub fn SLSWindowIteratorGetAttributes(iterator: *mut CFType) -> u64;
    pub fn SLSWindowIteratorGetLevel(iterator: *mut CFType) -> c_int;
    pub fn SLSWindowIteratorGetCount(iterator: *mut CFType) -> c_int;
    pub fn SLSWindowIteratorGetAttachedWindowCount(iterator: *mut CFType) -> c_int;
    pub fn SLSWindowIteratorGetPID(iterator: *mut CFType) -> c_int;
    pub fn SLSWindowIteratorGetBounds(iterator: *mut CFType) -> CGRect;

    pub fn SLSCopySpacesForWindows(
        cid: cid_t,
        selector: u32,
        windows: *mut CFArray<CFNumber>,
    ) -> *mut CFArray<CFNumber>;

    pub fn SLSGetConnectionIDForPSN(
        cid: cid_t,
        psn: *const ProcessSerialNumber,
        out_cid: *mut c_int,
    ) -> c_int;

    pub fn SLSHWCaptureWindowList(
        cid: cid_t,
        window_list: *const u32,
        window_count: c_int,
        options: u32,
    ) -> *mut CFArray<CGImage>;

    pub fn SLSNewWindowWithOpaqueShapeAndContext(
        cid: cid_t,
        r#type: c_int,
        region: *mut CFType,
        opaque_region: *mut CFType,
        options: c_int,
        tags: *mut u64,
        x: f32,
        y: f32,
        tag_count: c_int,
        out_wid: *mut u32,
        context: *mut c_void,
    ) -> CGError;
    pub fn SLSReleaseWindow(cid: cid_t, wid: u32) -> CGError;
    pub fn SLSSetWindowResolution(cid: cid_t, wid: u32, resolution: f64) -> CGError;
    pub fn SLSSetWindowAlpha(cid: cid_t, wid: u32, alpha: f32) -> CGError;
    pub fn SLSSetWindowBackgroundBlurRadiusStyle(
        cid: cid_t,
        wid: u32,
        radius: c_int,
        style: c_int,
    ) -> CGError;
    pub fn SLSSetWindowBackgroundBlurRadius(cid: cid_t, wid: u32, radius: c_int) -> CGError;
    pub fn SLSSetWindowLevel(cid: cid_t, wid: u32, level: c_int) -> CGError;
    pub fn SLSSetWindowSubLevel(cid: cid_t, wid: u32, sub_level: c_int) -> CGError;
    pub fn SLSSetWindowOpacity(cid: cid_t, wid: u32, opaque: bool) -> CGError;
    pub fn SLSSetWindowShape(
        cid: cid_t,
        wid: u32,
        x_offset: f32,
        y_offset: f32,
        shape: *mut CFType,
    ) -> CGError;
    pub fn SLSOrderWindow(cid: cid_t, wid: u32, order: c_int, relative_to: u32) -> CGError;
    pub fn SLSSetWindowTags(cid: cid_t, wid: u32, tags: *mut u64, tag_count: c_int) -> CGError;
    pub fn SLSClearWindowTags(cid: cid_t, wid: u32, tags: *mut u64, tag_count: c_int) -> CGError;
    pub fn CGSNewRegionWithRect(rect: *const CGRect, region: *mut *mut CFType) -> CGError;
    pub fn CGRegionCreateEmptyRegion() -> *mut CFType;
    pub fn SLWindowContextCreate(cid: cid_t, wid: u32, options: *mut CFType) -> *mut CGContext;
    pub fn SLSSetWindowProperty(
        cid: cid_t,
        wid: u32,
        property: *mut CFString,
        value: *mut CFType,
    ) -> CGError;
    pub fn SLSSetWindowShadowParameters(
        cid: cid_t,
        wid: u32,
        std: f64,
        density: f64,
        x_offset: u32,
        y_offset: u32,
    ) -> CGError;
    pub fn SLSFlushWindowContentRegion(cid: cid_t, wid: u32, dirty: *mut c_void) -> CGError;
}
