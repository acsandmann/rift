use std::f64;
use std::mem::MaybeUninit;
use std::num::NonZeroU64;

use core_foundation::array::CFArray;
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_graphics::display::{CGDisplayBounds, CGGetActiveDisplayList};
use core_graphics_types::base::{CGError, kCGErrorSuccess};
use objc2::{ClassType, msg_send};
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::{MainThreadMarker, NSNumber, ns_string};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::skylight::{
    CGSCopyBestManagedDisplayForRect, CGSCopyManagedDisplaySpaces, CGSCopyManagedDisplays,
    CGSCopySpaces, CGSGetActiveSpace, CGSMainConnectionID, CGSManagedDisplayGetCurrentSpace,
    CGSSpaceMask,
};
use crate::sys::geometry::ToICrate;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct SpaceId(NonZeroU64);

impl SpaceId {
    pub fn new(id: u64) -> SpaceId { SpaceId(NonZeroU64::new(id).unwrap()) }

    pub fn get(&self) -> u64 { self.0.get() }
}

impl ToString for SpaceId {
    fn to_string(&self) -> String { self.get().to_string() }
}

pub struct ScreenCache<S: System = Actual> {
    system: S,
    uuids: Vec<CFString>,
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
    pub fn update_screen_config(&mut self) -> (Vec<CGRect>, Vec<ScreenId>, CoordinateConverter) {
        let mut cg_screens = self.system.cg_screens().unwrap();
        debug!("cg_screens={cg_screens:?}");
        if cg_screens.is_empty() {
            return (vec![], vec![], CoordinateConverter::default());
        };

        if let Some(main_screen_idx) =
            cg_screens.iter().position(|s| s.bounds.origin == CGPoint::ZERO)
        {
            cg_screens.swap(0, main_screen_idx);
        } else {
            warn!("Could not find main screen. cg_screens={cg_screens:?}");
        }

        self.uuids = cg_screens
            .iter()
            .map(|screen| self.system.uuid_for_rect(screen.bounds))
            .collect();

        let ns_screens = self.system.ns_screens();
        debug!("ns_screens={ns_screens:?}");

        let converter = CoordinateConverter {
            screen_height: cg_screens[0].bounds.max().y,
        };

        let (visible_frames, ids) = cg_screens
            .iter()
            .flat_map(|&CGScreenInfo { cg_id, .. }| {
                let Some(ns_screen) = ns_screens.iter().find(|s| s.cg_id == cg_id) else {
                    warn!("Can't find NSScreen corresponding to {cg_id:?}");
                    return None;
                };
                let converted = converter.convert_rect(ns_screen.visible_frame).unwrap();
                Some((converted, cg_id))
            })
            .unzip();
        (visible_frames, ids, converter)
    }

    /// Returns a list of the active spaces on each screen. The order
    /// corresponds to the screens returned by `screen_frames`.
    pub fn get_screen_spaces(&self) -> Vec<Option<SpaceId>> {
        self.uuids
            .iter()
            .map(|screen| unsafe {
                CGSManagedDisplayGetCurrentSpace(
                    CGSMainConnectionID(),
                    screen.as_concrete_TypeRef(),
                )
            })
            .map(|id| Some(SpaceId(NonZeroU64::new(id)?)))
            .collect()
    }
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
    fn uuid_for_rect(&self, rect: CGRect) -> CFString;
    fn ns_screens(&self) -> Vec<NSScreenInfo>;
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
            if err != kCGErrorSuccess {
                return Err(err);
            }
            std::slice::from_raw_parts(ids.as_ptr() as *const u32, count as usize)
        };
        Ok(ids
            .iter()
            .map(|&cg_id| CGScreenInfo {
                cg_id: ScreenId(cg_id),
                bounds: unsafe { CGDisplayBounds(cg_id).to_icrate() },
            })
            .collect())
    }

    fn uuid_for_rect(&self, rect: CGRect) -> CFString {
        unsafe {
            CFString::wrap_under_create_rule(CGSCopyBestManagedDisplayForRect(
                CGSMainConnectionID(),
                rect,
            ))
        }
    }

    fn ns_screens(&self) -> Vec<NSScreenInfo> {
        NSScreen::screens(self.mtm)
            .iter()
            .flat_map(|s| {
                Some(NSScreenInfo {
                    frame: s.frame(),
                    visible_frame: s.visibleFrame(),
                    cg_id: s.get_number().ok()?,
                })
            })
            .collect()
    }
}

type CGDirectDisplayID = u32;

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]
pub struct ScreenId(CGDirectDisplayID);

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
                    unsafe { self.localizedName() },
                    val,
                );
                Err(())
            }
        }
    }
}

/// Utilities for querying the current system configuration. For diagnostic purposes only.
#[allow(dead_code)]
pub mod diagnostic {
    use super::*;

    pub fn cur_space() -> SpaceId {
        SpaceId(NonZeroU64::new(unsafe { CGSGetActiveSpace(CGSMainConnectionID()) }).unwrap())
    }

    pub fn visible_spaces() -> CFArray<SpaceId> {
        unsafe {
            let arr = CGSCopySpaces(CGSMainConnectionID(), CGSSpaceMask::ALL_VISIBLE_SPACES);
            CFArray::wrap_under_create_rule(arr)
        }
    }

    pub fn all_spaces() -> CFArray<SpaceId> {
        unsafe {
            let arr = CGSCopySpaces(CGSMainConnectionID(), CGSSpaceMask::ALL_SPACES);
            CFArray::wrap_under_create_rule(arr)
        }
    }

    pub fn managed_displays() -> CFArray {
        unsafe { CFArray::wrap_under_create_rule(CGSCopyManagedDisplays(CGSMainConnectionID())) }
    }

    pub fn managed_display_spaces() -> CFArray<SpaceId> {
        unsafe {
            CFArray::wrap_under_create_rule(CGSCopyManagedDisplaySpaces(CGSMainConnectionID()))
        }
    }
}

#[cfg(test)]
mod test {
    use core_foundation::string::CFString;
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::{CGScreenInfo, NSScreenInfo, ScreenCache, ScreenId, System};

    struct Stub {
        cg_screens: Vec<CGScreenInfo>,
        ns_screens: Vec<NSScreenInfo>,
    }
    impl System for Stub {
        fn cg_screens(&self) -> Result<Vec<CGScreenInfo>, core_graphics_types::base::CGError> {
            Ok(self.cg_screens.clone())
        }

        fn ns_screens(&self) -> Vec<NSScreenInfo> { self.ns_screens.clone() }

        fn uuid_for_rect(&self, _rect: CGRect) -> CFString { CFString::new("stub") }
    }

    #[test]
    fn it_calculates_the_visible_frame() {
        println!("test");
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
                },
                NSScreenInfo {
                    cg_id: ScreenId(1),
                    frame: CGRect::new(CGPoint::new(3840.0, 98.0), CGSize::new(1512.0, 982.0)),
                    visible_frame: CGRect::new(
                        CGPoint::new(3840.0, 98.0),
                        CGSize::new(1512.0, 950.0),
                    ),
                },
            ],
        };
        let mut sc = ScreenCache::new_with(stub);
        assert_eq!(
            vec![
                CGRect::new(CGPoint::new(0.0, 25.0), CGSize::new(3840.0, 2059.0)),
                CGRect::new(CGPoint::new(3840.0, 1112.0), CGSize::new(1512.0, 950.0)),
            ],
            sc.update_screen_config().0
        );
    }
}
