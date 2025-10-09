use tracing::warn;

use crate::sys::axuielement::{AXUIElement, Error as AxError};

const K_AX_ENHANCED_USER_INTERFACE: &str = "AXEnhancedUserInterface";

pub fn get_enhanced_user_interface(element: &AXUIElement) -> bool {
    element.bool_attribute(K_AX_ENHANCED_USER_INTERFACE).unwrap_or(false)
}

pub fn set_enhanced_user_interface(element: &AXUIElement, enabled: bool) -> Result<(), AxError> {
    element.set_bool_attribute(K_AX_ENHANCED_USER_INTERFACE, enabled)
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
