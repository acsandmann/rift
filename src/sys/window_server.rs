use std::ffi::{c_int, c_void};
use std::ptr::NonNull;
use std::time::Duration;

use accessibility::AXUIElement;
use accessibility_sys::{kAXErrorSuccess, pid_t};
use core_foundation::array::CFArray;
use core_foundation::base::{CFRelease, CFType, CFTypeRef, ItemRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::base::{CGError, kCGBitmapByteOrder32Little, kCGImageAlphaPremultipliedFirst};
use core_graphics::display::{
    CGWindowID, CGWindowListCopyWindowInfo, kCGNullWindowID, kCGWindowListOptionOnScreenOnly,
};
use core_graphics::window::{
    CGWindowListCreateDescriptionFromArray, kCGWindowBounds, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowNumber, kCGWindowOwnerPID,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapInfo, CGColorSpace, CGContext, CGImage, CGInterpolationQuality};
use objc2_foundation::MainThreadMarker;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use super::geometry::{CGRectDef, ToICrate};
use super::screen::CoordinateConverter;
use crate::layout_engine::Direction;
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

pub fn connection_id_for_pid(pid: pid_t) -> Option<i32> {
    let psn = ProcessSerialNumber::for_pid(pid).ok()?;
    let mut connection_id: c_int = 0;
    let result = unsafe { SLSGetConnectionIDForPSN(*G_CONNECTION, &psn, &mut connection_id) };
    (result == 0).then_some(connection_id)
}

pub fn window_parent(id: WindowServerId) -> Option<WindowServerId> {
    let cf_windows = CFArray::from_CFTypes(&[CFNumber::from(id.as_u32() as i64)]);
    let query =
        unsafe { SLSWindowQueryWindows(*G_CONNECTION, cf_windows.as_concrete_TypeRef(), 1) };
    if query.is_null() {
        return None;
    }

    let iterator = unsafe { SLSWindowQueryResultCopyWindows(query) };
    if iterator.is_null() {
        unsafe { CFRelease(query) };
        return None;
    }

    let count = unsafe { SLSWindowIteratorGetCount(iterator) };
    let mut parent = None;

    if count == 1 && unsafe { SLSWindowIteratorAdvance(iterator) } {
        let parent_id = unsafe { SLSWindowIteratorGetParentID(iterator) };
        if parent_id != 0 {
            parent = Some(WindowServerId::new(parent_id));
        }
    }

    unsafe {
        CFRelease(iterator);
        CFRelease(query);
    }

    parent
}

pub fn window_is_sticky(id: WindowServerId) -> bool {
    let cf_windows = CFArray::from_CFTypes(&[CFNumber::from(id.as_u32() as i64)]);
    let space_list_ref =
        unsafe { SLSCopySpacesForWindows(*G_CONNECTION, 0x7, cf_windows.as_concrete_TypeRef()) };
    if space_list_ref.is_null() {
        return false;
    }
    let spaces_cf: CFArray<CFNumber> = unsafe { CFArray::wrap_under_get_rule(space_list_ref) };
    spaces_cf.len() > 1
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
            if parent_id != 0 || !matches!(level, 0 | 3 | 8) {
                false
            } else if ((attributes & 0x2) != 0 || (tags & 0x0400_0000_0000_0000) != 0)
                && ((tags & 0x1) != 0 || ((tags & 0x2) != 0 && (tags & 0x8000_0000) != 0))
            {
                true
            } else {
                (attributes == 0 || attributes == 1)
                    && ((tags & 0x1000_0000_0000_0000) != 0 || (tags & 0x0300_0000_0000_0000) != 0)
                    && ((tags & 0x1) != 0 || ((tags & 0x2) != 0 && (tags & 0x8000_0000) != 0))
            }
        } else {
            parent_id == 0
                && matches!(level, 0 | 3 | 8)
                && (((attributes & 0x2) != 0) || (tags & 0x0400_0000_0000_0000) != 0)
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

#[derive(Clone)]
pub struct CapturedWindowImage(CFRetained<CGImage>);

impl CapturedWindowImage {
    #[inline]
    pub fn as_ptr(&self) -> *mut CGImage { CFRetained::as_ptr(&self.0).as_ptr() }

    #[inline]
    pub fn cg_image(&self) -> &CGImage { self.0.as_ref() }
}

/*pub fn capture_window_image(id: WindowServerId) -> Option<CapturedWindowImage> {
    let wid = id.as_u32();
    let images_ref = unsafe {
        SLSHWCaptureWindowList(
            *G_CONNECTION,
            &wid as *const u32,
            1,
            (1 << 11) | (1 << 9) | (1 << 19),
        )
    };

    if images_ref.is_null() {
        return None;
    }

    let images = unsafe { CFRetained::from_raw(NonNull::new_unchecked(images_ref)) };

    images.get(0).map(|img| CapturedWindowImage(img.retain()))
}*/

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *mut CGColorSpace,
        bitmap_info: CGBitmapInfo,
    ) -> *mut CGContext;

    pub fn CGBitmapContextCreateImage(c: *mut CGContext) -> *mut CGImage;
}

pub fn copy_image(src: &CGImage) -> Option<CapturedWindowImage> {
    unsafe {
        let w = CGImage::width(Some(src)) as usize;
        let h = CGImage::height(Some(src)) as usize;

        let cs = CGColorSpace::new_device_rgb()?;

        let bi = (kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little) as u32;
        let ctx = CFRetained::from_raw(NonNull::new_unchecked(CGBitmapContextCreate(
            std::ptr::null_mut(),
            w,
            h,
            8,
            0,
            CFRetained::as_ptr(&cs).as_ptr(),
            CGBitmapInfo(bi),
        )));

        CGContext::set_interpolation_quality(Some(ctx.as_ref()), CGInterpolationQuality::High);
        let dst = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
        CGContext::draw_image(Some(ctx.as_ref()), dst, Some(src));

        let out = CGBitmapContextCreateImage(CFRetained::as_ptr(&ctx).as_ptr());

        NonNull::new(out as *mut CGImage).map(|p| CapturedWindowImage(CFRetained::from_raw(p)))
    }
}

pub fn capture_window_image(id: WindowServerId) -> Option<CapturedWindowImage> {
    unsafe {
        let imgs_ref = SLSHWCaptureWindowList(
            *G_CONNECTION,
            &id.as_u32() as *const u32,
            1,
            (1 << 11) | (1 << 9) | (1 << 19),
        );
        if imgs_ref.is_null() {
            return None;
        }

        let imgs = CFRetained::from_raw(NonNull::new_unchecked(imgs_ref));
        if let Some(img) = imgs.get(0) {
            return copy_image(img.as_ref());
        }

        None
    }
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

// fast space switching with no animations
// credit: https://gist.github.com/amaanq/6991c7054b6c9816fafa9e29814b1509
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn switch_space(direction: Direction) {
    let magnitude = match direction {
        Direction::Left => -2.25,
        Direction::Right => 2.25,
        _ => return,
    };
    let gesture = 200.0 * magnitude;

    let event1a = CGEventCreate(std::ptr::null_mut());

    CGEventSetIntegerValueField(event1a, 0x37, 29);
    CGEventSetIntegerValueField(event1a, 0x29, 33231);

    let event1b = CGEventCreate(std::ptr::null_mut());
    CGEventSetIntegerValueField(event1b, 0x37, 30);
    CGEventSetIntegerValueField(event1b, 0x6E, 23);
    CGEventSetIntegerValueField(event1b, 0x84, 1);
    CGEventSetIntegerValueField(event1b, 0x86, 1);
    CGEventSetDoubleValueField(event1b, 0x7C, magnitude);

    let magnitude_bits = (magnitude as f32).to_bits() as i64;
    CGEventSetIntegerValueField(event1b, 0x87, magnitude_bits);

    CGEventSetIntegerValueField(event1b, 0x7B, 1);
    CGEventSetIntegerValueField(event1b, 0xA5, 1);
    CGEventSetDoubleValueField(event1b, 0x77, 1.401298464324817e-45);
    CGEventSetDoubleValueField(event1b, 0x8B, 1.401298464324817e-45);
    CGEventSetIntegerValueField(event1b, 0x29, 33231);
    CGEventSetIntegerValueField(event1b, 0x88, 0);

    CGEventPost(CGEventTapLocation::HID, event1b); // kCGHIDEventTap = 1
    CGEventPost(CGEventTapLocation::HID, event1a);

    CFRelease(event1a);
    CFRelease(event1b);

    crate::sys::timer::Timer::sleep(Duration::from_millis(15)); //(0x3A98); // 15ms

    let event2a = CGEventCreate(std::ptr::null_mut());
    CGEventSetIntegerValueField(event2a, 0x37, 29);
    CGEventSetIntegerValueField(event2a, 0x29, 33231);

    let event2b = CGEventCreate(std::ptr::null_mut());
    CGEventSetIntegerValueField(event2b, 0x37, 30);
    CGEventSetIntegerValueField(event2b, 0x6E, 23);
    CGEventSetIntegerValueField(event2b, 0x84, 4);
    CGEventSetIntegerValueField(event2b, 0x86, 4);
    CGEventSetDoubleValueField(event2b, 0x7C, magnitude);
    CGEventSetIntegerValueField(event2b, 0x87, magnitude_bits);
    CGEventSetIntegerValueField(event2b, 0x7B, 1);
    CGEventSetIntegerValueField(event2b, 0xA5, 1);
    CGEventSetDoubleValueField(event2b, 0x77, 1.401298464324817e-45);
    CGEventSetDoubleValueField(event2b, 0x8B, 1.401298464324817e-45);
    CGEventSetIntegerValueField(event2b, 0x29, 33231);
    CGEventSetIntegerValueField(event2b, 0x88, 0);

    CGEventSetDoubleValueField(event2b, 0x81, gesture);
    CGEventSetDoubleValueField(event2b, 0x82, gesture);

    CGEventPost(CGEventTapLocation::HID, event2b);
    CGEventPost(CGEventTapLocation::HID, event2a);

    CFRelease(event2a);
    CFRelease(event2b);
}
