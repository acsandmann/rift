use std::ffi::{c_int, c_void};
use std::ptr::NonNull;
use std::time::Duration;

use dispatchr::queue;
use dispatchr::time::Time;
use objc2_app_kit::NSWindowLevel;
use objc2_application_services::AXError;
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect,
    CGSize, Type, kCFBooleanTrue,
};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorSpace, CGContext, CGError, CGImage, CGInterpolationQuality, CGWindowID,
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowBounds,
    kCGWindowLayer, kCGWindowNumber, kCGWindowOwnerPID,
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use super::geometry::CGRectDef;
use crate::actor::app::WindowId;
use crate::layout_engine::Direction;
use crate::sys::app::pid_t;
use crate::sys::axuielement::{AXUIElement, Error as AxError};
use crate::sys::cg_ok;
use crate::sys::dispatch::DispatchExt;
use crate::sys::process::ProcessSerialNumber;
use crate::sys::skylight::*;
use crate::sys::timer::Timer;

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
    type Error = AxError;

    fn try_from(element: &AXUIElement) -> Result<Self, Self::Error> {
        let mut id = 0;
        let res = unsafe { _AXUIElementGetWindow(element.raw_ptr().as_ptr(), &mut id) };
        if res != AXError::Success {
            return Err(AxError::Ax(res));
        }
        Ok(Self(id))
    }
}

impl From<WindowId> for WindowServerId {
    fn from(id: WindowId) -> Self { Self(id.idx.into()) }
}

#[inline]
pub fn cf_array_from_ids(ids: &[WindowServerId]) -> CFRetained<CFArray<CFNumber>> {
    let nums: Vec<CFRetained<CFNumber>> =
        ids.iter().map(|w| CFNumber::new_i64(w.as_u32() as i64)).collect();
    CFArray::from_retained_objects(&nums)
}

pub struct WindowQuery {
    query: *mut CFType,
    iter: *mut CFType,
}

impl WindowQuery {
    pub fn new(ids: &[WindowServerId]) -> Option<Self> {
        if ids.is_empty() {
            return None;
        }
        let cf_numbers = cf_array_from_ids(ids);
        Self::new_from_cfarray(CFRetained::as_ptr(&cf_numbers).as_ptr(), ids.len() as c_int)
    }

    /// expected_count is a hint; keep whatever you used at call sites (0, 1, ids.len()).
    pub fn new_from_cfarray(
        cf_numbers: *mut CFArray<CFNumber>,
        expected_count: c_int,
    ) -> Option<Self> {
        let query = unsafe { SLSWindowQueryWindows(*G_CONNECTION, cf_numbers, expected_count) };
        if query.is_null() {
            return None;
        }
        let iter = unsafe { SLSWindowQueryResultCopyWindows(query) };
        if iter.is_null() {
            unsafe { CFRelease(query) };
            return None;
        }
        Some(Self { query, iter })
    }

    #[inline]
    pub fn count(&self) -> i32 { unsafe { SLSWindowIteratorGetCount(self.iter) } }

    #[inline]
    pub fn advance<'a>(&'a self) -> Option<&'a Self> {
        if unsafe { SLSWindowIteratorAdvance(self.iter) } {
            return Some(self);
        }

        None
    }

    #[inline]
    pub fn window_id(&self) -> u32 { unsafe { SLSWindowIteratorGetWindowID(self.iter) } }

    #[inline]
    pub fn level(&self) -> i32 { unsafe { SLSWindowIteratorGetLevel(self.iter) } }

    #[inline]
    pub fn pid(&self) -> i32 { unsafe { SLSWindowIteratorGetPID(self.iter) } }

    #[inline]
    pub fn parent_id(&self) -> u32 { unsafe { SLSWindowIteratorGetParentID(self.iter) } }

    #[inline]
    pub fn bounds(&self) -> CGRect { unsafe { SLSWindowIteratorGetBounds(self.iter) } }

    #[inline]
    #[allow(dead_code)]
    pub fn tags(&self) -> u64 { unsafe { SLSWindowIteratorGetTags(self.iter) } }

    #[inline]
    #[allow(dead_code)]
    pub fn attributes(&self) -> u64 { unsafe { SLSWindowIteratorGetAttributes(self.iter) } }
}

impl Drop for WindowQuery {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.iter);
            CFRelease(self.query);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
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
    let cf_windows = cf_array_from_ids(&[id]);
    let query = WindowQuery::new_from_cfarray(CFRetained::as_ptr(&cf_windows).as_ptr(), 1)?;
    if query.count() == 1 {
        let p = query.advance()?.parent_id();
        (p != 0).then(|| WindowServerId::new(p))
    } else {
        None
    }
}

pub fn associated_windows(id: WindowServerId) -> Vec<WindowServerId> {
    let assoc = unsafe { SLSCopyAssociatedWindows(*G_CONNECTION, id.as_u32()) };
    let Some(assoc) = NonNull::new(assoc) else {
        return Vec::new();
    };

    let assoc_cf: CFRetained<CFArray<CFNumber>> = unsafe { CFRetained::from_raw(assoc) };
    assoc_cf
        .iter()
        .filter_map(|num| num.as_i64())
        .map(|wid| WindowServerId::new(wid as u32))
        .collect()
}

pub fn window_is_sticky(id: WindowServerId) -> bool {
    let cf_windows = cf_array_from_ids(&[id]);
    let space_list_ref = unsafe {
        SLSCopySpacesForWindows(*G_CONNECTION, 0x7, CFRetained::as_ptr(&cf_windows).as_ptr())
    };
    if space_list_ref.is_null() {
        return false;
    }
    let spaces_cf: CFRetained<CFArray<CFNumber>> =
        unsafe { CFRetained::retain(NonNull::new_unchecked(space_list_ref)) };
    spaces_cf.len() > 1
}

pub fn window_spaces(id: WindowServerId) -> Vec<crate::sys::screen::SpaceId> {
    let cf_windows = cf_array_from_ids(&[id]);
    let space_list_ref = unsafe {
        SLSCopySpacesForWindows(*G_CONNECTION, 0x7, CFRetained::as_ptr(&cf_windows).as_ptr())
    };
    let Some(space_list_ref) = NonNull::new(space_list_ref) else {
        return Vec::new();
    };

    let spaces_cf: CFRetained<CFArray<CFNumber>> = unsafe { CFRetained::from_raw(space_list_ref) };
    spaces_cf
        .iter()
        .filter_map(|num| num.as_i64())
        .filter_map(|value| u64::try_from(value).ok())
        .filter_map(|value| (value != 0).then(|| crate::sys::screen::SpaceId::new(value)))
        .collect()
}

pub fn window_space(id: WindowServerId) -> Option<crate::sys::screen::SpaceId> {
    window_spaces(id).into_iter().next()
}

pub fn window_is_ordered_in(id: WindowServerId) -> bool {
    let mut ordered: u8 = 0;
    if let Ok(_) = cg_ok(unsafe { SLSWindowIsOrderedIn(*G_CONNECTION, id.as_u32(), &mut ordered) })
    {
        return ordered != 0;
    }

    false
}

fn get_visible_windows_raw<T: Type>() -> CFRetained<CFArray<T>> {
    unsafe {
        // TODO: cgwindowlistcopywindowinfo does not appear to order windows properly
        // SAFETY: this will almost always return (pre objc2 was not a result and just a cfarray)
        if let Some(windows) = CGWindowListCopyWindowInfo(
            CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
            kCGNullWindowID,
        ) {
            CFRetained::cast_unchecked(windows)
        } else {
            CFArray::empty()
        }
    }
}

fn make_info(
    win: CFRetained<CFDictionary<CFString, CFType>>,
    layer_filter: Option<i32>,
) -> Option<WindowServerInfo> {
    let layer = get_num(&win, unsafe { kCGWindowLayer })?.try_into().ok()?;
    if layer_filter.is_some() && layer_filter != Some(layer) {
        return None;
    }

    let id = get_num(&win, unsafe { kCGWindowNumber })?;
    let pid = get_num(&win, unsafe { kCGWindowOwnerPID })?;
    if let Ok(dict) = win.get(unsafe { kCGWindowBounds })?.downcast::<CFDictionary>() {
        let mut cg_frame = CGRect::default();
        unsafe {
            CGRectMakeWithDictionaryRepresentation(
                CFRetained::<CFDictionary<_, _>>::as_ptr(&dict).as_ptr(),
                &mut cg_frame,
            )
        };

        return Some(WindowServerInfo {
            id: WindowServerId(id.try_into().ok()?),
            pid: pid.try_into().ok()?,
            layer,
            frame: cg_frame,
        });
    }

    None
}

#[cfg(test)]
pub fn get_windows(ids: &[WindowServerId]) -> Vec<WindowServerInfo> {
    ids.iter()
        .map(|&id| WindowServerInfo {
            id,
            pid: 1234,
            layer: 0,
            frame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(800.0, 600.0)),
        })
        .collect()
}

#[cfg(not(test))]
pub fn get_windows(ids: &[WindowServerId]) -> Vec<WindowServerInfo> {
    if ids.is_empty() {
        return Vec::new();
    }
    let cf_ids = cf_array_from_ids(ids);

    let Some(query) =
        WindowQuery::new_from_cfarray(CFRetained::as_ptr(&cf_ids).as_ptr(), ids.len() as c_int)
    else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(ids.len());
    while query.advance().is_some() {
        out.push(WindowServerInfo {
            id: WindowServerId::new(query.window_id()),
            pid: query.pid() as i32,
            layer: query.level(),
            frame: query.bounds(),
        });
    }
    out
}

pub fn get_window(id: WindowServerId) -> Option<WindowServerInfo> {
    let mut ws = get_windows(&[id]);
    (ws.len() == 1).then(|| ws.remove(0))
}

fn get_num(dict: &CFDictionary<CFString, CFType>, key: &'static CFString) -> Option<i64> {
    dict.get(key)?.downcast::<CFNumber>().ok()?.as_i64()
}

pub fn get_window_at_point(mut point: CGPoint) -> Option<WindowServerId> {
    unsafe {
        let mut window_point = CGPoint { x: 0.0, y: 0.0 };
        let (mut window_id, mut window_cid) = (0u32, 0i32);

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
        (window_id != 0).then(|| WindowServerId(window_id))
    }
}

pub fn current_cursor_location() -> Result<CGPoint, CGError> {
    let mut point = CGPoint::new(0.0, 0.0);
    cg_ok(unsafe { SLSGetCurrentCursorLocation(*G_CONNECTION, &mut point) })?;
    Ok(point)
}

pub fn window_under_cursor() -> Option<WindowServerId> {
    let point = current_cursor_location().ok()?;
    get_window_at_point(point)
}

#[cfg(test)]
pub fn window_level(_wid: u32) -> Option<NSWindowLevel> { Some(0) }

#[cfg(not(test))]
pub fn window_level(wid: u32) -> Option<NSWindowLevel> {
    let cf = cf_array_from_ids(&[WindowServerId::new(wid)]);

    let query = WindowQuery::new_from_cfarray(
        CFRetained::as_ptr(&cf).as_ptr(),
        0x1, // preserve your hint
    )?;
    Some(query.advance()?.level() as NSWindowLevel)
}

fn iterator_window_suitable(iterator: *mut CFType) -> bool {
    let tags = unsafe { SLSWindowIteratorGetTags(iterator) };
    let attributes = unsafe { SLSWindowIteratorGetAttributes(iterator) };
    let parent_wid = unsafe { SLSWindowIteratorGetParentID(iterator) };

    if parent_wid == 0
        && ((attributes & 0x2) != 0 || (tags & 0x400000000000000) != 0)
        && (tags & SLSWindowTags::Attached) != 0
        && (tags & SLSWindowTags::IgnoresCycle) != 0
        && ((tags & SLSWindowTags::Document) != 0
            || ((tags & SLSWindowTags::Floating) != 0 && (tags & SLSWindowTags::Modal) != 0))
    {
        return true;
    }
    return false;
}

// credit to yabai
pub fn space_window_list_for_connection(
    spaces: &[u64],
    owner: u32,
    include_minimized: bool,
) -> Vec<u32> {
    let cf_numbers: Vec<CFRetained<CFNumber>> =
        spaces.iter().map(|&sid| CFNumber::new_i64(sid as i64)).collect();
    let cf_space_array = CFArray::from_retained_objects(&cf_numbers);

    let mut set_tags: u64 = 0;
    let mut clear_tags: u64 = 0;
    let options: u32 = if include_minimized { 0x7 } else { 0x2 };

    let window_list_ref = unsafe {
        SLSCopyWindowsWithOptionsAndTags(
            *G_CONNECTION,
            owner,
            CFRetained::as_ptr(&cf_space_array).as_ptr(),
            options,
            &mut set_tags,
            &mut clear_tags,
        )
    };

    if window_list_ref.is_null() {
        return Vec::new();
    }

    let expected = (unsafe { &*window_list_ref }).len() as i32;
    if expected == 0 {
        unsafe { CFRelease(window_list_ref as *mut CFType) };
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
        CFRelease(window_list_ref as *mut CFType);
    }

    windows.shrink_to_fit();
    windows
}

pub fn app_window_suitable(id: WindowServerId) -> bool {
    let cf = cf_array_from_ids(&[id]);

    let Some(query) = WindowQuery::new_from_cfarray(
        CFRetained::as_ptr(&cf).as_ptr(),
        0x0, // keep your original hint
    ) else {
        return false;
    };

    if query.count() > 0 && query.advance().is_some() {
        iterator_window_suitable(query.iter)
    } else {
        false
    }
}

pub fn get_front_window(cid: i32) -> u32 {
    let mut wid: u32 = 0;

    let active_sid: u64 = unsafe { CGSGetActiveSpace(cid) };

    let mut psn = ProcessSerialNumber::default();
    unsafe { _SLPSGetFrontProcess(&mut psn) };

    let mut target_cid: i32 = 0;
    unsafe {
        SLSGetConnectionIDForPSN(cid, &psn, &mut target_cid);
    }

    let cf_numbers: Vec<CFRetained<CFNumber>> =
        [active_sid].iter().map(|&sid| CFNumber::new_i64(sid as i64)).collect();
    let cf_space_array = CFArray::from_retained_objects(&cf_numbers);

    let mut set_tags: u64 = 1;
    let mut clear_tags: u64 = 0;
    let window_list_ref = unsafe {
        SLSCopyWindowsWithOptionsAndTags(
            cid,
            target_cid as u32,
            CFRetained::as_ptr(&cf_space_array).as_ptr(),
            0x2,
            &mut set_tags,
            &mut clear_tags,
        )
    };

    if window_list_ref.is_null() {
        return 0;
    }

    let count = unsafe { (&*window_list_ref).len() as i32 };
    if count > 0 {
        let query = unsafe { SLSWindowQueryWindows(cid, window_list_ref, 0x0) };
        if !query.is_null() {
            let iterator = unsafe { SLSWindowQueryResultCopyWindows(query) };
            if !iterator.is_null() && unsafe { SLSWindowIteratorGetCount(iterator) } > 0 {
                while unsafe { SLSWindowIteratorAdvance(iterator) } {
                    if iterator_window_suitable(iterator) {
                        wid = unsafe { SLSWindowIteratorGetWindowID(iterator) };
                        break;
                    }
                }
            }
            unsafe {
                if !iterator.is_null() {
                    CFRelease(iterator);
                }
                CFRelease(query);
            }
        }
    }

    unsafe { CFRelease(window_list_ref as *mut CFType) };

    wid
}

pub fn window_space_id(cid: i32, wid: u32) -> u64 {
    let mut sid: u64 = 0;

    let cf_windows = CFArray::from_retained_objects(&[CFNumber::new_i64(wid as i64)]);

    let space_list_ref =
        unsafe { SLSCopySpacesForWindows(cid, 0x7, CFRetained::as_ptr(&cf_windows).as_ptr()) };

    if !space_list_ref.is_null() {
        let spaces_cf: CFRetained<CFArray<CFNumber>> =
            unsafe { CFRetained::from_raw(NonNull::new_unchecked(space_list_ref)) };
        if spaces_cf.len() > 0 {
            if let Some(id_ref) = spaces_cf.get(0) {
                let n: &CFNumber = id_ref.as_ref();
                if let Some(v) = n.as_i64() {
                    sid = v as u64;
                }
            }
        }
    }

    if sid != 0 {
        return sid;
    }

    let mut frame = CGRect::default();
    unsafe {
        CGSGetWindowBounds(cid, wid, &mut frame);
    }
    let uuid = unsafe { CGSCopyBestManagedDisplayForRect(cid, frame) };
    if !uuid.is_null() {
        let s = unsafe { SLSManagedDisplayGetCurrentSpace(cid, uuid) };
        unsafe { CFRelease(uuid as *mut CFType) };
        return s;
    }

    0
}

pub fn space_is_user(sid: u64) -> bool { unsafe { SLSSpaceGetType(*G_CONNECTION, sid) == 0 } }
pub fn space_is_fullscreen(sid: u64) -> bool { unsafe { SLSSpaceGetType(*G_CONNECTION, sid) == 4 } }
pub fn space_is_system(sid: u64) -> bool { unsafe { SLSSpaceGetType(*G_CONNECTION, sid) == 2 } }
pub fn wait_for_native_fullscreen_transition() {
    while !space_is_user(unsafe { CGSGetActiveSpace(*G_CONNECTION) }) {
        Timer::sleep(Duration::from_millis(100));
    }
}

#[derive(Clone)]
pub struct CapturedWindowImage(CFRetained<CGImage>);

impl CapturedWindowImage {
    #[inline]
    pub fn as_ptr(&self) -> *mut CGImage { CFRetained::as_ptr(&self.0).as_ptr() }

    #[inline]
    pub fn cg_image(&self) -> &CGImage { self.0.as_ref() }
}

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

fn capture_window(id: WindowServerId) -> Option<CapturedWindowImage> {
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
            return Some(CapturedWindowImage(img));
        }

        None
    }
}

pub fn capture_window_image(
    id: WindowServerId,
    target_w: usize,
    target_h: usize,
) -> Option<CapturedWindowImage> {
    let img = capture_window(id)?;
    resize_cgimage_fit(img.cg_image(), target_w, target_h)
}

pub fn resize_cgimage_fit(
    src: &CGImage,
    target_w: usize,
    target_h: usize,
) -> Option<CapturedWindowImage> {
    unsafe {
        let src_w = CGImage::width(Some(src)) as f64;
        let src_h = CGImage::height(Some(src)) as f64;
        if src_w <= 0.0 || src_h <= 0.0 {
            return None;
        }

        let mut max_w = target_w.max(1) as f64;
        let mut max_h = target_h.max(1) as f64;
        max_w = max_w.min(src_w);
        max_h = max_h.min(src_h);

        let scale = (max_w / src_w).min(max_h / src_h);
        let dst_w = (src_w * scale).round().max(1.0) as usize;
        let dst_h = (src_h * scale).round().max(1.0) as usize;

        let cs = CGColorSpace::new_device_rgb()?;
        let ctx = CFRetained::from_raw(NonNull::new_unchecked(CGBitmapContextCreate(
            std::ptr::null_mut(),
            dst_w,
            dst_h,
            8,
            0,
            CFRetained::as_ptr(&cs).as_ptr(),
            // kCGImageAlphaPremultipliedFirst = 2
            // kCGBitmapByteOrder32Little = 2 << 12
            CGBitmapInfo(2u32 | 2 << 12),
        )));

        CGContext::set_interpolation_quality(Some(ctx.as_ref()), CGInterpolationQuality::None);

        let dst = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(dst_w as f64, dst_h as f64));
        CGContext::draw_image(Some(ctx.as_ref()), dst, Some(src));

        let out = CGBitmapContextCreateImage(CFRetained::as_ptr(&ctx).as_ptr());
        NonNull::new(out as *mut CGImage).map(|p| CapturedWindowImage(CFRetained::from_raw(p)))
    }
}

// credit: https://github.com/Hammerspoon/hammerspoon/issues/370#issuecomment-545545468
pub fn make_key_window(pid: pid_t, wsid: WindowServerId) -> Result<(), CGError> {
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

    unsafe {
        cg_ok(_SLPSSetFrontProcessWithOptions(&psn, wsid.0, kCPSUserGenerated))?;
        cg_ok(SLPSPostEventRecordTo(&psn, event1.as_ptr()))?;
        cg_ok(SLPSPostEventRecordTo(&psn, event2.as_ptr()))?;
    }
    Ok(())
}

pub fn allow_hide_mouse() -> Result<(), CGError> {
    let cid = unsafe { SLSMainConnectionID() };
    let property = CFString::from_str("SetsCursorInBackground");
    let value = CFBoolean::retain(unsafe { kCFBooleanTrue.unwrap_unchecked() });

    cg_ok(unsafe {
        CGSSetConnectionProperty(
            cid,
            cid,
            CFRetained::<CFString>::as_ptr(&property).as_ptr(),
            CFRetained::<CFBoolean>::as_ptr(&value).as_ptr() as *mut CFType,
        )
    })
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

    queue::main().after_f_s(
        Time::new_after(Time::NOW, 15 * 1000000),
        (magnitude, magnitude_bits),
        |(magnitude, magnitude_bits)| {
            unsafe {
                let gesture = 200.0 * magnitude;

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
            };
        },
    );
}
