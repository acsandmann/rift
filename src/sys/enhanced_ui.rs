use accessibility::AXUIElement;
use accessibility_sys::{
    AXError, AXUIElementCopyAttributeValue, AXUIElementSetAttributeValue, kAXErrorSuccess,
};
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::boolean::{CFBoolean, CFBooleanRef};
use core_foundation::string::CFString;
use tracing::warn;

const K_AX_ENHANCED_USER_INTERFACE: &str = "AXEnhancedUserInterface";

pub fn get_enhanced_user_interface(element: &AXUIElement) -> bool {
    unsafe {
        let mut value: CFTypeRef = std::ptr::null();
        let error = AXUIElementCopyAttributeValue(
            element.as_concrete_TypeRef(),
            CFString::from_static_string(K_AX_ENHANCED_USER_INTERFACE).as_concrete_TypeRef(),
            &mut value,
        );

        if error == kAXErrorSuccess && !value.is_null() {
            let boolean = CFBoolean::wrap_under_get_rule(value as CFBooleanRef);
            let result = boolean.into();
            result
        } else {
            false
        }
    }
}

pub fn set_enhanced_user_interface(element: &AXUIElement, enabled: bool) -> Result<(), AXError> {
    unsafe {
        let cf_bool = if enabled {
            CFBoolean::true_value()
        } else {
            CFBoolean::false_value()
        };

        let error = AXUIElementSetAttributeValue(
            element.as_concrete_TypeRef(),
            CFString::from_static_string(K_AX_ENHANCED_USER_INTERFACE).as_concrete_TypeRef(),
            cf_bool.as_CFTypeRef(),
        );

        if error == kAXErrorSuccess {
            Ok(())
        } else {
            Err(error)
        }
    }
}

pub fn with_enhanced_ui_disabled<F, R>(element: &AXUIElement, f: F) -> R
where F: FnOnce() -> R {
    let original_state = get_enhanced_user_interface(element);

    if original_state {
        if let Err(error) = set_enhanced_user_interface(element, false) {
            warn!("Failed to disable Enhanced User Interface: {:?}", error);
        }
    }

    let result = f();

    if original_state {
        if let Err(error) = set_enhanced_user_interface(element, true) {
            warn!("Failed to restore Enhanced User Interface: {:?}", error);
        }
    }

    result
}

pub fn with_system_enhanced_ui_disabled<F, R>(f: F) -> R
where F: FnOnce() -> R {
    let system_element = AXUIElement::system_wide();
    with_enhanced_ui_disabled(&system_element, f)
}
