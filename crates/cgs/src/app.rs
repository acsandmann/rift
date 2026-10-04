use objc2::rc::Retained;
use objc2_app_kit::NSApplication;

use crate::Ui;
pub struct Application(Retained<NSApplication>);
impl Application {
    pub fn shared(ui: &Ui) -> Self { Self(NSApplication::sharedApplication(ui.mtm())) }

    pub fn ns_application(&self) -> &NSApplication { &self.0 }

    pub fn activate(&self) { self.0.activate(); }

    pub fn hide(&self) { self.0.hide(None); }

    pub fn is_active(&self) -> bool { self.0.isActive() }
}
