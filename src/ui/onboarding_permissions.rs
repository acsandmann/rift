//! File-URL drag source; the installed signed bundle retains its identity.
use cgs::layout::{VStack, uniform_insets};
use cgs::text::WrappingLabel;
use cgs::view::NativeView;
use cgs::window::Window;
use cgs::{CGPoint, CGSize, Ui};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{NSArray, NSObjectProtocol, NSString, NSURL};

define_class!(
    #[unsafe(super(NSImageView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RiftSetupDragIcon"]
    #[ivars = Retained<NSURL>]
    struct DragIcon;
    unsafe impl NSObjectProtocol for DragIcon {}
    unsafe impl NSDraggingSource for DragIcon {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation(&self, _session: &NSDraggingSession, _context: NSDraggingContext) -> NSDragOperation {
            NSDragOperation::Copy
        }
    }
    impl DragIcon {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), ProtocolObject::from_ref(&**self.ivars()));
            unsafe { item.setDraggingFrame_contents(self.bounds(), self.image().as_deref().map(|i| &***i)); }
            self.beginDraggingSessionWithItems_event_source(&NSArray::from_slice(&[&*item]), event, ProtocolObject::from_ref(self));
        }
    }
);
struct Icon(Retained<DragIcon>);
impl NativeView for Icon {
    fn ns_view(&self) -> &NSView { &self.0 }
}

pub fn helper(ui: &Ui) -> Option<Window> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let bundle = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app"))?;
    let path = NSString::from_str(bundle.to_str()?);
    let url = NSURL::fileURLWithPath_isDirectory(&path, true);
    let icon: Retained<DragIcon> =
        unsafe { msg_send![super(DragIcon::alloc(ui.mtm()).set_ivars(url)), init] };
    icon.setImage(Some(&NSWorkspace::sharedWorkspace().iconForFile(&path)));
    icon.setAccessibilityLabel(Some(&NSString::from_str(
        "Drag Rift into the Accessibility application list",
    )));
    icon.heightAnchor().constraintEqualToConstant(64.).setActive(true);
    let panel = Window::new(ui).title("Allow Rift").size(CGSize::new(240., 180.)).content(
        VStack::new(ui).insets(uniform_insets(16.)).spacing(12.).push(Icon(icon)).push(
            WrappingLabel::new(ui, "Drag Rift into the Accessibility list, then turn it on."),
        ),
    );
    panel.ns_window().setLevel(3);
    if let Some(screen) = NSScreen::mainScreen(ui.mtm()) {
        let frame = screen.visibleFrame();
        panel
            .ns_window()
            .setFrameOrigin(CGPoint::new(frame.origin.x + 24., frame.origin.y + 48.));
    }
    Some(panel)
}
