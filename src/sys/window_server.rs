use std::ffi::c_int;

use accessibility::AXUIElement;
use accessibility_sys::{kAXErrorSuccess, pid_t};
use core_foundation::array::CFArray;
use core_foundation::base::{CFRelease, CFType, CFTypeRef, ItemRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::base::CGError;
use core_graphics::display::{
    CGWindowID, CGWindowListCopyWindowInfo, kCGNullWindowID, kCGWindowListOptionOnScreenOnly,
};
use core_graphics::window::{
    CGWindowListCreateDescriptionFromArray, kCGWindowBounds, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowNumber, kCGWindowOwnerPID,
};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::MainThreadMarker;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use super::geometry::{CGRectDef, ToICrate};
use super::screen::CoordinateConverter;
use crate::sys::process::ProcessSerialNumber;
use crate::sys::skylight::*;

static G_CONNECTION: Lazy<i32> = Lazy::new(|| unsafe { SLSMainConnectionID() });

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowServerId(pub CGWindowID);

impl WindowServerId {
    #[inline]
    pub fn new(id: CGWindowID) -> Self { Self(id) }

    #[inline]
    pub fn as_u32(self) -> u32 { self.0 }
}

impl From<WindowServerId> for u32 {
    #[inline]
    fn from(id: WindowServerId) -> Self { id.0 }
}

impl TryFrom<&AXUIElement> for WindowServerId {
    type Error = accessibility::Error;

    fn try_from(element: &AXUIElement) -> Result<Self, Self::Error> {
        let mut id = 0;
        let res = unsafe { _AXUIElementGetWindow(element.as_concrete_TypeRef(), &mut id) };
        if res != kAXErrorSuccess {
            return Err(accessibility::Error::Ax(res));
        }
        Ok(Self(id))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct WindowServerInfo {
    pub id: WindowServerId,
    pub pid: pid_t,
    pub layer: i32,
    #[serde(with = "CGRectDef")]
    pub frame: CGRect,
}

pub fn get_visible_windows_with_layer(layer: Option<i32>) -> Vec<WindowServerInfo> {
    get_visible_windows_raw()
        .iter()
        .filter_map(|win| make_info(win, layer))
        .collect()
}

pub fn get_visible_windows_raw() -> CFArray<CFDictionary<CFString, CFType>> {
    unsafe {
        // TODO: cgwindowlistcopywindowinfo does not appear to order windows properly
        CFArray::wrap_under_get_rule(CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        ))
    }
}

fn make_info(
    win: ItemRef<CFDictionary<CFString, CFType>>,
    layer_filter: Option<i32>,
) -> Option<WindowServerInfo> {
    let layer = get_num(&win, unsafe { kCGWindowLayer })?.try_into().ok()?;
    if layer_filter.is_some() && layer_filter != Some(layer) {
        return None;
    }

    let id = get_num(&win, unsafe { kCGWindowNumber })?;
    let pid = get_num(&win, unsafe { kCGWindowOwnerPID })?;
    let dict: CFDictionary = win.find(unsafe { kCGWindowBounds })?.downcast()?;
    let cg_frame = core_graphics_types::geometry::CGRect::from_dict_representation(&dict)?;

    Some(WindowServerInfo {
        id: WindowServerId(id.try_into().ok()?),
        pid: pid.try_into().ok()?,
        layer,
        frame: cg_frame.to_icrate(),
    })
}

pub fn get_windows(ids: &[WindowServerId]) -> Vec<WindowServerInfo> {
    if ids.is_empty() {
        return Vec::new();
    }
    get_windows_inner(ids).iter().flat_map(|w| make_info(w, None)).collect()
}

pub fn get_window(id: WindowServerId) -> Option<WindowServerInfo> {
    get_windows_inner(&[id]).iter().next().and_then(|w| make_info(w, None))
}

fn get_windows_inner(ids: &[WindowServerId]) -> CFArray<CFDictionary<CFString, CFType>> {
    let array = CFArray::from_copyable(ids);
    unsafe {
        CFArray::wrap_under_create_rule(CGWindowListCreateDescriptionFromArray(
            array.as_concrete_TypeRef(),
        ))
    }
}

fn get_num(dict: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<i64> {
    let item: CFNumber = dict.find(key)?.downcast()?;
    Some(item.to_i64()?)
}

pub fn get_window_at_point(
    mut point: CGPoint,
    _converter: CoordinateConverter,
    _mtm: MainThreadMarker,
) -> Option<WindowServerId> {
    let mut window_point = CGPoint { x: 0.0, y: 0.0 };
    let mut window_id: u32 = 0;
    let mut window_cid: c_int = 0;

    unsafe {
        SLSFindWindowAndOwner(
            *G_CONNECTION,
            0,
            1,
            0,
            &mut point,
            &mut window_point,
            &mut window_id,
            &mut window_cid,
        );
        if *G_CONNECTION == window_cid {
            SLSFindWindowAndOwner(
                *G_CONNECTION,
                window_id as i32,
                -1,
                0,
                &mut point,
                &mut window_point,
                &mut window_id,
                &mut window_cid,
            );
        }
    }

    (window_id != 0).then(|| WindowServerId(window_id))
}

// credit to yabai
pub fn space_window_list_for_connection(
    spaces: &[u64],
    owner: u32,
    include_minimized: bool,
) -> Vec<u32> {
    let cf_numbers: Vec<CFNumber> = spaces.iter().map(|&sid| CFNumber::from(sid as i64)).collect();
    let cf_space_array = CFArray::from_CFTypes(&cf_numbers);

    let mut set_tags: u64 = 0;
    let mut clear_tags: u64 = 0;
    let options: u32 = if include_minimized { 0x7 } else { 0x2 };

    let window_list_ref = unsafe {
        SLSCopyWindowsWithOptionsAndTags(
            *G_CONNECTION,
            owner,
            cf_space_array.as_concrete_TypeRef(),
            options,
            &mut set_tags,
            &mut clear_tags,
        )
    };

    if window_list_ref.is_null() {
        return Vec::new();
    }

    let list_cf = unsafe { CFArray::<CFTypeRef>::wrap_under_get_rule(window_list_ref) };
    let expected = list_cf.len() as i32;
    if expected == 0 {
        return Vec::new();
    }

    let query = unsafe { SLSWindowQueryWindows(*G_CONNECTION, window_list_ref, expected) };
    let iterator = unsafe { SLSWindowQueryResultCopyWindows(query) };

    let mut windows = Vec::with_capacity(expected as usize);

    while unsafe { SLSWindowIteratorAdvance(iterator) } {
        let tags = unsafe { SLSWindowIteratorGetTags(iterator) };
        let attributes = unsafe { SLSWindowIteratorGetAttributes(iterator) };
        let parent_id = unsafe { SLSWindowIteratorGetParentID(iterator) };
        let wid = unsafe { SLSWindowIteratorGetWindowID(iterator) };
        let level = unsafe { SLSWindowIteratorGetLevel(iterator) };

        let is_candidate = if include_minimized {
            if parent_id == 0 && matches!(level, 0 | 3 | 8) {
                ((attributes & 0x2) != 0 || (tags & 0x4000_0000_0000_0000) != 0)
                    && ((tags & 0x1) != 0 || ((tags & 0x2) != 0 && (tags & 0x8000_0000) != 0))
            } else {
                false
            }
        } else {
            parent_id == 0
                && matches!(level, 0 | 3 | 8)
                && (((attributes & 0x2) != 0) || (tags & 0x4000_0000_0000_0000) != 0)
                && ((tags & 0x1) != 0 || ((tags & 0x2) != 0 && (tags & 0x8000_0000) != 0))
        };

        if is_candidate {
            windows.push(wid);
        }
    }

    unsafe {
        CFRelease(iterator);
        CFRelease(query);
    }

    windows.shrink_to_fit();
    windows
}

// credit: https://github.com/Hammerspoon/hammerspoon/issues/370#issuecomment-545545468
pub fn make_key_window(pid: pid_t, wsid: WindowServerId) -> Result<(), ()> {
    #[allow(non_upper_case_globals)]
    const kCPSUserGenerated: u32 = 0x200;

    let mut event1 = [0u8; 0x100];
    event1[0x04] = 0xf8;
    event1[0x08] = 0x01;
    event1[0x3a] = 0x10;
    event1[0x3c..0x40].copy_from_slice(&wsid.0.to_le_bytes());
    event1[0x20..0x30].fill(0xff);

    let mut event2 = event1;
    event2[0x08] = 0x02;

    let psn = ProcessSerialNumber::for_pid(pid)?;
    let check = |err| if err == 0 { Ok(()) } else { Err(()) };

    unsafe {
        check(_SLPSSetFrontProcessWithOptions(&psn, wsid.0, kCPSUserGenerated))?;
        check(SLPSPostEventRecordTo(&psn, event1.as_ptr()))?;
        check(SLPSPostEventRecordTo(&psn, event2.as_ptr()))?;
    }
    Ok(())
}

pub fn allow_hide_mouse() -> Result<(), CGError> {
    let cid = unsafe { CGSMainConnectionID() };
    let property = CFString::from_static_string("SetsCursorInBackground");

    let err = unsafe {
        CGSSetConnectionProperty(
            cid,
            cid,
            property.as_concrete_TypeRef(),
            CFBoolean::true_value().as_CFTypeRef(),
        )
    };

    if err == 0 { Ok(()) } else { Err(err) }
}
