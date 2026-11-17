//! Screen-filling introduction, separate from the normal Setup Assistant window.
use cgs::Ui;
use cgs::controls::Button;
use cgs::view::NativeView;
use objc2::rc::Retained;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationPresentationOptions, NSBackingStoreType, NSColor, NSEvent,
    NSEventMask, NSScreen, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindow, NSWindowStyleMask, NSWorkspace,
};
use tokio::sync::mpsc::unbounded_channel;

define_class!(
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "RiftWelcomeWindow"]
    struct IntroWindow;
    impl IntroWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool { true }
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { true }
    }
);

pub(super) struct Introduction {
    window: Retained<IntroWindow>,
    previous: NSApplicationPresentationOptions,
}

impl Introduction {
    pub(super) async fn dismiss(self, app: &NSApplication) {
        if !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() {
            let (send, mut receive) = unbounded_channel();
            let window = self.window.clone();
            let changes = block2::RcBlock::new(
                move |context: std::ptr::NonNull<objc2_app_kit::NSAnimationContext>| unsafe {
                    context.as_ref().setDuration(0.3);
                    let animator: Retained<objc2::runtime::AnyObject> =
                        msg_send![&*window, animator];
                    let _: () = msg_send![&*animator, setAlphaValue: 0f64];
                },
            );
            let complete = block2::RcBlock::new(move || {
                let _ = send.send(());
            });
            objc2_app_kit::NSAnimationContext::runAnimationGroup_completionHandler(
                &changes,
                Some(&complete),
            );
            let _ = receive.recv().await;
        }
        self.window.orderOut(None);
        app.setPresentationOptions(self.previous);
        self.window.close();
    }
}

pub(super) async fn present(ui: &Ui, app: &NSApplication) -> Option<Introduction> {
    let Some(screen) = NSScreen::mainScreen(ui.mtm()) else {
        return None;
    };
    let frame = screen.frame(); // Includes the menu bar and Dock areas.
    let window: Retained<IntroWindow> = unsafe {
        msg_send![IntroWindow::alloc(ui.mtm()),
            initWithContentRect: frame,
            styleMask: NSWindowStyleMask::Borderless,
            backing: NSBackingStoreType::Buffered,
            defer: false]
    };
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.setHasShadow(false);
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setMovable(false);
    window.setLevel(3);
    // AppKit composites a live backdrop; this needs no screenshot or recording permission.
    let backdrop = NSVisualEffectView::new(ui.mtm());
    backdrop.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
    backdrop.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    backdrop.setState(NSVisualEffectState::Active);
    let canvas = &*backdrop;
    let scale = (frame.size.width * 0.36 / 205.)
        .clamp(1., 3.2)
        .min(frame.size.height * 0.65 / 200.);
    let mark = super::onboarding_wordmark::create_scaled(ui, true, scale);
    canvas.addSubview(mark.ns_view());
    mark.ns_view().setTranslatesAutoresizingMaskIntoConstraints(false);
    mark.ns_view()
        .centerXAnchor()
        .constraintEqualToAnchor(&canvas.centerXAnchor())
        .setActive(true);
    mark.ns_view()
        .centerYAnchor()
        .constraintEqualToAnchor(&canvas.centerYAnchor())
        .setActive(true);
    mark.ns_view()
        .widthAnchor()
        .constraintEqualToConstant(410. * scale)
        .setActive(true);
    let (send, mut receive) = unbounded_channel();
    let skip = send.clone();
    let button = Button::new(ui, "Click to skip").borderless().key_equivalent("\u{1b}").on_click(
        move || {
            let _ = skip.send(());
        },
    );
    canvas.addSubview(button.ns_view());
    button.ns_view().setTranslatesAutoresizingMaskIntoConstraints(false);
    button
        .ns_view()
        .trailingAnchor()
        .constraintEqualToAnchor_constant(&canvas.trailingAnchor(), -36.)
        .setActive(true);
    button
        .ns_view()
        .bottomAnchor()
        .constraintEqualToAnchor_constant(&canvas.bottomAnchor(), -32.)
        .setActive(true);
    window.setContentView(Some(canvas));
    let previous = app.presentationOptions();
    app.setPresentationOptions(
        NSApplicationPresentationOptions::HideDock | NSApplicationPresentationOptions::HideMenuBar,
    );
    let clicked = send.clone();
    let click = block2::RcBlock::new(move |_: std::ptr::NonNull<NSEvent>| {
        let _ = clicked.send(());
        std::ptr::null_mut() // Consume the skip click so it cannot activate setup controls.
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown,
            &click,
        )
    };
    window.makeKeyAndOrderFront(None);
    app.activate();
    let reduced = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(if reduced { 0 } else { 1850 }));
        let _ = send.send(());
    });
    let _ = receive.recv().await;
    if let Some(monitor) = monitor {
        unsafe {
            NSEvent::removeMonitor(&monitor);
        }
    }
    Some(Introduction { window, previous })
}
