//! Private CGS gesture and IOHID event helpers.
//!
//! WindowServer sends two interleaved kinds of type-29 event for one physical
//! trackpad interaction:
//!
//! - unphased type-11 digitizer collections containing the physical contacts;
//! - processed Scroll/NavigationSwipe/Force/etc. events containing CG gesture
//!   phases and deltas.
//!
//! Rift recognizes only from the first kind and uses the second kind solely as
//! a synchronously suppressible delivery stream. No AppKit objects are needed.

use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

use objc2_core_graphics::{CGEvent, CGEventField, CGEventType};

pub const CGS_EVENT_GESTURE: u32 = 29;
pub const CGS_EVENT_DOCK_CONTROL: u32 = 30;
pub const EVENT_MASK: u64 = (1u64 << CGS_EVENT_GESTURE) | (1u64 << CGS_EVENT_DOCK_CONTROL);

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
const K_IOHID_EVENT_FIELD_DIGITIZER_INDEX: u32 = K_IOHID_EVENT_FIELD_DIGITIZER_X + 5;
const K_IOHID_EVENT_FIELD_DIGITIZER_COLLECTION: u32 = K_IOHID_EVENT_FIELD_DIGITIZER_X + 22;
// `_mthid_pathIsResting` extracts bit 9 from the digitizer event mask. AppKit
// uses this to distinguish palm/resting paths from gesture participants.
const K_IOHID_DIGITIZER_EVENT_RESTING: isize = 1 << 9;
const MAX_CHILD_EVENTS: usize = 16;

type CFArrayRef = *const c_void;
type CFIndex = isize;
type IOHIDEventRef = *mut c_void;
type IOHIDEventField = u32;

#[repr(C)]
#[derive(Clone, Copy)]
struct CFRange {
    location: CFIndex,
    length: CFIndex,
}

unsafe extern "C" {
    fn CGEventCopyIOHIDEvent(event: *const CGEvent) -> IOHIDEventRef;

    fn IOHIDEventGetType(event: IOHIDEventRef) -> u32;
    fn IOHIDEventGetChildren(event: IOHIDEventRef) -> CFArrayRef;
    fn IOHIDEventGetIntegerValue(event: IOHIDEventRef, field: IOHIDEventField) -> CFIndex;
    fn IOHIDEventGetFloatValue(event: IOHIDEventRef, field: IOHIDEventField) -> f64;

    fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValues(array: CFArrayRef, range: CFRange, values: *mut *const c_void);
    fn CFRelease(value: *const c_void);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TouchFrame {
    pub contacts: usize,
    pub centroid_x: f64,
    pub centroid_y: f64,
}

#[derive(Clone, Copy, Debug)]
pub enum GesturePayload {
    /// Physical contact state. These events have CG gesture phase zero.
    Touch(TouchFrame),
    /// A processed gesture event. Its exact HID subtype is intentionally not
    /// used as a physical-session boundary.
    Processed,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TouchPath {
    /// The digitizer path index is stable for the life of a physical contact.
    /// The identity field is not: macOS may reassign it while fingers cross.
    pub index: isize,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct ScrollTouchFrame {
    pub paths: [TouchPath; MAX_CHILD_EVENTS],
    pub len: usize,
}

impl Default for ScrollTouchFrame {
    fn default() -> Self {
        Self {
            paths: [TouchPath::default(); MAX_CHILD_EVENTS],
            len: 0,
        }
    }
}

impl ScrollTouchFrame {
    #[inline(always)]
    pub fn paths(&self) -> &[TouchPath] { &self.paths[..self.len] }
}

#[derive(Clone, Copy, Debug)]
pub enum ScrollGesturePayload {
    Touch(ScrollTouchFrame),
    Processed,
}

#[inline(always)]
pub fn is_gesture(event_type: CGEventType) -> bool { event_type.0 == CGS_EVENT_GESTURE }

#[inline]
pub fn is_physical_horizontal_dock_swipe(event_type: CGEventType, event: &CGEvent) -> bool {
    if event_type.0 != CGS_EVENT_DOCK_CONTROL {
        return false;
    }

    CGEvent::integer_value_field(Some(event), K_GESTURE_HID_TYPE_FIELD)
        == K_IOHID_EVENT_TYPE_DOCK_SWIPE
        && CGEvent::integer_value_field(Some(event), K_GESTURE_SWIPE_MOTION_FIELD)
            == K_CG_GESTURE_MOTION_HORIZONTAL
}

/// Classify one type-29 event and decode a physical contact frame when present.
///
/// `CGEventCopyIOHIDEvent` is a retained lookup (about 41 ns on the measured
/// system). Processed events return immediately after the one type read. Raw
/// frames use a fixed stack array and allocate nothing.
#[inline]
pub fn payload(event: &CGEvent) -> Option<GesturePayload> { payload_with_centroid(event, true) }

/// Decode only contact presence. This is sufficient after a workspace swipe
/// has committed or been rejected, when the recognizer is waiting for lift.
#[inline]
pub fn contact_payload(event: &CGEvent) -> Option<GesturePayload> {
    payload_with_centroid(event, false)
}

#[inline]
fn payload_with_centroid(event: &CGEvent, centroid: bool) -> Option<GesturePayload> {
    if is_processed_gesture(event) {
        return Some(GesturePayload::Processed);
    }
    let hid = HidEvent::copy_from(event)?;
    if !is_path_collection(hid.as_ptr()) {
        return Some(GesturePayload::Processed);
    }
    unsafe { TouchFrame::from_digitizer(hid.as_ptr(), centroid).map(GesturePayload::Touch) }
}

/// Decode individual paths only for the scrolling layout. This is still one
/// retained HID lookup and one stack CFArray copy, with no heap allocation.
#[inline]
pub fn scroll_payload(event: &CGEvent) -> Option<ScrollGesturePayload> {
    if is_processed_gesture(event) {
        return Some(ScrollGesturePayload::Processed);
    }
    let hid = HidEvent::copy_from(event)?;
    if !is_path_collection(hid.as_ptr()) {
        return Some(ScrollGesturePayload::Processed);
    }
    unsafe { ScrollTouchFrame::from_digitizer(hid.as_ptr()).map(ScrollGesturePayload::Touch) }
}

/// Decode only the number of active, non-resting paths. Rejected scroll
/// gestures need this solely to detect that the physical session ended.
#[inline]
pub fn scroll_contact_payload(event: &CGEvent) -> Option<ScrollGesturePayload> {
    if is_processed_gesture(event) {
        return Some(ScrollGesturePayload::Processed);
    }
    let hid = HidEvent::copy_from(event)?;
    if !is_path_collection(hid.as_ptr()) {
        return Some(ScrollGesturePayload::Processed);
    }
    unsafe {
        ScrollTouchFrame::contact_presence_from_digitizer(hid.as_ptr())
            .map(ScrollGesturePayload::Touch)
    }
}

/// Physical digitizer collections are unphased. WindowServer emits processed
/// gesture events alongside them with a nonzero began/changed/ended/cancelled
/// phase. Checking the scalar CGEvent field first avoids retaining and
/// inspecting an IOHID event that the recognizers do not otherwise use.
#[inline(always)]
fn is_processed_gesture(event: &CGEvent) -> bool {
    CGEvent::integer_value_field(Some(event), K_GESTURE_PHASE_FIELD) != 0
}

#[inline(always)]
fn is_path_collection(hid: IOHIDEventRef) -> bool {
    unsafe {
        IOHIDEventGetType(hid) == K_IOHID_EVENT_TYPE_DIGITIZER
            && IOHIDEventGetIntegerValue(hid, K_IOHID_EVENT_FIELD_DIGITIZER_COLLECTION) != 0
    }
}

#[inline(always)]
unsafe fn copy_children(
    hid: IOHIDEventRef,
) -> Option<([MaybeUninit<*const c_void>; MAX_CHILD_EVENTS], usize)> {
    let children = unsafe { IOHIDEventGetChildren(hid) };
    let mut values = [MaybeUninit::<*const c_void>::uninit(); MAX_CHILD_EVENTS];
    if children.is_null() {
        return Some((values, 0));
    }
    let count = unsafe { CFArrayGetCount(children) };
    if count < 0 || count as usize > MAX_CHILD_EVENTS {
        return None;
    }
    if count > 0 {
        unsafe {
            CFArrayGetValues(
                children,
                CFRange { location: 0, length: count },
                values.as_mut_ptr().cast(),
            )
        };
    }
    Some((values, count as usize))
}

#[inline(always)]
unsafe fn is_path(child: IOHIDEventRef) -> bool {
    !child.is_null()
        && unsafe { IOHIDEventGetType(child) } == K_IOHID_EVENT_TYPE_DIGITIZER
        && unsafe { IOHIDEventGetIntegerValue(child, K_IOHID_EVENT_FIELD_DIGITIZER_COLLECTION) }
            == 0
}

impl TouchFrame {
    #[inline]
    unsafe fn from_digitizer(hid: IOHIDEventRef, centroid: bool) -> Option<Self> {
        let (values, count) = unsafe { copy_children(hid) }?;
        let mut contacts = 0usize;

        for value in &values[..count] {
            let child = unsafe { value.assume_init() } as IOHIDEventRef;
            if !unsafe { is_path(child) } {
                continue;
            }

            let touching =
                unsafe { IOHIDEventGetIntegerValue(child, K_IOHID_EVENT_FIELD_DIGITIZER_TOUCH) }
                    != 0;

            if !touching {
                continue;
            }

            contacts += 1;
        }

        if contacts == 0 || !centroid {
            return Some(Self { contacts, ..Self::default() });
        }

        // Preserve the workspace-swipe path: AppKit uses the collection's
        // aggregate coordinates, and that recognizer already behaves well.
        let x = unsafe { IOHIDEventGetFloatValue(hid, K_IOHID_EVENT_FIELD_DIGITIZER_X) };
        let y = unsafe { IOHIDEventGetFloatValue(hid, K_IOHID_EVENT_FIELD_DIGITIZER_Y) };
        if !x.is_finite() || !y.is_finite() {
            return None;
        }

        Some(Self {
            contacts,
            centroid_x: x.clamp(0.0, 1.0),
            centroid_y: y.clamp(0.0, 1.0),
        })
    }
}

impl ScrollTouchFrame {
    #[inline]
    unsafe fn contact_presence_from_digitizer(hid: IOHIDEventRef) -> Option<Self> {
        let (values, count) = unsafe { copy_children(hid) }?;
        let mut frame = Self::default();
        for value in &values[..count] {
            let child = unsafe { value.assume_init() } as IOHIDEventRef;
            if !unsafe { is_path(child) } {
                continue;
            }
            if unsafe { IOHIDEventGetIntegerValue(child, K_IOHID_EVENT_FIELD_DIGITIZER_TOUCH) } == 0
            {
                continue;
            }
            let mask = unsafe {
                IOHIDEventGetIntegerValue(child, K_IOHID_EVENT_FIELD_DIGITIZER_EVENT_MASK)
            };
            if mask & K_IOHID_DIGITIZER_EVENT_RESTING == 0 {
                frame.len += 1;
            }
        }
        Some(frame)
    }

    #[inline]
    unsafe fn from_digitizer(hid: IOHIDEventRef) -> Option<Self> {
        let (values, count) = unsafe { copy_children(hid) }?;
        let mut frame = Self::default();
        for value in &values[..count] {
            let child = unsafe { value.assume_init() } as IOHIDEventRef;
            if !unsafe { is_path(child) } {
                continue;
            }
            let touching =
                unsafe { IOHIDEventGetIntegerValue(child, K_IOHID_EVENT_FIELD_DIGITIZER_TOUCH) }
                    != 0;
            if !touching {
                continue;
            }
            let mask = unsafe {
                IOHIDEventGetIntegerValue(child, K_IOHID_EVENT_FIELD_DIGITIZER_EVENT_MASK)
            };
            if mask & K_IOHID_DIGITIZER_EVENT_RESTING != 0 {
                continue;
            }
            let x = unsafe { IOHIDEventGetFloatValue(child, K_IOHID_EVENT_FIELD_DIGITIZER_X) };
            let y = unsafe { IOHIDEventGetFloatValue(child, K_IOHID_EVENT_FIELD_DIGITIZER_Y) };
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            frame.paths[frame.len] = TouchPath {
                index: unsafe {
                    IOHIDEventGetIntegerValue(child, K_IOHID_EVENT_FIELD_DIGITIZER_INDEX)
                },
                x,
                y,
            };
            frame.len += 1;
        }
        Some(frame)
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
