use std::cmp::Ordering;
use std::f64;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::{ClassType, msg_send};
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CFRetained, CFString, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGDisplayBounds, CGError, CGGetActiveDisplayList, CGMainDisplayID};
use objc2_foundation::{MainThreadMarker, NSArray, NSNumber, ns_string};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::skylight::{
    CFRelease, CFUUIDCreateFromString, CFUUIDCreateString, CGDisplayCreateUUIDFromDisplayID,
    CGDisplayGetDisplayIDFromUUID, CGSCopyBestManagedDisplayForRect, CGSCopyManagedDisplaySpaces,
    CGSCopyManagedDisplays, CGSCopySpaces, CGSGetActiveSpace, CGSManagedDisplayGetCurrentSpace,
    CGSSpaceMask, CoreDockGetAutoHideEnabled, CoreDockGetOrientationAndPinning, G_CONNECTION,
    SLSGetDisplayMenubarHeight, SLSGetDockRectWithReason, SLSGetMenuBarAutohideEnabled,
    SLSGetSpaceManagementMode, SLSMainConnectionID,
};

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

pub struct ScreenCache<S: System = Actual> {
    system: S,
    uuids: Vec<CFRetained<CFString>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScreenDescriptor {
    pub id: ScreenId,
    pub frame: CGRect,
    pub display_uuid: String,
    pub name: Option<String>,
}

impl ScreenCache<Actual> {
    pub fn new(mtm: MainThreadMarker) -> Self { Self::new_with(Actual { mtm }) }
}

impl<S: System> ScreenCache<S> {
    fn new_with(system: S) -> ScreenCache<S> { ScreenCache { uuids: vec![], system } }

    /// Returns a list containing the usable frame for each screen.
    ///
    /// This method must be called when there is an update to the screen
    /// configuration. It updates the internal cache so that calls to
    /// screen_spaces are fast.
    ///
    /// The main screen (if any) is always first. Note that there may be no
    /// screens.
    #[forbid(unsafe_code)]
    pub fn update_screen_config(&mut self) -> Option<(Vec<ScreenDescriptor>, CoordinateConverter)> {
        let ns_screens = self.system.ns_screens();
        debug!("ns_screens={ns_screens:?}");
        let mut cg_screens = self.system.cg_screens().unwrap();
        debug!("cg_screens={cg_screens:?}");

        if cg_screens.is_empty() {
            self.uuids.clear();
            return Some((vec![], CoordinateConverter::default()));
        }

        cg_screens.sort_by(|a, b| {
            let x_order = a.bounds.origin.x.total_cmp(&b.bounds.origin.x);
            if x_order == Ordering::Equal {
                a.bounds.origin.y.total_cmp(&b.bounds.origin.y)
            } else {
                x_order
            }
        });

        let main_id = CGMainDisplayID();
        if let Some(main_screen_idx) = cg_screens.iter().position(|s| s.cg_id.0 == main_id) {
            cg_screens.swap(0, main_screen_idx);
        } else {
            warn!("Could not find main screen. cg_screens={cg_screens:?}");
        }

        self.uuids = cg_screens.iter().map(|screen| self.system.display_uuid(screen)).collect();
        let uuid_strings: Vec<String> = self.uuids.iter().map(|uuid| uuid.to_string()).collect();

        let union_max_y = cg_screens
            .iter()
            .map(|screen| screen.bounds.max().y)
            .fold(f64::NEG_INFINITY, f64::max);

        let converter = CoordinateConverter { screen_height: union_max_y };

        let descriptors = cg_screens
            .iter()
            .enumerate()
            .map(|(idx, &CGScreenInfo { cg_id, bounds })| {
                let notch_height = self.system.notch_height(cg_id.as_u32());
                let frame = constrain_display_bounds(cg_id.as_u32(), bounds, notch_height);
                let display_uuid =
                    uuid_strings.get(idx).cloned().filter(|uuid| !uuid.is_empty()).unwrap_or_else(
                        || {
                            warn!("Missing cached UUID for {:?}; using fallback", cg_id);
                            format!("cgdisplay-{}", cg_id.as_u32())
                        },
                    );
                ScreenDescriptor {
                    id: cg_id,
                    frame,
                    display_uuid,
                    name: ns_screens.iter().find(|s| s.cg_id == cg_id).and_then(|s| s.name.clone()),
                }
            })
            .collect();
        Some((descriptors, converter))
    }

    /// Returns a list of the active spaces on each screen. The order
    /// corresponds to the screens returned by `screen_frames`.
    pub fn get_screen_spaces(&self) -> Vec<Option<SpaceId>> {
        self.uuids
            .iter()
            .map(|screen| unsafe {
                CGSManagedDisplayGetCurrentSpace(
                    SLSMainConnectionID(),
                    CFRetained::<objc2_core_foundation::CFString>::as_ptr(&screen).as_ptr(),
                )
            })
            .map(|id| if id == 0 { None } else { Some(SpaceId(id)) })
            .collect()
    }
}

const DOCK_ORIENTATION_LEFT: i32 = 1;
const DOCK_ORIENTATION_BOTTOM: i32 = 2;
const DOCK_ORIENTATION_RIGHT: i32 = 3;

fn menu_bar_hidden() -> bool {
    let mut status = 0;
    unsafe { SLSGetMenuBarAutohideEnabled(*G_CONNECTION, &mut status) };
    status != 0
}

fn menu_bar_height(did: u32) -> f64 {
    let mut height: u32 = 0;
    unsafe { SLSGetDisplayMenubarHeight(did, &mut height) };
    height as f64
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

    if !menu_bar_hidden() {
        // macOS reports the menubar height without the topmost usable pixel; add 1 to avoid
        // leaving a dead strip or placing windows under the bar.
        let h = menu_bar_height(did) + 1.0;
        if h > 0.0 {
            frame.origin.y += h;
            frame.size.height = (frame.size.height - h).max(0.0);
        }
    } else if notch_height > 0.0 {
        frame.origin.y += notch_height;
        frame.size.height = (frame.size.height - notch_height).max(0.0);
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
    fn cg_screens(&self) -> Result<Vec<CGScreenInfo>, CGError>;
    fn display_uuid(&self, screen: &CGScreenInfo) -> CFRetained<CFString>;
    fn ns_screens(&self) -> Vec<NSScreenInfo>;
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
            let managed = CGSCopyBestManagedDisplayForRect(SLSMainConnectionID(), screen.bounds);
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
                    cg_id: s.get_number().ok()?,
                    name: Some(name),
                })
            })
            .collect()
    }

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

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]
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
    let active_id = unsafe { CGSGetActiveSpace(SLSMainConnectionID()) };
    if active_id == 0 {
        None
    } else {
        Some(SpaceId::new(active_id))
    }
}

pub fn displays_have_separate_spaces() -> bool {
    unsafe { SLSGetSpaceManagementMode(SLSMainConnectionID()) == 1 }
}

/// Utilities for querying the current system configuration. For diagnostic purposes only.
#[allow(dead_code)]
pub mod diagnostic {
    use objc2_core_foundation::CFArray;

    use super::*;

    pub fn cur_space() -> SpaceId { SpaceId(unsafe { CGSGetActiveSpace(SLSMainConnectionID()) }) }

    pub fn visible_spaces() -> CFRetained<CFArray<SpaceId>> {
        unsafe {
            let arr = CGSCopySpaces(SLSMainConnectionID(), CGSSpaceMask::ALL_VISIBLE_SPACES);
            CFRetained::from_raw(NonNull::new_unchecked(arr))
        }
    }

    pub fn all_spaces() -> CFRetained<CFArray<SpaceId>> {
        unsafe {
            let arr = CGSCopySpaces(SLSMainConnectionID(), CGSSpaceMask::ALL_SPACES);
            CFRetained::from_raw(NonNull::new_unchecked(arr))
        }
    }

    pub fn managed_displays() -> CFRetained<CFArray> {
        unsafe {
            CFRetained::from_raw(NonNull::new_unchecked(CGSCopyManagedDisplays(
                SLSMainConnectionID(),
            )))
        }
    }

    pub fn managed_display_spaces() -> Retained<NSArray> {
        unsafe {
            Retained::from_raw(CGSCopyManagedDisplaySpaces(SLSMainConnectionID()))
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

#[cfg(test)]
mod test {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use objc2_core_foundation::{CFRetained, CFString, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::CGError;

    use super::{CGScreenInfo, NSScreenInfo, ScreenCache, ScreenId, System};
    use crate::sys::screen::{SpaceId, order_visible_spaces_by_position};

    struct Stub {
        cg_screens: Vec<CGScreenInfo>,
        ns_screens: Vec<NSScreenInfo>,
    }
    impl System for Stub {
        fn cg_screens(&self) -> Result<Vec<CGScreenInfo>, CGError> { Ok(self.cg_screens.clone()) }

        fn display_uuid(&self, _screen: &CGScreenInfo) -> CFRetained<CFString> {
            CFString::from_str("stub")
        }

        fn ns_screens(&self) -> Vec<NSScreenInfo> { self.ns_screens.clone() }

        fn notch_height(&self, _did: u32) -> f64 { 0.0 }
    }

    struct SequenceSystem {
        cg_screens: RefCell<VecDeque<Vec<CGScreenInfo>>>,
        ns_screens: RefCell<VecDeque<Vec<NSScreenInfo>>>,
        uuids: RefCell<VecDeque<CFRetained<CFString>>>,
    }

    impl SequenceSystem {
        fn new(
            cg_screens: Vec<Vec<CGScreenInfo>>,
            ns_screens: Vec<Vec<NSScreenInfo>>,
            uuids: Vec<CFRetained<CFString>>,
        ) -> Self {
            Self {
                cg_screens: RefCell::new(VecDeque::from(cg_screens)),
                ns_screens: RefCell::new(VecDeque::from(ns_screens)),
                uuids: RefCell::new(VecDeque::from(uuids)),
            }
        }
    }

    impl System for SequenceSystem {
        fn cg_screens(&self) -> Result<Vec<CGScreenInfo>, CGError> {
            Ok(self.cg_screens.borrow_mut().pop_front().unwrap_or_default())
        }

        fn display_uuid(&self, _screen: &CGScreenInfo) -> CFRetained<CFString> {
            self.uuids
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| CFString::from_str("missing-uuid"))
        }

        fn ns_screens(&self) -> Vec<NSScreenInfo> {
            self.ns_screens.borrow_mut().pop_front().unwrap_or_default()
        }

        fn notch_height(&self, _did: u32) -> f64 { 0.0 }
    }

    #[test]
    fn it_calculates_the_visible_frame() {
        let stub = Stub {
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
                    visible_frame: CGRect::new(
                        CGPoint::new(0.0, 76.0),
                        CGSize::new(3840.0, 2059.0),
                    ),
                    name: None,
                },
                NSScreenInfo {
                    cg_id: ScreenId(1),
                    frame: CGRect::new(CGPoint::new(3840.0, 98.0), CGSize::new(1512.0, 982.0)),
                    visible_frame: CGRect::new(
                        CGPoint::new(3840.0, 98.0),
                        CGSize::new(1512.0, 950.0),
                    ),
                    name: None,
                },
            ],
        };
        let mut sc = ScreenCache::new_with(stub);
        let (descriptors, _) = sc.update_screen_config().unwrap();
        let frames: Vec<CGRect> = descriptors.iter().map(|d| d.frame).collect();
        assert_eq!(
            vec![
                CGRect::new(CGPoint::new(0.0, 25.0), CGSize::new(3840.0, 2059.0)),
                CGRect::new(CGPoint::new(3840.0, 1112.0), CGSize::new(1512.0, 950.0)),
            ],
            frames
        );
    }

    #[test]
    fn clears_cached_screen_identifiers_when_display_list_is_empty() {
        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1440.0, 900.0));
        let visible_frame = CGRect::new(CGPoint::new(0.0, 22.0), CGSize::new(1440.0, 878.0));

        let system = SequenceSystem::new(
            vec![vec![CGScreenInfo { cg_id: ScreenId(1), bounds }], vec![]],
            vec![
                vec![NSScreenInfo {
                    cg_id: ScreenId(1),
                    frame: bounds,
                    visible_frame,
                    name: None,
                }],
                vec![],
            ],
            vec![CFString::from_str("uuid-1")],
        );

        let mut cache = ScreenCache::new_with(system);

        let (descriptors, _) = cache.update_screen_config().unwrap();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(cache.uuids.len(), 1);

        let (descriptors, converter) = cache.update_screen_config().unwrap();
        assert!(descriptors.is_empty());
        assert!(cache.uuids.is_empty());
        assert!(converter.convert_point(CGPoint::new(0.0, 0.0)).is_none());
    }

    #[test]
    fn orders_spaces_by_horizontal_position() {
        let spaces = vec![
            (SpaceId::new(1), CGPoint::new(-500.0, 0.0)),
            (SpaceId::new(2), CGPoint::new(0.0, 0.0)),
            (SpaceId::new(3), CGPoint::new(500.0, 100.0)),
        ];

        let ordered = order_visible_spaces_by_position(spaces);
        assert_eq!(ordered, vec![SpaceId::new(1), SpaceId::new(2), SpaceId::new(3)]);
    }

    #[test]
    fn orders_spaces_by_vertical_position_when_aligned() {
        let spaces = vec![
            (SpaceId::new(10), CGPoint::new(0.0, -200.0)),
            (SpaceId::new(11), CGPoint::new(0.0, 150.0)),
        ];

        let ordered = order_visible_spaces_by_position(spaces);
        assert_eq!(ordered, vec![SpaceId::new(10), SpaceId::new(11)]);
    }
}
