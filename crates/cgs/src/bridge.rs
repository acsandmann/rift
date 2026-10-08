use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControl, NSControlTextEditingDelegate, NSMenu, NSMenuDelegate, NSPopoverDelegate,
    NSSearchFieldDelegate, NSTextDelegate, NSTextFieldDelegate, NSTextViewDelegate, NSTokenFieldDelegate,
    NSWindow, NSWindowDelegate,
};
use objc2_foundation::{NSNotification, NSObject};

use crate::Ui;

pub(crate) fn callback(f: impl FnOnce()) {
    // Rust unwinding must never cross an Objective-C callback boundary.
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
        tracing::error!("panic in native UI callback");
    }
}
type Action = Box<dyn FnMut(&AnyObject)>;
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiActionTarget"]
    #[ivars = RefCell<Option<Action>>]
    pub(crate) struct ActionTarget;
    unsafe impl NSObjectProtocol for ActionTarget {}
    impl ActionTarget {
        #[unsafe(method(invoke:))]
        fn invoke(&self, sender: &AnyObject) {
            let _keep_alive = self.retain();
            if let Ok(mut cb) = self.ivars().try_borrow_mut() {
                if let Some(cb) = cb.as_mut() { callback(|| cb(sender)); }
            }
        }
    }
);
impl ActionTarget {
    pub(crate) fn new(ui: &Ui) -> Retained<Self> {
        let this = Self::alloc(ui.mtm()).set_ivars(RefCell::new(None));
        unsafe { msg_send![super(this), init] }
    }

    pub(crate) fn set(&self, f: impl FnMut(&AnyObject) + 'static) {
        *self.ivars().borrow_mut() = Some(Box::new(f));
    }

    pub(crate) fn attach(&self, control: &NSControl) {
        unsafe {
            control.setTarget(Some(self));
            control.setAction(Some(sel!(invoke:)));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    TextChanged,
    TextCommitted,
    MenuOpened,
    MenuClosed,
    WindowClosed,
    PopoverClosed,
}
type EventCallback = Box<dyn FnMut(Event, &AnyObject)>;
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiDelegateBridge"]
    #[ivars = RefCell<Option<EventCallback>>]
    pub(crate) struct DelegateBridge;
    unsafe impl NSObjectProtocol for DelegateBridge {}
    unsafe impl NSControlTextEditingDelegate for DelegateBridge {
        #[unsafe(method(controlTextDidChange:))]
        fn changed(&self, note: &NSNotification) { if let Some(sender) = note.object() { self.emit(Event::TextChanged, &sender); } }
        #[unsafe(method(controlTextDidEndEditing:))]
        fn committed(&self, note: &NSNotification) { if let Some(sender) = note.object() { self.emit(Event::TextCommitted, &sender); } }
    }
    unsafe impl NSTextDelegate for DelegateBridge {
        #[unsafe(method(textDidChange:))]
        fn text_changed(&self,note:&NSNotification){if let Some(sender)=note.object(){self.emit(Event::TextChanged,&sender);}}
    }
    unsafe impl NSTextViewDelegate for DelegateBridge {}
    unsafe impl NSTextFieldDelegate for DelegateBridge {}
    unsafe impl NSSearchFieldDelegate for DelegateBridge {}
    unsafe impl NSTokenFieldDelegate for DelegateBridge {}
    unsafe impl NSMenuDelegate for DelegateBridge {
        #[unsafe(method(menuWillOpen:))]
        fn menu_open(&self, menu: &NSMenu) { self.emit(Event::MenuOpened, menu); }
        #[unsafe(method(menuDidClose:))]
        fn menu_close(&self, menu: &NSMenu) { self.emit(Event::MenuClosed, menu); }
    }
    unsafe impl NSWindowDelegate for DelegateBridge {
        #[unsafe(method(windowWillClose:))]
        fn window_close(&self, note: &NSNotification) { if let Some(sender) = note.object() { self.emit(Event::WindowClosed, &sender); } }
    }
    unsafe impl NSPopoverDelegate for DelegateBridge {
        #[unsafe(method(popoverDidClose:))]
        fn popover_close(&self, note: &NSNotification) { if let Some(sender) = note.object() { self.emit(Event::PopoverClosed, &sender); } }
    }
);
impl DelegateBridge {
    pub(crate) fn new(ui: &Ui, f: impl FnMut(Event, &AnyObject) + 'static) -> Retained<Self> {
        let this = Self::alloc(ui.mtm()).set_ivars(RefCell::new(Some(Box::new(f))));
        unsafe { msg_send![super(this), init] }
    }

    fn emit(&self, event: Event, sender: &AnyObject) {
        let _keep_alive = self.retain();
        if let Ok(mut cb) = self.ivars().try_borrow_mut() {
            if let Some(cb) = cb.as_mut() {
                callback(|| cb(event, sender));
            }
        }
    }

    pub(crate) fn for_window(&self, window: &NSWindow) {
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
    }
}
