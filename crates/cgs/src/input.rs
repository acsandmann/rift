use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::bridge::{DelegateBridge, Event, callback};
use crate::{Label, NativeControl, NativeView, Ui, Validation};

type TextCallback = Box<dyn FnMut(String)>;
type TextCallbacks = Rc<RefCell<[Option<TextCallback>; 2]>>;

macro_rules! text_input {
    ($name:ident,$native:ident,$accessor:ident) => {
        pub struct $name {
            native: Retained<$native>,
            delegate: Option<Retained<DelegateBridge>>,
            callbacks: TextCallbacks,
        }
        impl $name {
            pub fn new(ui: &Ui) -> Self {
                let native = $native::new(ui.mtm());
                native.setFont(Some(&crate::Font::body()));
                Self {
                    native,
                    delegate: None,
                    callbacks: Rc::new(RefCell::new([None, None])),
                }
            }

            pub fn value(self, value: &str) -> Self {
                self.set_value(value);
                self
            }

            pub fn set_value(&self, value: &str) {
                self.native.setStringValue(&NSString::from_str(value));
            }

            pub fn get_value(&self) -> String { self.native.stringValue().to_string() }

            pub fn placeholder(self, text: &str) -> Self {
                self.native.setPlaceholderString(Some(&NSString::from_str(text)));
                self
            }

            pub fn focus(&self) {
                if let Some(window) = self.native.window() {
                    window.makeFirstResponder(Some(&*self.native));
                }
            }

            fn listen(mut self, event: Event, f: impl FnMut(String) + 'static) -> Self {
                let index = usize::from(event == Event::TextCommitted);
                self.callbacks.borrow_mut()[index] = Some(Box::new(f));
                if self.delegate.is_none() {
                    let callbacks = self.callbacks.clone();
                    let bridge =
                        DelegateBridge::new(&Ui::new(self.native.mtm()), move |event, sender| {
                            let index = match event {
                                Event::TextChanged => 0,
                                Event::TextCommitted => 1,
                                _ => return,
                            };
                            if let Some(control) = sender.downcast_ref::<$native>() {
                                if let Ok(mut callbacks) = callbacks.try_borrow_mut() {
                                    if let Some(f) = callbacks[index].as_mut() {
                                        f(control.stringValue().to_string());
                                    }
                                }
                            }
                        });
                    unsafe {
                        self.native.setDelegate(Some(ProtocolObject::from_ref(&*bridge)));
                    }
                    self.delegate = Some(bridge);
                }
                self
            }

            pub fn on_change(self, f: impl FnMut(String) + 'static) -> Self {
                self.listen(Event::TextChanged, f)
            }

            pub fn on_commit(self, f: impl FnMut(String) + 'static) -> Self {
                self.listen(Event::TextCommitted, f)
            }

            pub fn set_validation(&self, validation: &Validation) {
                self.native
                    .setToolTip(validation.message().map(|s| NSString::from_str(s)).as_deref());
            }
        }
        crate::control::native_control!($name, $native, $accessor);
        impl Drop for $name {
            fn drop(&mut self) {
                unsafe {
                    self.native.setDelegate(None);
                }
            }
        }
    };
}
text_input!(TextField, NSTextField, ns_text_field);
impl TextField {
    pub fn with_validation(self, ui: &Ui) -> crate::Validated<Self> {
        crate::Validated::new(ui, self)
    }
}
text_input!(SearchField, NSSearchField, ns_search_field);
text_input!(TokenField, NSTokenField, ns_token_field);

pub struct TextArea {
    scroll: crate::ScrollView,
    native: Retained<NSTextView>,
    delegate: Option<Retained<DelegateBridge>>,
}
impl TextArea {
    pub fn new(ui: &Ui) -> Self {
        let native = NSTextView::new(ui.mtm());
        native.setRichText(false);
        native.setFont(Some(&crate::Font::body()));
        native.setVerticallyResizable(true);
        let view: Retained<NSView> = native.clone().into_super().into_super();
        let scroll = crate::ScrollView::new(ui, view);
        Self { scroll, native, delegate: None }
    }

    pub fn set_value(&self, text: &str) { self.native.setString(&NSString::from_str(text)); }

    pub fn get_value(&self) -> String { self.native.string().to_string() }

    pub fn focus(&self) {
        if let Some(w) = self.native.window() {
            w.makeFirstResponder(Some(&*self.native));
        }
    }

    pub fn on_change(mut self, mut f: impl FnMut(String) + 'static) -> Self {
        let bridge = DelegateBridge::new(&Ui::new(self.native.mtm()), move |event, sender| {
            if event == Event::TextChanged {
                if let Some(view) = sender.downcast_ref::<NSTextView>() {
                    f(view.string().to_string());
                }
            }
        });
        self.native.setDelegate(Some(ProtocolObject::from_ref(&*bridge)));
        self.delegate = Some(bridge);
        self
    }

    pub fn ns_text_view(&self) -> &NSTextView { &self.native }
}
impl NativeView for TextArea {
    fn ns_view(&self) -> &NSView { self.scroll.ns_view() }
}

impl Drop for TextArea {
    fn drop(&mut self) { self.native.setDelegate(None); }
}

pub type Modifiers = NSEventModifierFlags;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub code: u16,
    pub display: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyShortcut {
    pub modifiers: Modifiers,
    pub key: Key,
}
fn shortcut_modifiers(flags: Modifiers) -> Modifiers {
    flags & (Modifiers::Control | Modifiers::Option | Modifiers::Shift | Modifiers::Command)
}
pub fn modifier_glyphs(flags: Modifiers) -> String {
    let mut out = String::new();
    for (flag, glyph) in [
        (Modifiers::Control, '⌃'),
        (Modifiers::Option, '⌥'),
        (Modifiers::Shift, '⇧'),
        (Modifiers::Command, '⌘'),
    ] {
        if flags.contains(flag) {
            out.push(glyph);
        }
    }
    out
}
impl std::fmt::Display for KeyShortcut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", modifier_glyphs(self.modifiers), self.key.display)
    }
}
fn key_display(event: &NSEvent) -> String {
    match event.keyCode() {
        122 => "F1".into(),
        120 => "F2".into(),
        99 => "F3".into(),
        118 => "F4".into(),
        96 => "F5".into(),
        97 => "F6".into(),
        98 => "F7".into(),
        100 => "F8".into(),
        101 => "F9".into(),
        109 => "F10".into(),
        103 => "F11".into(),
        111 => "F12".into(),
        105 => "F13".into(),
        107 => "F14".into(),
        113 => "F15".into(),
        106 => "F16".into(),
        64 => "F17".into(),
        79 => "F18".into(),
        80 => "F19".into(),
        90 => "F20".into(),
        115 => "↖".into(),
        119 => "↘".into(),
        116 => "⇞".into(),
        121 => "⇟".into(),
        36 => "↩".into(),
        48 => "⇥".into(),
        49 => "Space".into(),
        51 => "⌫".into(),
        53 => "⎋".into(),
        76 => "⌤".into(),
        117 => "⌦".into(),
        123 => "←".into(),
        124 => "→".into(),
        125 => "↓".into(),
        126 => "↑".into(),
        _ => event
            .charactersIgnoringModifiers()
            .map(|v| v.to_string().to_uppercase())
            .unwrap_or_default(),
    }
}
type ShortcutCallback = Box<dyn FnMut(Option<KeyShortcut>)>;
type ConflictCallback = Box<dyn FnMut(&KeyShortcut) -> Option<String>>;
struct RecorderState {
    recording: Cell<bool>,
    modifiers_only: bool,
    modifiers: Cell<Modifiers>,
    value: RefCell<Option<KeyShortcut>>,
    changed: RefCell<Option<ShortcutCallback>>,
    conflict: RefCell<Option<ConflictCallback>>,
}
define_class!(
    #[unsafe(super(NSButton))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiKeyRecorder"]
    #[ivars = RecorderState]
    struct RecorderButton;
    impl RecorderButton {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self)->bool{true}
        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self)->bool{self.cancel();unsafe{msg_send![super(self),resignFirstResponder]}}
        #[unsafe(method(beginRecording:))]
        fn begin_action(&self,_sender:&AnyObject){self.begin();}
        #[unsafe(method(performKeyEquivalent:))]
        fn key_equivalent(&self,event:&NSEvent)->bool{
            if self.ivars().recording.get(){self.record(event);true}else{unsafe{msg_send![super(self),performKeyEquivalent:event]}}
        }
        #[unsafe(method(keyDown:))]
        fn key_down(&self,event:&NSEvent){
            if self.ivars().recording.get(){self.record(event);}else{unsafe{let _:()=msg_send![super(self),keyDown:event];}}
        }
        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self,event:&NSEvent){
            if self.ivars().recording.get() && self.ivars().modifiers_only {
                let flags=shortcut_modifiers(event.modifierFlags());
                if !flags.is_empty(){self.ivars().modifiers.set(flags);self.setTitle(&NSString::from_str(&modifier_glyphs(flags)));}
            }else{unsafe{let _:()=msg_send![super(self),flagsChanged:event];}}
        }
    }
);
impl RecorderButton {
    fn record(&self, event: &NSEvent) {
        match event.keyCode() {
            53 => self.cancel(),
            51 | 117 => self.accept(None),
            _ => {
                let modifiers = if self.ivars().modifiers_only {
                    self.ivars().modifiers.get()
                } else {
                    shortcut_modifiers(event.modifierFlags())
                };
                let key = if self.ivars().modifiers_only {
                    if event.keyCode() != 36 {
                        return;
                    }
                    Key {
                        code: 0,
                        display: String::new(),
                    }
                } else {
                    Key {
                        code: event.keyCode(),
                        display: key_display(event),
                    }
                };
                self.accept(Some(KeyShortcut { modifiers, key }));
            }
        }
    }

    fn new(ui: &Ui, modifiers_only: bool) -> Retained<Self> {
        let this = Self::alloc(ui.mtm()).set_ivars(RecorderState {
            recording: Cell::new(false),
            modifiers_only,
            modifiers: Cell::new(Modifiers::empty()),
            value: RefCell::new(None),
            changed: RefCell::new(None),
            conflict: RefCell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setBezelStyle(crate::control::action_button_bezel());
        unsafe {
            this.setTarget(Some(&this));
            this.setAction(Some(sel!(beginRecording:)));
        }
        this.setAccessibilityLabel(Some(&NSString::from_str(if modifiers_only {
            "Record modifiers"
        } else {
            "Record shortcut"
        })));
        this.refresh();
        this
    }

    fn begin(&self) {
        self.ivars().recording.set(true);
        self.ivars().modifiers.set(Modifiers::empty());
        self.setTitle(&NSString::from_str(if self.ivars().modifiers_only {
            "Press modifiers, then Return"
        } else {
            "Type shortcut…"
        }));
        if let Some(w) = self.window() {
            w.makeFirstResponder(Some(self));
        }
    }

    fn cancel(&self) {
        self.ivars().recording.set(false);
        self.setToolTip(None);
        self.setAccessibilityHelp(None);
        self.refresh();
    }

    fn refresh(&self) {
        let value = self
            .ivars()
            .value
            .borrow()
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "Record shortcut".into());
        self.setTitle(&NSString::from_str(&value));
        unsafe {
            self.setAccessibilityValue(Some(&NSString::from_str(&value)));
        }
    }

    fn accept(&self, value: Option<KeyShortcut>) {
        let _keep_alive = self.retain();
        if let Some(shortcut) = &value {
            let mut conflict = None;
            if let Ok(mut f) = self.ivars().conflict.try_borrow_mut() {
                if let Some(f) = f.as_mut() {
                    callback(|| conflict = f(shortcut));
                }
            }
            if let Some(message) = conflict {
                self.setToolTip(Some(&NSString::from_str(&message)));
                self.setAccessibilityHelp(Some(&NSString::from_str(&message)));
                return;
            }
        }
        *self.ivars().value.borrow_mut() = value.clone();
        self.cancel();
        if let Ok(mut f) = self.ivars().changed.try_borrow_mut() {
            if let Some(f) = f.as_mut() {
                callback(|| f(value));
            }
        }
    }
}
pub struct KeyRecorder {
    native: Retained<RecorderButton>,
}
impl KeyRecorder {
    pub fn new(ui: &Ui) -> Self {
        Self {
            native: RecorderButton::new(ui, false),
        }
    }

    pub fn set_value(&self, value: Option<KeyShortcut>) {
        *self.native.ivars().value.borrow_mut() = value;
        self.native.refresh();
    }

    pub fn get_value(&self) -> Option<KeyShortcut> { self.native.ivars().value.borrow().clone() }

    pub fn begin_recording(&self) { self.native.begin(); }

    pub fn cancel(&self) { self.native.cancel(); }

    pub fn on_change(self, f: impl FnMut(Option<KeyShortcut>) + 'static) -> Self {
        *self.native.ivars().changed.borrow_mut() = Some(Box::new(f));
        self
    }

    pub fn conflict(self, f: impl FnMut(&KeyShortcut) -> Option<String> + 'static) -> Self {
        *self.native.ivars().conflict.borrow_mut() = Some(Box::new(f));
        self
    }

    pub fn ns_button(&self) -> &NSButton { &self.native }
}
impl NativeView for KeyRecorder {
    fn ns_view(&self) -> &NSView { &self.native }
}
impl NativeControl for KeyRecorder {
    fn ns_control(&self) -> &NSControl { &self.native }
}
pub struct ModifierRecorder(KeyRecorder);
impl ModifierRecorder {
    pub fn new(ui: &Ui) -> Self {
        Self(KeyRecorder {
            native: RecorderButton::new(ui, true),
        })
    }

    pub fn set_value(&self, value: Modifiers) {
        self.0.set_value(Some(KeyShortcut {
            modifiers: shortcut_modifiers(value),
            key: Key {
                code: 0,
                display: String::new(),
            },
        }));
    }

    pub fn on_change(self, mut f: impl FnMut(Modifiers) + 'static) -> Self {
        Self(
            self.0
                .on_change(move |s| f(s.map(|s| s.modifiers).unwrap_or_else(Modifiers::empty))),
        )
    }

    pub fn ns_button(&self) -> &NSButton { self.0.ns_button() }
}
impl NativeView for ModifierRecorder {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
impl NativeControl for ModifierRecorder {
    fn ns_control(&self) -> &NSControl { self.0.ns_control() }
}

pub struct ShortcutLabel(Label);
impl ShortcutLabel {
    pub fn new(ui: &Ui, value: &KeyShortcut) -> Self { Self(Label::new(ui, &value.to_string())) }

    pub fn set_value(&self, value: &KeyShortcut) { self.0.set_text(&value.to_string()); }

    pub fn ns_text_field(&self) -> &NSTextField { self.0.ns_text_field() }
}
impl NativeView for ShortcutLabel {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
