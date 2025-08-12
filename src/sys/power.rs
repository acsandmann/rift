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

pub fn get_max_fps_for_power_state(base_fps: f64) -> f64 {
    if is_low_power_mode_enabled() {
        base_fps.min(60.0)
    } else {
        base_fps
    }
}

pub fn set_low_power_mode_state(new_state: bool) -> bool {
    LOW_POWER_MODE.swap(new_state, Ordering::Relaxed)
}

pub fn init_power_state() {
    let process_info = NSProcessInfo::process_info();
    let initial_state = process_info.is_low_power_mode_enabled();
    LOW_POWER_MODE.store(initial_state, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fps_limiting() {
        LOW_POWER_MODE.store(false, Ordering::Relaxed);
        assert_eq!(get_max_fps_for_power_state(120.0), 120.0);

        LOW_POWER_MODE.store(true, Ordering::Relaxed);
        assert_eq!(get_max_fps_for_power_state(120.0), 60.0);
        assert_eq!(get_max_fps_for_power_state(30.0), 30.0);
    }

    #[test]
    fn test_state_management() {
        let _old_state = set_low_power_mode_state(true);
        assert_eq!(is_low_power_mode_enabled(), true);

        let old_state2 = set_low_power_mode_state(false);
        assert_eq!(old_state2, true);
        assert_eq!(is_low_power_mode_enabled(), false);
    }
}
