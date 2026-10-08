use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadOnly, sel};
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::bridge::{ActionTarget, DelegateBridge, Event};
use crate::{CGPoint, NativeView, Ui};

pub struct MenuItem {
    native: Retained<NSMenuItem>,
    target: Retained<ActionTarget>,
    submenu: Option<Rc<Menu>>,
}
impl MenuItem {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let native = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(ui.mtm()),
                &NSString::from_str(title),
                Some(sel!(invoke:)),
                &NSString::from_str(""),
            )
        };
        let target = ActionTarget::new(ui);
        unsafe {
            native.setTarget(Some(&target));
        }
        Self { native, target, submenu: None }
    }

    pub fn on_activate(self, mut f: impl FnMut(&NSMenuItem) + 'static) -> Self {
        self.target.set(move |sender| {
            if let Some(item) = sender.downcast_ref::<NSMenuItem>() {
                f(item);
            }
        });
        self
    }

    pub fn on_click(self, mut f: impl FnMut() + 'static) -> Self {
        self.target.set(move |_| f());
        self
    }

    pub fn shortcut(self, key: &str, modifiers: NSEventModifierFlags) -> Self {
        self.native.setKeyEquivalent(&NSString::from_str(key));
        self.native.setKeyEquivalentModifierMask(modifiers);
        self
    }

    pub fn submenu(mut self, menu: impl Into<Rc<Menu>>) -> Self {
        let menu = menu.into();
        self.native.setSubmenu(Some(menu.ns_menu()));
        unsafe {
            self.native.setAction(None);
        }
        self.submenu = Some(menu);
        self
    }

    pub fn set_enabled(&self, value: bool) { self.native.setEnabled(value); }

    pub fn set_checked(&self, value: bool) {
        self.native.setState(if value {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }

    pub fn ns_menu_item(&self) -> &NSMenuItem { &self.native }
}
impl Drop for MenuItem {
    fn drop(&mut self) {
        unsafe {
            self.native.setTarget(None);
            self.native.setAction(None);
        }
    }
}

pub struct Menu {
    native: Retained<NSMenu>,
    items: RefCell<Vec<MenuItem>>,
    delegate: Option<Retained<DelegateBridge>>,
}
impl Menu {
    pub fn new(ui: &Ui) -> Self {
        let native = NSMenu::new(ui.mtm());
        native.setAutoenablesItems(false);
        unsafe {
            native.setFont(Some(&crate::Font::body()));
        }
        Self {
            native,
            items: RefCell::new(Vec::new()),
            delegate: None,
        }
    }

    pub fn item(self, item: MenuItem) -> Self {
        self.add(item);
        self
    }

    pub fn add(&self, item: MenuItem) {
        self.native.addItem(item.ns_menu_item());
        self.items.borrow_mut().push(item);
    }

    pub fn separator(self) -> Self {
        self.add_separator();
        self
    }

    pub fn add_separator(&self) { self.native.addItem(&NSMenuItem::separatorItem(self.native.mtm())); }

    pub fn remove(&self, item: &NSMenuItem) {
        self.native.removeItem(item);
        self.items.borrow_mut().retain(|i| !std::ptr::eq(i.ns_menu_item(), item));
    }

    pub fn clear(&self) {
        self.native.removeAllItems();
        self.items.borrow_mut().clear();
    }

    pub fn on_tracking(mut self, mut f: impl FnMut(bool) + 'static) -> Self {
        let bridge = DelegateBridge::new(&Ui::new(self.native.mtm()), move |e, _| match e {
            Event::MenuOpened => f(true),
            Event::MenuClosed => f(false),
            _ => {}
        });
        self.native.setDelegate(Some(ProtocolObject::from_ref(&*bridge)));
        self.delegate = Some(bridge);
        self
    }

    pub fn popup(&self, point: CGPoint, view: &impl NativeView) {
        self.native
            .popUpMenuPositioningItem_atLocation_inView(None, point, Some(view.ns_view()));
    }

    pub fn ns_menu(&self) -> &NSMenu { &self.native }
}
impl Drop for Menu {
    fn drop(&mut self) { self.native.setDelegate(None); }
}
pub type ContextMenu = Menu;
pub type StatusMenu = Menu;

pub struct StatusItem {
    ui: Ui,
    native: Retained<NSStatusItem>,
    menu: RefCell<Option<Menu>>,
    view: RefCell<Option<Box<dyn NativeView>>>,
}
impl StatusItem {
    pub fn new(ui: &Ui) -> Self {
        Self {
            ui: *ui,
            native: NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength),
            menu: RefCell::new(None),
            view: RefCell::new(None),
        }
    }

    pub fn set_menu(&self, menu: Menu) {
        self.native.setMenu(Some(menu.ns_menu()));
        *self.menu.borrow_mut() = Some(menu);
    }

    pub fn set_title(&self, title: &str) {
        if let Some(button) = self.native.button(self.ui.mtm()) {
            button.setTitle(&NSString::from_str(title));
        }
    }

    pub fn content(self, view: impl NativeView) -> Self {
        if let Some(button) = self.native.button(self.ui.mtm()) {
            button.addSubview(view.ns_view());
        }
        *self.view.borrow_mut() = Some(Box::new(view));
        self
    }

    pub fn set_visible(&self, value: bool) { self.native.setVisible(value); }

    pub fn ns_status_item(&self) -> &NSStatusItem { &self.native }
}
impl Drop for StatusItem {
    fn drop(&mut self) {
        self.native.setMenu(None);
        NSStatusBar::systemStatusBar().removeStatusItem(&self.native);
    }
}
