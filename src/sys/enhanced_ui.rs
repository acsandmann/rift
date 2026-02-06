use crate::sys::axuielement::AXUIElement;

const K_AX_ENHANCED_USER_INTERFACE: &str = "AXEnhancedUserInterface";

pub fn with_enhanced_ui_disabled<F, R>(element: &AXUIElement, f: F) -> R
where F: FnOnce() -> R {
    let _ = element.set_bool_attribute(K_AX_ENHANCED_USER_INTERFACE, false);
    let result = f();
    let _ = element.set_bool_attribute(K_AX_ENHANCED_USER_INTERFACE, true);

    result
}
