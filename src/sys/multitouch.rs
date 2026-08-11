use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use tokio::sync::mpsc;

const MAX_DEVICES: usize = 8;
const NO_ACTIVE_DEVICE: u8 = u8::MAX;
const MT_TOUCH_MAKE: u32 = 3;
const MT_TOUCHING: u32 = 4;

type OSStatus = i32;
type CFIndex = isize;
type CFArrayRef = *const c_void;
type MTDeviceRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct MTPoint {
    x: f32,
    y: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MTVector {
    position: MTPoint,
    velocity: MTPoint,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MTTouch {
    frame: i32,
    timestamp: f64,
    path_index: i32,
    state: u32,
    finger_id: i32,
    hand_id: i32,
    normalized: MTVector,
    z_total: f32,
    field9: i32,
    angle: f32,
    major_axis: f32,
    minor_axis: f32,
    absolute: MTVector,
    field14: i32,
    field15: i32,
    z_density: f32,
}

const _: [(); 96] = [(); std::mem::size_of::<MTTouch>()];

type MTFrameCallback =
    unsafe extern "C" fn(MTDeviceRef, *mut MTTouch, usize, f64, usize, *mut c_void);

unsafe extern "C" {
    fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;

    fn MTDeviceCreateList() -> CFArrayRef;
    fn MTDeviceStart(device: MTDeviceRef, mode: i32) -> OSStatus;
    fn MTRegisterContactFrameCallbackWithRefcon(
        device: MTDeviceRef,
        callback: MTFrameCallback,
        refcon: *mut c_void,
    );
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TouchSnapshot {
    pub contacts: usize,
    pub centroid_x: f64,
    pub centroid_y: f64,
    pub all_moved: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartError {
    AlreadyActive,
    NoDevices,
    StartFailed(OSStatus),
}

/// Process-wide raw contact input. MultitouchSupport callbacks are dispatched
/// from private worker threads, so the callback state intentionally has process
/// lifetime instead of trying to tear the private framework down asynchronously.
pub struct TouchTracker {
    shared: &'static Shared,
    _devices: NonNull<c_void>,
    device_count: u8,
}

static CLAIMED: AtomicBool = AtomicBool::new(false);

impl TouchTracker {
    pub fn start(wake: mpsc::Sender<()>) -> Result<Self, StartError> {
        if CLAIMED.swap(true, Ordering::AcqRel) {
            return Err(StartError::AlreadyActive);
        }

        let Some(devices) = NonNull::new(unsafe { MTDeviceCreateList() } as *mut c_void) else {
            CLAIMED.store(false, Ordering::Release);
            return Err(StartError::NoDevices);
        };
        let shared = Box::leak(Box::new(Shared::new(wake)));

        let mut device_count = 0usize;
        let mut last_error = None;
        let count = unsafe { CFArrayGetCount(devices.as_ptr()) }.max(0) as usize;

        for index in 0..count {
            if device_count == MAX_DEVICES {
                break;
            }

            let device = unsafe { CFArrayGetValueAtIndex(devices.as_ptr(), index as CFIndex) }
                as MTDeviceRef;
            if device.is_null() {
                continue;
            }

            let ctx = Box::leak(Box::new(CallbackCtx {
                shared,
                slot: device_count as u8,
            }));
            unsafe {
                MTRegisterContactFrameCallbackWithRefcon(
                    device,
                    contact_frame_callback,
                    ctx as *mut CallbackCtx as *mut c_void,
                )
            };

            let status = unsafe { MTDeviceStart(device, 0) };
            if status != 0 {
                last_error = Some(status);
                continue;
            }
            device_count += 1;
        }

        if device_count == 0 {
            shared.enabled.store(false, Ordering::Release);
            CLAIMED.store(false, Ordering::Release);
            return Err(last_error.map_or(StartError::NoDevices, StartError::StartFailed));
        }

        Ok(Self {
            shared,
            _devices: devices,
            device_count: device_count as u8,
        })
    }

    #[inline]
    pub fn set_enabled(&self, enabled: bool) {
        if self.shared.enabled.swap(enabled, Ordering::AcqRel) != enabled && !enabled {
            self.shared.reset();
        }
    }

    #[inline]
    pub fn snapshot(&self) -> TouchSnapshot {
        let active = self.shared.active.load(Ordering::Acquire) as usize;
        if active < self.device_count as usize {
            return self.shared.frames[active].load();
        }
        TouchSnapshot::default()
    }

    pub fn device_count(&self) -> usize { self.device_count as usize }
}

impl Drop for TouchTracker {
    fn drop(&mut self) { self.shared.enabled.store(false, Ordering::Release); }
}

struct Shared {
    enabled: AtomicBool,
    active: AtomicU8,
    wake: mpsc::Sender<()>,
    frames: [SharedFrame; MAX_DEVICES],
}

impl Shared {
    fn new(wake: mpsc::Sender<()>) -> Self {
        Self {
            enabled: AtomicBool::new(false),
            active: AtomicU8::new(NO_ACTIVE_DEVICE),
            wake,
            frames: [const { SharedFrame::new() }; MAX_DEVICES],
        }
    }

    fn reset(&self) {
        self.active.store(NO_ACTIVE_DEVICE, Ordering::Release);
        for frame in &self.frames {
            frame.reset();
        }
    }
}

struct CallbackCtx {
    shared: &'static Shared,
    slot: u8,
}

struct SharedFrame(AtomicU64);

impl SharedFrame {
    const fn new() -> Self { Self(AtomicU64::new(0)) }

    #[inline(always)]
    fn publish(&self, contacts: u8, x: f32, y: f32, all_moved: bool) {
        self.0.store(pack_snapshot(contacts, x, y, all_moved), Ordering::Release);
    }

    #[inline(always)]
    fn load(&self) -> TouchSnapshot {
        let (contacts, x, y, all_moved) = unpack_snapshot(self.0.load(Ordering::Acquire));
        TouchSnapshot {
            contacts: contacts as usize,
            centroid_x: x as f64,
            centroid_y: y as f64,
            all_moved,
        }
    }

    #[inline]
    fn reset(&self) { self.0.store(0, Ordering::Release); }
}

#[inline(always)]
fn pack_snapshot(contacts: u8, x: f32, y: f32, all_moved: bool) -> u64 {
    let x = (x.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16;
    let y = (y.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16;
    x as u64 | ((y as u64) << 16) | ((contacts as u64) << 32) | ((all_moved as u64) << 40)
}

#[inline(always)]
fn unpack_snapshot(value: u64) -> (u8, f32, f32, bool) {
    let scale = 1.0 / u16::MAX as f32;
    (
        ((value >> 32) & 0xff) as u8,
        (value as u16) as f32 * scale,
        ((value >> 16) as u16) as f32 * scale,
        value & (1 << 40) != 0,
    )
}

unsafe extern "C" fn contact_frame_callback(
    _device: MTDeviceRef,
    touches: *mut MTTouch,
    num_touches: usize,
    _timestamp: f64,
    _frame: usize,
    refcon: *mut c_void,
) {
    if refcon.is_null() {
        return;
    }
    let ctx = unsafe { &*(refcon as *const CallbackCtx) };
    let shared = ctx.shared;
    if !shared.enabled.load(Ordering::Relaxed) || (touches.is_null() && num_touches != 0) {
        return;
    }

    let touches = if num_touches == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(touches, num_touches) }
    };
    let mut count = 0u8;
    let mut sum_x = 0.0f32;
    let mut sum_y = 0.0f32;
    let mut all_moved = true;

    for touch in touches {
        if touch.state != MT_TOUCH_MAKE && touch.state != MT_TOUCHING {
            continue;
        }
        let position = touch.normalized.position;
        if !position.x.is_finite() || !position.y.is_finite() {
            continue;
        }

        count = count.saturating_add(1);
        sum_x += position.x;
        sum_y += position.y;
        let velocity = touch.normalized.velocity;
        all_moved &= touch.state == MT_TOUCHING && (velocity.x != 0.0 || velocity.y != 0.0);
    }

    let slot = &shared.frames[ctx.slot as usize];
    if count == 0 {
        slot.publish(0, 0.0, 0.0, false);
    } else {
        let scale = 1.0 / count as f32;
        slot.publish(count, sum_x * scale, sum_y * scale, all_moved);
        shared.active.store(ctx.slot, Ordering::Release);
    }
    let _ = shared.wake.try_send(());
}
