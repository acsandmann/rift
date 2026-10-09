use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::NSArray;

use crate::view::prepare;
use crate::{Insets, Metrics, NativeView, Ui, View};

pub struct Stack {
    native: Retained<NSStackView>,
    children: RefCell<Vec<Box<dyn NativeView>>>,
    owners: Vec<Box<dyn std::any::Any>>,
}
impl Stack {
    pub fn new(ui: &Ui, orientation: NSUserInterfaceLayoutOrientation) -> Self {
        let native = NSStackView::new(ui.mtm());
        prepare(&native);
        native.setOrientation(orientation);
        native.setSpacing(Metrics::CONTROL_SPACING);
        native.setAlignment(if orientation == NSUserInterfaceLayoutOrientation::Vertical {
            NSLayoutAttribute::Leading
        } else {
            NSLayoutAttribute::CenterY
        });
        Self {
            native,
            children: RefCell::new(Vec::new()),
            owners: Vec::new(),
        }
    }

    pub fn ns_stack_view(&self) -> &NSStackView { &self.native }

    /// Keep a non-view owner, such as a control wrapper whose view sits in a composite,
    /// alive for as long as the stack.
    pub fn keep(mut self, owner: impl std::any::Any) -> Self {
        self.owners.push(Box::new(owner));
        self
    }

    pub fn spacing(self, value: f64) -> Self {
        self.native.setSpacing(value);
        self
    }

    pub fn alignment(self, value: NSLayoutAttribute) -> Self {
        self.native.setAlignment(value);
        self
    }

    pub fn distribution(self, value: NSStackViewDistribution) -> Self {
        self.native.setDistribution(value);
        self
    }

    pub fn insets(self, value: Insets) -> Self {
        self.native.setEdgeInsets(value);
        self
    }

    pub fn push(self, view: impl NativeView) -> Self {
        self.add(view);
        self
    }

    pub fn add(&self, view: impl NativeView) {
        prepare(view.ns_view());
        self.native.addArrangedSubview(view.ns_view());
        if self.native.orientation() == NSUserInterfaceLayoutOrientation::Vertical {
            let insets = self.native.edgeInsets();
            view.ns_view()
                .widthAnchor()
                .constraintEqualToAnchor_constant(
                    &self.native.widthAnchor(),
                    -insets.left - insets.right,
                )
                .setActive(true);
        }
        self.children.borrow_mut().push(Box::new(view));
    }

    pub fn clear(&self) {
        for child in self.children.borrow_mut().drain(..) {
            self.native.removeArrangedSubview(child.ns_view());
            child.ns_view().removeFromSuperview();
        }
    }

    pub fn spacer(self, ui: &Ui) -> Self { self.push(Spacer::new(ui)) }
}
impl NativeView for Stack {
    fn ns_view(&self) -> &NSView { &self.native }
}
macro_rules! stack {
    ($name:ident, $axis:ident) => {
        pub struct $name(Stack);
        impl $name {
            pub fn new(ui: &Ui) -> Self {
                Self(Stack::new(ui, NSUserInterfaceLayoutOrientation::$axis))
            }

            pub fn spacing(self, value: f64) -> Self { Self(self.0.spacing(value)) }

            pub fn alignment(self, value: NSLayoutAttribute) -> Self {
                Self(self.0.alignment(value))
            }

            pub fn distribution(self, value: NSStackViewDistribution) -> Self {
                Self(self.0.distribution(value))
            }

            pub fn insets(self, value: Insets) -> Self { Self(self.0.insets(value)) }

            pub fn push(self, view: impl NativeView) -> Self { Self(self.0.push(view)) }

            pub fn add(&self, view: impl NativeView) { self.0.add(view); }

            pub fn clear(&self) { self.0.clear(); }

            pub fn spacer(self, ui: &Ui) -> Self { Self(self.0.spacer(ui)) }

            pub fn keep(self, owner: impl std::any::Any) -> Self { Self(self.0.keep(owner)) }

            pub fn ns_stack_view(&self) -> &NSStackView { self.0.ns_stack_view() }
        }
        impl NativeView for $name {
            fn ns_view(&self) -> &NSView { self.0.ns_view() }
        }
    };
}
stack!(VStack, Vertical);
stack!(HStack, Horizontal);

pub struct Spacer(View);
impl Spacer {
    pub fn new(ui: &Ui) -> Self {
        let view = View::new(ui);
        prepare(view.ns_view());
        for axis in [
            NSLayoutConstraintOrientation::Horizontal,
            NSLayoutConstraintOrientation::Vertical,
        ] {
            view.ns_view().setContentHuggingPriority_forOrientation(1.0, axis);
            view.ns_view().setContentCompressionResistancePriority_forOrientation(1.0, axis);
        }
        Self(view)
    }
}
impl NativeView for Spacer {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}

pub struct Grid {
    native: Retained<NSGridView>,
    children: Vec<Box<dyn NativeView>>,
}
impl Grid {
    pub fn new(ui: &Ui) -> Self {
        let native = NSGridView::new(ui.mtm());
        prepare(&native);
        native.setXPlacement(NSGridCellPlacement::Leading);
        native.setYPlacement(NSGridCellPlacement::Center);
        Self { native, children: Vec::new() }
    }

    pub fn row(mut self, cells: Vec<Box<dyn NativeView>>) -> Self {
        let views: Vec<&NSView> = cells.iter().map(|v| v.ns_view()).collect();
        self.native.addRowWithViews(&NSArray::from_slice(&views));
        self.children.extend(cells);
        self
    }

    pub fn spacing(self, rows: f64, columns: f64) -> Self {
        self.native.setRowSpacing(rows);
        self.native.setColumnSpacing(columns);
        self
    }

    /// Fixed column widths, leading columns first.
    pub fn column_widths(self, widths: &[f64]) -> Self {
        for (index, width) in widths.iter().enumerate() {
            self.native.columnAtIndex(index as isize).setWidth(*width);
        }
        self
    }

    /// Align each row's cells on their first text baseline.
    pub fn first_baseline(self) -> Self {
        self.native.setRowAlignment(objc2_app_kit::NSGridRowAlignment::FirstBaseline);
        self
    }

    pub fn ns_grid_view(&self) -> &NSGridView { &self.native }
}
impl NativeView for Grid {
    fn ns_view(&self) -> &NSView { &self.native }
}

pub struct Form {
    ui: Ui,
    grid: Grid,
}
impl Form {
    pub fn new(ui: &Ui) -> Self {
        Self {
            ui: *ui,
            grid: Grid::new(ui).spacing(Metrics::ROW_SPACING, Metrics::CONTROL_SPACING),
        }
    }

    pub fn row(mut self, title: &str, control: impl NativeView) -> Self {
        self.grid = self.grid.row(vec![
            Box::new(crate::Label::new(&self.ui, title)),
            Box::new(control),
        ]);
        self
    }

    pub fn help(mut self, text: &str) -> Self {
        self.grid = self.grid.row(vec![
            Box::new(crate::Label::new(&self.ui, "")),
            Box::new(crate::SecondaryLabel::new(&self.ui, text)),
        ]);
        self
    }

    pub fn ns_grid_view(&self) -> &NSGridView { self.grid.ns_grid_view() }
}
impl NativeView for Form {
    fn ns_view(&self) -> &NSView { self.grid.ns_view() }
}

pub struct Divider(Retained<NSBox>);
impl Divider {
    pub fn new(ui: &Ui) -> Self {
        let native = NSBox::new(ui.mtm());
        native.setBoxType(NSBoxType::Separator);
        Self(native)
    }

    pub fn ns_box(&self) -> &NSBox { &self.0 }
}
impl NativeView for Divider {
    fn ns_view(&self) -> &NSView { &self.0 }
}

/// A quiet native group surface; semantic fill follows the window appearance.
pub struct GroupBox {
    native: Retained<NSBox>,
    _content: Box<dyn NativeView>,
}
impl GroupBox {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        Self::with_insets(ui, content, Insets {
            top: 10.0,
            left: 12.0,
            bottom: 10.0,
            right: 12.0,
        })
    }

    pub fn with_insets(ui: &Ui, content: impl NativeView, insets: Insets) -> Self {
        let native = NSBox::new(ui.mtm());
        native.setBoxType(NSBoxType::Custom);
        native.setTitlePosition(NSTitlePosition::NoTitle);
        native.setBorderWidth(0.0);
        native.setCornerRadius(8.0);
        native.setFillColor(&NSColor::quaternarySystemFillColor());
        native.setContentViewMargins(crate::CGSize::new(0.0, 0.0));
        let host = native.contentView().unwrap();
        host.addSubview(content.ns_view());
        crate::view::pin(&host, content.ns_view(), insets);
        Self {
            native,
            _content: Box::new(content),
        }
    }
}
impl NativeView for GroupBox {
    fn ns_view(&self) -> &NSView { &self.native }
}

define_class!(
    #[unsafe(super(NSScrollView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiScrollView"]
    #[ivars = Cell<bool>]
    struct NativeScrollView;

    impl NativeScrollView {
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            if self.ivars().get() {
                if let Some(parent) = unsafe { self.superview() } {
                    parent.scrollWheel(event);
                }
            } else {
                unsafe { let _: () = msg_send![super(self), scrollWheel: event]; }
            }
        }
    }
);

pub struct ScrollView {
    native: Retained<NativeScrollView>,
    content: Box<dyn NativeView>,
}
impl ScrollView {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        let native: Retained<NativeScrollView> = unsafe {
            msg_send![
                super(NativeScrollView::alloc(ui.mtm()).set_ivars(Cell::new(false))),
                init
            ]
        };
        native.setHasVerticalScroller(true);
        native.setAutohidesScrollers(true);
        native.setDrawsBackground(false);
        native.setDocumentView(Some(content.ns_view()));
        Self {
            native,
            content: Box::new(content),
        }
    }

    pub fn ns_scroll_view(&self) -> &NSScrollView { &self.native }

    /// Forward wheel input to the enclosing scroll view when content is fully expanded.
    pub fn defer_scrolling(&self) {
        self.native.ivars().set(true);
        self.native.setHasVerticalScroller(false);
        self.native.setHasHorizontalScroller(false);
        self.native.setVerticalScrollElasticity(NSScrollElasticity::None);
        self.native.setHorizontalScrollElasticity(NSScrollElasticity::None);
    }

    pub fn content_view(&self) -> &NSView { self.content.ns_view() }

    /// Pin the document to the viewport width while allowing vertical scrolling.
    pub fn fit_width(&self) {
        let clip = self.native.contentView();
        let child = self.content.ns_view();
        prepare(child);
        child
            .leadingAnchor()
            .constraintEqualToAnchor(&clip.leadingAnchor())
            .setActive(true);
        child.topAnchor().constraintEqualToAnchor(&clip.topAnchor()).setActive(true);
        child.widthAnchor().constraintEqualToAnchor(&clip.widthAnchor()).setActive(true);
    }
}
impl NativeView for ScrollView {
    fn ns_view(&self) -> &NSView { &self.native }
}

pub struct Disclosure {
    stack: VStack,
    button: crate::Button,
    body: Box<dyn NativeView>,
}
impl Disclosure {
    pub fn new(ui: &Ui, title: &str, content: impl NativeView) -> Self {
        use objc2::rc::Weak;
        let body = Box::new(content);
        body.set_hidden(true);
        let weak = Weak::new(body.ns_view());
        let button = crate::Button::new(ui, title).disclosure().on_click(move || {
            if let Some(view) = weak.load() {
                view.setHidden(!view.isHidden());
            }
        });
        button.set_title("");
        button.accessibility_label(title);
        let heading = HStack::new(ui).push(crate::Label::new(ui, title));
        heading.ns_stack_view().insertArrangedSubview_atIndex(button.ns_view(), 0);
        let stack = VStack::new(ui);
        stack.add(heading);
        stack.ns_stack_view().addArrangedSubview(body.ns_view());
        Self { stack, button, body }
    }

    pub fn set_expanded(&self, value: bool) {
        self.body.set_hidden(!value);
        self.button.ns_button().setState(if value {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }

    pub fn ns_button(&self) -> &NSButton { self.button.ns_button() }
}
impl NativeView for Disclosure {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
