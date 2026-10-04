use std::cell::OnceCell;
use std::rc::Rc;

use objc2::Message;
use objc2::rc::Retained;
use objc2_app_kit::{NSTextField, NSView};

use crate::*;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Validation {
    #[default]
    None,
    Info(String),
    Warning(String),
    Error(String),
}
impl Validation {
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Info(s) | Self::Warning(s) | Self::Error(s) => Some(s),
        }
    }
}
pub struct ValidationMessage {
    stack: HStack,
    label: SecondaryLabel,
}
impl ValidationMessage {
    pub fn new(ui: &Ui) -> Self {
        let label = SecondaryLabel::new(ui, "");
        let stack = HStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(label.ns_view());
        let this = Self { stack, label };
        this.set_hidden(true);
        this
    }

    pub fn set_validation(&self, value: &Validation) {
        self.label.set_text(value.message().unwrap_or(""));
        self.set_hidden(matches!(value, Validation::None));
        self.accessibility_label(value.message().unwrap_or(""));
    }

    pub fn ns_text_field(&self) -> &NSTextField { self.label.ns_text_field() }
}
impl NativeView for ValidationMessage {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
pub struct Badge(Caption);
impl Badge {
    pub fn new(ui: &Ui, text: &str) -> Self { Self(Caption::new(ui, text)) }

    pub fn set_text(&self, text: &str) { self.0.set_text(text); }

    pub fn ns_text_field(&self) -> &NSTextField { self.0.ns_text_field() }
}
impl NativeView for Badge {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}

pub struct SettingsRow {
    ui: Ui,
    grid: OnceCell<Grid>,
    label: VStack,
    value: VStack,
    line: Rc<HStack>,
    control: Box<dyn NativeView>,
    validation: Rc<ValidationMessage>,
}
impl SettingsRow {
    pub fn new(ui: &Ui, title: &str, control: impl NativeView) -> Self {
        Self::with_validation(ui, title, control, Rc::new(ValidationMessage::new(ui)))
    }

    pub fn with_validation(
        ui: &Ui,
        title: &str,
        control: impl NativeView,
        validation: Rc<ValidationMessage>,
    ) -> Self {
        let label = VStack::new(ui).spacing(4.0).push(WrappingLabel::new(ui, title));
        let line = Rc::new(HStack::new(ui));
        control.accessibility_label(title);
        line.ns_stack_view().addArrangedSubview(control.ns_view());
        let value = VStack::new(ui).spacing(4.0).push(line.clone()).push(validation.clone());
        Self {
            ui: *ui,
            grid: OnceCell::new(),
            label,
            value,
            line,
            control: Box::new(control),
            validation,
        }
    }

    pub fn description(self, text: &str) -> Self {
        self.label.add(Caption::new(&self.ui, text));
        self
    }

    pub fn suffix(self, text: &str) -> Self {
        self.line.add(SecondaryLabel::new(&self.ui, text));
        self
    }

    pub fn set_validation(&self, value: &Validation) { self.validation.set_validation(value); }

    pub fn control_view(&self) -> &NSView { self.control.ns_view() }

    fn take_form_cells(&self) -> [&NSView; 2] {
        if let Some(grid) = self.grid.get() {
            if grid.ns_grid_view().numberOfRows() > 0 {
                grid.ns_grid_view().removeRowAtIndex(0);
            }
        }
        [self.label.ns_view(), self.value.ns_view()]
    }
}
impl NativeView for SettingsRow {
    fn ns_view(&self) -> &NSView {
        self.grid
            .get_or_init(|| {
                let grid = Grid::new(&self.ui).spacing(10.0, 20.0);
                grid.ns_grid_view().addRowWithViews(&objc2_foundation::NSArray::from_slice(&[
                    self.label.ns_view(),
                    self.value.ns_view(),
                ]));
                grid.ns_grid_view()
                    .columnAtIndex(0)
                    .setXPlacement(objc2_app_kit::NSGridCellPlacement::Fill);
                grid.ns_grid_view()
                    .columnAtIndex(1)
                    .setXPlacement(objc2_app_kit::NSGridCellPlacement::Trailing);
                grid
            })
            .ns_view()
    }
}
macro_rules! setting_row {
    ($name:ident) => {
        pub struct $name(SettingsRow);
        impl $name {
            pub fn new(ui: &Ui, title: &str, control: impl NativeControl) -> Self {
                Self(SettingsRow::new(ui, title, control))
            }

            pub fn description(self, text: &str) -> Self { Self(self.0.description(text)) }

            pub fn suffix(self, text: &str) -> Self { Self(self.0.suffix(text)) }

            pub fn set_validation(&self, value: &Validation) { self.0.set_validation(value); }

            pub fn control_view(&self) -> &NSView { self.0.control_view() }
        }
        impl NativeView for $name {
            fn ns_view(&self) -> &NSView { self.0.ns_view() }
        }
        impl From<$name> for SettingsRow {
            fn from(row: $name) -> Self { row.0 }
        }
    };
}
setting_row!(SwitchRow);
setting_row!(PopupRow);
setting_row!(TextRow);
setting_row!(NumberRow);
setting_row!(SliderRow);
setting_row!(ColorRow);
setting_row!(ButtonRow);
pub type DisclosureRow = Disclosure;

/// Compact preference content; AppKit controls provide their own appearance.
pub struct SettingsGroup {
    surface: GroupBox,
    grid: Rc<Grid>,
    rows: Vec<SettingsRow>,
    dividers: Vec<Divider>,
}
impl SettingsGroup {
    pub fn new(ui: &Ui) -> Self {
        let grid = Rc::new(Grid::new(ui).spacing(8.0, 20.0));
        let surface = GroupBox::new(ui, grid.clone());
        Self {
            surface,
            grid,
            rows: Vec::new(),
            dividers: Vec::new(),
        }
    }

    pub fn row(mut self, row: impl Into<SettingsRow>) -> Self {
        let row = row.into();
        let grid = self.grid.ns_grid_view();
        if !self.rows.is_empty() {
            let divider = Divider::new(&row.ui);
            let separator = grid.addRowWithViews(&objc2_foundation::NSArray::from_slice(&[
                divider.ns_view(),
                &objc2_app_kit::NSGridCell::emptyContentView(row.ui.mtm()),
            ]));
            separator.mergeCellsInRange(objc2_foundation::NSRange::new(0, 2));
            grid.cellAtColumnIndex_rowIndex(0, grid.numberOfRows() - 1)
                .setXPlacement(objc2_app_kit::NSGridCellPlacement::Fill);
            self.dividers.push(divider);
        }
        grid.addRowWithViews(&objc2_foundation::NSArray::from_slice(&row.take_form_cells()));
        grid.columnAtIndex(0).setXPlacement(objc2_app_kit::NSGridCellPlacement::Fill);
        grid.columnAtIndex(1)
            .setXPlacement(objc2_app_kit::NSGridCellPlacement::Trailing);
        self.rows.push(row);
        self
    }
}
impl NativeView for SettingsGroup {
    fn ns_view(&self) -> &NSView { self.surface.ns_view() }
}

pub struct Section {
    ui: Ui,
    stack: VStack,
    group: Option<SettingsGroup>,
}
impl Section {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let mut stack = VStack::new(ui).spacing(6.0);
        if !title.is_empty() {
            stack = stack.push(SectionTitle::new(ui, title));
        }
        Self { ui: *ui, stack, group: None }
    }

    pub fn subsection(ui: &Ui, title: &str) -> Self {
        Self::new(ui, "").content(SubsectionTitle::new(ui, title))
    }

    pub fn description(self, text: &str) -> Self {
        self.stack.add(Caption::new(&self.ui, text));
        self
    }

    pub fn row(mut self, row: impl Into<SettingsRow>) -> Self {
        let group = self.group.take().unwrap_or_else(|| {
            let group = SettingsGroup::new(&self.ui);
            self.stack.ns_stack_view().addArrangedSubview(group.ns_view());
            group
                .ns_view()
                .widthAnchor()
                .constraintEqualToAnchor(&self.stack.ns_view().widthAnchor())
                .setActive(true);
            group
        });
        self.group = Some(group.row(row));
        self
    }

    pub fn footer(self, content: impl NativeView) -> Self {
        self.stack.add(content);
        self
    }

    pub fn content(self, content: impl NativeView) -> Self {
        self.stack.add(content);
        self
    }
}
impl NativeView for Section {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}

pub struct SettingsPage {
    ui: Ui,
    scroll: ScrollView,
    content: VStack,
    top: Retained<objc2_app_kit::NSLayoutConstraint>,
}
impl SettingsPage {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let content = VStack::new(ui).spacing(Metrics::SECTION_SPACING).insets(Insets {
            top: Metrics::PAGE_TOP_INSET,
            left: Metrics::PAGE_INSET,
            bottom: Metrics::PAGE_INSET,
            right: Metrics::PAGE_INSET,
        });
        if !title.is_empty() {
            content.add(Title::new(ui, title));
        }
        let outer = View::flipped(ui);
        outer.ns_view().addSubview(content.ns_view());
        crate::view::prepare(content.ns_view());
        let parent = outer.ns_view();
        let child = content.ns_view();
        let top = child.topAnchor().constraintEqualToAnchor(&parent.topAnchor());
        top.setActive(true);
        child
            .bottomAnchor()
            .constraintEqualToAnchor(&parent.bottomAnchor())
            .setActive(true);
        child
            .centerXAnchor()
            .constraintEqualToAnchor(&parent.centerXAnchor())
            .setActive(true);
        child
            .widthAnchor()
            .constraintLessThanOrEqualToAnchor(&parent.widthAnchor())
            .setActive(true);
        child.widthAnchor().constraintLessThanOrEqualToConstant(580.0).setActive(true);
        let fill = child.widthAnchor().constraintEqualToAnchor(&parent.widthAnchor());
        fill.setPriority(750.0);
        fill.setActive(true);
        let scroll = ScrollView::new(ui, outer);
        scroll.ns_scroll_view().setDrawsBackground(true);
        scroll.ns_scroll_view().setBackgroundColor(&Color::window_background());
        scroll.fit_width();
        Self { ui: *ui, scroll, content, top }
    }

    /// Let a collection editor own the viewport; its table supplies scrolling.
    pub fn into_editor(self) -> EditorPage {
        self.content.ns_stack_view().setSpacing(Metrics::CONTROL_SPACING);
        let native = self.scroll.content_view().retain();
        self.scroll.ns_scroll_view().setDocumentView(None);
        native.removeFromSuperview();
        self.top.setActive(false);
        self.content
            .ns_view()
            .topAnchor()
            .constraintEqualToAnchor(&native.safeAreaLayoutGuide().topAnchor())
            .setActive(true);
        EditorPage { native, _content: self.content }
    }

    pub fn subtitle(self, text: &str) -> Self {
        self.content.add(SecondaryLabel::new(&self.ui, text));
        self
    }

    pub fn section(self, section: impl NativeView) -> Self {
        self.content.add(section);
        self
    }

    pub fn ns_scroll_view(&self) -> &objc2_app_kit::NSScrollView { self.scroll.ns_scroll_view() }
}
impl NativeView for SettingsPage {
    fn ns_view(&self) -> &NSView { self.scroll.ns_view() }
}

pub struct EditorPage {
    native: Retained<NSView>,
    _content: VStack,
}
impl NativeView for EditorPage {
    fn ns_view(&self) -> &NSView { &self.native }
}

pub struct ResettableRow {
    stack: HStack,
    reset: Button,
    row: Box<dyn NativeView>,
}
impl ResettableRow {
    pub fn new(ui: &Ui, row: impl NativeView) -> Self {
        let reset = Button::new(ui, "Reset");
        let stack = HStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(row.ns_view());
        stack.ns_stack_view().addArrangedSubview(reset.ns_view());
        Self {
            stack,
            reset,
            row: Box::new(row),
        }
    }

    pub fn on_reset(mut self, f: impl FnMut() + 'static) -> Self {
        self.reset = self.reset.on_click(f);
        self
    }

    pub fn set_reset_enabled(&self, value: bool) { self.reset.set_enabled(value); }

    pub fn row_view(&self) -> &NSView { self.row.ns_view() }
}
impl NativeView for ResettableRow {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
pub struct OverrideRow {
    stack: VStack,
    toggle: Checkbox,
    row: Box<dyn NativeView>,
    inherited: SecondaryLabel,
}
impl OverrideRow {
    pub fn new(ui: &Ui, row: impl NativeView, inherited: &str) -> Self {
        let label = SecondaryLabel::new(ui, inherited);
        let weak = objc2::rc::Weak::new(row.ns_view());
        row.set_hidden(true);
        let inherited_label = objc2::rc::Weak::new(label.ns_view());
        let toggle = Checkbox::new(ui, "Use custom value").on_change(move |v| {
            if let Some(row) = weak.load() {
                row.setHidden(!v);
            }
            if let Some(label) = inherited_label.load() {
                label.setHidden(v);
            }
        });
        let stack = VStack::new(ui);
        for view in [toggle.ns_view(), label.ns_view(), row.ns_view()] {
            stack.ns_stack_view().addArrangedSubview(view);
        }
        Self {
            stack,
            toggle,
            row: Box::new(row),
            inherited: label,
        }
    }

    pub fn set_overridden(&self, value: bool) {
        self.toggle.set_value(value);
        self.row.set_hidden(!value);
        self.inherited.set_hidden(value);
    }

    pub fn set_inherited_text(&self, text: &str) { self.inherited.set_text(text); }

    pub fn on_change(mut self, mut f: impl FnMut(bool) + 'static) -> Self {
        let weak = objc2::rc::Weak::new(self.row.ns_view());
        let label = objc2::rc::Weak::new(self.inherited.ns_view());
        self.toggle = self.toggle.on_change(move |v| {
            if let Some(row) = weak.load() {
                row.setHidden(!v);
            }
            if let Some(label) = label.load() {
                label.setHidden(v);
            }
            f(v);
        });
        self
    }
}
impl NativeView for OverrideRow {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}

pub struct Validated<T: NativeControl> {
    stack: VStack,
    input: T,
    message: ValidationMessage,
}
impl<T: NativeControl> Validated<T> {
    pub fn new(ui: &Ui, input: T) -> Self {
        let stack = VStack::new(ui).spacing(4.0);
        let message = ValidationMessage::new(ui);
        stack.ns_stack_view().addArrangedSubview(input.ns_view());
        stack.ns_stack_view().addArrangedSubview(message.ns_view());
        Self { stack, input, message }
    }

    pub fn input(&self) -> &T { &self.input }

    pub fn set_validation(&self, value: &Validation) { self.message.set_validation(value); }
}
impl<T: NativeControl> NativeView for Validated<T> {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
impl<T: NativeControl> NativeControl for Validated<T> {
    fn ns_control(&self) -> &objc2_app_kit::NSControl { self.input.ns_control() }
}
