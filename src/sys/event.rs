use objc2_app_kit::NSEvent;
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{
    CGDisplayHideCursor, CGDisplayShowCursor, CGError, kCGNullDirectDisplay,
};
use serde::{Deserialize, Serialize};
use tracing::{error, warn};

use super::screen::CoordinateConverter;
use crate::actor::reactor::Command;
use crate::actor::wm_controller::{Sender, WmCommand};
use crate::sys::cg_ok;
use crate::sys::hotkey::HotkeyManager as InnerHotkeyManager;
pub use crate::sys::hotkey::{Hotkey, KeyCode, Modifiers};
use crate::sys::skylight::CGWarpMouseCursorPosition;

pub struct HotkeyManager {
    inner: Option<InnerHotkeyManager>,
    _events_tx: Sender,
}

impl HotkeyManager {
    pub fn new(events_tx: Sender) -> Self {
        match InnerHotkeyManager::new(events_tx.clone()) {
            Ok(mgr) => HotkeyManager {
                inner: Some(mgr),
                _events_tx: events_tx,
            },
            Err(e) => {
                error!(
                    "Failed to create EventTap-based HotkeyManager: {e:?}. Hotkeys will be disabled."
                );
                HotkeyManager {
                    inner: None,
                    _events_tx: events_tx,
                }
            }
        }
    }

    pub fn register(&self, modifiers: Modifiers, key_code: KeyCode, cmd: Command) {
        self.register_wm(modifiers, key_code, WmCommand::ReactorCommand(cmd))
    }

    pub fn register_wm(&self, modifiers: Modifiers, key_code: KeyCode, cmd: WmCommand) {
        if let Some(inner) = &self.inner {
            inner.register_wm(modifiers, key_code, cmd);
        } else {
            warn!(
                "HotkeyManager not initialized; ignoring registration for {} + {}",
                modifiers, key_code
            );
        }
    }
}

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
