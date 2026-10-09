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
    label: SecondaryLabel,
}
impl ValidationMessage {
    pub fn new(ui: &Ui) -> Self {
        let label = SecondaryLabel::new(ui, "");
        let this = Self { label };
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
    fn ns_view(&self) -> &NSView { self.label.ns_view() }
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
    title: String,
    grid: OnceCell<Grid>,
    label: Rc<dyn NativeView>,
    value: VStack,
    line: OnceCell<Rc<HStack>>,
    unit: Option<crate::UnitField>,
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
        let label = Rc::new(WrappingLabel::new(ui, title));
        control.accessibility_label(title);
        let value = VStack::new(ui)
            .spacing(4.0)
            .push(control.ns_view().retain())
            .push(validation.clone());
        Self {
            ui: *ui,
            title: title.to_owned(),
            grid: OnceCell::new(),
            label,
            value,
            line: OnceCell::new(),
            unit: None,
            control: Box::new(control),
            validation,
        }
    }

    /// Add a discoverable description beside the label, without expanding the row.
    pub fn help(mut self, text: &str) -> Self {
        if !text.trim().is_empty() {
            self.label = Rc::new(
                HStack::new(&self.ui)
                    .spacing(6.0)
                    .push(self.label)
                    .push(InfoButton::new(&self.ui, &self.title, text))
                    .push(Spacer::new(&self.ui)),
            );
        }
        self
    }

    pub fn description(mut self, text: &str) -> Self {
        self.label = Rc::new(
            VStack::new(&self.ui)
                .spacing(4.0)
                .push(self.label)
                .push(Caption::new(&self.ui, text)),
        );
        self
    }

    pub fn suffix(mut self, text: &str) -> Self {
        fn input(view: &NSView) -> Option<Retained<NSTextField>> {
            if let Some(field) = view.downcast_ref::<NSTextField>() {
                if field.isEditable() {
                    return Some(field.retain());
                }
            }
            view.subviews().iter().find_map(|child| input(&child))
        }
        if let Some(field) = input(self.control.ns_view()) {
            if let Some(parent) = unsafe { field.superview() }
                .and_then(|view| view.downcast::<objc2_app_kit::NSStackView>().ok())
            {
                let index = parent
                    .arrangedSubviews()
                    .iter()
                    .position(|view| std::ptr::eq::<NSView>(&*view, &***field))
                    .unwrap_or(0);
                parent.removeArrangedSubview(&field);
                field.removeFromSuperview();
                let unit = crate::UnitField::new(&self.ui, &field, text);
                parent.insertArrangedSubview_atIndex(unit.ns_view(), index as isize);
                if parent.orientation() == objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical
                {
                    unit.ns_view()
                        .widthAnchor()
                        .constraintEqualToAnchor(&parent.widthAnchor())
                        .setActive(true);
                }
                self.unit = Some(unit);
                return self;
            }
        }
        let line = self.line.get_or_init(|| {
            self.value.ns_stack_view().removeArrangedSubview(self.control.ns_view());
            self.control.ns_view().removeFromSuperview();
            let line = Rc::new(HStack::new(&self.ui).push(self.control.ns_view().retain()));
            self.value.ns_stack_view().insertArrangedSubview_atIndex(line.ns_view(), 0);
            line.ns_view()
                .widthAnchor()
                .constraintEqualToAnchor(&self.value.ns_view().widthAnchor())
                .setActive(true);
            line
        });
        line.add(SecondaryLabel::new(&self.ui, text));
        self
    }

    pub fn set_validation(&self, value: &Validation) { self.validation.set_validation(value); }

    pub fn control_view(&self) -> &NSView { self.control.ns_view() }

    /// A weak handle to the row's control, for enabling it from sync callbacks.
    pub fn control(&self) -> crate::WeakView {
        crate::WeakView(objc2::rc::Weak::new(self.control_view()))
    }

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

            pub fn help(self, text: &str) -> Self { Self(self.0.help(text)) }

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
    surface: Option<GroupBox>,
    grid: Rc<Grid>,
    rows: Vec<SettingsRow>,
    dividers: Vec<Divider>,
}
impl SettingsGroup {
    pub fn new(ui: &Ui) -> Self {
        let grid = Rc::new(Grid::new(ui).spacing(8.0, 20.0));
        let surface = GroupBox::new(ui, grid.clone());
        Self {
            surface: Some(surface),
            grid,
            rows: Vec::new(),
            dividers: Vec::new(),
        }
    }

    pub fn plain(ui: &Ui) -> Self {
        Self {
            surface: None,
            grid: Rc::new(Grid::new(ui).spacing(8.0, 20.0)),
            rows: Vec::new(),
            dividers: Vec::new(),
        }
    }

    pub fn row(mut self, row: impl Into<SettingsRow>) -> Self {
        let row = row.into();
        let grid = self.grid.ns_grid_view();
        if self.surface.is_some() && !self.rows.is_empty() {
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
        if row.control_view().downcast_ref::<objc2_app_kit::NSPopUpButton>().is_some() {
            row.control_view()
                .widthAnchor()
                .constraintEqualToConstant(220.0)
                .setActive(true);
        }
        grid.addRowWithViews(&objc2_foundation::NSArray::from_slice(&row.take_form_cells()));
        grid.columnAtIndex(0).setXPlacement(objc2_app_kit::NSGridCellPlacement::Fill);
        grid.columnAtIndex(1).setXPlacement(if self.surface.is_none() {
            objc2_app_kit::NSGridCellPlacement::Leading
        } else {
            objc2_app_kit::NSGridCellPlacement::Trailing
        });
        if self.surface.is_none() {
            grid.columnAtIndex(1).setWidth(240.0);
        }
        self.rows.push(row);
        self
    }
}
impl NativeView for SettingsGroup {
    fn ns_view(&self) -> &NSView {
        self.surface.as_ref().map_or(self.grid.ns_view(), NativeView::ns_view)
    }
}

pub struct Section {
    ui: Ui,
    stack: VStack,
    group: Option<SettingsGroup>,
    plain: bool,
}
impl Section {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let mut stack = VStack::new(ui).spacing(6.0);
        if !title.is_empty() {
            stack = stack.push(SectionTitle::new(ui, title));
        }
        Self {
            ui: *ui,
            stack,
            group: None,
            plain: false,
        }
    }

    /// An aligned native preferences form without a grouped card or row dividers.
    pub fn form(ui: &Ui, title: &str) -> Self {
        Self {
            plain: true,
            ..Self::new(ui, title)
        }
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
            let group = if self.plain {
                SettingsGroup::plain(&self.ui)
            } else {
                SettingsGroup::new(&self.ui)
            };
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
    root: View,
    scroll_bottom: Retained<objc2_app_kit::NSLayoutConstraint>,
    width: Retained<objc2_app_kit::NSLayoutConstraint>,
    footer: Option<Box<dyn NativeView>>,
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
        let width = child.widthAnchor().constraintLessThanOrEqualToConstant(640.0);
        width.setActive(true);
        let fill = child.widthAnchor().constraintEqualToAnchor(&parent.widthAnchor());
        fill.setPriority(750.0);
        fill.setActive(true);
        let scroll = ScrollView::new(ui, outer);
        scroll.ns_scroll_view().setDrawsBackground(true);
        scroll.ns_scroll_view().setBackgroundColor(&Color::window_background());
        scroll.fit_width();
        let root = View::new(ui);
        root.ns_view().addSubview(scroll.ns_view());
        crate::view::prepare(scroll.ns_view());
        let parent = root.ns_view();
        let child = scroll.ns_view();
        child.topAnchor().constraintEqualToAnchor(&parent.topAnchor()).setActive(true);
        child
            .leadingAnchor()
            .constraintEqualToAnchor(&parent.leadingAnchor())
            .setActive(true);
        child
            .trailingAnchor()
            .constraintEqualToAnchor(&parent.trailingAnchor())
            .setActive(true);
        let scroll_bottom = child.bottomAnchor().constraintEqualToAnchor(&parent.bottomAnchor());
        scroll_bottom.setActive(true);
        Self {
            ui: *ui,
            scroll,
            content,
            top,
            root,
            scroll_bottom,
            width,
            footer: None,
        }
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

    /// Adjust the rhythm of a compact visual editor without changing other panes.
    pub fn content_spacing(self, spacing: f64) -> Self {
        self.content.ns_stack_view().setSpacing(spacing);
        self
    }

    /// Collections and visual editors need more room than short preference forms.
    pub fn content_width(self, width: f64) -> Self {
        self.width.setConstant(width);
        self
    }

    pub fn subtitle(self, text: &str) -> Self {
        self.content.add(SecondaryLabel::new(&self.ui, text));
        self
    }

    pub fn section(self, section: impl NativeView) -> Self {
        self.content.add(section);
        self
    }

    /// Keep collection actions reachable while the page content scrolls.
    pub fn bottom_bar(mut self, content: impl NativeView) -> Self {
        self.scroll_bottom.setActive(false);
        let row = VStack::new(&self.ui)
            .insets(Insets {
                top: 6.0,
                left: Metrics::PAGE_INSET,
                bottom: 6.0,
                right: Metrics::PAGE_INSET,
            })
            .push(content);
        let footer = VStack::new(&self.ui).spacing(0.0).push(Divider::new(&self.ui)).push(row);
        let view = footer.ns_view();
        crate::view::prepare(view);
        self.root.ns_view().addSubview(view);
        view.bottomAnchor()
            .constraintEqualToAnchor(&self.root.ns_view().safeAreaLayoutGuide().bottomAnchor())
            .setActive(true);
        view.centerXAnchor()
            .constraintEqualToAnchor(&self.root.ns_view().centerXAnchor())
            .setActive(true);
        view.widthAnchor().constraintLessThanOrEqualToConstant(580.0).setActive(true);
        view.widthAnchor()
            .constraintLessThanOrEqualToAnchor(&self.root.ns_view().widthAnchor())
            .setActive(true);
        let fill = view.widthAnchor().constraintEqualToAnchor(&self.root.ns_view().widthAnchor());
        fill.setPriority(750.0);
        fill.setActive(true);
        self.scroll
            .ns_view()
            .bottomAnchor()
            .constraintEqualToAnchor(&view.topAnchor())
            .setActive(true);
        self.footer = Some(Box::new(footer));
        self
    }

    pub fn ns_scroll_view(&self) -> &objc2_app_kit::NSScrollView { self.scroll.ns_scroll_view() }
}
impl NativeView for SettingsPage {
    fn ns_view(&self) -> &NSView { self.root.ns_view() }
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
