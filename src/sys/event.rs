use objc2_app_kit::NSEvent;
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{
    CGDisplayHideCursor, CGDisplayShowCursor, CGError, kCGNullDirectDisplay,
};
use serde::{Deserialize, Serialize};

use super::screen::CoordinateConverter;
use crate::sys::cg_ok;
pub use crate::sys::hotkey::{Hotkey, KeyCode, Modifiers};
use crate::sys::skylight::CGWarpMouseCursorPosition;

#[derive(Serialize, Deserialize, Debug, Copy, Clone, Eq, PartialEq)]
pub enum MouseState {
    Down,
    Up,
}

pub fn get_mouse_state() -> MouseState {
    let left_button = NSEvent::pressedMouseButtons() & 0x1 != 0;
    if left_button {
        MouseState::Down
    } else {
        MouseState::Up
    }
}

pub fn get_mouse_pos(converter: CoordinateConverter) -> Option<CGPoint> {
    let ns_loc = NSEvent::mouseLocation();
    converter.convert_point(ns_loc)
}

pub fn warp_mouse(point: CGPoint) -> Result<(), CGError> {
    cg_ok(unsafe { CGWarpMouseCursorPosition(point) })
}

pub fn hide_mouse() -> Result<(), CGError> { cg_ok(CGDisplayHideCursor(kCGNullDirectDisplay)) }

pub fn show_mouse() -> Result<(), CGError> { cg_ok(CGDisplayShowCursor(kCGNullDirectDisplay)) }
