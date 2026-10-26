use std::cmp::Ordering;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

use cgs::G_CONNECTION;
use objc2::rc::Retained;
use objc2::{ClassType, msg_send};
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{CGDisplayBounds, CGError, CGGetActiveDisplayList, CGMainDisplayID};
use objc2_foundation::{MainThreadMarker, NSArray, NSNumber, ns_string};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::skylight::{
    CFRelease, CFUUIDCreateFromString, CFUUIDCreateString, CGDisplayCreateUUIDFromDisplayID,
    CGDisplayGetDisplayIDFromUUID, CGSCopyBestManagedDisplayForRect, CGSCopyManagedDisplaySpaces,
    CGSCopyManagedDisplays, CGSCopySpaces, CGSGetActiveSpace, CGSManagedDisplayGetCurrentSpace,
    CGSSpaceMask, CoreDockGetAutoHideEnabled, CoreDockGetOrientationAndPinning,
    SLSCopyActiveMenuBarDisplayIdentifier, SLSGetDisplayMenubarHeight, SLSGetDockRectWithReason,
    SLSGetMenuBarAutohideEnabled, SLSGetSpaceManagementMode,
};
use crate::common::collections::HashMap;
use crate::sys::geometry::CGRectDef;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct SpaceId(u64);

impl SpaceId {
    pub fn new(id: u64) -> SpaceId { SpaceId(id) }

    pub fn get(&self) -> u64 { self.0 }
}

impl Into<u64> for SpaceId {
    fn into(self) -> u64 { self.get() }
}

impl ToString for SpaceId {
    fn to_string(&self) -> String { self.get().to_string() }
}

#[derive(Debug, Clone)]
struct ScreenState {
    screens: Vec<ScreenInfo>,
    converter: CoordinateConverter,
}

pub struct ScreenCache<S: System = Actual> {
    system: S,
    uuids: Vec<CFRetained<CFString>>,
    state: Option<ScreenState>,
    pending_generation: u64,
    processed_generation: u64,
    sleeping: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenInfo {
    pub id: ScreenId,
    #[serde(with = "CGRectDef")]
    pub frame: CGRect,
    #[serde(default = "unit_scale")]
    pub backing_scale: f64,
    #[serde(default)]
    pub is_builtin: bool,
    pub display_uuid: String,
    pub name: Option<String>,
    pub space: Option<SpaceId>,
}

fn unit_scale() -> f64 { 1.0 }

impl ScreenInfo {
    pub fn display_uuid_opt(&self) -> Option<&str> {
        if self.display_uuid.is_empty() {
            None
        } else {
            Some(self.display_uuid.as_str())
        }
    }

    pub fn display_uuid_owned(&self) -> Option<String> {
        self.display_uuid_opt().map(|uuid| uuid.to_string())
    }
}

/// Resolve ordered intent against the caller's accepted managed user desktops.
/// `eligible` excludes fullscreen/unknown Spaces without another WindowServer query.
/// CoreGraphics coordinates increase downwards: top has the smallest center Y.
pub fn resolve_display_preference<'a>(
    preferences: &[String],
    screens: &'a [ScreenInfo],
    eligible: impl Fn(&ScreenInfo) -> bool,
) -> Option<&'a ScreenInfo> {
    for preference in preferences {
        let directional = matches!(preference.as_str(), "left" | "right" | "top" | "bottom");
        let matches = screens.iter().filter(|screen| {
            screen.space.is_some()
                && screen.display_uuid_opt().is_some()
                && eligible(screen)
                && match preference.as_str() {
                    "builtin" => screen.is_builtin,
                    "left" | "right" | "top" | "bottom" => true,
                    value => screen.display_uuid == value || screen.name.as_deref() == Some(value),
                }
        });
        let compare = |a: &&ScreenInfo, b: &&ScreenInfo| {
            let a_center = a.frame.mid();
            let b_center = b.frame.mid();
            let geometric = match preference.as_str() {
                "left" => a_center.x.total_cmp(&b_center.x).then(a_center.y.total_cmp(&b_center.y)),
                "right" => {
                    b_center.x.total_cmp(&a_center.x).then(a_center.y.total_cmp(&b_center.y))
                }
                "top" => a_center.y.total_cmp(&b_center.y).then(a_center.x.total_cmp(&b_center.x)),
                "bottom" => {
                    b_center.y.total_cmp(&a_center.y).then(a_center.x.total_cmp(&b_center.x))
                }
                _ => Ordering::Equal,
            };
            geometric.then(a.display_uuid.cmp(&b.display_uuid))
        };
        let mut count = 0;
        let target = matches.inspect(|_| count += 1).min_by(compare);
        if target.is_some() {
            if count > 1 && !directional && preference != "builtin" {
                warn!(preference, "Workspace rule display is ambiguous; use a UUID");
            }
            return target;
        }
    }
    None
}

impl ScreenCache<Actual> {
    pub fn new(mtm: MainThreadMarker) -> Self { Self::new_with(Actual { mtm }) }
}

impl<S: System> ScreenCache<S> {
    fn new_with(system: S) -> ScreenCache<S> {
        ScreenCache {
            system,
            uuids: Vec::new(),
            state: None,
            pending_generation: 0,
            processed_generation: 0,
            sleeping: false,
        }
    }

    pub fn mark_dirty(&mut self) {
        self.pending_generation = self.pending_generation.wrapping_add(1);
    }

    pub fn mark_sleeping(&mut self, sleeping: bool) {
        self.sleeping = sleeping;
        if !sleeping {
            self.mark_dirty();
        }
    }

    pub fn refresh(&mut self) -> Option<(Vec<ScreenInfo>, CoordinateConverter)> {
        self.refresh_snapshot().map(|s| (s.screens, s.converter))
    }

    fn refresh_snapshot(&mut self) -> Option<ScreenState> {
        if self.sleeping {
            return self.state.clone();
        }

        let dirty = self.pending_generation != self.processed_generation;
        let should_rebuild = self.state.is_none() || dirty;

        if !should_rebuild {
            // Even when displays are unchanged, the active space per display can change.
            // Recompute spaces against cached UUIDs to avoid stale space ids.
            let spaces: Vec<Option<SpaceId>> = self
                .uuids
                .iter()
                .map(|screen| unsafe {
                    CGSManagedDisplayGetCurrentSpace(
                        cgs::main_connection(),
                        CFRetained::<objc2_core_foundation::CFString>::as_ptr(screen).as_ptr(),
                    )
                })
                .map(|id| if id == 0 { None } else { Some(SpaceId(id)) })
                .collect();

            if let Some(state) = self.state.clone() {
                let screens = state
                    .screens
                    .into_iter()
                    .zip(spaces)
                    .map(|(mut screen, space)| {
                        screen.space = space;
                        screen
                    })
                    .collect();
                return Some(ScreenState { screens, ..state });
            }
            return None;
        }

        let ns_screens = self.system.ns_screens();
        debug!("ns_screens={ns_screens:?}");
        let mut cg_screens = self.system.cg_screens().ok()?;
        debug!("cg_screens={cg_screens:?}");

        if cg_screens.is_empty() {
            // No connected display is a transient sample, not a new coordinate
            // system. Keep the last good cache and leave the generation dirty.
            return None;
        }

        cg_screens.sort_by(|a, b| {
            let x_order = a.bounds.origin.x.total_cmp(&b.bounds.origin.x);
            if x_order == Ordering::Equal {
                a.bounds.origin.y.total_cmp(&b.bounds.origin.y)
            } else {
                x_order
            }
        });

        let main_id = self.system.main_display_id();
        if let Some(main_screen_idx) = cg_screens.iter().position(|s| s.cg_id.0 == main_id) {
            cg_screens.swap(0, main_screen_idx);
        } else {
            warn!("Could not find main screen. cg_screens={cg_screens:?}");
            return None;
        }

        let uuids: Vec<CFRetained<CFString>> =
            cg_screens.iter().map(|screen| self.system.display_uuid(screen)).collect();
        let uuid_strings: Vec<String> = uuids.iter().map(|uuid| uuid.to_string()).collect();

        let mut identities = std::collections::HashSet::new();
        if uuid_strings.iter().any(|uuid| uuid.is_empty() || !identities.insert(uuid)) {
            warn!("Display identities unavailable; retaining previous screen cache");
            return None;
        }

        // Cocoa's global origin is the bottom left of the primary display, not
        // the bottom of the desktop union. Both APIs use logical points here.
        let converter = cg_screens
            .iter()
            .find(|s| s.cg_id.0 == main_id)
            .map(|s| CoordinateConverter::from_height(s.bounds.max().y))
            .unwrap_or_default();

        let screens: Vec<ScreenInfo> = cg_screens
            .iter()
            .enumerate()
            .map(|(idx, &CGScreenInfo { cg_id, bounds })| {
                let notch_height = self.system.notch_height(cg_id.as_u32());
                let frame = constrain_display_bounds(cg_id.as_u32(), bounds, notch_height);
                let display_uuid = uuid_strings[idx].clone();
                ScreenInfo {
                    id: cg_id,
                    frame,
                    is_builtin: self.system.is_builtin(cg_id.as_u32()),
                    backing_scale: ns_screens
                        .iter()
                        .find(|s| s.cg_id == cg_id)
                        .map_or(1.0, |s| s.backing_scale),
                    display_uuid,
                    name: ns_screens.iter().find(|s| s.cg_id == cg_id).and_then(|s| s.name.clone()),
                    space: None,
                }
            })
            .collect();

        let spaces: Vec<Option<SpaceId>> = uuids
            .iter()
            .map(|screen| unsafe {
                CGSManagedDisplayGetCurrentSpace(
                    cgs::main_connection(),
                    CFRetained::<objc2_core_foundation::CFString>::as_ptr(screen).as_ptr(),
                )
            })
            .map(|id| if id == 0 { None } else { Some(SpaceId(id)) })
            .collect();

        self.uuids = uuids;
        self.processed_generation = self.pending_generation;
        let screens = screens
            .into_iter()
            .zip(spaces)
            .map(|(mut screen, space)| {
                screen.space = space;
                screen
            })
            .collect();
        self.state = Some(ScreenState { screens, converter });
        self.state.clone()
    }
}

const DOCK_ORIENTATION_TOP: i32 = 1;
const DOCK_ORIENTATION_BOTTOM: i32 = 2;
const DOCK_ORIENTATION_LEFT: i32 = 3;
const DOCK_ORIENTATION_RIGHT: i32 = 4;

fn menu_bar_hidden() -> bool {
    let mut status = 0;
    unsafe { SLSGetMenuBarAutohideEnabled(*G_CONNECTION, &mut status) };
    status != 0
}

pub fn menu_bar_height(did: u32) -> f64 {
    let mut height: u32 = 0;
    unsafe { SLSGetDisplayMenubarHeight(did, &mut height) };
    height as f64
}

fn menu_bar_inset(hidden: bool, height: f64, notch_height: f64) -> f64 {
    if hidden {
        // An auto-hidden menu bar does not reserve space on displays without a notch.
        // Notched built-in displays must retain their safe-area inset.
        notch_height
    } else {
        // macOS reports the menubar height without the topmost usable pixel; add 1 to
        // avoid leaving a dead strip or placing windows under the bar.
        height + 1.0
    }
}

fn dock_hidden() -> bool { unsafe { CoreDockGetAutoHideEnabled() } }

fn dock_orientation() -> i32 {
    let mut orientation = 0;
    let mut pinning = 0;
    unsafe { CoreDockGetOrientationAndPinning(&mut orientation, &mut pinning) };
    orientation
}

fn dock_rect() -> CGRect {
    let mut rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
    let mut reason = 0;
    unsafe { SLSGetDockRectWithReason(*G_CONNECTION, &mut rect, &mut reason) };
    rect
}

fn dock_rect_with_reason() -> (CGRect, i32) {
    let mut rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
    let mut reason = 0;
    unsafe { SLSGetDockRectWithReason(*G_CONNECTION, &mut rect, &mut reason) };
    (rect, reason)
}

fn dock_display_id() -> Option<u32> {
    unsafe {
        let dock = dock_rect();
        let uuid_ref = CGSCopyBestManagedDisplayForRect(*G_CONNECTION, dock);
        if uuid_ref.is_null() {
            return None;
        }
        let uuid = CFUUIDCreateFromString(std::ptr::null_mut(), uuid_ref);
        if uuid.is_null() {
            CFRelease(uuid_ref as *mut _);
            return None;
        }
        let did = CGDisplayGetDisplayIDFromUUID(uuid);
        CFRelease(uuid as *mut _);
        CFRelease(uuid_ref as *mut _);
        if did == 0 { None } else { Some(did) }
    }
}

fn rects_intersect(a: &CGRect, b: &CGRect) -> bool {
    let ax2 = a.origin.x + a.size.width;
    let ay2 = a.origin.y + a.size.height;
    let bx2 = b.origin.x + b.size.width;
    let by2 = b.origin.y + b.size.height;

    !(ax2 <= b.origin.x || bx2 <= a.origin.x || ay2 <= b.origin.y || by2 <= a.origin.y)
}

fn constrain_display_bounds(did: u32, raw: CGRect, notch_height: f64) -> CGRect {
    let mut frame = raw;

    let menu_bar_inset = menu_bar_inset(menu_bar_hidden(), menu_bar_height(did), notch_height);
    if menu_bar_inset > 0.0 {
        frame.origin.y += menu_bar_inset;
        frame.size.height = (frame.size.height - menu_bar_inset).max(0.0);
    }

    let auto_hide = dock_hidden();
    let (dock, dock_reason) = dock_rect_with_reason();

    let dock_display = dock_display_id();

    let dock_visible = (!auto_hide || dock_reason == 0)
        && dock_display.map(|dock_did| dock_did == did).unwrap_or(false)
        && rects_intersect(&frame, &dock);

    if dock_visible {
        match dock_orientation() {
            DOCK_ORIENTATION_LEFT => {
                frame.origin.x += dock.size.width;
                frame.size.width = (frame.size.width - dock.size.width).max(0.0);
            }
            DOCK_ORIENTATION_RIGHT => {
                frame.size.width = (frame.size.width - dock.size.width).max(0.0);
            }
            DOCK_ORIENTATION_BOTTOM => {
                frame.size.height = (frame.size.height - dock.size.height).max(0.0);
            }
            DOCK_ORIENTATION_TOP => {
                frame.origin.y += dock.size.height;
                frame.size.height = (frame.size.height - dock.size.height).max(0.0);
            }
            _ => {
                if dock.size.width > dock.size.height {
                    frame.origin.y += dock.size.height;
                    frame.size.height = (frame.size.height - dock.size.height).max(0.0);
                } else {
                    frame.origin.x += dock.size.width;
                    frame.size.width = (frame.size.width - dock.size.width).max(0.0);
                }
            }
        }
    }

    frame
}

/// Converts between Quartz and Cocoa coordinate systems.
#[derive(Clone, Copy, Debug)]
pub struct CoordinateConverter {
    /// The y offset of the Cocoa origin in the Quartz coordinate system, and
    /// vice versa. This is the height of the first screen. The origins
    /// are the bottom left and top left of the screen, respectively.
    screen_height: f64,
}

/// Creates a `CoordinateConverter` that returns None for any conversion.
impl Default for CoordinateConverter {
    fn default() -> Self { Self { screen_height: f64::NAN } }
}

impl CoordinateConverter {
    pub fn from_height(height: f64) -> Self { Self { screen_height: height } }

    pub fn from_screen(screen: &NSScreen) -> Option<Self> {
        let screen_id = screen.get_number().ok()?;
        let bounds = CGDisplayBounds(screen_id.as_u32());
        Some(Self::from_height(bounds.origin.y + bounds.size.height))
    }

    pub fn screen_height(&self) -> Option<f64> {
        if self.screen_height.is_nan() {
            None
        } else {
            Some(self.screen_height)
        }
    }

    pub fn convert_point(&self, point: CGPoint) -> Option<CGPoint> {
        if self.screen_height.is_nan() {
            return None;
        }
        Some(CGPoint::new(point.x, self.screen_height - point.y))
    }

    pub fn convert_rect(&self, rect: CGRect) -> Option<CGRect> {
        if self.screen_height.is_nan() {
            return None;
        }
        Some(CGRect::new(
            CGPoint::new(rect.origin.x, self.screen_height - rect.max().y),
            rect.size,
        ))
    }
}

#[allow(private_interfaces)]
pub trait System {
    fn main_display_id(&self) -> u32 { CGMainDisplayID() }
    fn cg_screens(&self) -> Result<Vec<CGScreenInfo>, CGError>;
    fn display_uuid(&self, screen: &CGScreenInfo) -> CFRetained<CFString>;
    fn ns_screens(&self) -> Vec<NSScreenInfo>;
    fn is_builtin(&self, _did: u32) -> bool { false }
    fn notch_height(&self, _did: u32) -> f64 { 0.0 }
}

#[derive(Debug, Clone)]
struct CGScreenInfo {
    cg_id: ScreenId,
    bounds: CGRect,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct NSScreenInfo {
    frame: CGRect,
    visible_frame: CGRect,
    backing_scale: f64,
    cg_id: ScreenId,
    name: Option<String>,
}

pub struct Actual {
    mtm: MainThreadMarker,
}
#[allow(private_interfaces)]
impl System for Actual {
    fn cg_screens(&self) -> Result<Vec<CGScreenInfo>, CGError> {
        const MAX_SCREENS: usize = 64;
        let mut ids: MaybeUninit<[CGDirectDisplayID; MAX_SCREENS]> = MaybeUninit::uninit();
        let mut count: u32 = 0;
        let ids = unsafe {
            let err = CGGetActiveDisplayList(
                MAX_SCREENS as u32,
                ids.as_mut_ptr() as *mut CGDirectDisplayID,
                &mut count,
            );
            if err != CGError::Success {
                return Err(err);
            }
            std::slice::from_raw_parts(ids.as_ptr() as *const u32, count as usize)
        };
        Ok(ids
            .iter()
            .map(|&cg_id| CGScreenInfo {
                cg_id: ScreenId(cg_id),
                bounds: CGDisplayBounds(cg_id),
            })
            .collect())
    }

    fn display_uuid(&self, screen: &CGScreenInfo) -> CFRetained<CFString> {
        unsafe {
            if let Some(uuid) = NonNull::new(CGDisplayCreateUUIDFromDisplayID(screen.cg_id.0)) {
                let uuid_str = CFUUIDCreateString(std::ptr::null_mut(), uuid.as_ptr());
                CFRelease(uuid.as_ptr());
                if let Some(uuid_str) = NonNull::new(uuid_str) {
                    return CFRetained::from_raw(uuid_str);
                } else {
                    warn!(
                        "CGDisplayCreateUUIDFromDisplayID returned invalid string for {:?}",
                        screen
                    );
                }
            } else {
                warn!(
                    "CGDisplayCreateUUIDFromDisplayID returned null for display {:?}",
                    screen.cg_id
                );
            }
            let managed = CGSCopyBestManagedDisplayForRect(cgs::main_connection(), screen.bounds);
            if let Some(managed) = NonNull::new(managed) {
                CFRetained::from_raw(managed)
            } else {
                warn!(
                    "CGSCopyBestManagedDisplayForRect returned null for display {:?}",
                    screen.cg_id
                );
                CFString::from_str("")
            }
        }
    }

    fn ns_screens(&self) -> Vec<NSScreenInfo> {
        NSScreen::screens(self.mtm)
            .iter()
            .flat_map(|s| {
                let name = s.localizedName().to_string();
                Some(NSScreenInfo {
                    frame: s.frame(),
                    visible_frame: s.visibleFrame(),
                    backing_scale: s.backingScaleFactor(),
                    cg_id: s.get_number().ok()?,
                    name: Some(name),
                })
            })
            .collect()
    }

    fn is_builtin(&self, did: u32) -> bool { unsafe { super::skylight::CGDisplayIsBuiltin(did) } }

    fn notch_height(&self, did: u32) -> f64 {
        let screens = NSScreen::screens(self.mtm);
        let builtin = unsafe { super::skylight::CGDisplayIsBuiltin(did) };
        if !builtin {
            return 0.0;
        }

        for screen in screens {
            if let Ok(screen_id) = screen.get_number() {
                if screen_id.as_u32() == did {
                    #[allow(deprecated)]
                    let insets = screen.safeAreaInsets();
                    return insets.top;
                }
            }
        }
        0.0
    }
}

type CGDirectDisplayID = u32;

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScreenId(CGDirectDisplayID);

impl ScreenId {
    pub fn new(id: u32) -> Self { ScreenId(id) }

    pub fn as_u32(&self) -> u32 { self.0 }
}

pub trait NSScreenExt {
    fn get_number(&self) -> Result<ScreenId, ()>;
}
impl NSScreenExt for NSScreen {
    fn get_number(&self) -> Result<ScreenId, ()> {
        let desc = self.deviceDescription();
        match desc.objectForKey(ns_string!("NSScreenNumber")) {
            Some(val) if unsafe { msg_send![&*val, isKindOfClass:NSNumber::class() ] } => {
                let number: &NSNumber = unsafe { std::mem::transmute(val) };
                Ok(ScreenId(number.as_u32()))
            }
            val => {
                warn!(
                    "Could not get NSScreenNumber for screen with name {:?}: {:?}",
                    self.localizedName(),
                    val,
                );
                Err(())
            }
        }
    }
}

pub fn get_active_space_number() -> Option<SpaceId> {
    active_menu_bar_display_uuid().and_then(|uuid| current_space_for_display_uuid(&uuid))
}

pub fn active_menu_bar_display_uuid() -> Option<String> {
    Some(
        unsafe {
            CFRetained::<CFString>::from_raw(NonNull::new(SLSCopyActiveMenuBarDisplayIdentifier(
                cgs::main_connection(),
            ))?)
        }
        .to_string(),
    )
}

pub fn set_active_menu_bar_display_uuid(display_uuid: &str) -> bool {
    if display_uuid.is_empty() {
        return false;
    }
    #[cfg(test)]
    {
        TEST_ACTIVE_DISPLAY.with(|display| display.replace(Some(display_uuid.to_string())));
        return true;
    }
    #[cfg(not(test))]
    {
        let uuid = CFString::from_str(display_uuid);
        let ptr = CFRetained::as_ptr(&uuid).as_ptr();
        let result = crate::sys::cg_ok(unsafe {
            super::skylight::SLSSetActiveMenuBarDisplayIdentifier(cgs::main_connection(), ptr, ptr)
        });
        if let Err(error) = result {
            warn!(?error, display_uuid, "Failed to activate menu-bar display");
            return false;
        }
        let active = unsafe {
            NonNull::new(SLSCopyActiveMenuBarDisplayIdentifier(cgs::main_connection()))
                .map(|ptr| CFRetained::<CFString>::from_raw(ptr))
        };
        let confirmed = active.as_deref().is_some_and(|active| active == &*uuid);
        if !confirmed {
            debug!(display_uuid, "Menu-bar display activation was not confirmed");
        }
        confirmed
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_ACTIVE_DISPLAY: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

pub fn current_space_for_display_uuid(display_uuid: &str) -> Option<SpaceId> {
    if display_uuid.is_empty() {
        return None;
    }

    let uuid = CFString::from_str(display_uuid);
    let id = unsafe {
        CGSManagedDisplayGetCurrentSpace(
            cgs::main_connection(),
            CFRetained::<CFString>::as_ptr(&uuid).as_ptr(),
        )
    };

    if id == 0 {
        None
    } else {
        Some(SpaceId::new(id as u64))
    }
}

pub fn displays_have_separate_spaces() -> bool {
    unsafe { SLSGetSpaceManagementMode(cgs::main_connection()) == 1 }
}

/// Utilities for querying the current system configuration. For diagnostic purposes only.
#[allow(dead_code)]
pub mod diagnostic {
    use objc2_core_foundation::CFArray;

    use super::*;

    pub fn cur_space() -> SpaceId { SpaceId(unsafe { CGSGetActiveSpace(cgs::main_connection()) }) }

    pub fn visible_spaces() -> CFRetained<CFArray<SpaceId>> {
        unsafe {
            let arr = CGSCopySpaces(cgs::main_connection(), CGSSpaceMask::ALL_VISIBLE_SPACES);
            CFRetained::from_raw(NonNull::new_unchecked(arr))
        }
    }

    pub fn all_spaces() -> CFRetained<CFArray<SpaceId>> {
        unsafe {
            let arr = CGSCopySpaces(cgs::main_connection(), CGSSpaceMask::ALL_SPACES);
            CFRetained::from_raw(NonNull::new_unchecked(arr))
        }
    }

    pub fn managed_displays() -> CFRetained<CFArray> {
        unsafe {
            CFRetained::from_raw(NonNull::new_unchecked(CGSCopyManagedDisplays(
                cgs::main_connection(),
            )))
        }
    }

    pub fn managed_display_spaces() -> Retained<NSArray> {
        unsafe {
            Retained::from_raw(CGSCopyManagedDisplaySpaces(cgs::main_connection()))
                .expect("CGSCopyManagedDisplaySpaces returned null")
        }
    }
}

pub fn order_visible_spaces_by_position(
    spaces: impl IntoIterator<Item = (SpaceId, CGPoint)>,
) -> Vec<SpaceId> {
    let mut spaces: Vec<_> = spaces.into_iter().collect();

    // order spaces by the physical screen coordinates (left-to-right, then bottom-to-top).
    spaces.sort_by(|(_, a_center), (_, b_center)| {
        let x_order = a_center.x.total_cmp(&b_center.x);
        if x_order == Ordering::Equal {
            a_center.y.total_cmp(&b_center.y)
        } else {
            x_order
        }
    });

    spaces.into_iter().map(|(space, _)| space).collect()
}

pub fn managed_display_space_ids() -> HashMap<String, Vec<SpaceId>> {
    let mut out: HashMap<String, Vec<SpaceId>> = HashMap::default();
    unsafe {
        let raw = CGSCopyManagedDisplaySpaces(cgs::main_connection());
        if raw.is_null() {
            return out;
        }

        let displays: CFRetained<CFArray<CFDictionary<CFString, CFType>>> =
            CFRetained::from_raw(std::ptr::NonNull::new_unchecked(raw.cast()));
        let display_id_key = CFString::from_static_str("Display Identifier");
        let spaces_key = CFString::from_static_str("Spaces");
        let managed_space_id_key = CFString::from_static_str("ManagedSpaceID");

        for idx in 0..displays.len() {
            let Some(display_entry) = displays.get(idx) else {
                continue;
            };
            let display_id = display_entry
                .get(&display_id_key)
                .and_then(|v| v.downcast::<CFString>().ok())
                .map(|v| v.to_string())
                .unwrap_or_default();
            if display_id.is_empty() {
                continue;
            }

            let mut space_ids = Vec::new();
            if let Some(spaces_value) = display_entry.get(&spaces_key) {
                if let Ok(spaces) = spaces_value.downcast::<CFArray>() {
                    let spaces = CFRetained::cast_unchecked::<CFArray<CFType>>(spaces);
                    for space_entry in spaces.iter() {
                        if let Ok(space_dict) = space_entry.downcast::<CFDictionary>() {
                            let space_dict = CFRetained::cast_unchecked::<
                                CFDictionary<CFString, CFType>,
                            >(space_dict);
                            let space_id = space_dict
                                .get(&managed_space_id_key)
                                .and_then(|v| v.downcast::<CFNumber>().ok())
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0);
                            if space_id != 0 {
                                space_ids.push(SpaceId::new(space_id as u64));
                            }
                        }
                    }
                }
            }

            out.insert(display_id, space_ids);
        }
    }

    out
}

#[cfg(test)]
mod test {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use objc2_core_foundation::{CFRetained, CFString, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::CGError;

    use super::{CGScreenInfo, NSScreenInfo, ScreenCache, ScreenId, System};
    use crate::sys::screen::{SpaceId, order_visible_spaces_by_position};

    #[test]
    fn ordered_preferences_resolve_identity_geometry_and_eligibility_deterministically() {
        use super::{ScreenInfo, resolve_display_preference};
        let make = |uuid: &str, x, y, builtin| ScreenInfo {
            id: ScreenId::new(1),
            frame: CGRect::new(CGPoint::new(x, y), CGSize::new(100., 100.)),
            backing_scale: 1.,
            is_builtin: builtin,
            display_uuid: uuid.into(),
            name: Some("Built-in".into()),
            space: Some(SpaceId::new(1)),
        };
        let mut screens = vec![
            make("c", 100., 0., false),
            make("b", 0., -100., true),
            make("a", -100., 0., false),
            make("d", -100., 0., false),
        ];
        for _ in 0..screens.len() {
            for (preferences, expected) in [
                (vec!["builtin", "right"], Some("b")),
                (vec!["missing", "right"], Some("c")),
                (vec!["left"], Some("a")),
                (vec!["right"], Some("c")),
                (vec!["top"], Some("b")),
                (vec!["bottom"], Some("a")),
                (vec!["d"], Some("d")),
                (vec!["Built-in"], Some("a")),
                (vec!["missing"], None),
            ] {
                let values = preferences.iter().map(|v| v.to_string()).collect::<Vec<_>>();
                assert_eq!(
                    resolve_display_preference(&values, &screens, |_| true)
                        .map(|s| s.display_uuid.as_str()),
                    expected,
                    "{preferences:?}"
                );
            }
            let values = vec!["builtin".into(), "right".into()];
            assert_eq!(
                resolve_display_preference(&values, &screens, |s| !s.is_builtin)
                    .unwrap()
                    .display_uuid,
                "c"
            );
            assert!(resolve_display_preference(&values, &screens, |_| false).is_none());
            screens.rotate_left(1);
        }
    }

    #[test]
    fn auto_hidden_menu_bar_does_not_inset_a_display_without_a_notch() {
        assert_eq!(super::menu_bar_inset(true, 24.0, 0.0), 0.0);
    }

    #[test]
    fn auto_hidden_menu_bar_preserves_the_notch_safe_area() {
        assert_eq!(super::menu_bar_inset(true, 37.0, 32.0), 32.0);
    }

    #[test]
    fn visible_menu_bar_reserves_its_reported_height() {
        assert_eq!(super::menu_bar_inset(false, 24.0, 0.0), 25.0);
    }

    struct Stub {
        main_id: u32,
        cg_screens: Vec<CGScreenInfo>,
        ns_screens: Vec<NSScreenInfo>,
    }
    impl System for Stub {
        fn main_display_id(&self) -> u32 { self.main_id }

        fn cg_screens(&self) -> Result<Vec<CGScreenInfo>, CGError> { Ok(self.cg_screens.clone()) }

        fn display_uuid(&self, screen: &CGScreenInfo) -> CFRetained<CFString> {
            CFString::from_str(&format!("stub-{}", screen.cg_id.0))
        }

        fn ns_screens(&self) -> Vec<NSScreenInfo> { self.ns_screens.clone() }

        fn notch_height(&self, _did: u32) -> f64 { 0.0 }
    }

    struct SequenceSystem {
        cg_screens: RefCell<VecDeque<Result<Vec<CGScreenInfo>, CGError>>>,
        uuids: RefCell<VecDeque<CFRetained<CFString>>>,
    }

    impl SequenceSystem {
        fn new(samples: Vec<Result<Vec<CGScreenInfo>, CGError>>, uuids: Vec<&str>) -> Self {
            Self {
                cg_screens: RefCell::new(samples.into()),
                uuids: RefCell::new(uuids.into_iter().map(CFString::from_str).collect()),
            }
        }
    }

    impl System for SequenceSystem {
        fn main_display_id(&self) -> u32 { 1 }

        fn cg_screens(&self) -> Result<Vec<CGScreenInfo>, CGError> {
            self.cg_screens.borrow_mut().pop_front().expect("unexpected display sample")
        }

        fn display_uuid(&self, _screen: &CGScreenInfo) -> CFRetained<CFString> {
            self.uuids.borrow_mut().pop_front().expect("unexpected UUID read")
        }

        fn ns_screens(&self) -> Vec<NSScreenInfo> { Vec::new() }
    }

    #[test]
    fn it_calculates_the_visible_frame() {
        let stub = Stub {
            main_id: 3,
            cg_screens: vec![
                CGScreenInfo {
                    cg_id: ScreenId(1),
                    bounds: CGRect::new(CGPoint::new(3840.0, 1080.0), CGSize::new(1512.0, 982.0)),
                },
                CGScreenInfo {
                    cg_id: ScreenId(3),
                    bounds: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(3840.0, 2160.0)),
                },
            ],
            ns_screens: vec![
                NSScreenInfo {
                    cg_id: ScreenId(3),
                    frame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(3840.0, 2160.0)),
                    backing_scale: 1.0,
                    visible_frame: CGRect::new(
                        CGPoint::new(0.0, 76.0),
                        CGSize::new(3840.0, 2059.0),
                    ),
                    name: None,
                },
                NSScreenInfo {
                    cg_id: ScreenId(1),
                    frame: CGRect::new(CGPoint::new(3840.0, 98.0), CGSize::new(1512.0, 982.0)),
                    backing_scale: 2.0,
                    visible_frame: CGRect::new(
                        CGPoint::new(3840.0, 98.0),
                        CGSize::new(1512.0, 950.0),
                    ),
                    name: None,
                },
            ],
        };
        let mut sc = ScreenCache::new_with(stub);
        let (screens, _) = sc.refresh().unwrap();

        let secondary = screens.iter().find(|screen| screen.id == ScreenId(1)).unwrap();
        assert_eq!(secondary.backing_scale, 2.0);
        assert_eq!(
            secondary.frame,
            super::constrain_display_bounds(
                1,
                CGRect::new(CGPoint::new(3840.0, 1080.0), CGSize::new(1512.0, 982.0)),
                0.0,
            )
        );

        let primary = screens.iter().find(|screen| screen.id == ScreenId(3)).unwrap();
        assert_eq!(
            primary.frame,
            super::constrain_display_bounds(
                3,
                CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(3840.0, 2160.0)),
                0.0,
            )
        );
        // Changing the primary and rearranging a taller secondary must preserve
        // the exact reserved rectangle, regardless of backing scale.
        let rect = |x, y, w, h| CGRect::new(CGPoint::new(x, y), CGSize::new(w, h));
        for secondary in [
            None,
            Some(rect(-1080., 0., 1080., 1920.)),
            Some(rect(1440., 0., 1080., 1920.)),
            Some(rect(-1080., -500., 1080., 1920.)),
            Some(rect(0., -1080., 1920., 1080.)),
            Some(rect(0., 900., 1920., 1080.)),
        ] {
            for scale in [1., 2.] {
                sc.system.main_id += 1;
                let id = sc.system.main_id;
                sc.system.cg_screens = vec![CGScreenInfo {
                    cg_id: ScreenId(id),
                    bounds: rect(0., 0., 1440., 900.),
                }];
                if let Some(bounds) = secondary {
                    sc.system.cg_screens.push(CGScreenInfo { cg_id: ScreenId(1), bounds });
                }
                sc.system.ns_screens[0].cg_id = ScreenId(id);
                for screen in &mut sc.system.ns_screens {
                    screen.backing_scale = scale;
                }
                sc.mark_sleeping(true);
                sc.mark_sleeping(false);
                let (_, converter) = sc.refresh().unwrap();
                for display in &sc.system.cg_screens {
                    let (x, y) = (display.bounds.origin.x + 100., display.bounds.origin.y + 100.);
                    let (w, h) = (
                        display.bounds.size.width - 200.,
                        display.bounds.size.height - 200.,
                    );
                    use crate::common::config::HorizontalPlacement::*;
                    use crate::common::config::StackLineReserve;
                    use crate::common::config::VerticalPlacement::*;
                    for (horizontal, horiz, vert, expected) in [
                        (true, Top, Left, rect(x, 860. - y, w, 40.)),
                        (true, Bottom, Left, rect(x, 900. - y - h, w, 40.)),
                        (false, Top, Left, rect(x, 900. - y - h, 40., h)),
                        (false, Top, Right, rect(x + w - 40., 900. - y - h, 40., h)),
                    ] {
                        let reserve = StackLineReserve {
                            thickness: 40.,
                            spacing: 0.,
                            horiz,
                            vert,
                        };
                        assert_eq!(
                            converter.convert_rect(reserve.bar(rect(x, y, w, h), horizontal)),
                            Some(expected)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_display_samples_preserve_cache_and_retry_without_another_notification() {
        let bounds = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1440., 900.));
        let screen = |id| CGScreenInfo { cg_id: ScreenId(id), bounds };
        for (case, sample, invalid_uuids) in [
            ("empty list", Ok(vec![]), vec![]),
            ("missing main", Ok(vec![screen(2)]), vec![]),
            ("empty UUID", Ok(vec![screen(1)]), vec![""]),
            ("duplicate UUID", Ok(vec![screen(1), screen(2)]), vec![
                "same", "same",
            ]),
            ("CG failure", Err(CGError::Failure), vec![]),
        ] {
            let mut uuids = vec!["original"];
            uuids.extend(invalid_uuids);
            uuids.extend(["returned-main", "new-display"]);
            let system = SequenceSystem::new(
                vec![Ok(vec![screen(1)]), sample, Ok(vec![screen(1), screen(2)])],
                uuids,
            );
            let mut cache = ScreenCache::new_with(system);
            let (original, converter) = cache.refresh().unwrap();
            cache.mark_dirty();
            assert!(cache.refresh().is_none(), "{case}");
            assert_eq!(cache.state.as_ref().unwrap().screens, original, "{case}");
            assert_eq!(
                cache.uuids.iter().map(|uuid| uuid.to_string()).collect::<Vec<_>>(),
                vec!["original"],
                "{case}"
            );
            assert_eq!(
                cache.state.as_ref().unwrap().converter.screen_height(),
                converter.screen_height(),
                "{case}"
            );

            // The rejected sample must leave its rebuild pending: no mark_dirty here.
            let (recovered, _) = cache.refresh().unwrap();
            assert_eq!(
                recovered.iter().map(|s| s.display_uuid.as_str()).collect::<Vec<_>>(),
                vec!["returned-main", "new-display"],
                "{case}"
            );
            assert_eq!(cache.processed_generation, cache.pending_generation, "{case}");
        }
    }

    #[test]
    fn sleeping_keeps_the_last_snapshot_and_waking_rebuilds_without_a_display_notification() {
        let screen = |height| CGScreenInfo {
            cg_id: ScreenId(1),
            bounds: CGRect::new(CGPoint::new(0., 0.), CGSize::new(1440., height)),
        };
        let system =
            SequenceSystem::new(vec![Ok(vec![screen(900.)]), Ok(vec![screen(1080.)])], vec![
                "before-sleep",
                "after-wake",
            ]);
        let mut cache = ScreenCache::new_with(system);
        let (before, _) = cache.refresh().unwrap();
        cache.mark_sleeping(true);
        assert_eq!(cache.refresh().unwrap().0, before);
        assert_eq!(cache.refresh().unwrap().0, before);
        assert_eq!(cache.system.cg_screens.borrow().len(), 1);
        cache.mark_sleeping(false);
        let (after, converter) = cache.refresh().unwrap();
        assert_eq!(after[0].display_uuid, "after-wake");
        assert_eq!(converter.screen_height(), Some(1080.));
        assert!(cache.system.cg_screens.borrow().is_empty());
    }

    #[test]
    fn orders_shuffled_spaces_by_horizontal_then_vertical_position() {
        let spaces = vec![
            (SpaceId::new(4), CGPoint::new(500., -300.)),
            (SpaceId::new(3), CGPoint::new(0., 150.)),
            (SpaceId::new(1), CGPoint::new(-500., 200.)),
            (SpaceId::new(2), CGPoint::new(0., -200.)),
        ];
        assert_eq!(order_visible_spaces_by_position(spaces), vec![
            SpaceId::new(1),
            SpaceId::new(2),
            SpaceId::new(3),
            SpaceId::new(4)
        ]);
    }
}
