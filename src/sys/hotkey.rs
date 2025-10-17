use std::fmt;
use std::str::FromStr;

use anyhow::anyhow;
use objc2_core_graphics::{CGEvent, CGEventField, CGEventFlags};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const ALT: Modifiers = Modifiers(0b0100);
    pub const CONTROL: Modifiers = Modifiers(0b0010);
    pub const META: Modifiers = Modifiers(0b1000);
    pub const SHIFT: Modifiers = Modifiers(0b0001);

    pub fn empty() -> Self { Modifiers(0) }

    pub fn contains(&self, other: Modifiers) -> bool { (self.0 & other.0) == other.0 }

    pub fn insert(&mut self, other: Modifiers) { self.0 |= other.0; }

    pub fn remove(&mut self, other: Modifiers) { self.0 &= !other.0; }

    pub fn insert_from_token(&mut self, token: &str) -> bool {
        match token.to_lowercase().as_str() {
            "alt" | "option" => {
                self.insert(Modifiers::ALT);
                true
            }
            "ctrl" | "control" => {
                self.insert(Modifiers::CONTROL);
                true
            }
            "shift" => {
                self.insert(Modifiers::SHIFT);
                true
            }
            "meta" | "cmd" | "command" => {
                self.insert(Modifiers::META);
                true
            }
            _ => false,
        }
    }
}

impl fmt::Display for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<&str> = Vec::new();
        if self.contains(Modifiers::CONTROL) {
            parts.push("Ctrl");
        }
        if self.contains(Modifiers::ALT) {
            parts.push("Alt");
        }
        if self.contains(Modifiers::SHIFT) {
            parts.push("Shift");
        }
        if self.contains(Modifiers::META) {
            parts.push("Meta");
        }
        write!(f, "{}", parts.join(" + "))
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

impl FromStr for KeyCode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use KeyCode::*;
        match s.to_uppercase().as_str() {
            "A" => Ok(KeyA),
            "B" => Ok(KeyB),
            "C" => Ok(KeyC),
            "D" => Ok(KeyD),
            "E" => Ok(KeyE),
            "F" => Ok(KeyF),
            "G" => Ok(KeyG),
            "H" => Ok(KeyH),
            "I" => Ok(KeyI),
            "J" => Ok(KeyJ),
            "K" => Ok(KeyK),
            "L" => Ok(KeyL),
            "M" => Ok(KeyM),
            "N" => Ok(KeyN),
            "O" => Ok(KeyO),
            "P" => Ok(KeyP),
            "Q" => Ok(KeyQ),
            "R" => Ok(KeyR),
            "S" => Ok(KeyS),
            "T" => Ok(KeyT),
            "U" => Ok(KeyU),
            "V" => Ok(KeyV),
            "W" => Ok(KeyW),
            "X" => Ok(KeyX),
            "Y" => Ok(KeyY),
            "Z" => Ok(KeyZ),
            "FN" => Ok(Fn),
            "LEFT" | "ARROWLEFT" => Ok(ArrowLeft),
            "RIGHT" | "ARROWRIGHT" => Ok(ArrowRight),
            "UP" | "ARROWUP" => Ok(ArrowUp),
            "DOWN" | "ARROWDOWN" => Ok(ArrowDown),
            "TAB" => Ok(Tab),
            "SPACE" => Ok(Space),
            "ENTER" | "RETURN" => Ok(Enter),
            "ESC" | "ESCAPE" => Ok(Escape),
            "0" => Ok(Digit0),
            "1" => Ok(Digit1),
            "2" => Ok(Digit2),
            "3" => Ok(Digit3),
            "4" => Ok(Digit4),
            "5" => Ok(Digit5),
            "6" => Ok(Digit6),
            "7" => Ok(Digit7),
            "8" => Ok(Digit8),
            "9" => Ok(Digit9),
            "-" => Ok(Minus),
            "MINUS" | "HYPHEN" => Ok(Minus),
            "=" => Ok(Equal),
            "EQUAL" | "EQUALS" => Ok(Equal),
            "," => Ok(Comma),
            "COMMA" => Ok(Comma),
            "." => Ok(Period),
            "DOT" | "PERIOD" => Ok(Period),
            "/" => Ok(Slash),
            "SLASH" | "FORWARD_SLASH" => Ok(Slash),
            ";" => Ok(Semicolon),
            "SEMICOLON" => Ok(Semicolon),
            "'" => Ok(Quote),
            "QUOTE" | "APOSTROPHE" => Ok(Quote),
            "`" => Ok(Backquote),
            "BACKQUOTE" | "GRAVE" | "TILDE" => Ok(Backquote),
            "\\" => Ok(Backslash),
            "BACKSLASH" => Ok(Backslash),
            _ => match s.to_lowercase().as_str() {
                "left" => Ok(ArrowLeft),
                "right" => Ok(ArrowRight),
                "up" => Ok(ArrowUp),
                "down" => Ok(ArrowDown),
                "space" => Ok(Space),
                "tab" => Ok(Tab),
                other => Err(anyhow!("Unrecognized key token: {}", other)),
            },
        }
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

pub fn modifiers_from_flags(flags: CGEventFlags) -> Modifiers {
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

pub fn modifier_flag_for_key(key_code: KeyCode) -> Option<CGEventFlags> {
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

pub fn is_modifier_key(key_code: KeyCode) -> bool { modifier_flag_for_key(key_code).is_some() }

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
    if mods.contains(Modifiers::CONTROL) {
        Some(KeyCode::ControlLeft)
    } else if mods.contains(Modifiers::ALT) {
        Some(KeyCode::AltLeft)
    } else if mods.contains(Modifiers::META) {
        Some(KeyCode::MetaLeft)
    } else if mods.contains(Modifiers::SHIFT) {
        Some(KeyCode::ShiftLeft)
    } else {
        None
    }
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

pub fn key_code_from_event(event: &CGEvent) -> Option<KeyCode> {
    let raw = CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventKeycode);
    if raw < 0 {
        return None;
    }
    cg_keycode_to_keycode(raw as u16)
}

pub fn cg_keycode_to_keycode(code: u16) -> Option<KeyCode> {
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
