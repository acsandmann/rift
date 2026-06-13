use std::collections::BTreeMap;
use std::mem::size_of;

use dispatchr::queue;
use dispatchr::time::Time;
use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_graphics::{CGEvent, CGEventField};
use objc2_foundation::NSProcessInfo;
use once_cell::sync::Lazy;

use crate::layout_engine::Direction;
use crate::sys::dispatch::DispatchExt;
use crate::sys::skylight::{CGEventPost, CGEventTapLocation};

const K_CGS_EVENT_TYPE_FIELD: CGEventField = CGEventField(55);
const K_CGS_EVENT_MARKER: i64 = 29;
const K_CGS_EVENT_DOCK_CONTROL: i64 = 30;

const K_GESTURE_SUBTYPE_FIELD: CGEventField = CGEventField(41);
const K_GESTURE_SUBTYPE: i64 = 33231;

const K_GESTURE_HID_TYPE_FIELD: CGEventField = CGEventField(110);
const K_GESTURE_SWIPE_MASK_FIELD: CGEventField = CGEventField(115);
const K_GESTURE_SWIPE_MOTION_FIELD: CGEventField = CGEventField(123);
const K_GESTURE_SWIPE_PROGRESS_FIELD: CGEventField = CGEventField(124);
const K_GESTURE_SWIPE_POSITION_X_FIELD: CGEventField = CGEventField(125);
const K_GESTURE_SWIPE_POSITION_Y_FIELD: CGEventField = CGEventField(126);
const K_GESTURE_SWIPE_VELOCITY_X_FIELD: CGEventField = CGEventField(129);
const K_GESTURE_SWIPE_VELOCITY_Y_FIELD: CGEventField = CGEventField(130);
const K_GESTURE_PHASE_FIELD: CGEventField = CGEventField(132);
const K_GESTURE_PHASE_MIRROR_FIELD: CGEventField = CGEventField(134);
const K_GESTURE_PROGRESS_BITS_FIELD: CGEventField = CGEventField(135);
const K_GESTURE_FLAVOR_FIELD: CGEventField = CGEventField(138);
const K_GESTURE_POSITION_FALLBACK_FIELD: CGEventField = CGEventField(139);
const K_GESTURE_TIMESTAMP_FIELD: CGEventField = CGEventField(169);
const K_GESTURE_UNUSED_ZERO_FIELD: CGEventField = CGEventField(136);
const K_GESTURE_LEGACY_ONE_FIELD: CGEventField = CGEventField(165);

const K_IOHID_EVENT_TYPE_DOCK_SWIPE: i64 = 23;
const K_IOHID_EVENT_TYPE_VELOCITY: u32 = 9;
const K_IOHID_EVENT_TYPE_FLUID_TOUCH_GESTURE: u32 = 23;

const K_CG_GESTURE_MOTION_HORIZONTAL: i64 = 1;

const K_GESTURE_BEGAN: i64 = 1;
const K_GESTURE_ENDED: i64 = 4;

const K_EPSILON: f64 = 1e-15;
const K_INSTANT_SWITCH_VELOCITY: f64 = 100.0;
const K_GESTURE_FLAVOR_DOCK_PRIMARY: f64 = 3.0;
const K_GESTURE_SWIPE_POSITION_X: f64 = 0.1;
const K_LEGACY_TINY_FLOAT: f64 = f32::MIN_POSITIVE as f64;
const K_LEGACY_ONE: i64 = 1;
const K_GESTURE_DELAY_NS: i64 = 15 * 1_000_000;
const K_CGEVENT_DATA_HID_FIELD: u16 = 4205;

static IS_MACOS_27_OR_NEWER: Lazy<bool> = Lazy::new(|| {
    let version = NSProcessInfo::processInfo().operatingSystemVersion();
    version.majorVersion >= 27
});

#[derive(Clone)]
enum CGEventDataElement {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Blob(Vec<u8>),
}

struct CGEventData {
    version: i32,
    fields: BTreeMap<u16, CGEventDataElement>,
}

#[repr(C, packed)]
struct IOHIDSystemQueueElement {
    timestamp: u64,
    sender_id: u64,
    options: u32,
    attribute_length: u32,
    event_count: u32,
}

#[repr(C, packed)]
struct IOHIDEventBase {
    size: u32,
    event_type: u32,
    options: u32,
    depth: u8,
    reserved: [u8; 3],
}

#[repr(C, packed)]
struct IOHIDFluidTouchGestureData {
    base: IOHIDEventBase,
    position_x: i32,
    position_y: i32,
    position_z: i32,
    swipe_mask: u32,
    gesture_motion: u16,
    gesture_flavor: u16,
    swipe_progress: i32,
}

#[repr(C, packed)]
struct IOHIDVelocityEventData {
    base: IOHIDEventBase,
    velocity_x: i32,
    velocity_y: i32,
    velocity_z: i32,
}

pub unsafe fn switch_space(direction: Direction) {
    if *IS_MACOS_27_OR_NEWER {
        unsafe { switch_space_macos_27(direction) };
    } else {
        unsafe { switch_space_legacy(direction) };
    }
}

unsafe fn switch_space_legacy(direction: Direction) {
    let magnitude = match direction {
        Direction::Left => -2.25,
        Direction::Right => 2.25,
        _ => return,
    };

    let event1a = raw_marker_event();
    let event1b = raw_legacy_gesture_event(K_GESTURE_BEGAN, magnitude, None);

    unsafe {
        CGEventPost(
            CGEventTapLocation::HID,
            CFRetained::as_ptr(&event1b).as_ptr().cast(),
        );
        CGEventPost(
            CGEventTapLocation::HID,
            CFRetained::as_ptr(&event1a).as_ptr().cast(),
        );
    }

    queue::main().after_f_s(
        Time::new_after(Time::NOW, K_GESTURE_DELAY_NS),
        magnitude,
        |magnitude| {
            let gesture = 200.0 * magnitude;
            let event2a = raw_marker_event();
            let event2b = raw_legacy_gesture_event(K_GESTURE_ENDED, magnitude, Some(gesture));

            unsafe {
                CGEventPost(
                    CGEventTapLocation::HID,
                    CFRetained::as_ptr(&event2b).as_ptr().cast(),
                );
                CGEventPost(
                    CGEventTapLocation::HID,
                    CFRetained::as_ptr(&event2a).as_ptr().cast(),
                );
            }
        },
    );
}

unsafe fn switch_space_macos_27(direction: Direction) {
    let gesture_sign = match direction {
        Direction::Left => 1.0,
        Direction::Right => -1.0,
        _ => return,
    };

    let begin_progress = K_EPSILON * gesture_sign;
    let end_velocity = K_INSTANT_SWITCH_VELOCITY * gesture_sign;

    let begin_event = dock_control_gesture_event(direction, K_GESTURE_BEGAN, begin_progress, None);
    post_augmented_session_event(&begin_event);

    queue::main().after_f_s(
        Time::new_after(Time::NOW, K_GESTURE_DELAY_NS),
        (direction, begin_progress, end_velocity),
        |(direction, end_progress, end_velocity)| {
            let end_event = dock_control_gesture_event(
                direction,
                K_GESTURE_ENDED,
                end_progress,
                Some(end_velocity),
            );
            post_augmented_session_event(&end_event);
        },
    );
}

fn raw_marker_event() -> CFRetained<CGEvent> {
    let event = CGEvent::new(None).expect("CGEventCreate should succeed");
    CGEvent::set_integer_value_field(Some(&event), K_CGS_EVENT_TYPE_FIELD, K_CGS_EVENT_MARKER);
    CGEvent::set_integer_value_field(Some(&event), K_GESTURE_SUBTYPE_FIELD, K_GESTURE_SUBTYPE);
    event
}

fn raw_legacy_gesture_event(
    phase: i64,
    magnitude: f64,
    velocity_x: Option<f64>,
) -> CFRetained<CGEvent> {
    let event = CGEvent::new(None).expect("CGEventCreate should succeed");
    let magnitude_bits = (magnitude as f32).to_bits() as i64;

    CGEvent::set_integer_value_field(
        Some(&event),
        K_CGS_EVENT_TYPE_FIELD,
        K_CGS_EVENT_DOCK_CONTROL,
    );
    CGEvent::set_integer_value_field(
        Some(&event),
        K_GESTURE_HID_TYPE_FIELD,
        K_IOHID_EVENT_TYPE_DOCK_SWIPE,
    );
    CGEvent::set_integer_value_field(Some(&event), K_GESTURE_PHASE_FIELD, phase);
    CGEvent::set_integer_value_field(Some(&event), K_GESTURE_PHASE_MIRROR_FIELD, phase);
    CGEvent::set_double_value_field(Some(&event), K_GESTURE_SWIPE_PROGRESS_FIELD, magnitude);
    CGEvent::set_integer_value_field(Some(&event), K_GESTURE_PROGRESS_BITS_FIELD, magnitude_bits);
    CGEvent::set_integer_value_field(
        Some(&event),
        K_GESTURE_SWIPE_MOTION_FIELD,
        K_CG_GESTURE_MOTION_HORIZONTAL,
    );
    CGEvent::set_integer_value_field(Some(&event), K_GESTURE_LEGACY_ONE_FIELD, K_LEGACY_ONE);
    CGEvent::set_double_value_field(
        Some(&event),
        K_GESTURE_SWIPE_POSITION_X_FIELD,
        K_LEGACY_TINY_FLOAT,
    );
    CGEvent::set_double_value_field(
        Some(&event),
        K_GESTURE_POSITION_FALLBACK_FIELD,
        K_LEGACY_TINY_FLOAT,
    );
    CGEvent::set_integer_value_field(Some(&event), K_GESTURE_SUBTYPE_FIELD, K_GESTURE_SUBTYPE);
    CGEvent::set_integer_value_field(Some(&event), K_GESTURE_UNUSED_ZERO_FIELD, 0);

    if let Some(velocity_x) = velocity_x {
        CGEvent::set_double_value_field(Some(&event), K_GESTURE_SWIPE_VELOCITY_X_FIELD, velocity_x);
        CGEvent::set_double_value_field(Some(&event), K_GESTURE_SWIPE_VELOCITY_Y_FIELD, velocity_x);
    }

    event
}

fn dock_control_gesture_event(
    direction: Direction,
    phase: i64,
    progress: f64,
    velocity_x: Option<f64>,
) -> CFRetained<CGEvent> {
    let event = CGEvent::new(None).expect("CGEventCreate should succeed");
    CGEvent::set_integer_value_field(
        Some(&event),
        K_CGS_EVENT_TYPE_FIELD,
        K_CGS_EVENT_DOCK_CONTROL,
    );
    CGEvent::set_integer_value_field(
        Some(&event),
        K_GESTURE_HID_TYPE_FIELD,
        K_IOHID_EVENT_TYPE_DOCK_SWIPE,
    );
    CGEvent::set_integer_value_field(Some(&event), K_GESTURE_PHASE_FIELD, phase);
    CGEvent::set_integer_value_field(Some(&event), K_GESTURE_PHASE_MIRROR_FIELD, phase);
    CGEvent::set_integer_value_field(
        Some(&event),
        K_GESTURE_SWIPE_MOTION_FIELD,
        K_CG_GESTURE_MOTION_HORIZONTAL,
    );
    CGEvent::set_integer_value_field(
        Some(&event),
        K_GESTURE_SWIPE_MASK_FIELD,
        swipe_mask_for_direction(direction) as i64,
    );
    CGEvent::set_double_value_field(Some(&event), K_GESTURE_SWIPE_PROGRESS_FIELD, progress);

    if let Some(velocity_x) = velocity_x {
        CGEvent::set_double_value_field(Some(&event), K_GESTURE_SWIPE_VELOCITY_X_FIELD, velocity_x);
    }

    CGEvent::set_double_value_field(
        Some(&event),
        K_GESTURE_FLAVOR_FIELD,
        K_GESTURE_FLAVOR_DOCK_PRIMARY,
    );
    CGEvent::set_double_value_field(Some(&event), K_GESTURE_TIMESTAMP_FIELD, unsafe {
        mach_absolute_time() as f64
    });
    CGEvent::set_double_value_field(
        Some(&event),
        K_GESTURE_SWIPE_POSITION_X_FIELD,
        K_GESTURE_SWIPE_POSITION_X,
    );
    event
}

fn post_augmented_session_event(event: &CGEvent) {
    let posted = augment_event_with_hid_payload(event).or_else(|| CGEvent::new_copy(Some(event)));
    if let Some(posted) = posted {
        unsafe {
            CGEventPost(
                CGEventTapLocation::Session,
                CFRetained::as_ptr(&posted).as_ptr().cast(),
            )
        };
    }
}

fn augment_event_with_hid_payload(event: &CGEvent) -> Option<CFRetained<CGEvent>> {
    let serialized = CGEvent::new_data(None, Some(event))?;
    let event_data = deserialize_cgevent_data(unsafe { serialized.as_bytes_unchecked() })?;
    let mut fields = event_data.fields;
    fields.insert(
        K_CGEVENT_DATA_HID_FIELD,
        CGEventDataElement::Blob(generate_iohid_system_queue_element(event)),
    );

    let serialized = serialize_cgevent_data(CGEventData {
        version: event_data.version,
        fields,
    })?;
    let data = CFData::from_bytes(&serialized);
    CGEvent::from_data(None, Some(&data))
}

fn deserialize_cgevent_data(mut data: &[u8]) -> Option<CGEventData> {
    let version = read_be_i32(&mut data)?;
    if version != 2 {
        return None;
    }

    let mut fields = BTreeMap::new();
    while !data.is_empty() {
        let element_size = read_be_u16(&mut data)?;
        let tag_and_field = read_be_u16(&mut data)?;
        let tag = (tag_and_field >> 14) & 0x0003;
        let field = tag_and_field & 0x3FFF;

        let element = match tag {
            0 => {
                if element_size == 1 {
                    CGEventDataElement::I64(read_be_i64(&mut data)?)
                } else {
                    CGEventDataElement::Blob(read_exact(&mut data, element_size as usize)?.to_vec())
                }
            }
            1 => {
                if element_size != 1 {
                    return None;
                }
                CGEventDataElement::I32(read_be_i32(&mut data)?)
            }
            3 => match element_size {
                1 => CGEventDataElement::F32(read_be_f32(&mut data)?),
                2 => CGEventDataElement::F64(read_be_f64(&mut data)?),
                _ => return None,
            },
            _ => return None,
        };

        fields.insert(field, element);
    }

    Some(CGEventData { version, fields })
}

fn serialize_cgevent_data(event_data: CGEventData) -> Option<Vec<u8>> {
    if event_data.version != 2 {
        return None;
    }

    let mut out = Vec::new();
    out.extend_from_slice(&event_data.version.to_be_bytes());

    for (field, element) in event_data.fields {
        match element {
            CGEventDataElement::I32(value) => {
                write_field_header(&mut out, 1, 0b01, field);
                out.extend_from_slice(&value.to_be_bytes());
            }
            CGEventDataElement::I64(value) => {
                write_field_header(&mut out, 1, 0b00, field);
                out.extend_from_slice(&value.to_be_bytes());
            }
            CGEventDataElement::F32(value) => {
                write_field_header(&mut out, 1, 0b11, field);
                out.extend_from_slice(&value.to_bits().to_be_bytes());
            }
            CGEventDataElement::F64(value) => {
                write_field_header(&mut out, 2, 0b11, field);
                out.extend_from_slice(&value.to_bits().to_be_bytes());
            }
            CGEventDataElement::Blob(value) => {
                let element_size = u16::try_from(value.len()).ok()?;
                write_field_header(&mut out, element_size, 0b00, field);
                out.extend_from_slice(&value);
            }
        }
    }

    Some(out)
}

fn write_field_header(out: &mut Vec<u8>, element_size: u16, tag: u16, field: u16) {
    out.extend_from_slice(&element_size.to_be_bytes());
    out.extend_from_slice(&(((tag & 0x0003) << 14) | (field & 0x3FFF)).to_be_bytes());
}

fn generate_iohid_system_queue_element(event: &CGEvent) -> Vec<u8> {
    let phase = CGEvent::integer_value_field(Some(event), K_GESTURE_PHASE_FIELD);
    let motion = CGEvent::integer_value_field(Some(event), K_GESTURE_SWIPE_MOTION_FIELD) as u16;
    let progress = CGEvent::double_value_field(Some(event), K_GESTURE_SWIPE_PROGRESS_FIELD);
    let position_x = CGEvent::double_value_field(Some(event), K_GESTURE_SWIPE_POSITION_X_FIELD);
    let position_y = CGEvent::double_value_field(Some(event), K_GESTURE_SWIPE_POSITION_Y_FIELD);
    let velocity_x = CGEvent::double_value_field(Some(event), K_GESTURE_SWIPE_VELOCITY_X_FIELD);
    let velocity_y = CGEvent::double_value_field(Some(event), K_GESTURE_SWIPE_VELOCITY_Y_FIELD);
    let swipe_mask = CGEvent::integer_value_field(Some(event), K_GESTURE_SWIPE_MASK_FIELD) as u32;

    let has_velocity = velocity_x != 0.0 || velocity_y != 0.0 || phase == K_GESTURE_ENDED;
    let event_count = if has_velocity { 2 } else { 1 };

    let header = IOHIDSystemQueueElement {
        timestamp: cg_event_timestamp_or_now(event),
        sender_id: 0,
        options: 0,
        attribute_length: 0,
        event_count,
    };
    let gesture = IOHIDFluidTouchGestureData {
        base: IOHIDEventBase {
            size: size_of::<IOHIDFluidTouchGestureData>() as u32,
            event_type: K_IOHID_EVENT_TYPE_FLUID_TOUCH_GESTURE,
            options: ((phase as u32) & 0xFF) << 24,
            depth: 0,
            reserved: [0; 3],
        },
        position_x: double_to_fixed_16_16(position_x),
        position_y: double_to_fixed_16_16(position_y),
        position_z: 0,
        swipe_mask,
        gesture_motion: motion,
        gesture_flavor: K_GESTURE_FLAVOR_DOCK_PRIMARY as u16,
        swipe_progress: double_to_fixed_16_16(progress),
    };

    let mut out = Vec::with_capacity(
        size_of::<IOHIDSystemQueueElement>()
            + size_of::<IOHIDFluidTouchGestureData>()
            + if has_velocity {
                size_of::<IOHIDVelocityEventData>()
            } else {
                0
            },
    );
    extend_packed(&mut out, &header);
    extend_packed(&mut out, &gesture);

    if has_velocity {
        let velocity = IOHIDVelocityEventData {
            base: IOHIDEventBase {
                size: size_of::<IOHIDVelocityEventData>() as u32,
                event_type: K_IOHID_EVENT_TYPE_VELOCITY,
                options: 0,
                depth: 1,
                reserved: [0; 3],
            },
            velocity_x: double_to_fixed_16_16(velocity_x),
            velocity_y: double_to_fixed_16_16(velocity_y),
            velocity_z: 0,
        };
        extend_packed(&mut out, &velocity);
    }

    out
}

fn swipe_mask_for_direction(direction: Direction) -> u32 {
    match direction {
        Direction::Left => 8,
        Direction::Right => 4,
        Direction::Up => 2,
        Direction::Down => 1,
    }
}

fn cg_event_timestamp_or_now(event: &CGEvent) -> u64 {
    let timestamp = CGEvent::timestamp(Some(event));
    if timestamp == 0 {
        unsafe { mach_absolute_time() }
    } else {
        timestamp
    }
}

fn double_to_fixed_16_16(value: f64) -> i32 {
    let fixed = (value * 65536.0) as i32;
    if fixed == 0 && value != 0.0 {
        if value.is_sign_negative() { -1 } else { 1 }
    } else {
        fixed
    }
}

fn extend_packed<T>(bytes: &mut Vec<u8>, value: &T) {
    let ptr = std::ptr::from_ref(value).cast::<u8>();
    let slice = unsafe { std::slice::from_raw_parts(ptr, size_of::<T>()) };
    bytes.extend_from_slice(slice);
}

fn read_exact<'a>(data: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
    if data.len() < len {
        return None;
    }
    let (head, tail) = data.split_at(len);
    *data = tail;
    Some(head)
}

fn read_be_u16(data: &mut &[u8]) -> Option<u16> {
    Some(u16::from_be_bytes(read_exact(data, 2)?.try_into().ok()?))
}

fn read_be_i32(data: &mut &[u8]) -> Option<i32> {
    Some(i32::from_be_bytes(read_exact(data, 4)?.try_into().ok()?))
}

fn read_be_i64(data: &mut &[u8]) -> Option<i64> {
    Some(i64::from_be_bytes(read_exact(data, 8)?.try_into().ok()?))
}

fn read_be_f32(data: &mut &[u8]) -> Option<f32> {
    Some(f32::from_bits(u32::from_be_bytes(
        read_exact(data, 4)?.try_into().ok()?,
    )))
}

fn read_be_f64(data: &mut &[u8]) -> Option<f64> {
    Some(f64::from_bits(u64::from_be_bytes(
        read_exact(data, 8)?.try_into().ok()?,
    )))
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
}

#[cfg(test)]
mod tests {
    use super::{
        IOHIDFluidTouchGestureData, IOHIDSystemQueueElement, IOHIDVelocityEventData,
        double_to_fixed_16_16,
    };

    #[test]
    fn fixed_point_keeps_non_zero_epsilon() {
        assert_eq!(double_to_fixed_16_16(0.0), 0);
        assert_eq!(double_to_fixed_16_16(f64::MIN_POSITIVE), 1);
        assert_eq!(double_to_fixed_16_16(-f64::MIN_POSITIVE), -1);
    }

    #[test]
    fn hid_layout_sizes_match_expected() {
        assert_eq!(std::mem::size_of::<IOHIDSystemQueueElement>(), 28);
        assert_eq!(std::mem::size_of::<IOHIDFluidTouchGestureData>(), 40);
        assert_eq!(std::mem::size_of::<IOHIDVelocityEventData>(), 28);
    }
}
