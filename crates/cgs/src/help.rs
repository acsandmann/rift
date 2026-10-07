use std::cell::{OnceCell, RefCell};

use objc2::rc::{Retained, Weak};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{NSRectEdge, NSString};

use crate::{Insets, NativeView, Popover, Symbol, Ui, View, WrappingLabel};

struct HelpState {
    text: String,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    popover: OnceCell<Popover>,
}
define_class!(
    #[unsafe(super(NSButton))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgSettingsInfoButton"]
    #[ivars = HelpState]
    struct HelpButton;
    impl HelpButton {
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking(&self) {
            unsafe { let _: () = msg_send![super(self), updateTrackingAreas]; }
            if let Some(old) = self.ivars().tracking.borrow_mut().take() { self.removeTrackingArea(&old); }
            let area = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(), crate::CGRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::ActiveInKeyWindow | NSTrackingAreaOptions::InVisibleRect,
                Some(self), None,
            ) };
            self.addTrackingArea(&area);
            *self.ivars().tracking.borrow_mut() = Some(area);
        }
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
            self.show_help();
        }
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.setContentTintColor(Some(&NSColor::tertiaryLabelColor()));
            self.close_help();
        }
        #[unsafe(method(viewWillMoveToWindow:))]
        fn move_to_window(&self, window: Option<&NSWindow>) {
            self.setContentTintColor(Some(&NSColor::tertiaryLabelColor()));
            self.close_help();
            unsafe { let _: () = msg_send![super(self), viewWillMoveToWindow: window]; }
        }
    }
);
impl HelpButton {
    fn close_help(&self) {
        if let Some(popover) = self.ivars().popover.get() {
            popover.close();
        }
    }

    fn show_help(&self) {
        if self.window().is_none() {
            return;
        }
        let popover = self.ivars().popover.get_or_init(|| {
            let ui = Ui::new(self.mtm());
            let label = WrappingLabel::new(&ui, &self.ivars().text);
            label.ns_text_field().setPreferredMaxLayoutWidth(260.0);
            label.width(260.0);
            let content = View::new(&ui).content(label, Insets {
                top: 12.0,
                left: 14.0,
                bottom: 12.0,
                right: 14.0,
            });
            let size = content.ns_view().fittingSize();
            let popover = Popover::new(&ui, content);
            popover
                .ns_popover()
                .setContentSize(crate::CGSize::new(288.0, size.height.max(44.0)));
            popover
        });
        if !popover.ns_popover().isShown() {
            popover.ns_popover().showRelativeToRect_ofView_preferredEdge(
                self.bounds(),
                self,
                NSRectEdge::MaxX,
            );
        }
    }
}

/// Small native info button with a lazily created hover/click description popover.
/// Tracking is active only in the key window; there are no timers or polling.
pub struct InfoButton {
    native: Retained<HelpButton>,
    _target: Retained<crate::bridge::ActionTarget>,
}
impl InfoButton {
    pub fn new(ui: &Ui, title: &str, text: &str) -> Self {
        let this = HelpButton::alloc(ui.mtm()).set_ivars(HelpState {
            text: text.to_owned(),
            tracking: RefCell::new(None),
            popover: OnceCell::new(),
        });
        let native: Retained<HelpButton> = unsafe { msg_send![super(this), init] };
        native.setTitle(&NSString::from_str(""));
        native.setBordered(false);
        native.setButtonType(NSButtonType::MomentaryPushIn);
        native.setImagePosition(NSCellImagePosition::ImageOnly);
        native.setImage(Symbol::named("info.circle.fill").as_deref());
        native.setContentTintColor(Some(&NSColor::tertiaryLabelColor()));
        native.setAccessibilityLabel(Some(&NSString::from_str(&format!("About {title}"))));
        native.setAccessibilityHelp(Some(&NSString::from_str(text)));
        let target = crate::bridge::ActionTarget::new(ui);
        target.attach(&*native);
        let weak = Weak::new(&*native);
        target.set(move |_| {
            if let Some(button) = weak.load() {
                button.show_help();
            }
        });
        let this = Self { native, _target: target };
        this.width(16.0);
        this.height(16.0);
        this
    }
}
impl NativeView for InfoButton {
    fn ns_view(&self) -> &NSView { &self.native }
}
impl Drop for InfoButton {
    fn drop(&mut self) { self.native.close_help(); }
}
