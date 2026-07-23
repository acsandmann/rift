#[cfg(test)]
use std::cell::RefCell;
use std::ffi::{CStr, c_int, c_void};
use std::num::NonZeroU32;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use nix::libc::{RTLD_DEFAULT, dlsym};
use objc2_app_kit::NSWindowLevel;
use objc2_application_services::AXError;
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect,
    CGSize, Type, kCFBooleanTrue,
};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorSpace, CGContext, CGError, CGImage, CGInterpolationQuality, CGWindowID,
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowLayer, kCGWindowName,
    kCGWindowOwnerName,
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use super::geometry::{CGRectDef, CGSizeDef};
use crate::actor::app::WindowId;
#[cfg(test)]
use crate::common::collections::HashMap;
use crate::sys::app::pid_t;
use crate::sys::axuielement::{AXUIElement, Error as AxError};
use crate::sys::cg_ok;
#[cfg(not(test))]
use crate::sys::geometry::CGRectExt;
use crate::sys::mach::mach_get_window_sub_level;
use crate::sys::process::ProcessSerialNumber;
use crate::sys::screen::{ScreenInfo, SpaceId};
use crate::sys::skylight::*;

static G_CONNECTION: Lazy<i32> = Lazy::new(|| unsafe { SLSMainConnectionID() });
static LAST_WINDOWSERVER_ACTIVITY_US: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
thread_local! {
    static TEST_SPACE_WINDOW_LIST_OVERRIDE: RefCell<Option<Vec<u32>>> = const { RefCell::new(None) };
    static TEST_SPACE_WINDOW_LIST_BY_SPACE_OVERRIDE: RefCell<HashMap<u64, Vec<u32>>> = RefCell::new(HashMap::default());
    static TEST_WINDOW_SPACES_OVERRIDE: RefCell<HashMap<u32, Vec<u64>>> = RefCell::new(HashMap::default());
    static TEST_WINDOW_ORDERED_IN_OVERRIDE: RefCell<HashMap<u32, bool>> = RefCell::new(HashMap::default());
}

pub const WINDOWSERVER_QUIET_US: u64 = 350_000;
#[cfg_attr(test, allow(dead_code))]
const EFFECTIVELY_INVISIBLE_WINDOW_ALPHA: f32 = 0.01;

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowServerId(pub CGWindowID);

impl WindowServerId {
    #[inline]
    pub fn new(id: CGWindowID) -> Self { Self(id) }

    #[inline]
    pub fn as_u32(self) -> u32 { self.0 }

    #[inline]
    pub fn as_nonzero(self) -> Option<NonZeroU32> { NonZeroU32::new(self.0) }
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
        if id == 0 {
            return Err(AxError::NotFound);
        }
        Ok(Self(id))
    }
}

impl From<WindowId> for WindowServerId {
    fn from(id: WindowId) -> Self { Self(id.idx.into()) }
}

#[inline]
fn now_us() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_micros() as u64
}

pub fn note_windowserver_activity(wsid: u32) {
    LAST_WINDOWSERVER_ACTIVITY_US.store(now_us(), Ordering::SeqCst);
    // Keep this trace low-cost; it's only used to stabilize display churn.
    tracing::trace!(wsid, "windowserver activity");
}

pub fn windowserver_quiet_for_us(quiet_us: u64) -> bool {
    let last = LAST_WINDOWSERVER_ACTIVITY_US.load(Ordering::SeqCst);
    if last == 0 {
        return true;
    }
    now_us().saturating_sub(last) >= quiet_us
}

#[inline]
fn cf_array_from_ids(ids: &[WindowServerId]) -> CFRetained<CFArray<CFNumber>> {
    if let [id] = ids {
        let number = CFNumber::new_i64(id.as_u32() as i64);
        return CFArray::from_retained_objects(std::slice::from_ref(&number));
    }
    let nums: Vec<CFRetained<CFNumber>> =
        ids.iter().map(|w| CFNumber::new_i64(w.as_u32() as i64)).collect();
    CFArray::from_retained_objects(&nums)
}

#[inline]
fn cf_array_from_u64s(ids: &[u64]) -> CFRetained<CFArray<CFNumber>> {
    if let [id] = ids {
        let number = CFNumber::new_i64(*id as i64);
        return CFArray::from_retained_objects(std::slice::from_ref(&number));
    }
    let nums: Vec<CFRetained<CFNumber>> =
        ids.iter().map(|&id| CFNumber::new_i64(id as i64)).collect();
    CFArray::from_retained_objects(&nums)
}

pub struct WindowIterator {
    iter: *mut CFType,
}

impl WindowIterator {
    pub fn new(ids: &[WindowServerId]) -> Option<Self> {
        if ids.is_empty() {
            return None;
        }
        let cf_numbers = cf_array_from_ids(ids);
        Self::new_from_cfarray(CFRetained::as_ptr(&cf_numbers).as_ptr(), 0)
    }

    /// `flags` controls optional result decoding; bit 0 requests window titles.
    fn new_from_cfarray(cf_numbers: *mut CFArray<CFNumber>, flags: c_int) -> Option<Self> {
        let query = unsafe { SLSWindowQueryWindows(*G_CONNECTION, cf_numbers, flags) };
        if query.is_null() {
            return None;
        }
        let iter = unsafe { SLSWindowQueryResultCopyWindows(query) };
        unsafe { CFRelease(query) };
        if iter.is_null() {
            return None;
        }
        Self::from_owned_iterator(iter)
    }

    fn from_owned_iterator(iter: *mut CFType) -> Option<Self> {
        if iter.is_null() {
            return None;
        }
        let iterator = Self { iter };
        // Settle the SLS iterator before its first Advance. Some reply shapes
        // otherwise produce a valid iterator that initially appears empty.
        let _ = iterator.count();
        Some(iterator)
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
    pub fn alpha(&self) -> f32 { unsafe { SLSWindowIteratorGetAlpha(self.iter) } }

    #[inline]
    #[allow(dead_code)]
    pub fn tags(&self) -> u64 { unsafe { SLSWindowIteratorGetTags(self.iter) } }

    #[inline]
    #[allow(dead_code)]
    pub fn attributes(&self) -> u64 { unsafe { SLSWindowIteratorGetAttributes(self.iter) } }

    #[inline]
    pub fn constraints(&self) -> (CGSize, CGSize) {
        let mut min = CGSize::ZERO;
        let mut max = CGSize::ZERO;
        let mut cur = CGSize::ZERO;
        unsafe { SLSWindowIteratorGetConstraints(self.iter, &mut min, &mut max, &mut cur) };

        if min.width == 0.0 && min.height == 0.0 && max.width == 0.0 && max.height == 0.0 {
            unsafe {
                SLSPackagesGetWindowConstraints(
                    *G_CONNECTION,
                    self.window_id(),
                    &mut min,
                    &mut max,
                    &mut cur,
                )
            };
        }

        (min, max)
    }
}

impl Drop for WindowIterator {
    fn drop(&mut self) { unsafe { CFRelease(self.iter) } }
}

/// Server-side filter for `SLSWindowQueryRun`.
///
/// Unlike [`WindowIterator`], this describes which windows WindowServer should
/// materialize. Results are returned in native z-order.
#[derive(Debug, Clone, Copy)]
struct WindowQueryFilter<'a> {
    owner: i32,
    spaces: &'a [u64],
    space_list_options: i32,
    window_list_options: i32,
    query_flags: i32,
    include_tags: u64,
    exclude_tags: u64,
}

#[derive(Clone, Copy)]
struct WindowQueryKeys {
    owner: usize,
    spaces: usize,
    space_options: usize,
    window_options: usize,
    include_tags: usize,
    exclude_tags: usize,
}

fn resolve_window_query_key(name: &CStr) -> Option<usize> {
    let slot = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr()) };
    if slot.is_null() {
        return None;
    }
    let key = unsafe { *(slot.cast::<*mut CFString>()) };
    (!key.is_null()).then_some(key as usize)
}

static WINDOW_QUERY_KEYS: Lazy<Option<WindowQueryKeys>> = Lazy::new(|| {
    Some(WindowQueryKeys {
        owner: resolve_window_query_key(c"SLSWindowQueryKeyOwner")?,
        spaces: resolve_window_query_key(c"SLSWindowQueryKeySpaces").unwrap_or(0),
        space_options: resolve_window_query_key(c"SLSWindowQueryKeySpaceListOptions").unwrap_or(0),
        // This key appeared later than the core query API. A missing value is
        // represented by zero and simply leaves the server default in place.
        window_options: resolve_window_query_key(c"SLSWindowQueryKeyWorkspaceWindowListOptions")
            .unwrap_or(0),
        include_tags: resolve_window_query_key(c"SLSWindowQueryKeyIncludeTags")?,
        exclude_tags: resolve_window_query_key(c"SLSWindowQueryKeyExcludeTags")?,
    })
});

unsafe fn set_window_query_value<T: Type>(query: *mut CFType, key: usize, value: &CFRetained<T>) {
    if key != 0 {
        unsafe {
            SLSWindowQuerySetValue(
                query,
                key as *mut CFString,
                CFRetained::as_ptr(value).as_ptr().cast::<CFType>(),
            )
        };
    }
}

/// Run a native, server-side window query and return its z-ordered iterator.
fn window_query_run(filter: &WindowQueryFilter<'_>) -> Option<WindowIterator> {
    let keys = (*WINDOW_QUERY_KEYS)?;
    let uses_explicit_spaces = !filter.spaces.is_empty();
    if (uses_explicit_spaces && keys.spaces == 0)
        || (!uses_explicit_spaces && keys.space_options == 0)
    {
        return None;
    }

    let query = unsafe { SLSWindowQueryCreate(std::ptr::null_mut()) };
    if query.is_null() {
        return None;
    }

    let owner = CFNumber::new_i32(filter.owner);
    let include_tags = CFNumber::new_i64(filter.include_tags as i64);
    let exclude_tags = CFNumber::new_i64(filter.exclude_tags as i64);
    let window_options =
        (keys.window_options != 0).then(|| CFNumber::new_i32(filter.window_list_options));
    unsafe {
        set_window_query_value(query, keys.owner, &owner);
        set_window_query_value(query, keys.include_tags, &include_tags);
        set_window_query_value(query, keys.exclude_tags, &exclude_tags);
        if let Some(window_options) = &window_options {
            set_window_query_value(query, keys.window_options, window_options);
        }
    }

    let space_array = uses_explicit_spaces.then(|| cf_array_from_u64s(filter.spaces));
    let space_options =
        (!uses_explicit_spaces).then(|| CFNumber::new_i32(filter.space_list_options));
    unsafe {
        if let Some(space_array) = &space_array {
            set_window_query_value(query, keys.spaces, space_array);
        } else if let Some(space_options) = &space_options {
            set_window_query_value(query, keys.space_options, space_options);
        }
    }

    let result = unsafe { SLSWindowQueryRun(*G_CONNECTION, query, filter.query_flags) };
    let iterator = if result.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { SLSWindowQueryResultCopyWindows(result) }
    };
    unsafe {
        if !result.is_null() {
            CFRelease(result);
        }
        CFRelease(query);
    }
    WindowIterator::from_owned_iterator(iterator)
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
#[allow(unused)]
pub struct WindowServerInfo {
    pub id: WindowServerId,
    pub pid: pid_t,
    pub layer: i32,
    #[serde(with = "CGRectDef")]
    pub frame: CGRect,
    #[serde(with = "CGSizeDef")]
    pub min_frame: CGSize,
    #[serde(with = "CGSizeDef")]
    pub max_frame: CGSize,
}

/// Global CG on-screen window snapshot.
///
/// This is intentionally *not* space-aware and should not be used for ordinary
/// reactor/spaces reconciliation. Native-space truth comes from the spaces actor
/// via `space_window_list_for_connection(...)`.
/// Returns whether Mission Control's Dock-owned layer-18 overlay is still
/// visible in the global on-screen window list.
///
/// This intentionally uses the global CG on-screen snapshot because the signal
/// we want is the presence of the Dock's Mission Control UI itself, not
/// ordinary native-space membership.
pub fn mission_control_dock_overlay_visible() -> bool {
    #[cfg(test)]
    if let Some(override_value) =
        TEST_MISSION_CONTROL_DOCK_OVERLAY_VISIBLE.with(|value| *value.borrow())
    {
        return override_value;
    }

    const MISSION_CONTROL_DOCK_LAYER: i64 = 18;

    get_visible_windows_raw::<CFDictionary<CFString, CFType>>()
        .iter()
        .any(|window| {
            if window.get(unsafe { kCGWindowName }).is_some() {
                return false;
            }

            let Some(owner_name) = get_string(&window, unsafe { kCGWindowOwnerName }) else {
                return false;
            };
            if owner_name != "Dock" {
                return false;
            }

            get_num(&window, unsafe { kCGWindowLayer }) == Some(MISSION_CONTROL_DOCK_LAYER)
        })
}

pub fn window_parent(id: WindowServerId) -> Option<WindowServerId> {
    let query = WindowIterator::new(&[id])?;
    if query.count() == 1 {
        let p = query.advance()?.parent_id();
        (p != 0).then(|| WindowServerId::new(p))
    } else {
        None
    }
}

pub fn window_is_sticky(id: WindowServerId) -> bool {
    let cf_windows = cf_array_from_ids(&[id]);
    let space_list_ref = unsafe {
        SLSCopySpacesForWindows(*G_CONNECTION, 0x7, CFRetained::as_ptr(&cf_windows).as_ptr())
    };
    let Some(space_list_ref) = NonNull::new(space_list_ref) else {
        return false;
    };
    let spaces_cf: CFRetained<CFArray<CFNumber>> = unsafe { CFRetained::from_raw(space_list_ref) };
    spaces_cf.len() > 1
}

pub fn window_spaces(id: WindowServerId) -> Vec<crate::sys::screen::SpaceId> {
    #[cfg(test)]
    if let Some(override_spaces) =
        TEST_WINDOW_SPACES_OVERRIDE.with(|spaces| spaces.borrow().get(&id.as_u32()).cloned())
    {
        return override_spaces.into_iter().map(crate::sys::screen::SpaceId::new).collect();
    }

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
    let spaces = window_spaces(id);
    // SLSCopySpacesForWindows can return multiple space IDs for a window during
    // Mission Control or fullscreen transitions — the window's real home space plus
    // a transient fullscreen space. Prefer any user space (type 0) in the list so
    // that Desktop windows are not misidentified as belonging to a fullscreen space.
    spaces
        .iter()
        .copied()
        .find(|s| space_is_user(s.get()))
        .or_else(|| spaces.into_iter().next())
}

pub fn window_is_ordered_in(id: WindowServerId) -> bool {
    #[cfg(test)]
    if let Some(ordered) = TEST_WINDOW_ORDERED_IN_OVERRIDE
        .with(|override_ordered| override_ordered.borrow().get(&id.as_u32()).copied())
    {
        return ordered;
    }

    let mut ordered: u8 = 0;
    if let Ok(_) = cg_ok(unsafe { SLSWindowIsOrderedIn(*G_CONNECTION, id.as_u32(), &mut ordered) })
    {
        return ordered != 0;
    }

    false
}

fn get_windows_raw<T: Type>(
    options: CGWindowListOption,
    relative_to_window: CGWindowID,
) -> CFRetained<CFArray<T>> {
    unsafe {
        // TODO: cgwindowlistcopywindowinfo does not appear to order windows properly
        // SAFETY: this will almost always return (pre objc2 was not a result and just a cfarray)
        if let Some(windows) = CGWindowListCopyWindowInfo(options, relative_to_window) {
            CFRetained::cast_unchecked(windows)
        } else {
            CFArray::empty()
        }
    }
}

fn get_visible_windows_raw<T: Type>() -> CFRetained<CFArray<T>> {
    get_windows_raw(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    )
}

#[cfg(test)]
pub fn get_windows(ids: &[WindowServerId]) -> Vec<WindowServerInfo> {
    ids.iter()
        .map(|&id| WindowServerInfo {
            id,
            pid: 1234,
            layer: 0,
            frame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(800.0, 600.0)),
            min_frame: CGSize::ZERO,
            max_frame: CGSize::ZERO,
        })
        .collect()
}

#[cfg(not(test))]
pub fn get_windows(ids: &[WindowServerId]) -> Vec<WindowServerInfo> {
    let Some(query) = WindowIterator::new(ids) else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(ids.len());
    while query.advance().is_some() {
        if let Some(info) = window_info_from_query(&query) {
            out.push(info);
        }
    }
    out
}

pub fn get_window(id: WindowServerId) -> Option<WindowServerInfo> {
    #[cfg(test)]
    {
        return get_windows(&[id]).into_iter().next();
    }

    #[cfg(not(test))]
    {
        let query = WindowIterator::new(&[id])?;
        if query.count() != 1 || query.advance().is_none() {
            return None;
        }
        return window_info_from_query(&query);
    }
}

fn get_num(dict: &CFDictionary<CFString, CFType>, key: &'static CFString) -> Option<i64> {
    dict.get(key)?.downcast::<CFNumber>().ok()?.as_i64()
}

fn get_string(dict: &CFDictionary<CFString, CFType>, key: &'static CFString) -> Option<String> {
    Some(dict.get(key)?.downcast::<CFString>().ok()?.to_string())
}

#[cfg(not(test))]
pub fn focus_desktop_window(screen: &ScreenInfo) -> bool {
    let Some(display_uuid) = screen.display_uuid_opt() else {
        return false;
    };
    let uuid = CFString::from_str(display_uuid);
    let displays = CFArray::from_objects(&[&*uuid]);
    let Some(windows) = NonNull::new(unsafe {
        SLSManagedDisplaysCopyRoleWindows(*G_CONNECTION, CFRetained::as_ptr(&displays).as_ptr(), 1)
    }) else {
        return false;
    };
    let windows = unsafe { CFRetained::from_raw(windows) };
    windows.iter().any(|number| {
        let Some(id) = number.as_i64().and_then(|id| u32::try_from(id).ok()) else {
            return false;
        };
        let wsid = WindowServerId::new(id);
        let Some(info) = get_window(wsid) else {
            return false;
        };
        info.layer < 0
            && screen.frame.contains(info.frame.mid())
            && make_key_window(info.pid, wsid).is_ok()
    })
}

#[cfg(test)]
pub fn focus_desktop_window(_screen: &ScreenInfo) -> bool { false }

#[cfg_attr(test, allow(dead_code))]
fn window_is_effectively_invisible(alpha: f32, layer: i32) -> bool {
    layer == 0 && alpha <= EFFECTIVELY_INVISIBLE_WINDOW_ALPHA
}

#[cfg_attr(test, allow(dead_code))]
fn window_info_from_query(query: &WindowIterator) -> Option<WindowServerInfo> {
    let layer = query.level();
    if window_is_effectively_invisible(query.alpha(), layer) {
        return None;
    }
    let (min_frame, max_frame) = query.constraints();
    Some(WindowServerInfo {
        id: WindowServerId::new(query.window_id()),
        pid: query.pid() as i32,
        layer,
        frame: query.bounds(),
        min_frame,
        max_frame,
    })
}

#[cfg(test)]
thread_local! {
    static TEST_MISSION_CONTROL_DOCK_OVERLAY_VISIBLE: RefCell<Option<bool>> = const { RefCell::new(None) };
}

/// Find the topmost window at `point`, or the next window below
/// `below_window_id` when given. Returns `(window_id, owner_connection_id)`,
/// or `None` when no window is found.
fn find_window_at_point(point: &mut CGPoint, below_window_id: Option<u32>) -> Option<(u32, i32)> {
    let mut window_point = CGPoint { x: 0.0, y: 0.0 };
    let (mut wid, mut wcid) = (0u32, 0i32);

    let (start_id, direction) = match below_window_id {
        Some(id) => (id as i32, -1),
        None => (0, 1),
    };

    unsafe {
        SLSFindWindowAndOwner(
            *G_CONNECTION,
            start_id,
            direction,
            0,
            point,
            &mut window_point,
            &mut wid,
            &mut wcid,
        );
    }

    (wid != 0).then_some((wid, wcid))
}

fn is_own_window(cid: i32) -> bool { *G_CONNECTION == cid }

pub fn get_window_at_point(mut point: CGPoint) -> Option<WindowServerId> {
    let (mut wid, cid) = find_window_at_point(&mut point, None)?;
    if is_own_window(cid) {
        wid = find_window_at_point(&mut point, Some(wid))?.0;
    }
    Some(WindowServerId(wid))
}

/// Returns `true` if an external application window at normal level or above
/// occludes the given screen point.
///
/// Walks down the window stack at `point`, skipping all Rift-owned CGS
/// windows (there may be more than one at the same point), until a non-Rift
/// window is found. Desktop/wallpaper windows sit well below
/// `NSNormalWindowLevel` and are not considered occluders.
pub fn is_point_occluded_by_external_window(mut point: CGPoint) -> bool {
    use objc2_app_kit::NSNormalWindowLevel;

    let mut hit = find_window_at_point(&mut point, None);

    // Skip past any Rift-owned windows stacked at this point.
    while let Some((wid, cid)) = hit {
        if !is_own_window(cid) {
            let level = window_level(wid).unwrap_or(NSWindowLevel::MIN);
            return level >= NSNormalWindowLevel;
        }
        hit = find_window_at_point(&mut point, Some(wid));
    }

    false
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
    let query = WindowIterator::new(&[WindowServerId::new(wid)])?;
    Some(query.advance()?.level() as NSWindowLevel)
}

pub fn window_sub_level(wid: u32) -> c_int { unsafe { mach_get_window_sub_level(wid) } }

/// Returns the typed Skylight tags exposed by a window-query iterator.
fn iterator_window_tags(iterator: *mut CFType) -> SLSWindowTags {
    SLSWindowTags::from_bits_retain(unsafe { SLSWindowIteratorGetTags(iterator) })
}

/// Returns whether the tags describe a document or floating app window.
fn tags_match_app_window_role(tags: SLSWindowTags) -> bool {
    tags.contains(SLSWindowTags::DOCUMENT) || tags.contains(SLSWindowTags::FLOATING)
}

/// Returns whether the iterator points at a top-level application window.
fn iterator_window_suitable(iterator: *mut CFType) -> bool {
    let tags = iterator_window_tags(iterator);
    let parent_wid = unsafe { SLSWindowIteratorGetParentID(iterator) };

    // Previous Rust filter also required attribute/high-bit hints plus
    // ATTACHED, IGNORES_CYCLE, and DOCUMENT or (FLOATING && MODAL).
    parent_wid == 0 && tags_match_app_window_role(tags)
}

// credit to yabai
pub fn space_window_list_for_connection(
    spaces: &[u64],
    owner: u32,
    include_minimized: bool,
) -> Vec<u32> {
    #[cfg(test)]
    if spaces.len() == 1
        && let Some(override_ids) = TEST_SPACE_WINDOW_LIST_BY_SPACE_OVERRIDE
            .with(|ids| ids.borrow().get(&spaces[0]).cloned())
    {
        let _ = (owner, include_minimized);
        return override_ids;
    }

    #[cfg(test)]
    if let Some(override_ids) = TEST_SPACE_WINDOW_LIST_OVERRIDE.with(|ids| ids.borrow().clone()) {
        let _ = (spaces, owner, include_minimized);
        return override_ids;
    }

    let cf_space_array = cf_array_from_u64s(spaces);

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

    let iterator = WindowIterator::new_from_cfarray(window_list_ref, 0);
    unsafe { CFRelease(window_list_ref as *mut CFType) };
    let Some(iterator) = iterator else {
        return Vec::new();
    };

    let mut windows = Vec::with_capacity(expected as usize);

    while iterator.advance().is_some() {
        let tags = iterator_window_tags(iterator.iter);
        let parent_id = iterator.parent_id();
        let wid = iterator.window_id();
        // Previous Rust path also checked level, attributes, and
        // fullscreen/minimized tag hints before accepting the window.
        let is_candidate = parent_id == 0 && tags_match_app_window_role(tags);

        if is_candidate {
            windows.push(wid);
        }
    }

    windows
}

/// Resolve the actual key window on `space` from WindowServer state.
///
/// The key-focus process can differ briefly from the globally frontmost process,
/// especially during rapid focus changes. Scoping the native, z-ordered window
/// list to that process avoids the delayed or missing `AXMainWindow` read used by
/// the application actors. The returned id is intentionally independent of the
/// reactor's tracked-window state so callers can use it to trigger discovery of
/// a newly materialized native tab.
pub fn key_focused_window(space: SpaceId) -> Option<WindowId> {
    let mut psn = ProcessSerialNumber::default();
    let mut fallback = 0u8;
    if cg_ok(unsafe { SLPSGetKeyFocusProcess(&mut psn, &mut fallback) }).is_err() {
        return None;
    }

    let mut owner = 0;
    if unsafe { SLSGetConnectionIDForPSN(*G_CONNECTION, &psn, &mut owner) } != 0 || owner == 0 {
        return None;
    }

    let filter = WindowQueryFilter {
        owner,
        spaces: &[space.get()],
        space_list_options: 0,
        window_list_options: 0x2,
        query_flags: 0x2,
        include_tags: SLSWindowTags::DOCUMENT.bits(),
        exclude_tags: (SLSWindowTags::ON_ALL_WORKSPACES
            | SLSWindowTags::HIDDEN
            | SLSWindowTags::MINIATURIZED)
            .bits(),
    };
    let query = window_query_run(&filter)?;
    query.advance()?;
    let wsid = WindowServerId::new(query.window_id());

    Some(WindowId {
        pid: query.pid(),
        idx: wsid.as_nonzero()?,
    })
}

/// The space on the display currently holding WindowServer focus.
pub fn active_space() -> SpaceId { SpaceId::new(unsafe { CGSGetActiveSpace(*G_CONNECTION) }) }

#[cfg(test)]
pub fn set_space_window_list_for_connection_override(ids: Option<Vec<u32>>) {
    TEST_SPACE_WINDOW_LIST_OVERRIDE.with(|override_ids| *override_ids.borrow_mut() = ids);
}

#[cfg(test)]
pub fn set_space_window_list_for_space_override(space: u64, ids: Option<Vec<u32>>) {
    TEST_SPACE_WINDOW_LIST_BY_SPACE_OVERRIDE.with(|override_ids| {
        let mut override_ids = override_ids.borrow_mut();
        if let Some(ids) = ids {
            override_ids.insert(space, ids);
        } else {
            override_ids.remove(&space);
        }
    });
}

#[cfg(test)]
pub fn set_window_spaces_override(id: WindowServerId, spaces: Option<Vec<u64>>) {
    TEST_WINDOW_SPACES_OVERRIDE.with(|override_spaces| {
        let mut override_spaces = override_spaces.borrow_mut();
        if let Some(spaces) = spaces {
            override_spaces.insert(id.as_u32(), spaces);
        } else {
            override_spaces.remove(&id.as_u32());
        }
    });
}

#[cfg(test)]
pub fn set_window_ordered_in_override(id: WindowServerId, ordered: Option<bool>) {
    TEST_WINDOW_ORDERED_IN_OVERRIDE.with(|override_ordered| {
        let mut override_ordered = override_ordered.borrow_mut();
        if let Some(ordered) = ordered {
            override_ordered.insert(id.as_u32(), ordered);
        } else {
            override_ordered.remove(&id.as_u32());
        }
    });
}

pub fn app_window_suitable(id: WindowServerId) -> bool {
    let Some(query) = WindowIterator::new(&[id]) else {
        return false;
    };

    if query.count() > 0 && query.advance().is_some() {
        iterator_window_suitable(query.iter)
    } else {
        false
    }
}

pub fn space_is_user(sid: u64) -> bool { unsafe { SLSSpaceGetType(*G_CONNECTION, sid) == 0 } }
pub fn space_is_fullscreen(sid: u64) -> bool { unsafe { SLSSpaceGetType(*G_CONNECTION, sid) == 4 } }

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
pub unsafe fn switch_space(direction: crate::layout_engine::Direction) {
    unsafe { crate::sys::space_switch::switch_space(direction) };
}

#[cfg(test)]
mod tests {
    use super::WindowServerId;

    #[test]
    fn zero_window_server_id_is_not_a_window_id() {
        assert!(WindowServerId::new(0).as_nonzero().is_none());
        assert_eq!(WindowServerId::new(42).as_nonzero().map(|id| id.get()), Some(42));
    }
}
