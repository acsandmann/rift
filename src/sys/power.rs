use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use objc2::rc::Retained;
use objc2::{class, msg_send};
use once_cell::sync::Lazy;

#[repr(C)]
pub struct NSProcessInfo {
    _private: [u8; 0],
}

unsafe impl objc2::RefEncode for NSProcessInfo {
    const ENCODING_REF: objc2::Encoding = objc2::Encoding::Object;
}

unsafe impl objc2::Message for NSProcessInfo {}

impl NSProcessInfo {
    pub fn process_info() -> Retained<Self> {
        unsafe { msg_send![class!(NSProcessInfo), processInfo] }
    }

    pub fn is_low_power_mode_enabled(&self) -> bool {
        unsafe { msg_send![self, isLowPowerModeEnabled] }
    }

    pub fn thermal_state(&self) -> i64 { unsafe { msg_send![self, thermalState] } }
}

static LOW_POWER_MODE: Lazy<Arc<AtomicBool>> = Lazy::new(|| Arc::new(AtomicBool::new(false)));

pub fn is_low_power_mode_enabled() -> bool { LOW_POWER_MODE.load(Ordering::Relaxed) }

pub fn set_low_power_mode_state(new_state: bool) -> bool {
    LOW_POWER_MODE.swap(new_state, Ordering::Relaxed)
}

pub fn init_power_state() {
    let process_info = NSProcessInfo::process_info();
    let initial_state = process_info.is_low_power_mode_enabled();
    LOW_POWER_MODE.store(initial_state, Ordering::Relaxed);
}
