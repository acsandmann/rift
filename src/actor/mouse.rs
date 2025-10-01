use std::cell::RefCell;
use std::mem::replace;
use std::rc::Rc;

use objc2_app_kit::NSEvent;
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapProxy, CGEventType,
};
use objc2_foundation::{MainThreadMarker, NSInteger};
use tracing::{debug, error, trace, warn};

use super::reactor::{self, Event};
use crate::actor;
use crate::common::collections::HashSet;
use crate::common::config::Config;
use crate::common::log::trace_misc;
use crate::sys::event::{self, Hotkey, KeyCode, Modifiers};
use crate::sys::geometry::CGRectExt;
use crate::sys::screen::CoordinateConverter;
use crate::sys::window_server::{self, WindowServerId, get_window};

#[derive(Debug)]
pub enum Request {
    Warp(CGPoint),
    EnforceHidden,
    ScreenParametersChanged(Vec<CGRect>, CoordinateConverter),
    SetEventProcessing(bool),
    SetFocusFollowsMouseEnabled(bool),
}

pub struct Mouse {
    config: Config,
    events_tx: reactor::Sender,
    requests_rx: Option<Receiver>,
    state: RefCell<State>,
    tap: RefCell<Option<crate::sys::event_tap::EventTap>>,
    disable_hotkey: Option<Hotkey>,
}

struct State {
    hidden: bool,
    above_window: Option<WindowServerId>,
    above_window_level: NSWindowLevel,
    converter: CoordinateConverter,
    screens: Vec<CGRect>,
    event_processing_enabled: bool,
    focus_follows_mouse_enabled: bool,
    disable_hotkey_active: bool,
    pressed_keys: HashSet<KeyCode>,
    current_flags: CGEventFlags,
}

impl Default for State {
    fn default() -> Self {
        Self {
            hidden: false,
            above_window: None,
            above_window_level: NSWindowLevel::MIN,
            converter: CoordinateConverter::default(),
            screens: Vec::new(),
            event_processing_enabled: false,
            focus_follows_mouse_enabled: true,
            disable_hotkey_active: false,
            pressed_keys: HashSet::default(),
            current_flags: CGEventFlags::empty(),
        }
    }
}

pub type Sender = actor::Sender<Request>;
pub type Receiver = actor::Receiver<Request>;

struct CallbackCtx {
    this: Rc<Mouse>,
    mtm: MainThreadMarker,
}

unsafe fn drop_mouse_ctx(ptr: *mut std::ffi::c_void) {
    unsafe { drop(Box::from_raw(ptr as *mut CallbackCtx)) };
}

impl Mouse {
    pub fn new(config: Config, events_tx: reactor::Sender, requests_rx: Receiver) -> Self {
        let disable_hotkey = config.settings.focus_follows_mouse_disable_hotkey;
        Mouse {
            config,
            events_tx,
            requests_rx: Some(requests_rx),
            state: RefCell::new(State::default()),
            tap: RefCell::new(None),
            disable_hotkey,
        }
    }

    pub async fn run(mut self) {
        let mut requests_rx = self.requests_rx.take().unwrap();

        let this = Rc::new(self);

        let mask: CGEventMask = {
            let mut m = 0u64;
            for ty in [
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGEventType::MouseMoved,
                CGEventType::LeftMouseDragged,
                CGEventType::RightMouseDragged,
            ] {
                m |= 1u64 << (ty.0 as u64);
            }
            if this.disable_hotkey.is_some() {
                for ty in [
                    CGEventType::KeyDown,
                    CGEventType::KeyUp,
                    CGEventType::FlagsChanged,
                ] {
                    m |= 1u64 << (ty.0 as u64);
                }
            }
            m
        };

        let ctx = Box::new(CallbackCtx {
            this: Rc::clone(&this),
            mtm: MainThreadMarker::new().unwrap(),
        });
        let ctx_ptr = Box::into_raw(ctx) as *mut std::ffi::c_void;

        let tap = unsafe {
            crate::sys::event_tap::EventTap::new_listen_only(
                mask,
                Some(mouse_callback),
                ctx_ptr,
                Some(drop_mouse_ctx),
            )
        };

        if let Some(tap) = tap {
            *this.tap.borrow_mut() = Some(tap);
        } else {
            unsafe { drop(Box::from_raw(ctx_ptr as *mut CallbackCtx)) };
            return;
        }

        if this.config.settings.mouse_hides_on_focus {
            if let Err(e) = window_server::allow_hide_mouse() {
                error!(
                    "Could not enable mouse hiding: {e:?}. \
                    mouse_hides_on_focus will have no effect."
                );
            }
        }

        while let Some((span, request)) = requests_rx.recv().await {
            let _ = span.enter();
            this.on_request(request);
        }
    }

    fn on_request(self: &Rc<Self>, request: Request) {
        let mut state = self.state.borrow_mut();
        match request {
            Request::Warp(point) => {
                if let Err(e) = event::warp_mouse(point) {
                    warn!("Failed to warp mouse: {e:?}");
                }
                if self.config.settings.mouse_hides_on_focus && !state.hidden {
                    debug!("Hiding mouse");
                    if let Err(e) = event::hide_mouse() {
                        warn!("Failed to hide mouse: {e:?}");
                    }
                    state.hidden = true;
                }
            }
            Request::EnforceHidden => {
                if state.hidden {
                    if let Err(e) = event::hide_mouse() {
                        warn!("Failed to hide mouse: {e:?}");
                    }
                }
            }
            Request::ScreenParametersChanged(frames, converter) => {
                state.screens = frames;
                state.converter = converter;
            }
            Request::SetEventProcessing(enabled) => {
                state.event_processing_enabled = enabled;
            }
            Request::SetFocusFollowsMouseEnabled(enabled) => {
                debug!(
                    "focus_follows_mouse temporarily {}",
                    if enabled { "enabled" } else { "disabled" }
                );
                state.focus_follows_mouse_enabled = enabled;
            }
        }
    }

    fn on_event(self: &Rc<Self>, event_type: CGEventType, event: &CGEvent, mtm: MainThreadMarker) {
        let mut state = self.state.borrow_mut();

        if matches!(
            event_type,
            CGEventType::KeyDown | CGEventType::KeyUp | CGEventType::FlagsChanged
        ) {
            self.handle_keyboard_event(event_type, event, &mut state);
            return;
        }

        if !state.event_processing_enabled {
            trace!("Mouse event processing disabled, ignoring {:?}", event_type);
            return;
        }

        if state.hidden {
            debug!("Showing mouse");
            if let Err(e) = event::show_mouse() {
                warn!("Failed to show mouse: {e:?}");
            }
            state.hidden = false;
        }
        match event_type {
            CGEventType::LeftMouseUp => {
                _ = self.events_tx.send(Event::MouseUp);
            }
            CGEventType::MouseMoved
                if self.config.settings.focus_follows_mouse
                    && state.focus_follows_mouse_enabled
                    && !state.disable_hotkey_active =>
            {
                let loc = unsafe { NSEvent::mouseLocation() };
                trace!("Mouse moved {loc:?}");
                if let Some(wsid) = state.track_mouse_move(loc, mtm) {
                    _ = self.events_tx.send(Event::MouseMovedOverWindow(wsid));
                }
            }
            _ => (),
        }
    }

    fn handle_keyboard_event(&self, event_type: CGEventType, event: &CGEvent, state: &mut State) {
        let Some(target) = self.disable_hotkey else {
            return;
        };

        let prev_active = state.disable_hotkey_active;

        if let Some(key_code) = key_code_from_event(event) {
            match event_type {
                CGEventType::KeyDown => state.note_key_down(key_code),
                CGEventType::KeyUp => state.note_key_up(key_code),
                CGEventType::FlagsChanged => state.note_flags_changed(key_code),
                _ => {}
            }
        }

        let flags = unsafe { CGEvent::flags(Some(event)) };
        state.current_flags = flags;
        state.disable_hotkey_active = state.compute_disable_hotkey_active(target);

        if state.disable_hotkey_active != prev_active {
            if state.disable_hotkey_active {
                debug!(?target, "focus_follows_mouse disabled while hotkey held");
            } else {
                debug!(?target, "focus_follows_mouse re-enabled after hotkey release");
            }
        }
    }
}

unsafe extern "C-unwind" fn mouse_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event_ref: core::ptr::NonNull<CGEvent>,
    user_info: *mut std::ffi::c_void,
) -> *mut CGEvent {
    let ctx = unsafe { &*(user_info as *const CallbackCtx) };
    // kCGEventTapDisabledByTimeout (-2) and kCGEventTapDisabledByUserInput (-1).
    let ety = event_type.0 as i64;
    if ety == -1 || ety == -2 {
        if let Some(tap) = ctx.this.tap.borrow().as_ref() {
            tap.set_enabled(true);
        }
        return event_ref.as_ptr();
    }

    let event = unsafe { event_ref.as_ref() };
    ctx.this.on_event(event_type, event, ctx.mtm);
    event_ref.as_ptr()
}

impl State {
    fn note_key_down(&mut self, key_code: KeyCode) { self.pressed_keys.insert(key_code); }

    fn note_key_up(&mut self, key_code: KeyCode) { self.pressed_keys.remove(&key_code); }

    fn note_flags_changed(&mut self, key_code: KeyCode) {
        if is_modifier_key(key_code) {
            self.pressed_keys.remove(&key_code);
        }
    }

    fn compute_disable_hotkey_active(&self, target: Hotkey) -> bool {
        let active_mods = modifiers_from_flags(self.current_flags);
        if !active_mods.contains(target.modifiers) {
            return false;
        }

        self.base_key_active(target.key_code)
    }

    fn base_key_active(&self, key_code: KeyCode) -> bool {
        if is_modifier_key(key_code) {
            modifier_flag_for_key(key_code)
                .map(|flag| self.current_flags.contains(flag))
                .unwrap_or(false)
        } else {
            self.pressed_keys.contains(&key_code)
        }
    }

    fn track_mouse_move(&mut self, loc: CGPoint, mtm: MainThreadMarker) -> Option<WindowServerId> {
        let new_window = trace_misc("get_window_at_point", || {
            window_server::get_window_at_point(loc, self.converter, mtm)
        });
        if self.above_window == new_window {
            return None;
        }
        debug!("Mouse is now above window {new_window:?} at {loc:?}");

        // There is a gap between the menu bar and the actual menu pop-ups when
        // a menu is opened. When the mouse goes over this gap, the system
        // reports it to be over whatever window happens to be below the menu
        // bar and behind the pop-up. Ignore anything in this gap so we don't
        // dismiss the pop-up. Strangely, it only seems to happen when the mouse
        // travels down from the menu bar and not when it travels back up.
        // First observed on 13.5.2.
        if self.above_window_level == NSMainMenuWindowLevel {
            const WITHIN: f64 = 1.0;
            for screen in &self.screens {
                if screen.contains(CGPoint::new(loc.x, loc.y + WITHIN))
                    && loc.y < screen.min().y + WITHIN
                {
                    return None;
                }
            }
        }

        let old_window = replace(&mut self.above_window, new_window);
        let new_window_level = new_window
            .and_then(|id| trace_misc("get_window", || get_window(id)))
            .map(|info| info.layer as NSWindowLevel)
            .unwrap_or(NSWindowLevel::MIN);
        let old_window_level = replace(&mut self.above_window_level, new_window_level);
        debug!(?old_window, ?old_window_level, ?new_window, ?new_window_level);

        if old_window_level >= NSPopUpMenuWindowLevel {
            return None;
        }

        if !(0..NSPopUpMenuWindowLevel).contains(&new_window_level) {
            return None;
        }

        new_window
    }
}

fn modifiers_from_flags(flags: CGEventFlags) -> Modifiers {
    let mut mods = Modifiers::empty();
    if flags.contains(CGEventFlags::MaskControl) {
        mods.insert(Modifiers::CONTROL);
    }
    if flags.contains(CGEventFlags::MaskAlternate) {
        mods.insert(Modifiers::ALT);
    }
    if flags.contains(CGEventFlags::MaskCommand) {
        mods.insert(Modifiers::META);
    }
    if flags.contains(CGEventFlags::MaskShift) {
        mods.insert(Modifiers::SHIFT);
    }
    mods
}

fn modifier_flag_for_key(key_code: KeyCode) -> Option<CGEventFlags> {
    match key_code {
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Some(CGEventFlags::MaskShift),
        KeyCode::ControlLeft | KeyCode::ControlRight => Some(CGEventFlags::MaskControl),
        KeyCode::AltLeft | KeyCode::AltRight => Some(CGEventFlags::MaskAlternate),
        KeyCode::MetaLeft | KeyCode::MetaRight => Some(CGEventFlags::MaskCommand),
        KeyCode::CapsLock => Some(CGEventFlags::MaskAlphaShift),
        KeyCode::Fn => Some(CGEventFlags::MaskSecondaryFn),
        KeyCode::NumLock => Some(CGEventFlags::MaskNumericPad),
        _ => None,
    }
}

fn is_modifier_key(key_code: KeyCode) -> bool { modifier_flag_for_key(key_code).is_some() }

fn key_code_from_event(event: &CGEvent) -> Option<KeyCode> {
    let raw =
        unsafe { CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventKeycode) };
    if raw < 0 {
        return None;
    }
    cg_keycode_to_keycode(raw as u16)
}

fn cg_keycode_to_keycode(code: u16) -> Option<KeyCode> {
    use KeyCode::*;

    let key = match code {
        0x00 => KeyA,
        0x01 => KeyS,
        0x02 => KeyD,
        0x03 => KeyF,
        0x04 => KeyH,
        0x05 => KeyG,
        0x06 => KeyZ,
        0x07 => KeyX,
        0x08 => KeyC,
        0x09 => KeyV,
        0x0A => IntlBackslash,
        0x0B => KeyB,
        0x0C => KeyQ,
        0x0D => KeyW,
        0x0E => KeyE,
        0x0F => KeyR,
        0x10 => KeyY,
        0x11 => KeyT,
        0x12 => Digit1,
        0x13 => Digit2,
        0x14 => Digit3,
        0x15 => Digit4,
        0x16 => Digit6,
        0x17 => Digit5,
        0x18 => Equal,
        0x19 => Digit9,
        0x1A => Digit7,
        0x1B => Minus,
        0x1C => Digit8,
        0x1D => Digit0,
        0x1E => BracketRight,
        0x1F => KeyO,
        0x20 => KeyU,
        0x21 => BracketLeft,
        0x22 => KeyI,
        0x23 => KeyP,
        0x24 => Enter,
        0x25 => KeyL,
        0x26 => KeyJ,
        0x27 => Quote,
        0x28 => KeyK,
        0x29 => Semicolon,
        0x2A => Backslash,
        0x2B => Comma,
        0x2C => Slash,
        0x2D => KeyN,
        0x2E => KeyM,
        0x2F => Period,
        0x30 => Tab,
        0x31 => Space,
        0x32 => Backquote,
        0x33 => Backspace,
        0x34 => NumpadEnter,
        0x35 => Escape,
        0x36 => MetaRight,
        0x37 => MetaLeft,
        0x38 => ShiftLeft,
        0x39 => CapsLock,
        0x3A => AltLeft,
        0x3B => ControlLeft,
        0x3C => ShiftRight,
        0x3D => AltRight,
        0x3E => ControlRight,
        0x3F => Fn,
        0x40 => F17,
        0x41 => NumpadDecimal,
        0x43 => NumpadMultiply,
        0x45 => NumpadAdd,
        0x47 => NumLock,
        0x48 => AudioVolumeUp,
        0x49 => AudioVolumeDown,
        0x4A => AudioVolumeMute,
        0x4B => NumpadDivide,
        0x4C => NumpadEnter,
        0x4E => NumpadSubtract,
        0x4F => F18,
        0x50 => F19,
        0x51 => NumpadEqual,
        0x52 => Numpad0,
        0x53 => Numpad1,
        0x54 => Numpad2,
        0x55 => Numpad3,
        0x56 => Numpad4,
        0x57 => Numpad5,
        0x58 => Numpad6,
        0x59 => Numpad7,
        0x5A => F20,
        0x5B => Numpad8,
        0x5C => Numpad9,
        0x5D => IntlYen,
        0x5E => IntlRo,
        0x5F => NumpadComma,
        0x60 => F5,
        0x61 => F6,
        0x62 => F7,
        0x63 => F3,
        0x64 => F8,
        0x65 => F9,
        0x66 => Lang2,
        0x67 => F11,
        0x68 => Lang1,
        0x69 => F13,
        0x6A => F16,
        0x6B => F14,
        0x6D => F10,
        0x6E => ContextMenu,
        0x6F => F12,
        0x71 => F15,
        0x72 => Insert,
        0x73 => Home,
        0x74 => PageUp,
        0x75 => Delete,
        0x76 => F4,
        0x77 => End,
        0x78 => F2,
        0x79 => PageDown,
        0x7A => F1,
        0x7B => ArrowLeft,
        0x7C => ArrowRight,
        0x7D => ArrowDown,
        0x7E => ArrowUp,
        _ => return None,
    };

    Some(key)
}

/// https://developer.apple.com/documentation/appkit/nswindowlevel?language=objc
pub type NSWindowLevel = NSInteger;
#[allow(non_upper_case_globals)]
pub const NSMainMenuWindowLevel: NSWindowLevel = 24;
#[allow(non_upper_case_globals)]
pub const NSPopUpMenuWindowLevel: NSWindowLevel = 101;
