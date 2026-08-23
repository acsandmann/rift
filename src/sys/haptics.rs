#![allow(non_camel_case_types)]
use std::ffi::{CStr, c_char, c_void};

use objc2_core_foundation::{CFNumber, CFNumberType, CFRetained, CFString, CFType};
use once_cell::sync::OnceCell;

use crate::common::config::HapticPattern;
use crate::sys::skylight::CFRelease;

#[inline]
fn pattern_index(pattern: HapticPattern) -> i32 {
    match pattern {
        HapticPattern::Generic => 0,
        HapticPattern::Alignment => 1,
        HapticPattern::LevelChange => 2,
    }
}

type kern_return_t = i32;
type io_object_t = u32;
type io_iterator_t = u32;
type io_registry_entry_t = u32;
type mach_port_t = u32;

unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut CFType;
    fn IOServiceGetMatchingServices(
        master: mach_port_t,
        matching: *mut CFType,
        iter: *mut io_iterator_t,
    ) -> kern_return_t;
    fn IOIteratorNext(iter: io_iterator_t) -> io_object_t;
    fn IOObjectRelease(obj: io_object_t) -> kern_return_t;
    fn IORegistryEntryCreateCFProperty(
        entry: io_registry_entry_t,
        key: *mut CFString,
        allocator: *const c_void,
        options: u32,
    ) -> *mut CFType;

    fn MTActuatorCreateFromDeviceID(device_id: u64) -> *mut CFType;
    fn MTActuatorOpen(actuator: *mut CFType) -> i32; // IOReturn
    fn MTActuatorIsOpen(actuator: *mut CFType) -> bool;
    fn MTActuatorActuate(actuator: *mut CFType, pattern: i32, unk: i32, f1: f32, f2: f32) -> i32; // IOReturn
    //fn MTActuatorClose(actuator: CFTypeRef);

    fn CFGetTypeID(cf: *mut CFType) -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(number: *mut CFNumber, theType: i32, valuePtr: *mut u64) -> bool;
}

#[inline]
fn k_iomain_port_default() -> mach_port_t {
    0
}

struct MtsState {
    actuators: Vec<*mut CFType>,
}

unsafe impl Send for MtsState {}
unsafe impl Sync for MtsState {}

impl MtsState {
    fn open_default_or_all() -> Option<Self> {
        let mut iter: io_iterator_t = 0;
        unsafe {
            let name = CStr::from_bytes_with_nul_unchecked(b"AppleMultitouchDevice\0");
            let matching = IOServiceMatching(name.as_ptr());
            if matching.is_null() {
                return None;
            }
            if IOServiceGetMatchingServices(k_iomain_port_default(), matching, &mut iter) != 0 {
                return None;
            }
        }

        let key = CFString::from_str("Multitouch ID");
        let mut actuators: Vec<*mut CFType> = Vec::new();

        unsafe {
            loop {
                let dev = IOIteratorNext(iter);
                if dev == 0 {
                    break;
                }

                let id_ref = IORegistryEntryCreateCFProperty(
                    dev,
                    CFRetained::<CFString>::as_ptr(&key).as_ptr(),
                    std::ptr::null(),
                    0,
                );

                if !id_ref.is_null() && CFGetTypeID(id_ref) == CFNumberGetTypeID() {
                    let mut device_id: u64 = 0;
                    if CFNumberGetValue(
                        id_ref as *mut CFNumber,
                        CFNumberType::SInt64Type.0 as i32,
                        &mut device_id as *mut u64,
                    ) {
                        let act = MTActuatorCreateFromDeviceID(device_id);
                        if !act.is_null() {
                            if MTActuatorOpen(act) == 0 {
                                actuators.push(act);
                            } else {
                                CFRelease(act);
                            }
                        }
                    }
                }

                if !id_ref.is_null() {
                    CFRelease(id_ref);
                }
                IOObjectRelease(dev);
            }

            if iter != 0 {
                IOObjectRelease(iter);
            }
        }

        if actuators.is_empty() {
            None
        } else {
            Some(Self { actuators })
        }
    }
}

static MTS: OnceCell<Option<MtsState>> = OnceCell::new();

fn mts_state() -> Option<&'static MtsState> {
    MTS.get_or_init(|| MtsState::open_default_or_all()).as_ref()
}

pub fn perform_haptic(pattern: HapticPattern) -> bool {
    if let Some(state) = mts_state() {
        let pat = pattern_index(pattern);
        let mut any_ok = false;
        unsafe {
            for &act in &state.actuators {
                if !act.is_null() && MTActuatorIsOpen(act) {
                    let kr = MTActuatorActuate(act, pat, 0, 0.0, 0.0);
                    any_ok |= kr == 0;
                }
            }
        }
        return any_ok;
    }
    false
}
