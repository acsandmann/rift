//! Private CGS gesture and IOHID event helpers.
//!
//! A physical `kCGSEventGesture` already carries its backing IOHID digitizer
//! collection. Read that collection directly instead of materializing an
//! `NSEvent`/`NSTouch` graph.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_core_graphics::{CGEvent, CGEventField, CGEventType};

pub const CGS_EVENT_GESTURE: u32 = 29;
pub const CGS_EVENT_DOCK_CONTROL: u32 = 30;
pub const EVENT_MASK: u64 = (1u64 << CGS_EVENT_GESTURE) | (1u64 << CGS_EVENT_DOCK_CONTROL);

const K_CGS_EVENT_TYPE_FIELD: CGEventField = CGEventField(55);
const K_GESTURE_HID_TYPE_FIELD: CGEventField = CGEventField(110);
const K_GESTURE_SWIPE_MOTION_FIELD: CGEventField = CGEventField(123);
const K_GESTURE_PHASE_FIELD: CGEventField = CGEventField(132);

const K_IOHID_EVENT_TYPE_DIGITIZER: u32 = 11;
const K_IOHID_EVENT_TYPE_DOCK_SWIPE: i64 = 23;
const K_CG_GESTURE_MOTION_HORIZONTAL: i64 = 1;

const K_IOHID_EVENT_FIELD_DIGITIZER_X: u32 = K_IOHID_EVENT_TYPE_DIGITIZER << 16;
const K_IOHID_EVENT_FIELD_DIGITIZER_Y: u32 = K_IOHID_EVENT_FIELD_DIGITIZER_X + 1;
const K_IOHID_EVENT_FIELD_DIGITIZER_EVENT_MASK: u32 = K_IOHID_EVENT_FIELD_DIGITIZER_X + 7;
const K_IOHID_EVENT_FIELD_DIGITIZER_TOUCH: u32 = K_IOHID_EVENT_FIELD_DIGITIZER_X + 9;
const K_IOHID_DIGITIZER_EVENT_POSITION: isize = 1 << 2;

const K_GESTURE_PHASE_BEGAN: i64 = 1;
const K_GESTURE_PHASE_ENDED: i64 = 4;
const K_GESTURE_PHASE_CANCELLED: i64 = 8;

type CFArrayRef = *const c_void;
type CFIndex = isize;
type IOHIDEventRef = *mut c_void;
type IOHIDEventField = u32;
type IOHIDFloat = f64;

unsafe extern "C" {
    fn CGEventCopyIOHIDEvent(event: *const CGEvent) -> IOHIDEventRef;

    fn IOHIDEventGetType(event: IOHIDEventRef) -> u32;
    fn IOHIDEventGetChildren(event: IOHIDEventRef) -> CFArrayRef;
    fn IOHIDEventGetIntegerValue(event: IOHIDEventRef, field: IOHIDEventField) -> CFIndex;
    fn IOHIDEventGetFloatValue(event: IOHIDEventRef, field: IOHIDEventField) -> IOHIDFloat;

    fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;
    fn CFRelease(value: *const c_void);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TouchFrame {
    pub contacts: usize,
    pub centroid_x: f64,
    pub centroid_y: f64,
    pub all_moved: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct EventPhase(i64);

impl EventPhase {
    #[inline(always)]
    pub fn began(self) -> bool { self.0 & K_GESTURE_PHASE_BEGAN != 0 }

    #[inline(always)]
    pub fn terminal(self) -> bool {
        self.0 & (K_GESTURE_PHASE_ENDED | K_GESTURE_PHASE_CANCELLED) != 0
    }
}

#[inline(always)]
pub fn is_gesture(event_type: CGEventType) -> bool { event_type.0 == CGS_EVENT_GESTURE }

#[inline(always)]
pub fn phase(event: &CGEvent) -> EventPhase {
    EventPhase(CGEvent::integer_value_field(Some(event), K_GESTURE_PHASE_FIELD))
}

#[inline]
pub fn is_physical_horizontal_dock_swipe(event_type: CGEventType, event: &CGEvent) -> bool {
    let cgs_type = CGEvent::integer_value_field(Some(event), K_CGS_EVENT_TYPE_FIELD);
    let hid_type = CGEvent::integer_value_field(Some(event), K_GESTURE_HID_TYPE_FIELD);
    let motion = CGEvent::integer_value_field(Some(event), K_GESTURE_SWIPE_MOTION_FIELD);

    (event_type.0 == CGS_EVENT_DOCK_CONTROL || cgs_type == CGS_EVENT_DOCK_CONTROL as i64)
        && hid_type == K_IOHID_EVENT_TYPE_DOCK_SWIPE
        && motion == K_CG_GESTURE_MOTION_HORIZONTAL
}

/// Extract the current physical contact frame from a type-29 CG gesture event.
///
/// The backing HID event is a digitizer collection whose parent X/Y is the
/// centroid of its touching children. Counting children still lets us preserve
/// Rift's configurable finger count and "all fingers moved" scroll semantics,
/// while avoiding two X/Y field reads per finger.
#[inline]
pub fn touch_frame(event: &CGEvent) -> Option<TouchFrame> {
    let hid = HidEvent::copy_from(event)?;
    unsafe { TouchFrame::from_hid(hid.as_ptr()) }
}

impl TouchFrame {
    #[inline]
    unsafe fn from_hid(hid: IOHIDEventRef) -> Option<Self> {
        if unsafe { IOHIDEventGetType(hid) } != K_IOHID_EVENT_TYPE_DIGITIZER {
            return None;
        }

        let children = unsafe { IOHIDEventGetChildren(hid) };
        if children.is_null() {
            return None;
        }

        let count = unsafe { CFArrayGetCount(children) };
        if count <= 0 {
            return Some(Self::default());
        }

        let mut contacts = 0usize;
        let mut all_moved = true;

        for index in 0..count {
            let child = unsafe { CFArrayGetValueAtIndex(children, index) } as IOHIDEventRef;
            if child.is_null()
                || unsafe { IOHIDEventGetType(child) } != K_IOHID_EVENT_TYPE_DIGITIZER
                || unsafe { IOHIDEventGetIntegerValue(child, K_IOHID_EVENT_FIELD_DIGITIZER_TOUCH) }
                    == 0
            {
                continue;
            }

            contacts += 1;
            let mask = unsafe {
                IOHIDEventGetIntegerValue(child, K_IOHID_EVENT_FIELD_DIGITIZER_EVENT_MASK)
            };
            all_moved &= mask & K_IOHID_DIGITIZER_EVENT_POSITION != 0;
        }

        if contacts == 0 {
            return Some(Self::default());
        }

        // CG gesture digitizer positions are normalized trackpad coordinates.
        // The collection parent tracks the centroid of touching children.
        let x = unsafe { IOHIDEventGetFloatValue(hid, K_IOHID_EVENT_FIELD_DIGITIZER_X) };
        let y = unsafe { IOHIDEventGetFloatValue(hid, K_IOHID_EVENT_FIELD_DIGITIZER_Y) };
        if !x.is_finite() || !y.is_finite() {
            return None;
        }

        Some(Self {
            contacts,
            centroid_x: x.clamp(0.0, 1.0),
            centroid_y: y.clamp(0.0, 1.0),
            all_moved,
        })
    }
}

struct HidEvent(NonNull<c_void>);

impl HidEvent {
    #[inline(always)]
    fn copy_from(event: &CGEvent) -> Option<Self> {
        NonNull::new(unsafe { CGEventCopyIOHIDEvent(event as *const CGEvent) }).map(Self)
    }

    #[inline(always)]
    fn as_ptr(&self) -> IOHIDEventRef { self.0.as_ptr() }
}

impl Drop for HidEvent {
    #[inline(always)]
    fn drop(&mut self) { unsafe { CFRelease(self.0.as_ptr()) }; }
}
