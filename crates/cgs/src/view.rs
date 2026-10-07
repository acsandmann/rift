use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAutoresizingMaskOptions, NSControl, NSControlSize, NSLayoutAttribute,
    NSLayoutConstraint, NSLayoutRelation, NSUserInterfaceItemIdentification, NSView,
    NSViewController,
};
use objc2_foundation::NSString;

use crate::{Insets, Ui};

pub trait NativeView: 'static {
    fn ns_view(&self) -> &NSView;
    fn view_controller(&self) -> Option<&objc2_app_kit::NSViewController> { None }
    fn set_hidden(&self, value: bool) {
        if self.ns_view().isHidden() != value {
            self.ns_view().setHidden(value);
        }
    }
    fn tooltip(&self, text: &str) { self.ns_view().setToolTip(Some(&NSString::from_str(text))); }
    fn identifier(&self, text: &str) {
        self.ns_view().setIdentifier(Some(&NSString::from_str(text)));
    }
    fn accessibility_label(&self, text: &str) {
        self.ns_view().setAccessibilityLabel(Some(&NSString::from_str(text)));
    }
    fn accessibility_help(&self, text: &str) {
        self.ns_view().setAccessibilityHelp(Some(&NSString::from_str(text)));
    }
    fn accessibility_identifier(&self, text: &str) {
        self.ns_view().setAccessibilityIdentifier(Some(&NSString::from_str(text)));
    }
    fn width(&self, value: f64) {
        dimension(
            self.ns_view(),
            NSLayoutAttribute::Width,
            NSLayoutRelation::Equal,
            value,
        );
    }
    fn height(&self, value: f64) {
        dimension(
            self.ns_view(),
            NSLayoutAttribute::Height,
            NSLayoutRelation::Equal,
            value,
        );
    }
    fn min_height(&self, value: f64) {
        dimension(
            self.ns_view(),
            NSLayoutAttribute::Height,
            NSLayoutRelation::GreaterThanOrEqual,
            value,
        );
    }
    fn min_width(&self, value: f64) {
        dimension(
            self.ns_view(),
            NSLayoutAttribute::Width,
            NSLayoutRelation::GreaterThanOrEqual,
            value,
        );
    }
    fn hugging_priority(&self, priority: f32, axis: objc2_app_kit::NSLayoutConstraintOrientation) {
        self.ns_view().setContentHuggingPriority_forOrientation(priority, axis);
    }
    fn compression_resistance(
        &self,
        priority: f32,
        axis: objc2_app_kit::NSLayoutConstraintOrientation,
    ) {
        self.ns_view()
            .setContentCompressionResistancePriority_forOrientation(priority, axis);
    }
    fn max_width(&self, value: f64) {
        dimension(
            self.ns_view(),
            NSLayoutAttribute::Width,
            NSLayoutRelation::LessThanOrEqual,
            value,
        );
    }
}
pub trait NativeControl: NativeView {
    fn ns_control(&self) -> &NSControl;
    fn set_enabled(&self, value: bool) {
        if self.ns_control().isEnabled() != value {
            self.ns_control().setEnabled(value);
        }
    }
    fn control_size(&self, size: NSControlSize) { self.ns_control().setControlSize(size); }
}
impl NativeView for Retained<NSView> {
    fn ns_view(&self) -> &NSView { self }
}

impl<T: NativeView + ?Sized> NativeView for std::rc::Rc<T> {
    fn ns_view(&self) -> &NSView { (**self).ns_view() }

    fn view_controller(&self) -> Option<&NSViewController> { (**self).view_controller() }
}
impl<T: NativeControl> NativeControl for std::rc::Rc<T> {
    fn ns_control(&self) -> &NSControl { (**self).ns_control() }
}

pub(crate) fn dimension(
    view: &NSView,
    attribute: NSLayoutAttribute,
    relation: NSLayoutRelation,
    value: f64,
) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    unsafe { NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
        view, attribute, relation, None, NSLayoutAttribute::NotAnAttribute, 1.0, value
    ) }.setActive(true);
}
pub(crate) fn pin(parent: &NSView, child: &NSView, insets: Insets) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    for (attribute, constant) in [
        (NSLayoutAttribute::Leading, insets.left),
        (NSLayoutAttribute::Trailing, -insets.right),
        (NSLayoutAttribute::Top, insets.top),
        (NSLayoutAttribute::Bottom, -insets.bottom),
    ] {
        unsafe { NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(child, attribute, NSLayoutRelation::Equal, Some(parent), attribute, 1.0, constant) }.setActive(true);
    }
}
pub(crate) fn prepare(view: &NSView) { view.setTranslatesAutoresizingMaskIntoConstraints(false); }

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiDocumentView"]
    struct DocumentView;
    impl DocumentView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self)->bool{true}
    }
);

pub struct View {
    native: Retained<NSView>,
    children: RefCell<Vec<Box<dyn NativeView>>>,
}
impl View {
    pub fn new(ui: &Ui) -> Self {
        Self {
            native: NSView::new(ui.mtm()),
            children: RefCell::new(Vec::new()),
        }
    }

    pub fn flipped(ui: &Ui) -> Self {
        let native: Retained<DocumentView> =
            unsafe { msg_send![super(DocumentView::alloc(ui.mtm()).set_ivars(())), init] };
        Self {
            native: native.into_super(),
            children: RefCell::new(Vec::new()),
        }
    }

    pub fn ns_view(&self) -> &NSView { &self.native }

    /// Keep controls below full-size window chrome using AppKit's safe area.
    pub fn safe_area_content(self, child: impl NativeView) -> Self {
        self.native.addSubview(child.ns_view());
        prepare(child.ns_view());
        let guide = self.native.safeAreaLayoutGuide();
        child.ns_view().topAnchor().constraintEqualToAnchor(&guide.topAnchor()).setActive(true);
        child.ns_view().leadingAnchor().constraintEqualToAnchor(&guide.leadingAnchor()).setActive(true);
        child.ns_view().trailingAnchor().constraintEqualToAnchor(&guide.trailingAnchor()).setActive(true);
        child.ns_view().bottomAnchor().constraintEqualToAnchor(&guide.bottomAnchor()).setActive(true);
        self.children.borrow_mut().push(Box::new(child));
        self
    }

    pub fn content(self, child: impl NativeView, insets: Insets) -> Self {
        self.native.addSubview(child.ns_view());
        pin(&self.native, child.ns_view(), insets);
        self.children.borrow_mut().push(Box::new(child));
        self
    }
}
impl NativeView for View {
    fn ns_view(&self) -> &NSView { &self.native }
}

pub struct PageHost {
    view: View,
    controller: Retained<NSViewController>,
    page: RefCell<Option<Box<dyn NativeView>>>,
}
impl PageHost {
    pub fn new(ui: &Ui) -> Self {
        let view = View::new(ui);
        let controller = NSViewController::new(ui.mtm());
        controller.setView(view.ns_view());
        Self {
            view,
            controller,
            page: RefCell::new(None),
        }
    }

    pub fn ns_view_controller(&self) -> &NSViewController { &self.controller }

    pub fn set_page(&self, page: impl NativeView) {
        if self
            .page
            .borrow()
            .as_ref()
            .is_some_and(|current| std::ptr::eq(current.ns_view(), page.ns_view()))
        {
            return;
        }
        self.clear();
        if let Some(controller) = page.view_controller() {
            self.controller.addChildViewController(controller);
        }
        let view = page.ns_view();
        view.setTranslatesAutoresizingMaskIntoConstraints(true);
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.setFrame(self.view.ns_view().bounds());
        self.view.ns_view().addSubview(view);
        *self.page.borrow_mut() = Some(Box::new(page));
    }

    /// The caller retains cached pages; only the active root is attached to this host.
    pub fn set_cached_page(&self, page: impl NativeView) { self.set_page(page); }

    pub fn clear(&self) {
        if let Some(page) = self.page.borrow_mut().take() {
            page.ns_view().removeFromSuperview();
            if let Some(controller) = page.view_controller() {
                controller.removeFromParentViewController();
            }
        }
    }
}
impl NativeView for PageHost {
    fn view_controller(&self) -> Option<&NSViewController> { Some(&self.controller) }

    fn ns_view(&self) -> &NSView { self.view.ns_view() }
}

pub struct Metrics;
impl Metrics {
    pub const CONTROL_SPACING: f64 = 8.0;
    pub const DETAIL_MIN_WIDTH: f64 = 400.0;
    pub const PAGE_INSET: f64 = 20.0;
    pub const PAGE_TOP_INSET: f64 = 20.0;
    pub const ROW_SPACING: f64 = 6.0;
    pub const SECTION_SPACING: f64 = 16.0;
    pub const SIDEBAR_MIN_WIDTH: f64 = 160.0;
    pub const SIDEBAR_WIDTH: f64 = 200.0;
}

struct ResponsiveContent {
    wide: Box<dyn NativeView>,
    compact: Box<dyn NativeView>,
    size: crate::CGSize,
}
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsResponsiveView"]
    #[ivars = ResponsiveContent]
    struct ResponsiveNativeView;
    impl ResponsiveNativeView {
        #[unsafe(method(intrinsicContentSize))]
        fn intrinsic_size(&self) -> crate::CGSize {
            crate::CGSize::new(-1.0, self.ivars().size.height)
        }
        #[unsafe(method(layout))]
        fn layout_content(&self) {
            unsafe { let _: () = msg_send![super(self), layout]; }
            let content = self.ivars();
            let width = self.bounds().size.width;
            let compact = width < content.size.width;
            content.wide.set_hidden(compact);
            content.compact.set_hidden(!compact);
            let view = if compact { content.compact.ns_view() } else { content.wide.ns_view() };
            let size = view.fittingSize();
            view.setFrame(crate::CGRect::new(crate::CGPoint::new(0.0, 0.0), crate::CGSize::new(size.width.min(width), content.size.height)));
        }
    }
);

/// Uses a compact native alternative when the preferred content no longer fits.
pub struct ResponsiveView(Retained<ResponsiveNativeView>);
impl ResponsiveView {
    pub fn new(ui: &Ui, wide: impl NativeView, compact: impl NativeView) -> Self {
        let mut size = wide.ns_view().fittingSize();
        size.height = size.height.max(compact.ns_view().fittingSize().height);
        let native: Retained<ResponsiveNativeView> = unsafe {
            msg_send![
                super(
                    ResponsiveNativeView::alloc(ui.mtm()).set_ivars(ResponsiveContent {
                        wide: Box::new(wide),
                        compact: Box::new(compact),
                        size,
                    })
                ),
                init
            ]
        };
        native.addSubview(native.ivars().wide.ns_view());
        native.addSubview(native.ivars().compact.ns_view());
        native.ivars().compact.set_hidden(true);
        Self(native)
    }
}
impl NativeView for ResponsiveView {
    fn ns_view(&self) -> &NSView { &self.0 }
}

/// Apple's native glass surface, falling back to unmodified content before macOS 26.
pub struct GlassEffectView {
    native: Option<Retained<objc2_app_kit::NSGlassEffectView>>,
    content: Box<dyn NativeView>,
}
#[derive(Clone, Copy, Debug, Default)]
pub enum GlassStyle {
    #[default]
    Regular,
    Clear,
}
impl GlassEffectView {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        let native = objc2::runtime::AnyClass::get(c"NSGlassEffectView").map(|_| {
            let view = objc2_app_kit::NSGlassEffectView::new(ui.mtm());
            view.setContentView(Some(content.ns_view()));
            pin(&view, content.ns_view(), Insets {
                top: 0.0,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            });
            view
        });
        Self {
            native,
            content: Box::new(content),
        }
    }

    pub fn style(self, style: GlassStyle) -> Self {
        if let Some(view) = &self.native {
            view.setStyle(match style {
                GlassStyle::Regular => objc2_app_kit::NSGlassEffectViewStyle::Regular,
                GlassStyle::Clear => objc2_app_kit::NSGlassEffectViewStyle::Clear,
            });
        }
        self
    }

    pub fn corner_radius(self, radius: f64) -> Self {
        if let Some(view) = &self.native {
            view.setCornerRadius(radius);
        }
        self
    }

    /// Interactive glass is available on macOS 27 and newer.
    pub fn interactive(self, interactive: bool) -> Self {
        if let Some(view) = &self.native {
            let supported: bool = unsafe {
                msg_send![view, respondsToSelector: objc2::sel!(setEffectIsInteractive:)]
            };
            if supported {
                let _: () = unsafe { msg_send![view, setEffectIsInteractive: interactive] };
            }
        }
        self
    }

    pub fn ns_glass_effect_view(&self) -> Option<&objc2_app_kit::NSGlassEffectView> {
        self.native.as_deref()
    }
}
impl NativeView for GlassEffectView {
    fn ns_view(&self) -> &NSView {
        self.native
            .as_ref()
            .map_or_else(|| self.content.ns_view(), |view| view.as_ref())
    }
}

/// Shallow navigation that keeps visited pages and their scroll positions alive.
pub struct NavigationHost {
    host: PageHost,
    root: RefCell<Option<Rc<dyn NativeView>>>,
}
impl NavigationHost {
    pub fn new(ui: &Ui) -> Self {
        Self {
            host: PageHost::new(ui),
            root: RefCell::new(None),
        }
    }

    pub fn set_root(&self, page: Rc<dyn NativeView>) {
        self.host.set_cached_page(page.clone());
        *self.root.borrow_mut() = Some(page);
    }

    pub fn push(&self, page: Rc<dyn NativeView>) { self.host.set_cached_page(page); }

    pub fn pop(&self) {
        if let Some(root) = self.root.borrow().as_ref() {
            self.host.set_cached_page(root.clone());
        }
    }
}
impl NativeView for NavigationHost {
    fn ns_view(&self) -> &NSView { self.host.ns_view() }

    fn view_controller(&self) -> Option<&NSViewController> { self.host.view_controller() }
}
