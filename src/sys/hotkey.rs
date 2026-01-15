use std::collections::HashMap as StdHashMap;
use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;
use std::str::FromStr;
use std::sync::LazyLock;

use anyhow::anyhow;
use objc2_core_foundation::CFData;
use objc2_core_graphics::{CGEvent, CGEventField, CGEventFlags};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const ALT: Modifiers = Modifiers(0b0011_0000);
    pub const ALT_LEFT: Modifiers = Modifiers(0b0001_0000);
    pub const ALT_RIGHT: Modifiers = Modifiers(0b0010_0000);
    pub const CONTROL: Modifiers = Modifiers(0b0000_1100);
    pub const CONTROL_LEFT: Modifiers = Modifiers(0b0000_0100);
    pub const CONTROL_RIGHT: Modifiers = Modifiers(0b0000_1000);
    pub const META: Modifiers = Modifiers(0b1100_0000);
    pub const META_LEFT: Modifiers = Modifiers(0b0100_0000);
    pub const META_RIGHT: Modifiers = Modifiers(0b1000_0000);
    // Generic modifiers (match either left or right)
    pub const SHIFT: Modifiers = Modifiers(0b0000_0011);
    // Specific left/right modifier bits
    pub const SHIFT_LEFT: Modifiers = Modifiers(0b0000_0001);
    pub const SHIFT_RIGHT: Modifiers = Modifiers(0b0000_0010);

    pub fn empty() -> Self { Modifiers(0) }

    pub fn contains(&self, other: Modifiers) -> bool { (self.0 & other.0) == other.0 }

    pub fn intersects(&self, other: Modifiers) -> bool { (self.0 & other.0) != 0 }

    pub fn insert(&mut self, other: Modifiers) { self.0 |= other.0; }

    pub fn remove(&mut self, other: Modifiers) { self.0 &= !other.0; }

    pub fn has_generic_modifiers(&self) -> bool {
        MOD_FAMILIES.iter().any(|m| self.contains(m.generic))
    }

    pub fn expand_to_specific(&self) -> Vec<Modifiers> {
        let mut variants = vec![Modifiers::empty()];

        for m in MOD_FAMILIES {
            let has_left = self.contains(m.left);
            let has_right = self.contains(m.right);

            if has_left && has_right {
                let mut new_variants = Vec::with_capacity(variants.len() * 2);
                for v in &variants {
                    let mut vl = *v;
                    vl.insert(m.left);
                    new_variants.push(vl);

                    let mut vr = *v;
                    vr.insert(m.right);
                    new_variants.push(vr);
                }
                variants = new_variants;
            } else if has_left {
                for v in &mut variants {
                    v.insert(m.left);
                }
            } else if has_right {
                for v in &mut variants {
                    v.insert(m.right);
                }
            }
        }

        variants
    }

    pub fn insert_from_token(&mut self, token: &str) -> bool {
        if let Some(mods) = modifier_from_token(token) {
            self.insert(mods);
            return true;
        }
        false
    }
}

impl fmt::Display for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<&str> = Vec::new();

        for m in MOD_FAMILIES {
            let l = self.contains(m.left);
            let r = self.contains(m.right);

            match (l, r) {
                (true, true) => parts.push(m.name),
                (true, false) => parts.push(m.left_name),
                (false, true) => parts.push(m.right_name),
                (false, false) => {}
            }
        }

        write!(f, "{}", parts.join(" + "))
    }
}

#[derive(Clone, Copy)]
struct ModFamily {
    name: &'static str,
    left_name: &'static str,
    right_name: &'static str,

    generic: Modifiers,
    left: Modifiers,
    right: Modifiers,

    left_key: KeyCode,
    right_key: KeyCode,

    mask: CGEventFlags,
}

const MOD_FAMILIES: &[ModFamily] = &[
    ModFamily {
        name: "Ctrl",
        left_name: "CtrlLeft",
        right_name: "CtrlRight",
        generic: Modifiers::CONTROL,
        left: Modifiers::CONTROL_LEFT,
        right: Modifiers::CONTROL_RIGHT,
        left_key: KeyCode::ControlLeft,
        right_key: KeyCode::ControlRight,
        mask: CGEventFlags::MaskControl,
    },
    ModFamily {
        name: "Alt",
        left_name: "AltLeft",
        right_name: "AltRight",
        generic: Modifiers::ALT,
        left: Modifiers::ALT_LEFT,
        right: Modifiers::ALT_RIGHT,
        left_key: KeyCode::AltLeft,
        right_key: KeyCode::AltRight,
        mask: CGEventFlags::MaskAlternate,
    },
    ModFamily {
        name: "Shift",
        left_name: "ShiftLeft",
        right_name: "ShiftRight",
        generic: Modifiers::SHIFT,
        left: Modifiers::SHIFT_LEFT,
        right: Modifiers::SHIFT_RIGHT,
        left_key: KeyCode::ShiftLeft,
        right_key: KeyCode::ShiftRight,
        mask: CGEventFlags::MaskShift,
    },
    ModFamily {
        name: "Meta",
        left_name: "MetaLeft",
        right_name: "MetaRight",
        generic: Modifiers::META,
        left: Modifiers::META_LEFT,
        right: Modifiers::META_RIGHT,
        left_key: KeyCode::MetaLeft,
        right_key: KeyCode::MetaRight,
        mask: CGEventFlags::MaskCommand,
    },
];

fn normalize_token(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[derive(Copy, Clone)]
enum Side {
    Left,
    Right,
}

fn split_side(token: &str) -> (Option<Side>, &str) {
    if let Some(rest) = token.strip_prefix("left") {
        return (Some(Side::Left), rest);
    }
    if let Some(rest) = token.strip_prefix("right") {
        return (Some(Side::Right), rest);
    }
    if let Some(rest) = token.strip_suffix("left") {
        return (Some(Side::Left), rest);
    }
    if let Some(rest) = token.strip_suffix("right") {
        return (Some(Side::Right), rest);
    }
    if let Some(rest) = token.strip_prefix('l') {
        return (Some(Side::Left), rest);
    }
    if let Some(rest) = token.strip_prefix('r') {
        return (Some(Side::Right), rest);
    }
    (None, token)
}

fn modifier_from_token(token: &str) -> Option<Modifiers> {
    let t = normalize_token(token);
    let (side, base) = split_side(&t);
    let family = match base {
        "alt" | "option" => &MOD_FAMILIES[1],
        "ctrl" | "control" => &MOD_FAMILIES[0],
        "shift" => &MOD_FAMILIES[2],
        "meta" | "cmd" | "command" => &MOD_FAMILIES[3],
        _ => return None,
    };

    match side {
        None => Some(family.generic),
        Some(Side::Left) => Some(family.left),
        Some(Side::Right) => Some(family.right),
    }
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum KeyCode {
    KeyA,
    KeyS,
    KeyD,
    KeyF,
    KeyH,
    KeyG,
    KeyZ,
    KeyX,
    KeyC,
    KeyV,
    IntlBackslash,
    KeyB,
    KeyQ,
    KeyW,
    KeyE,
    KeyR,
    KeyY,
    KeyT,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit6,
    Digit5,
    Equal,
    Digit9,
    Digit7,
    Minus,
    Digit8,
    Digit0,
    BracketRight,
    KeyO,
    KeyU,
    BracketLeft,
    KeyI,
    KeyP,
    Enter,
    KeyL,
    KeyJ,
    Quote,
    KeyK,
    Semicolon,
    Backslash,
    Comma,
    Slash,
    KeyN,
    KeyM,
    Period,
    Tab,
    Space,
    Backquote,
    Backspace,
    NumpadEnter,
    NumpadSubtract,
    Escape,
    MetaRight,
    MetaLeft,
    ShiftLeft,
    CapsLock,
    AltLeft,
    ControlLeft,
    ShiftRight,
    AltRight,
    ControlRight,
    Fn,
    F17,
    NumpadDecimal,
    NumpadMultiply,
    NumpadAdd,
    NumLock,
    AudioVolumeUp,
    AudioVolumeDown,
    AudioVolumeMute,
    NumpadDivide,
    F18,
    F19,
    NumpadEqual,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    F20,
    Numpad8,
    Numpad9,
    IntlYen,
    IntlRo,
    NumpadComma,
    F5,
    F6,
    F7,
    F3,
    F8,
    F9,
    Lang2,
    F11,
    Lang1,
    F13,
    F16,
    F14,
    F10,
    ContextMenu,
    F12,
    F15,
    Insert,
    Home,
    PageUp,
    Delete,
    F4,
    End,
    F2,
    PageDown,
    F1,
    ArrowLeft,
    ArrowRight,
    ArrowDown,
    ArrowUp,
}

impl fmt::Display for KeyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use KeyCode::*;
        let s = match self {
            KeyA => "A",
            KeyS => "S",
            KeyD => "D",
            KeyF => "F",
            KeyH => "H",
            KeyG => "G",
            KeyZ => "Z",
            KeyX => "X",
            KeyC => "C",
            KeyV => "V",
            KeyB => "B",
            KeyQ => "Q",
            KeyW => "W",
            KeyE => "E",
            KeyR => "R",
            KeyY => "Y",
            KeyT => "T",
            Digit1 => "1",
            Digit2 => "2",
            Digit3 => "3",
            Digit4 => "4",
            Digit5 => "5",
            Digit6 => "6",
            Digit7 => "7",
            Digit8 => "8",
            Digit9 => "9",
            Digit0 => "0",
            ArrowLeft => "Left",
            ArrowRight => "Right",
            ArrowUp => "Up",
            ArrowDown => "Down",
            Tab => "Tab",
            Space => "Space",
            Enter => "Enter",
            Escape => "Escape",
            _ => "Other",
        };
        write!(f, "{}", s)
    }
}

const F_KEYS: [KeyCode; 20] = [
    KeyCode::F1,
    KeyCode::F2,
    KeyCode::F3,
    KeyCode::F4,
    KeyCode::F5,
    KeyCode::F6,
    KeyCode::F7,
    KeyCode::F8,
    KeyCode::F9,
    KeyCode::F10,
    KeyCode::F11,
    KeyCode::F12,
    KeyCode::F13,
    KeyCode::F14,
    KeyCode::F15,
    KeyCode::F16,
    KeyCode::F17,
    KeyCode::F18,
    KeyCode::F19,
    KeyCode::F20,
];

impl FromStr for KeyCode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(anyhow!("Unrecognized key token: <empty>"));
        }

        if s.chars().count() == 1 {
            if let Some(k) = keycode_from_char(s) {
                return Ok(k);
            }

            return Err(anyhow!("carbon keymap failed"));
        }

        let t = normalize_token(s);

        // function keys
        if let Some(rest) = t.strip_prefix('f') {
            if let Ok(n) = rest.parse::<u8>() {
                if (1..=20).contains(&n) {
                    return Ok(F_KEYS[(n - 1) as usize]);
                }
            }
        }

        let key = match t.as_str() {
            "left" | "arrowleft" => KeyCode::ArrowLeft,
            "right" | "arrowright" => KeyCode::ArrowRight,
            "up" | "arrowup" => KeyCode::ArrowUp,
            "down" | "arrowdown" => KeyCode::ArrowDown,

            "tab" => KeyCode::Tab,
            "space" => KeyCode::Space,
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Escape,
            "fn" => KeyCode::Fn,

            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "insert" => KeyCode::Insert,
            "delete" | "del" => KeyCode::Delete,

            "minus" | "hyphen" => KeyCode::Minus,
            "equal" | "equals" => KeyCode::Equal,
            "comma" => KeyCode::Comma,
            "period" | "dot" => KeyCode::Period,
            "slash" | "forwardslash" => KeyCode::Slash,
            "semicolon" => KeyCode::Semicolon,
            "quote" | "apostrophe" => KeyCode::Quote,
            "backquote" | "grave" | "tilde" => KeyCode::Backquote,
            "backslash" => KeyCode::Backslash,
            "bracketleft" | "leftbracket" | "leftsquarebracket" => KeyCode::BracketLeft,
            "bracketright" | "rightbracket" | "rightsquarebracket" => KeyCode::BracketRight,

            other => return Err(anyhow!("Unrecognized key token: {}", other)),
        };

        Ok(key)
    }
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key_code: KeyCode,
}

impl Hotkey {
    pub fn new(modifiers: Modifiers, key_code: KeyCode) -> Self { Self { modifiers, key_code } }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers == Modifiers::empty() {
            write!(f, "{}", self.key_code)
        } else {
            write!(f, "{} + {}", self.modifiers, self.key_code)
        }
    }
}

fn parse_mods_and_optional_key(s: &str) -> Result<(Modifiers, Option<KeyCode>), anyhow::Error> {
    let parts: Vec<&str> = s.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()).collect();

    let mut mods = Modifiers::empty();
    let mut key_opt: Option<KeyCode> = None;

    for part in parts {
        if mods.insert_from_token(part) {
            continue;
        }
        let code = KeyCode::from_str(part)?;
        key_opt = Some(code);
    }

    Ok((mods, key_opt))
}

impl FromStr for Hotkey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (mods, key_opt) = parse_mods_and_optional_key(s)?;
        let key_code = key_opt.ok_or_else(|| anyhow!("No key specified in hotkey: {}", s))?;
        Ok(Hotkey::new(mods, key_code))
    }
}

impl<'de> Deserialize<'de> for Hotkey {
    fn deserialize<D>(deserializer: D) -> Result<Hotkey, D::Error>
    where D: serde::Deserializer<'de> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum HotkeyRepr {
            Str(String),
            Map {
                modifiers: Modifiers,
                key_code: KeyCode,
            },
        }

        let repr = HotkeyRepr::deserialize(deserializer)?;
        match repr {
            HotkeyRepr::Str(s) => Hotkey::from_str(&s).map_err(serde::de::Error::custom),
            HotkeyRepr::Map { modifiers, key_code } => Ok(Hotkey::new(modifiers, key_code)),
        }
    }
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq, Hash)]
pub enum HotkeySpec {
    Hotkey(Hotkey),
    ModifiersOnly { modifiers: Modifiers },
}

impl<'de> serde::de::Deserialize<'de> for HotkeySpec {
    fn deserialize<D>(deserializer: D) -> Result<HotkeySpec, D::Error>
    where D: serde::Deserializer<'de> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum HotkeyRepr {
            Str(String),
            Map {
                modifiers: Option<Modifiers>,
                key_code: Option<KeyCode>,
            },
        }

        let repr = HotkeyRepr::deserialize(deserializer)?;
        match repr {
            HotkeyRepr::Str(s) => {
                let (mods, key_opt) =
                    parse_mods_and_optional_key(&s).map_err(serde::de::Error::custom)?;
                if let Some(k) = key_opt {
                    Ok(HotkeySpec::Hotkey(Hotkey::new(mods, k)))
                } else if mods != Modifiers::empty() {
                    Ok(HotkeySpec::ModifiersOnly { modifiers: mods })
                } else {
                    Err(serde::de::Error::custom(format!(
                        "No key specified in hotkey: {}",
                        s
                    )))
                }
            }
            HotkeyRepr::Map { modifiers, key_code } => {
                let m = modifiers.unwrap_or(Modifiers::empty());
                if let Some(k) = key_code {
                    Ok(HotkeySpec::Hotkey(Hotkey::new(m, k)))
                } else if m != Modifiers::empty() {
                    Ok(HotkeySpec::ModifiersOnly { modifiers: m })
                } else {
                    Err(serde::de::Error::custom("No key specified in hotkey map"))
                }
            }
        }
    }
}

fn default_key_for_modifiers(mods: Modifiers) -> Option<KeyCode> {
    for m in MOD_FAMILIES {
        if !mods.intersects(m.generic) {
            continue;
        }
        if mods.contains(m.right) && !mods.contains(m.left) {
            return Some(m.right_key);
        }
        return Some(m.left_key);
    }
    None
}

impl HotkeySpec {
    pub fn to_hotkey(&self) -> Option<Hotkey> {
        match self {
            HotkeySpec::Hotkey(h) => Some(h.clone()),
            HotkeySpec::ModifiersOnly { modifiers } => {
                default_key_for_modifiers(*modifiers).map(|k| Hotkey::new(*modifiers, k))
            }
        }
    }
}

impl From<HotkeySpec> for Hotkey {
    fn from(spec: HotkeySpec) -> Hotkey {
        match spec {
            HotkeySpec::Hotkey(h) => h,
            HotkeySpec::ModifiersOnly { modifiers } => {
                if let Some(k) = default_key_for_modifiers(modifiers) {
                    Hotkey::new(modifiers, k)
                } else {
                    Hotkey::new(modifiers, KeyCode::ShiftLeft)
                }
            }
        }
    }
}

pub fn modifiers_from_flags(flags: CGEventFlags) -> Modifiers {
    let mut mods = Modifiers::empty();
    for m in MOD_FAMILIES {
        if flags.contains(m.mask) {
            mods.insert(m.generic);
        }
    }
    mods
}

pub fn modifiers_from_flags_with_keys<S: std::hash::BuildHasher>(
    flags: CGEventFlags,
    pressed_keys: &std::collections::HashSet<KeyCode, S>,
) -> Modifiers {
    let mut mods = Modifiers::empty();

    for m in MOD_FAMILIES {
        if !flags.contains(m.mask) {
            continue;
        }

        let has_left = pressed_keys.contains(&m.left_key);
        let has_right = pressed_keys.contains(&m.right_key);

        if has_left {
            mods.insert(m.left);
        }
        if has_right {
            mods.insert(m.right);
        }

        if !has_left && !has_right {
            mods.insert(m.left);
        }
    }

    mods
}

pub fn modifier_flag_for_key(key_code: KeyCode) -> Option<CGEventFlags> {
    for m in MOD_FAMILIES {
        if key_code == m.left_key || key_code == m.right_key {
            return Some(m.mask);
        }
    }

    match key_code {
        KeyCode::CapsLock => Some(CGEventFlags::MaskAlphaShift),
        KeyCode::Fn => Some(CGEventFlags::MaskSecondaryFn),
        KeyCode::NumLock => Some(CGEventFlags::MaskNumericPad),
        _ => None,
    }
}

pub fn is_modifier_key(key_code: KeyCode) -> bool { modifier_flag_for_key(key_code).is_some() }

pub fn key_code_from_event(event: &CGEvent) -> Option<KeyCode> {
    let raw = CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventKeycode);
    if raw < 0 {
        return None;
    }
    cg_keycode_to_keycode(raw as u16)
}

pub fn cg_keycode_to_keycode(code: u16) -> Option<KeyCode> {
    CG_KEYCODE_TABLE.get(code as usize).copied().flatten()
}

const fn build_cg_keycode_table() -> [Option<KeyCode>; 0x80] {
    let mut t: [Option<KeyCode>; 0x80] = [None; 0x80];

    t[0x00] = Some(KeyCode::KeyA);
    t[0x01] = Some(KeyCode::KeyS);
    t[0x02] = Some(KeyCode::KeyD);
    t[0x03] = Some(KeyCode::KeyF);
    t[0x04] = Some(KeyCode::KeyH);
    t[0x05] = Some(KeyCode::KeyG);
    t[0x06] = Some(KeyCode::KeyZ);
    t[0x07] = Some(KeyCode::KeyX);
    t[0x08] = Some(KeyCode::KeyC);
    t[0x09] = Some(KeyCode::KeyV);
    t[0x0A] = Some(KeyCode::IntlBackslash);
    t[0x0B] = Some(KeyCode::KeyB);
    t[0x0C] = Some(KeyCode::KeyQ);
    t[0x0D] = Some(KeyCode::KeyW);
    t[0x0E] = Some(KeyCode::KeyE);
    t[0x0F] = Some(KeyCode::KeyR);
    t[0x10] = Some(KeyCode::KeyY);
    t[0x11] = Some(KeyCode::KeyT);
    t[0x12] = Some(KeyCode::Digit1);
    t[0x13] = Some(KeyCode::Digit2);
    t[0x14] = Some(KeyCode::Digit3);
    t[0x15] = Some(KeyCode::Digit4);
    t[0x16] = Some(KeyCode::Digit6);
    t[0x17] = Some(KeyCode::Digit5);
    t[0x18] = Some(KeyCode::Equal);
    t[0x19] = Some(KeyCode::Digit9);
    t[0x1A] = Some(KeyCode::Digit7);
    t[0x1B] = Some(KeyCode::Minus);
    t[0x1C] = Some(KeyCode::Digit8);
    t[0x1D] = Some(KeyCode::Digit0);
    t[0x1E] = Some(KeyCode::BracketRight);
    t[0x1F] = Some(KeyCode::KeyO);
    t[0x20] = Some(KeyCode::KeyU);
    t[0x21] = Some(KeyCode::BracketLeft);
    t[0x22] = Some(KeyCode::KeyI);
    t[0x23] = Some(KeyCode::KeyP);
    t[0x24] = Some(KeyCode::Enter);
    t[0x25] = Some(KeyCode::KeyL);
    t[0x26] = Some(KeyCode::KeyJ);
    t[0x27] = Some(KeyCode::Quote);
    t[0x28] = Some(KeyCode::KeyK);
    t[0x29] = Some(KeyCode::Semicolon);
    t[0x2A] = Some(KeyCode::Backslash);
    t[0x2B] = Some(KeyCode::Comma);
    t[0x2C] = Some(KeyCode::Slash);
    t[0x2D] = Some(KeyCode::KeyN);
    t[0x2E] = Some(KeyCode::KeyM);
    t[0x2F] = Some(KeyCode::Period);
    t[0x30] = Some(KeyCode::Tab);
    t[0x31] = Some(KeyCode::Space);
    t[0x32] = Some(KeyCode::Backquote);
    t[0x33] = Some(KeyCode::Backspace);
    t[0x34] = Some(KeyCode::NumpadEnter);
    t[0x35] = Some(KeyCode::Escape);
    t[0x36] = Some(KeyCode::MetaRight);
    t[0x37] = Some(KeyCode::MetaLeft);
    t[0x38] = Some(KeyCode::ShiftLeft);
    t[0x39] = Some(KeyCode::CapsLock);
    t[0x3A] = Some(KeyCode::AltLeft);
    t[0x3B] = Some(KeyCode::ControlLeft);
    t[0x3C] = Some(KeyCode::ShiftRight);
    t[0x3D] = Some(KeyCode::AltRight);
    t[0x3E] = Some(KeyCode::ControlRight);
    t[0x3F] = Some(KeyCode::Fn);
    t[0x40] = Some(KeyCode::F17);
    t[0x41] = Some(KeyCode::NumpadDecimal);
    t[0x43] = Some(KeyCode::NumpadMultiply);
    t[0x45] = Some(KeyCode::NumpadAdd);
    t[0x47] = Some(KeyCode::NumLock);
    t[0x48] = Some(KeyCode::AudioVolumeUp);
    t[0x49] = Some(KeyCode::AudioVolumeDown);
    t[0x4A] = Some(KeyCode::AudioVolumeMute);
    t[0x4B] = Some(KeyCode::NumpadDivide);
    t[0x4C] = Some(KeyCode::NumpadEnter);
    t[0x4E] = Some(KeyCode::NumpadSubtract);
    t[0x4F] = Some(KeyCode::F18);
    t[0x50] = Some(KeyCode::F19);
    t[0x51] = Some(KeyCode::NumpadEqual);
    t[0x52] = Some(KeyCode::Numpad0);
    t[0x53] = Some(KeyCode::Numpad1);
    t[0x54] = Some(KeyCode::Numpad2);
    t[0x55] = Some(KeyCode::Numpad3);
    t[0x56] = Some(KeyCode::Numpad4);
    t[0x57] = Some(KeyCode::Numpad5);
    t[0x58] = Some(KeyCode::Numpad6);
    t[0x59] = Some(KeyCode::Numpad7);
    t[0x5A] = Some(KeyCode::F20);
    t[0x5B] = Some(KeyCode::Numpad8);
    t[0x5C] = Some(KeyCode::Numpad9);
    t[0x5D] = Some(KeyCode::IntlYen);
    t[0x5E] = Some(KeyCode::IntlRo);
    t[0x5F] = Some(KeyCode::NumpadComma);
    t[0x60] = Some(KeyCode::F5);
    t[0x61] = Some(KeyCode::F6);
    t[0x62] = Some(KeyCode::F7);
    t[0x63] = Some(KeyCode::F3);
    t[0x64] = Some(KeyCode::F8);
    t[0x65] = Some(KeyCode::F9);
    t[0x66] = Some(KeyCode::Lang2);
    t[0x67] = Some(KeyCode::F11);
    t[0x68] = Some(KeyCode::Lang1);
    t[0x69] = Some(KeyCode::F13);
    t[0x6A] = Some(KeyCode::F16);
    t[0x6B] = Some(KeyCode::F14);
    t[0x6D] = Some(KeyCode::F10);
    t[0x6E] = Some(KeyCode::ContextMenu);
    t[0x6F] = Some(KeyCode::F12);
    t[0x71] = Some(KeyCode::F15);
    t[0x72] = Some(KeyCode::Insert);
    t[0x73] = Some(KeyCode::Home);
    t[0x74] = Some(KeyCode::PageUp);
    t[0x75] = Some(KeyCode::Delete);
    t[0x76] = Some(KeyCode::F4);
    t[0x77] = Some(KeyCode::End);
    t[0x78] = Some(KeyCode::F2);
    t[0x79] = Some(KeyCode::PageDown);
    t[0x7A] = Some(KeyCode::F1);
    t[0x7B] = Some(KeyCode::ArrowLeft);
    t[0x7C] = Some(KeyCode::ArrowRight);
    t[0x7D] = Some(KeyCode::ArrowDown);
    t[0x7E] = Some(KeyCode::ArrowUp);

    t
}

const CG_KEYCODE_TABLE: [Option<KeyCode>; 0x80] = build_cg_keycode_table();

#[cfg(target_os = "macos")]
type CFStringRef = *const c_void;

#[cfg(target_os = "macos")]
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> *mut c_void;
    fn TISGetInputSourceProperty(keyboard: *const c_void, property: CFStringRef) -> *mut CFData;
    fn UCKeyTranslate(
        keyLayoutPtr: *const u8,
        virtualKeyCode: u16,
        keyAction: u16,
        modifierKeyState: u32,
        keyboardType: u32,
        keyTranslateOptions: u32,
        deadKeyState: *mut u32,
        maxStringLength: usize,
        actualStringLength: *mut isize,
        unicodeString: *mut u16,
    ) -> i32;
    fn LMGetKbdType() -> u8;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
}

#[cfg(target_os = "macos")]
const VIRTUAL_KEYCODE_NUMS: &[u16] = &[
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F,
    0x20, 0x21, 0x22, 0x23, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F,
    0x32, // backquote
    // keypad subset
    0x41, 0x43, 0x45, 0x47, 0x4B, 0x4C, 0x4E, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
    0x5B, 0x5C,
];

#[cfg(target_os = "macos")]
fn generate_virtual_keymap() -> StdHashMap<String, KeyCode> {
    let mut keymap = StdHashMap::new();

    let keyboard = unsafe { TISCopyCurrentASCIICapableKeyboardLayoutInputSource() };
    if keyboard.is_null() {
        tracing::warn!("Could not get ASCII-capable keyboard layout input source");
        return keymap;
    }

    let layout_data = NonNull::new(unsafe {
        TISGetInputSourceProperty(keyboard, kTISPropertyUnicodeKeyLayoutData)
    });

    unsafe {
        super::skylight::CFRelease(keyboard.cast());
    }

    let Some(layout_data) = layout_data else {
        tracing::warn!("Could not get keyboard layout data");
        return keymap;
    };

    let layout_ptr = unsafe { CFData::byte_ptr(layout_data.as_ref()) };

    const K_UC_KEY_ACTION_DOWN: u16 = 0;
    const K_UC_NO_DEAD_KEYS: u32 = 1;

    let kbd_type: u32 = unsafe { LMGetKbdType() }.into();
    #[allow(unused_assignments)]
    let mut dead_key_state: u32 = 0;
    let mut chars = [0u16; 4];
    let mut actual_len: isize = 0;

    for &vk in VIRTUAL_KEYCODE_NUMS {
        let Some(key_code_enum) = cg_keycode_to_keycode(vk) else {
            continue;
        };

        dead_key_state = 0;
        let status = unsafe {
            UCKeyTranslate(
                layout_ptr,
                vk,
                K_UC_KEY_ACTION_DOWN,
                0, // no modifiers
                kbd_type,
                K_UC_NO_DEAD_KEYS,
                &mut dead_key_state,
                chars.len(),
                &mut actual_len,
                chars.as_mut_ptr(),
            )
        };

        if status == 0 && actual_len > 0 {
            let len = usize::try_from(actual_len).unwrap_or(0);
            if len == 0 {
                continue;
            }

            let s = String::from_utf16_lossy(&chars[..len]).to_lowercase();

            keymap.entry(s).or_insert(key_code_enum);
        }
    }

    keymap
}

pub static VIRTUAL_KEYMAP: LazyLock<StdHashMap<String, KeyCode>> =
    LazyLock::new(generate_virtual_keymap);

pub fn keycode_from_char(ch: &str) -> Option<KeyCode> {
    VIRTUAL_KEYMAP
        .get(&ch.to_lowercase())
        .copied()
        .or_else(|| fallback_keycode_from_char(ch))
}

fn fallback_keycode_from_char(ch: &str) -> Option<KeyCode> {
    let mut chars = ch.chars();
    let first = chars.next()?.to_ascii_lowercase();
    if chars.next().is_some() {
        return None;
    }

    use KeyCode::*;

    let code = match first {
        'a' => KeyA,
        'b' => KeyB,
        'c' => KeyC,
        'd' => KeyD,
        'e' => KeyE,
        'f' => KeyF,
        'g' => KeyG,
        'h' => KeyH,
        'i' => KeyI,
        'j' => KeyJ,
        'k' => KeyK,
        'l' => KeyL,
        'm' => KeyM,
        'n' => KeyN,
        'o' => KeyO,
        'p' => KeyP,
        'q' => KeyQ,
        'r' => KeyR,
        's' => KeyS,
        't' => KeyT,
        'u' => KeyU,
        'v' => KeyV,
        'w' => KeyW,
        'x' => KeyX,
        'y' => KeyY,
        'z' => KeyZ,
        '0' => Digit0,
        '1' => Digit1,
        '2' => Digit2,
        '3' => Digit3,
        '4' => Digit4,
        '5' => Digit5,
        '6' => Digit6,
        '7' => Digit7,
        '8' => Digit8,
        '9' => Digit9,
        _ => return None,
    };
    Some(code)
}

mod tests {
    #[allow(unused)]
    use super::*;

    #[test]
    fn test_virtual_keymap_generation() {
        let keymap = generate_virtual_keymap();
        assert!(!keymap.is_empty(), "Virtual keymap should not be empty");
        assert!(keymap.len() >= 10, "Expected at least 10 mapped characters");
    }

    #[test]
    fn test_keycode_from_char_basic() {
        let keymap = &*VIRTUAL_KEYMAP;
        if !keymap.is_empty() {
            let first_char = keymap.keys().next().unwrap();
            let result = keycode_from_char(first_char);
            assert!(result.is_some(), "Should find keycode for mapped character");
        }
    }

    #[test]
    fn test_fallback_keycode_from_char_basic() {
        assert_eq!(fallback_keycode_from_char("h"), Some(KeyCode::KeyH));
        assert_eq!(fallback_keycode_from_char("1"), Some(KeyCode::Digit1));
        assert_eq!(fallback_keycode_from_char("Z"), Some(KeyCode::KeyZ));
    }

    #[test]
    fn test_from_str_uses_virtual_keymap() {
        let result = KeyCode::from_str("h");
        assert!(result.is_ok(), "Should parse single character 'h'");
    }
}
