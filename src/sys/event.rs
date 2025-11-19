use std::convert::TryFrom;
use std::sync::atomic::{AtomicU8, Ordering};

use objc2_app_kit::NSEvent;
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{
    CGDisplayHideCursor, CGDisplayShowCursor, CGError, kCGNullDirectDisplay,
};
use serde::{Deserialize, Serialize};

pub use super::window_server::current_cursor_location;
use crate::sys::cg_ok;
pub use crate::sys::hotkey::{Hotkey, HotkeySpec, KeyCode, Modifiers};
use crate::sys::skylight::CGWarpMouseCursorPosition;

#[derive(Serialize, Deserialize, Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
pub enum MouseState {
    Up = 1,
    Down = 2,
}

const MOUSE_STATE_UNKNOWN: u8 = 0;

static MOUSE_STATE_ATOM: AtomicU8 = AtomicU8::new(MOUSE_STATE_UNKNOWN);

impl From<MouseState> for u8 {
    fn from(state: MouseState) -> u8 { state as u8 }
}

impl TryFrom<u8> for MouseState {
    type Error = ();

    fn try_from(val: u8) -> Result<Self, Self::Error> {
        match val {
            x if x == MouseState::Up as u8 => Ok(MouseState::Up),
            x if x == MouseState::Down as u8 => Ok(MouseState::Down),
            _ => Err(()),
        }
    }
}

pub fn set_mouse_state(state: MouseState) {
    MOUSE_STATE_ATOM.store(state.into(), Ordering::Relaxed);
}

pub fn get_mouse_state() -> MouseState {
    let raw = MOUSE_STATE_ATOM.load(Ordering::Relaxed);
    MouseState::try_from(raw).unwrap_or_else(|_| {
        if NSEvent::pressedMouseButtons() & 0x1 != 0 {
            MouseState::Down
        } else {
            MouseState::Up
        }
    })
}

pub fn warp_mouse(point: CGPoint) -> Result<(), CGError> {
    cg_ok(unsafe { CGWarpMouseCursorPosition(point) })
}

pub fn hide_mouse() -> Result<(), CGError> { cg_ok(CGDisplayHideCursor(kCGNullDirectDisplay)) }

pub fn show_mouse() -> Result<(), CGError> { cg_ok(CGDisplayShowCursor(kCGNullDirectDisplay)) }
