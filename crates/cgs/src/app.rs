use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSEventType, NSImage, NSMenu, NSMenuItem, NSWorkspace,
};
use objc2_foundation::NSString;

use crate::Ui;

/// A native image, such as an application icon or SF Symbol.
pub type Image = Retained<NSImage>;

pub struct Application(Retained<NSApplication>);
impl Application {
    pub fn shared(ui: &Ui) -> Self { Self(NSApplication::sharedApplication(ui.mtm())) }

    pub fn ns_application(&self) -> &NSApplication { &self.0 }

    pub fn activate(&self) { self.0.activate(); }

    pub fn hide(&self) { self.0.hide(None); }

    pub fn is_active(&self) -> bool { self.0.isActive() }

    pub fn icon(&self) -> Option<Image> { self.0.applicationIconImage() }

    /// The Finder icon of an installed application.
    pub fn icon_for_bundle(bundle_id: &str) -> Option<Image> {
        let workspace = NSWorkspace::sharedWorkspace();
        let url =
            workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
        Some(workspace.iconForFile(&*url.path()?))
    }

    /// Whether the event being handled is part of a left-button press or drag.
    pub fn is_mouse_dragging(&self) -> bool {
        self.0.currentEvent().is_some_and(|event| {
            matches!(
                event.r#type(),
                NSEventType::LeftMouseDragged | NSEventType::LeftMouseDown
            )
        })
    }

    /// Ensure File › Close Window (⌘W) exists; its nil target follows the key window's
    /// responder chain, so accessory apps' windows close like standard ones.
    pub fn install_close_window_command(&self) {
        let mtm = self.0.mtm();
        let action = objc2::sel!(performClose:);
        let main = self.0.mainMenu().unwrap_or_else(|| NSMenu::new(mtm));
        let installed = main.itemArray().iter().filter_map(|item| item.submenu()).any(|menu| {
            menu.itemArray().iter().any(|item| {
                item.keyEquivalent().to_string() == "w"
                    && item.keyEquivalentModifierMask() == NSEventModifierFlags::Command
                    && item.action() == Some(action)
                    && item.target().is_none()
            })
        });
        if installed {
            return;
        }
        let item = |title: &str, action, key: &str| unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                action,
                &NSString::from_str(key),
            )
        };
        let command = item("Close Window", Some(action), "w");
        command.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
        let file = main
            .itemArray()
            .iter()
            .find(|item| item.title().to_string() == "File")
            .and_then(|item| item.submenu())
            .unwrap_or_else(|| {
                let menu = NSMenu::new(mtm);
                let file = item("File", None, "");
                file.setSubmenu(Some(&menu));
                main.addItem(&file);
                menu
            });
        file.addItem(&command);
        self.0.setMainMenu(Some(&main));
    }
}
