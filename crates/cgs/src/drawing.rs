use std::cell::RefCell;
use std::time::Duration;

use objc2::rc::{Retained, Weak};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_core_graphics::CGContext;
use objc2_foundation::{NSString, NSTimer};
use objc2_quartz_core::CALayer;

use crate::bridge::callback;
use crate::{CGRect, Label, NativeView, SecondaryLabel, Ui, VStack, View};

type DrawCallback = Box<dyn FnMut(&CGContext, CGRect)>;
type KeyCallback = Box<dyn FnMut(&NSEvent) -> bool>;
struct CanvasState {
    draw: RefCell<DrawCallback>,
    key: RefCell<Option<KeyCallback>>,
}
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiCanvas"]
    #[ivars = CanvasState]
    struct CanvasView;
    impl CanvasView {
        #[unsafe(method(drawRect:))]
        fn draw(&self,_dirty:CGRect){let _keep_alive=self.retain();if let Some(context)=NSGraphicsContext::currentContext(){let context=context.CGContext();let cg=context.as_ref();CGContext::save_g_state(Some(cg));if let Ok(mut draw)=self.ivars().draw.try_borrow_mut(){callback(||draw(cg,self.bounds()));}CGContext::restore_g_state(Some(cg));}}
        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self){unsafe{let _:()=msg_send![super(self),viewDidChangeEffectiveAppearance];}self.setNeedsDisplay(true);}
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self)->bool{self.ivars().key.borrow().is_some()}
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self,event:&NSEvent){
            if self.ivars().key.borrow().is_some(){if let Some(window)=self.window(){window.makeFirstResponder(Some(self));}}
            else{unsafe{let _:()=msg_send![super(self),mouseDown:event];}}
        }
        #[unsafe(method(focusRingMaskBounds))]
        fn focus_ring_bounds(&self)->CGRect{self.bounds()}
        #[unsafe(method(drawFocusRingMask))]
        fn focus_ring_mask(&self){if let Some(context)=NSGraphicsContext::currentContext(){let context=context.CGContext();CGContext::fill_rect(Some(context.as_ref()),self.bounds());}}
        #[unsafe(method(keyDown:))]
        fn key_down(&self,event:&NSEvent){let _keep_alive=self.retain();let mut handled=false;if let Ok(mut key)=self.ivars().key.try_borrow_mut(){if let Some(key)=key.as_mut(){callback(||handled=key(event));}}if !handled{unsafe{let _:()=msg_send![super(self),keyDown:event];}}}
    }
);
pub struct Canvas(Retained<CanvasView>);
impl Canvas {
    pub fn new(ui: &Ui, draw: impl FnMut(&CGContext, CGRect) + 'static) -> Self {
        let this = CanvasView::alloc(ui.mtm()).set_ivars(CanvasState {
            draw: RefCell::new(Box::new(draw)),
            key: RefCell::new(None),
        });
        Self(unsafe { msg_send![super(this), init] })
    }

    pub fn on_key_down(self, f: impl FnMut(&NSEvent) -> bool + 'static) -> Self {
        *self.0.ivars().key.borrow_mut() = Some(Box::new(f));
        self.0.setFocusRingType(NSFocusRingType::Exterior);
        self.0.setAccessibilityElement(true);
        self.0.setAccessibilityRole(Some(unsafe { NSAccessibilityGroupRole }));
        self
    }

    pub fn redraw(&self) { self.0.setNeedsDisplay(true); }

    pub fn ns_view(&self) -> &NSView { &self.0 }
}
impl NativeView for Canvas {
    fn ns_view(&self) -> &NSView { &self.0 }
}

pub struct LayerHost(View);
impl LayerHost {
    pub fn new(ui: &Ui, layer: &CALayer) -> Self {
        let view = View::new(ui);
        view.ns_view().setWantsLayer(true);
        view.ns_view().setLayer(Some(layer));
        Self(view)
    }

    pub fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
impl NativeView for LayerHost {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}

pub struct VisualEffect {
    native: Retained<NSVisualEffectView>,
    content: Box<dyn NativeView>,
}
impl VisualEffect {
    pub fn new(ui: &Ui, material: NSVisualEffectMaterial, content: impl NativeView) -> Self {
        let native = NSVisualEffectView::new(ui.mtm());
        native.setMaterial(material);
        native.setState(NSVisualEffectState::FollowsWindowActiveState);
        native.addSubview(content.ns_view());
        crate::view::pin(&native, content.ns_view(), crate::Insets {
            top: 0.0,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });
        Self {
            native,
            content: Box::new(content),
        }
    }

    pub fn sidebar(ui: &Ui, content: impl NativeView) -> Self {
        Self::new(ui, NSVisualEffectMaterial::Sidebar, content)
    }

    pub fn ns_visual_effect_view(&self) -> &NSVisualEffectView { &self.native }

    pub fn content_view(&self) -> &NSView { self.content.ns_view() }
}
impl NativeView for VisualEffect {
    fn ns_view(&self) -> &NSView { &self.native }
}
pub struct BackgroundView(View);
impl BackgroundView {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        Self(View::new(ui).content(content, crate::Insets {
            top: 0.0,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        }))
    }

    pub fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
impl NativeView for BackgroundView {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}

pub struct ProgressIndicator(Retained<NSProgressIndicator>);
impl ProgressIndicator {
    pub fn new(ui: &Ui) -> Self {
        let native = NSProgressIndicator::new(ui.mtm());
        native.setStyle(NSProgressIndicatorStyle::Spinning);
        native.setIndeterminate(true);
        Self(native)
    }

    pub fn start(&self) {
        unsafe {
            self.0.startAnimation(None);
        }
    }

    pub fn stop(&self) {
        unsafe {
            self.0.stopAnimation(None);
        }
    }

    pub fn fraction(&self, value: f64) {
        self.0.setIndeterminate(false);
        self.0.setMinValue(0.0);
        self.0.setMaxValue(1.0);
        self.0.setDoubleValue(value);
    }

    pub fn ns_progress_indicator(&self) -> &NSProgressIndicator { &self.0 }
}
impl NativeView for ProgressIndicator {
    fn ns_view(&self) -> &NSView { &self.0 }
}
pub struct EmptyState(VStack);
impl EmptyState {
    pub fn new(ui: &Ui, title: &str, description: &str) -> Self {
        Self(
            VStack::new(ui)
                .push(crate::SectionTitle::new(ui, title))
                .push(SecondaryLabel::new(ui, description)),
        )
    }

    pub fn action(self, button: crate::Button) -> Self { Self(self.0.push(button)) }
}
impl NativeView for EmptyState {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
pub struct LoadingView(VStack);
impl LoadingView {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let progress = ProgressIndicator::new(ui);
        progress.start();
        Self(VStack::new(ui).push(progress).push(Label::new(ui, title)))
    }
}
impl NativeView for LoadingView {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
pub type StatusLabel = SecondaryLabel;
pub struct TransientStatus {
    label: StatusLabel,
    timer: RefCell<Option<Retained<NSTimer>>>,
    target: Retained<crate::bridge::ActionTarget>,
}
impl TransientStatus {
    pub fn new(ui: &Ui) -> Self {
        let label = StatusLabel::new(ui, "");
        label.set_hidden(true);
        let target = crate::bridge::ActionTarget::new(ui);
        let weak = Weak::new(label.ns_view());
        target.set(move |_| {
            if let Some(view) = weak.load() {
                view.setHidden(true);
            }
        });
        Self {
            label,
            timer: RefCell::new(None),
            target,
        }
    }

    pub fn show(&self, text: &str, duration: Duration) {
        self.clear();
        self.label.set_text(text);
        self.label.set_hidden(false);
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                duration.as_secs_f64(),
                &self.target,
                objc2::sel!(invoke:),
                None,
                false,
            )
        };
        *self.timer.borrow_mut() = Some(timer);
    }

    pub fn clear(&self) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.invalidate();
        }
        self.label.set_hidden(true);
    }

    pub fn ns_text_field(&self) -> &NSTextField { self.label.ns_text_field() }
}
impl NativeView for TransientStatus {
    fn ns_view(&self) -> &NSView { self.label.ns_view() }
}
impl Drop for TransientStatus {
    fn drop(&mut self) { self.clear(); }
}
pub struct Clipboard;
impl Clipboard {
    pub fn set_text(ui: &Ui, text: &str) -> bool {
        let _ = ui;
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
    }

    pub fn text(ui: &Ui) -> Option<String> {
        let _ = ui;
        NSPasteboard::generalPasteboard()
            .stringForType(unsafe { NSPasteboardTypeString })
            .map(|s| s.to_string())
    }
}
