use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use block2::RcBlock;
use objc2::MainThreadOnly;
use objc2::rc::{Retained, Weak};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::*;
use objc2_foundation::{NSRectEdge, NSString, NSURL};

use crate::bridge::{DelegateBridge, Event, callback};
use crate::{CGPoint, CGRect, CGSize, NativeView, Ui, View};

pub struct Window {
    native: Retained<NSWindow>,
    content: RefCell<Option<Box<dyn NativeView>>>,
    delegate: Option<Retained<DelegateBridge>>,
    toolbar: Option<crate::Toolbar>,
}
impl Window {
    pub fn new(ui: &Ui) -> Self {
        let native = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(ui.mtm()),
                CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(800.0, 600.0)),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            native.setReleasedWhenClosed(false);
        }
        Self {
            native,
            content: RefCell::new(None),
            delegate: None,
            toolbar: None,
        }
    }

    pub fn ns_window(&self) -> &NSWindow { &self.native }

    pub fn title(self, title: &str) -> Self {
        self.native.setTitle(&NSString::from_str(title));
        self
    }

    pub fn size(self, size: CGSize) -> Self {
        self.native.setContentSize(size);
        self
    }

    pub fn min_size(self, size: CGSize) -> Self {
        self.native.setContentMinSize(size);
        self
    }

    pub fn content(self, content: impl NativeView) -> Self {
        self.set_content(content);
        self
    }

    pub fn set_content(&self, content: impl NativeView) {
        if let Some(controller) = content.view_controller() {
            self.native.setContentViewController(Some(controller));
        } else {
            self.native.setContentViewController(None);
            self.native.setContentView(Some(content.ns_view()));
        }
        if let Some(toolbar) = &self.toolbar {
            toolbar.attach(&self.native);
        }
        *self.content.borrow_mut() = Some(Box::new(content));
    }

    pub fn show(&self) { self.native.makeKeyAndOrderFront(None); }

    pub fn hide(&self) { self.native.orderOut(None); }

    pub fn close(&self) { self.native.close(); }

    pub fn center(&self) { self.native.center(); }

    pub fn make_key(&self) { self.native.makeKeyWindow(); }

    pub fn autosave_frame(self, name: &str) -> Self {
        self.native.setFrameAutosaveName(&NSString::from_str(name));
        self
    }

    pub fn on_close(mut self, mut f: impl FnMut() + 'static) -> Self {
        let ui = Ui::new(self.native.mtm());
        let bridge = DelegateBridge::new(&ui, move |event, _| {
            if event == Event::WindowClosed {
                f();
            }
        });
        bridge.for_window(&self.native);
        self.delegate = Some(bridge);
        self
    }

    pub fn appearance(&self, value: Appearance) {
        let name = match value {
            Appearance::System => None,
            Appearance::Light => Some(unsafe { NSAppearanceNameAqua }),
            Appearance::Dark => Some(unsafe { NSAppearanceNameDarkAqua }),
        };
        let appearance = name.and_then(NSAppearance::appearanceNamed);
        self.native.setAppearance(appearance.as_deref());
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        self.native.setDelegate(None);
        self.native.orderOut(None);
        self.native.setToolbar(None);
        self.native.setContentViewController(None);
        self.native.setContentView(None);
    }
}
#[derive(Clone, Copy, Default)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

pub struct SettingsWindow(Window);
impl SettingsWindow {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let mut window = Window::new(ui)
            .title(title)
            .min_size(CGSize::new(
                crate::Metrics::SIDEBAR_MIN_WIDTH + crate::Metrics::DETAIL_MIN_WIDTH,
                420.0,
            ))
            .autosave_frame(title);
        window.native.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::Automatic);
        window.native.setToolbarStyle(NSWindowToolbarStyle::Automatic);
        window.native.setTitleVisibility(NSWindowTitleVisibility::Visible);
        window
            .native
            .setStyleMask(window.native.styleMask() | NSWindowStyleMask::FullSizeContentView);
        let toolbar = crate::Toolbar::navigation(ui, "cgs.settings");
        toolbar.attach(&window.native);
        window.toolbar = Some(toolbar);
        Self(window)
    }

    pub fn content(self, content: impl NativeView) -> Self { Self(self.0.content(content)) }

    pub fn show(&self) { self.0.show(); }

    pub fn hide(&self) { self.0.hide(); }

    pub fn close(&self) { self.0.close(); }

    pub fn ns_window(&self) -> &NSWindow { self.0.ns_window() }

    pub fn on_close(self, f: impl FnMut() + 'static) -> Self { Self(self.0.on_close(f)) }
}
pub struct Sheet {
    window: Window,
    parent: RefCell<Weak<NSWindow>>,
}
impl Sheet {
    pub fn new(ui: &Ui, title: &str, content: impl NativeView) -> Self {
        Self {
            window: Window::new(ui).title(title).content(content),
            parent: RefCell::new(Weak::default()),
        }
    }

    /// Size a small editor from its Auto Layout content, rather than Window's default frame.
    pub fn fit_content(&self) {
        if let Some(content) = self.window.ns_window().contentView() {
            content.layoutSubtreeIfNeeded();
            self.window.ns_window().setContentSize(content.fittingSize());
        }
    }

    pub fn show(&self, parent: &NSWindow) {
        *self.parent.borrow_mut() = Weak::new(parent);
        parent.beginSheet_completionHandler(self.window.ns_window(), None);
    }

    pub fn end(&self) {
        if let Some(parent) = self.parent.borrow().load() {
            parent.endSheet(self.window.ns_window());
        }
    }

    pub fn ns_window(&self) -> &NSWindow { self.window.ns_window() }
}
impl Drop for Sheet {
    fn drop(&mut self) { self.end(); }
}

pub struct ViewController {
    native: Retained<NSViewController>,
    content: Box<dyn NativeView>,
}
impl ViewController {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        let native = NSViewController::new(ui.mtm());
        if let Some(child) = content.view_controller() {
            native.addChildViewController(child);
        }
        native.setView(content.ns_view());
        Self {
            native,
            content: Box::new(content),
        }
    }

    pub fn ns_view_controller(&self) -> &NSViewController { &self.native }
}
impl NativeView for ViewController {
    fn ns_view(&self) -> &NSView { self.content.ns_view() }

    fn view_controller(&self) -> Option<&NSViewController> { Some(&self.native) }
}

pub struct Popover {
    native: Retained<NSPopover>,
    controller: ViewController,
    delegate: Option<Retained<DelegateBridge>>,
}
impl Popover {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        let native = NSPopover::new(ui.mtm());
        native.setBehavior(NSPopoverBehavior::Transient);
        let controller = ViewController::new(ui, content);
        native.setContentViewController(Some(controller.ns_view_controller()));
        Self {
            native,
            controller,
            delegate: None,
        }
    }

    pub fn show(&self, view: &impl NativeView) {
        self.native.showRelativeToRect_ofView_preferredEdge(
            view.ns_view().bounds(),
            view.ns_view(),
            NSRectEdge::MaxY,
        );
    }

    pub fn close(&self) { self.native.close(); }

    pub fn on_close(mut self, mut f: impl FnMut() + 'static) -> Self {
        let bridge = DelegateBridge::new(&Ui::new(self.native.mtm()), move |e, _| {
            if e == Event::PopoverClosed {
                f();
            }
        });
        self.native.setDelegate(Some(ProtocolObject::from_ref(&*bridge)));
        self.delegate = Some(bridge);
        self
    }

    pub fn ns_popover(&self) -> &NSPopover { &self.native }

    pub fn ns_view_controller(&self) -> &NSViewController { self.controller.ns_view_controller() }
}
impl Drop for Popover {
    fn drop(&mut self) {
        self.native.setDelegate(None);
        self.native.close();
    }
}
pub struct HelpPopover(Popover);
impl HelpPopover {
    pub fn new(ui: &Ui, text: &str) -> Self {
        Self(Popover::new(
            ui,
            View::new(ui).content(crate::WrappingLabel::new(ui, text), crate::Insets {
                top: 12.0,
                left: 12.0,
                bottom: 12.0,
                right: 12.0,
            }),
        ))
    }

    pub fn show(&self, view: &impl NativeView) { self.0.show(view); }

    pub fn ns_popover(&self) -> &NSPopover { self.0.ns_popover() }
}

pub struct Alert(Retained<NSAlert>);
impl Alert {
    pub fn new(ui: &Ui, title: &str, message: &str) -> Self {
        let native = NSAlert::new(ui.mtm());
        native.setMessageText(&NSString::from_str(title));
        native.setInformativeText(&NSString::from_str(message));
        Self(native)
    }

    pub fn button(self, title: &str) -> Self {
        self.0.addButtonWithTitle(&NSString::from_str(title));
        self
    }

    pub fn show_sheet(&self, parent: &NSWindow, f: impl FnOnce(isize) + 'static) {
        let f = RefCell::new(Some(f));
        let completion = RcBlock::new(move |response| {
            if let Some(f) = f.borrow_mut().take() {
                callback(|| f(response));
            }
        });
        self.0.beginSheetModalForWindow_completionHandler(parent, Some(&completion));
    }

    pub fn ns_alert(&self) -> &NSAlert { &self.0 }
}
fn path_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}
fn url_path(url: Option<Retained<NSURL>>) -> Option<PathBuf> {
    url?.path().map(|s| PathBuf::from(s.to_string()))
}
macro_rules! panel {
    ($wrapper:ident,$native:ident,$ctor:ident,$accessor:ident) => {
        pub struct $wrapper(Retained<$native>);
        impl $wrapper {
            pub fn new(ui: &Ui) -> Self { Self($native::$ctor(ui.mtm())) }

            pub fn directory(self, path: &Path) -> Self {
                self.0.setDirectoryURL(Some(&path_url(path)));
                self
            }

            pub fn choose(&self) -> Option<PathBuf> {
                if self.0.runModal() == NSModalResponseOK {
                    url_path(self.0.URL())
                } else {
                    None
                }
            }

            pub fn show_sheet(&self, parent: &NSWindow, f: impl FnOnce(Option<PathBuf>) + 'static) {
                let f = RefCell::new(Some(f));
                let panel = Weak::new(&*self.0);
                let completion = RcBlock::new(move |response| {
                    if let Some(f) = f.borrow_mut().take() {
                        let path = if response == NSModalResponseOK {
                            panel.load().and_then(|panel| url_path(panel.URL()))
                        } else {
                            None
                        };
                        callback(|| f(path));
                    }
                });
                self.0.beginSheetModalForWindow_completionHandler(parent, &completion);
            }

            pub fn $accessor(&self) -> &$native { &self.0 }
        }
    };
}
panel!(OpenPanel, NSOpenPanel, openPanel, ns_open_panel);
panel!(SavePanel, NSSavePanel, savePanel, ns_save_panel);
impl OpenPanel {
    pub fn directories(self) -> Self {
        self.0.setCanChooseDirectories(true);
        self.0.setCanChooseFiles(false);
        self
    }

    pub fn files(self) -> Self {
        self.0.setCanChooseFiles(true);
        self.0.setCanChooseDirectories(false);
        self
    }
}
impl SavePanel {
    pub fn filename(self, name: &str) -> Self {
        self.0.setNameFieldStringValue(&NSString::from_str(name));
        self
    }
}

pub struct PathField {
    stack: crate::HStack,
    field: crate::TextField,
    choose: crate::Button,
    directories: Rc<Cell<bool>>,
}
impl PathField {
    pub fn new(ui: &Ui) -> Self {
        let field = crate::TextField::new(ui);
        let weak = Weak::new(field.ns_text_field());
        let ui_copy = *ui;
        let directories = Rc::new(Cell::new(false));
        let directory_mode = directories.clone();
        let choose = crate::Button::new(ui, "Choose…").on_click(move || {
            let panel = OpenPanel::new(&ui_copy);
            let panel = if directory_mode.get() {
                panel.directories()
            } else {
                panel
            };
            if let Some(path) = panel.choose() {
                if let Some(field) = weak.load() {
                    field.setStringValue(&NSString::from_str(&path.to_string_lossy()));
                }
            }
        });
        let stack = crate::HStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(field.ns_view());
        stack.ns_stack_view().addArrangedSubview(choose.ns_view());
        Self {
            stack,
            field,
            choose,
            directories,
        }
    }

    pub fn directories(self) -> Self {
        self.directories.set(true);
        self
    }

    pub fn set_value(&self, path: &Path) { self.field.set_value(&path.to_string_lossy()); }

    pub fn on_change(mut self, mut f: impl FnMut(PathBuf) + 'static) -> Self {
        use std::rc::Rc;
        let cb = Rc::new(RefCell::new(move |path| f(path)));
        let c = cb.clone();
        self.field = self.field.on_commit(move |s| (c.borrow_mut())(PathBuf::from(s)));
        let weak = Weak::new(self.field.ns_text_field());
        let ui = Ui::new(self.field.ns_text_field().mtm());
        let directories = self.directories.clone();
        self.choose = self.choose.on_click(move || {
            if let Some(field) = weak.load() {
                let cb = cb.clone();
                let weak = Weak::new(&*field);
                let apply = move |path: Option<PathBuf>| {
                    if let Some(path) = path {
                        if let Some(field) = weak.load() {
                            field.setStringValue(&NSString::from_str(&path.to_string_lossy()));
                        }
                        (cb.borrow_mut())(path);
                    }
                };
                let panel = OpenPanel::new(&ui);
                let panel = if directories.get() {
                    panel.directories()
                } else {
                    panel
                };
                if let Some(window) = field.window() {
                    panel.show_sheet(&window, apply);
                } else {
                    apply(panel.choose());
                }
            }
        });
        self
    }

    pub fn ns_text_field(&self) -> &NSTextField { self.field.ns_text_field() }
}
impl NativeView for PathField {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
