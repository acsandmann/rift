use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadOnly, Message};
use objc2_app_kit::*;
use objc2_foundation::{
    NSArray, NSNumber, NSNumberFormatter, NSNumberFormatterStyle, NSObjectProtocol, NSString,
};

use crate::bridge::ActionTarget;
use crate::{HStack, NativeControl, NativeView, Ui};

macro_rules! native_control {
    ($wrapper:ident, $native:ident, $accessor:ident) => {
        impl $wrapper {
            pub fn $accessor(&self) -> &$native { &self.native }
        }
        impl NativeView for $wrapper {
            fn ns_view(&self) -> &NSView { &self.native }
        }
        impl NativeControl for $wrapper {
            fn ns_control(&self) -> &NSControl { &self.native }
        }
    };
}
pub(crate) use native_control;

// The glass bezel and NSGlassEffectView were introduced together in macOS 26.
// Check runtime availability so older systems retain their standard push buttons.
pub(crate) fn action_button_bezel() -> NSBezelStyle {
    if objc2::runtime::AnyClass::get(c"NSGlassEffectView").is_some() {
        NSBezelStyle::Glass
    } else {
        NSBezelStyle::Push
    }
}

pub struct Button {
    native: Retained<NSButton>,
    target: Retained<ActionTarget>,
}
impl Button {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let native = NSButton::new(ui.mtm());
        native.setTitle(&NSString::from_str(title));
        native.setBezelStyle(action_button_bezel());
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    pub fn on_click(self, mut f: impl FnMut() + 'static) -> Self {
        self.target.set(move |_| f());
        self
    }

    pub fn key_equivalent(self, value: &str) -> Self {
        self.native.setKeyEquivalent(&NSString::from_str(value));
        self.native.setKeyEquivalentModifierMask(NSEventModifierFlags::empty());
        self
    }

    pub fn set_title(&self, title: &str) { self.native.setTitle(&NSString::from_str(title)); }

    pub fn borderless(self) -> Self {
        self.native.setBordered(false);
        self
    }

    pub fn disclosure(self) -> Self {
        self.native.setBezelStyle(NSBezelStyle::Disclosure);
        self.native.setButtonType(NSButtonType::PushOnPushOff);
        self
    }

    pub fn symbol_with_title(self, name: &str) -> Self {
        let button = self.symbol(name);
        button.native.setImagePosition(NSCellImagePosition::ImageLeading);
        button
    }

    pub fn symbol(self, name: &str) -> Self {
        if let Some(image) = crate::Symbol::named(name) {
            self.native.setImage(Some(&image));
            self.native.setImagePosition(NSCellImagePosition::ImageOnly);
        }
        self
    }
}
native_control!(Button, NSButton, ns_button);
pub struct IconButton(Button);
impl IconButton {
    pub fn new(ui: &Ui, symbol: &str, label: &str) -> Self {
        let button = Button::new(ui, label).symbol(symbol);
        button.ns_button().setBezelStyle(NSBezelStyle::AccessoryBar);
        button.ns_button().setControlSize(NSControlSize::Small);
        button.accessibility_label(label);
        Self(button)
    }

    pub fn on_click(self, f: impl FnMut() + 'static) -> Self { Self(self.0.on_click(f)) }

    pub fn ns_button(&self) -> &NSButton { self.0.ns_button() }
}
impl NativeView for IconButton {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
impl NativeControl for IconButton {
    fn ns_control(&self) -> &NSControl { self.0.ns_control() }
}
pub type SymbolButton = IconButton;
pub struct HelpButton(Button);
impl HelpButton {
    pub fn new(ui: &Ui) -> Self {
        let b = Button::new(ui, "Help");
        b.native.setBezelStyle(NSBezelStyle::HelpButton);
        Self(b)
    }

    pub fn on_click(self, f: impl FnMut() + 'static) -> Self { Self(self.0.on_click(f)) }

    pub fn ns_button(&self) -> &NSButton { self.0.ns_button() }
}
impl NativeView for HelpButton {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}

pub struct Switch {
    native: Retained<NSSwitch>,
    target: Retained<ActionTarget>,
}
impl Switch {
    pub fn new(ui: &Ui) -> Self {
        let native = NSSwitch::new(ui.mtm());
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    pub fn value(self, value: bool) -> Self {
        self.set_value(value);
        self
    }

    pub fn set_value(&self, value: bool) {
        if (self.native.state() == NSControlStateValueOn) == value {
            return;
        }
        self.native.setState(if value {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }

    pub fn get_value(&self) -> bool { self.native.state() == NSControlStateValueOn }

    pub fn on_change(self, mut f: impl FnMut(bool) + 'static) -> Self {
        self.target.set(move |sender| {
            if let Some(control) = sender.downcast_ref::<NSSwitch>() {
                f(control.state() == NSControlStateValueOn);
            }
        });
        self
    }
}
native_control!(Switch, NSSwitch, ns_switch);

pub struct Checkbox {
    native: Retained<NSButton>,
    target: Retained<ActionTarget>,
}
impl Checkbox {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let native = NSButton::new(ui.mtm());
        native.setTitle(&NSString::from_str(title));
        native.setButtonType(NSButtonType::Switch);
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    pub fn set_value(&self, value: bool) {
        if (self.native.state() == NSControlStateValueOn) == value {
            return;
        }
        self.native.setState(if value {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }

    pub fn value(self, value: bool) -> Self {
        self.set_value(value);
        self
    }

    pub fn on_change(self, mut f: impl FnMut(bool) + 'static) -> Self {
        self.target.set(move |sender| {
            if let Some(control) = sender.downcast_ref::<NSButton>() {
                f(control.state() == NSControlStateValueOn);
            }
        });
        self
    }
}
native_control!(Checkbox, NSButton, ns_button);

pub(crate) fn header_menu_style(button: &NSPopUpButton) {
    button.setControlSize(NSControlSize::Regular);
    button.setBezelStyle(action_button_bezel());
    button.setFont(Some(&crate::Font::section_title()));
    if let Some(menu) = button.menu() {
        unsafe {
            menu.setFont(Some(&crate::Font::body()));
        }
    }
}

pub struct Popup {
    native: Retained<NSPopUpButton>,
    target: Retained<ActionTarget>,
    menu: Option<crate::Menu>,
}
impl Popup {
    pub fn new(ui: &Ui) -> Self {
        let native = NSPopUpButton::new(ui.mtm());
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target, menu: None }
    }

    /// Native pull-down menu for secondary collection actions.
    pub fn actions(ui: &Ui, label: &str, menu: crate::Menu) -> Self {
        let header = crate::MenuItem::new(ui, "");
        header.ns_menu_item().setImage(crate::Symbol::named("ellipsis").as_deref());
        menu.ns_menu().insertItem_atIndex(header.ns_menu_item(), 0);
        let mut popup = Self::new(ui);
        popup.native.setPullsDown(true);
        if let Some(cell) = popup.native.cell().and_then(|cell| cell.downcast::<NSPopUpButtonCell>().ok()) {
            cell.setArrowPosition(NSPopUpArrowPosition::NoArrow);
        }
        popup.native.setMenu(Some(menu.ns_menu()));
        popup.native.setControlSize(NSControlSize::Small);
        popup.accessibility_label(label);
        popup.menu = Some(menu);
        popup
    }

    pub fn toolbar_style(self) -> Self {
        header_menu_style(&self.native);
        self
    }

    pub fn items(self, values: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        self.set_items(values);
        self
    }

    pub fn set_items(&self, values: impl IntoIterator<Item = impl AsRef<str>>) {
        let values: Vec<_> = values.into_iter().collect();
        let titles = self.native.itemTitles();
        if titles.len() == values.len()
            && titles
                .iter()
                .zip(&values)
                .all(|(title, value)| title.to_string() == value.as_ref())
        {
            return;
        }
        self.native.removeAllItems();
        for value in values {
            // addItemWithTitle merges duplicate titles, which breaks index-based choices.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.native.mtm()),
                    &NSString::from_str(value.as_ref()),
                    None,
                    &NSString::from_str(""),
                )
            };
            self.native.menu().unwrap().addItem(&item);
        }
    }

    /// Let AppKit render a native menu badge beside a choice.
    pub fn set_item_badge(&self, index: usize, label: Option<&str>) {
        use objc2::AnyThread;
        use objc2_foundation::NSObjectProtocol;
        let Some(item) = self.native.itemAtIndex(index as isize) else {
            return;
        };
        if !item.respondsToSelector(objc2::sel!(setBadge:)) {
            return;
        }
        if item
            .badge()
            .and_then(|badge| badge.stringValue())
            .map(|s| s.to_string())
            .as_deref()
            == label
        {
            return;
        }
        let badge = label.map(|label| {
            objc2_app_kit::NSMenuItemBadge::initWithString(
                objc2_app_kit::NSMenuItemBadge::alloc(),
                &NSString::from_str(label),
            )
        });
        item.setBadge(badge.as_deref());
    }

    pub fn set_selected(&self, index: usize) {
        if self.native.indexOfSelectedItem() != index as isize {
            self.native.selectItemAtIndex(index as isize);
        }
    }

    pub fn selected(&self) -> Option<usize> { usize::try_from(self.native.indexOfSelectedItem()).ok() }

    pub fn on_change(self, mut f: impl FnMut(usize) + 'static) -> Self {
        self.target.set(move |sender| {
            if let Some(control) = sender.downcast_ref::<NSPopUpButton>() {
                if let Ok(index) = usize::try_from(control.indexOfSelectedItem()) {
                    f(index);
                }
            }
        });
        self
    }
}
native_control!(Popup, NSPopUpButton, ns_popup_button);

pub struct SegmentedControl {
    native: Retained<NSSegmentedControl>,
    target: Retained<ActionTarget>,
}
impl SegmentedControl {
    pub fn new(ui: &Ui, labels: &[&str]) -> Self {
        let native = NSSegmentedControl::new(ui.mtm());
        native.setSegmentCount(labels.len() as isize);
        for (index, label) in labels.iter().enumerate() {
            native.setLabel_forSegment(&NSString::from_str(label), index as isize);
        }
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    pub fn set_selected(&self, index: usize) {
        if self.native.selectedSegment() != index as isize {
            self.native.setSelectedSegment(index as isize);
        }
    }

    pub fn on_change(self, mut f: impl FnMut(usize) + 'static) -> Self {
        self.target.set(move |sender| {
            if let Some(control) = sender.downcast_ref::<NSSegmentedControl>() {
                if let Ok(index) = usize::try_from(control.selectedSegment()) {
                    f(index);
                }
            }
        });
        self
    }
}
native_control!(SegmentedControl, NSSegmentedControl, ns_segmented_control);

pub struct ComboBox {
    native: Retained<NSComboBox>,
    target: Retained<ActionTarget>,
}
impl ComboBox {
    pub fn new(ui: &Ui) -> Self {
        let native = NSComboBox::new(ui.mtm());
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    pub fn items(self, values: &[&str]) -> Self {
        let strings: Vec<_> = values.iter().map(|v| NSString::from_str(v)).collect();
        let objects: Vec<&AnyObject> = strings.iter().map(|v| &**v as &AnyObject).collect();
        unsafe {
            self.native.addItemsWithObjectValues(&NSArray::from_slice(&objects));
        }
        self
    }

    pub fn set_value(&self, value: &str) {
        if self.native.stringValue().to_string() != value {
            self.native.setStringValue(&NSString::from_str(value));
        }
    }

    pub fn on_change(self, mut f: impl FnMut(String) + 'static) -> Self {
        self.target.set(move |sender| {
            if let Some(control) = sender.downcast_ref::<NSComboBox>() {
                f(control.stringValue().to_string());
            }
        });
        self
    }
}
native_control!(ComboBox, NSComboBox, ns_combo_box);

pub struct RadioGroup {
    stack: crate::VStack,
    buttons: Vec<Retained<NSButton>>,
    targets: Vec<Retained<ActionTarget>>,
}
impl RadioGroup {
    pub fn new(ui: &Ui, labels: &[&str]) -> Self {
        let stack = crate::VStack::new(ui);
        let mut buttons = Vec::new();
        let mut targets = Vec::new();
        for title in labels {
            let native = NSButton::new(ui.mtm());
            native.setTitle(&NSString::from_str(title));
            native.setButtonType(NSButtonType::Radio);
            stack.ns_stack_view().addArrangedSubview(&native);
            let target = ActionTarget::new(ui);
            target.attach(&native);
            buttons.push(native);
            targets.push(target);
        }
        Self { stack, buttons, targets }
    }

    pub fn set_selected(&self, index: usize) {
        for (i, b) in self.buttons.iter().enumerate() {
            if (b.state() == NSControlStateValueOn) == (i == index) {
                continue;
            }
            b.setState(if i == index {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
    }

    pub fn on_change(self, f: impl FnMut(usize) + 'static) -> Self {
        use std::cell::RefCell;
        use std::rc::Rc;

        use objc2::rc::Weak;
        let f = Rc::new(RefCell::new(f));
        let buttons: Vec<_> = self.buttons.iter().map(|b| Weak::new(&**b)).collect();
        let buttons = Rc::new(buttons);
        for (index, target) in self.targets.iter().enumerate() {
            let f = f.clone();
            let buttons = buttons.clone();
            target.set(move |_| {
                for (i, b) in buttons.iter().enumerate() {
                    if let Some(b) = b.load() {
                        b.setState(if i == index {
                            NSControlStateValueOn
                        } else {
                            NSControlStateValueOff
                        });
                    }
                }
                (f.borrow_mut())(index);
            });
        }
        self
    }
}
impl NativeView for RadioGroup {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}

macro_rules! numeric_control {
    ($name:ident, $native:ident, $accessor:ident) => {
        pub struct $name {
            native: Retained<$native>,
            target: Retained<ActionTarget>,
        }
        impl $name {
            pub fn new(ui: &Ui) -> Self {
                let native = $native::new(ui.mtm());
                let target = ActionTarget::new(ui);
                target.attach(&native);
                Self { native, target }
            }

            pub fn range(self, min: f64, max: f64) -> Self {
                self.set_range(min, max);
                self
            }

            pub fn set_range(&self, min: f64, max: f64) {
                self.native.setMinValue(min);
                self.native.setMaxValue(max);
            }

            /// Report every change while tracking, rather than only on release.
            pub fn continuous(self, value: bool) -> Self {
                self.native.setContinuous(value);
                self
            }

            pub fn value(self, value: f64) -> Self {
                self.set_value(value);
                self
            }

            pub fn set_value(&self, value: f64) {
                if self.native.doubleValue() != value {
                    self.native.setDoubleValue(value);
                }
            }

            pub fn get_value(&self) -> f64 { self.native.doubleValue() }

            pub fn on_change(self, mut f: impl FnMut(f64) + 'static) -> Self {
                self.target.set(move |sender| {
                    if let Some(control) = sender.downcast_ref::<$native>() {
                        f(control.doubleValue());
                    }
                });
                self
            }
        }
        native_control!($name, $native, $accessor);
    };
}
numeric_control!(Slider, NSSlider, ns_slider);
numeric_control!(Stepper, NSStepper, ns_stepper);
impl Stepper {
    pub fn increment(self, value: f64) -> Self {
        self.native.setIncrement(value);
        self
    }
}

fn formatted_number(formatter: &NSNumberFormatter, text: &NSString) -> Option<f64> {
    let value = formatter.numberFromString(text)?.doubleValue();
    if !value.is_finite()
        || (!formatter.allowsFloats() && value.fract() != 0.0)
        || formatter.minimum().is_some_and(|min| value < min.doubleValue())
        || formatter.maximum().is_some_and(|max| value > max.doubleValue())
    {
        None
    } else {
        Some(value)
    }
}

pub struct NumberField {
    field: crate::TextField,
    formatter: Retained<NSNumberFormatter>,
}
impl NumberField {
    pub fn with_validation(self, ui: &Ui) -> crate::Validated<Self> { crate::Validated::new(ui, self) }

    /// A leading-aligned value inside one bezel with a trailing unit, such as "pt".
    pub fn unit_field(&self, ui: &Ui, unit: &str) -> crate::UnitField {
        self.ns_text_field().setAlignment(NSTextAlignment::Left);
        crate::UnitField::new(ui, self.ns_text_field(), unit)
    }

    pub fn new(ui: &Ui) -> Self {
        let field = crate::TextField::new(ui);
        let formatter = NSNumberFormatter::new();
        formatter.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
        formatter.setUsesGroupingSeparator(false);
        formatter.setMaximumFractionDigits(6);
        field.ns_text_field().setFormatter(Some(&formatter));
        Self { field, formatter }
    }

    pub fn integer(self) -> Self {
        self.formatter.setAllowsFloats(false);
        self.formatter.setMaximumFractionDigits(0);
        self
    }

    pub fn range(self, min: f64, max: f64) -> Self {
        assert!(min.is_finite() && max.is_finite() && min <= max);
        self.formatter.setMinimum(Some(&NSNumber::new_f64(min)));
        self.formatter.setMaximum(Some(&NSNumber::new_f64(max)));
        self
    }

    pub fn value(self, value: f64) -> Self {
        self.set_value(value);
        self
    }

    pub fn set_value(&self, value: f64) {
        if self.get_value() != Some(value) {
            self.field.ns_text_field().setDoubleValue(value);
        }
    }

    pub fn get_value(&self) -> Option<f64> {
        formatted_number(&self.formatter, &self.field.ns_text_field().stringValue())
    }

    pub fn on_change(mut self, mut f: impl FnMut(f64) + 'static) -> Self {
        let formatter = self.formatter.clone();
        self.field = self.field.on_commit(move |text| {
            if let Some(value) = formatted_number(&formatter, &NSString::from_str(&text)) {
                f(value);
            }
        });
        self
    }

    /// Update a local draft as valid numeric input is entered.
    pub fn on_edit(mut self, mut f: impl FnMut(f64) + 'static) -> Self {
        let formatter = self.formatter.clone();
        self.field = self.field.on_change(move |text| {
            if let Some(value) = formatted_number(&formatter, &NSString::from_str(&text)) {
                f(value);
            }
        });
        self
    }

    pub fn ns_text_field(&self) -> &NSTextField { self.field.ns_text_field() }

    pub fn ns_number_formatter(&self) -> &NSNumberFormatter { &self.formatter }

    pub fn set_validation(&self, validation: &crate::Validation) { self.field.set_validation(validation); }
}
impl NativeView for NumberField {
    fn ns_view(&self) -> &NSView { self.field.ns_view() }
}
impl NativeControl for NumberField {
    fn ns_control(&self) -> &NSControl { self.field.ns_control() }
}

pub struct NumberStepper {
    stack: HStack,
    field: NumberField,
    stepper: Stepper,
}
impl NumberStepper {
    pub fn new(ui: &Ui, min: f64, max: f64, increment: f64) -> Self {
        use objc2::rc::Weak;
        let field = NumberField::new(ui).range(min, max);
        let weak = Weak::new(field.ns_text_field());
        let stepper = Stepper::new(ui).range(min, max).increment(increment).on_change(move |value| {
            if let Some(field) = weak.load() {
                field.setDoubleValue(value);
            }
        });
        let weak = Weak::new(stepper.ns_stepper());
        let field = field.on_change(move |value| {
            if let Some(stepper) = weak.load() {
                stepper.setDoubleValue(value);
            }
        });
        let stack = HStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(field.ns_view());
        stack.ns_stack_view().addArrangedSubview(stepper.ns_view());
        Self { stack, field, stepper }
    }

    pub fn integer(mut self) -> Self {
        self.field = self.field.integer();
        self
    }

    pub fn accessibility_label(&self, label: &str) {
        self.field.accessibility_label(label);
        self.stepper.accessibility_label(label);
    }

    pub fn set_value(&self, value: f64) {
        self.field.set_value(value);
        self.stepper.set_value(value);
    }

    pub fn on_change(mut self, mut f: impl FnMut(f64) + 'static) -> Self {
        use std::cell::RefCell;
        use std::rc::Rc;

        use objc2::rc::Weak;
        let cb = Rc::new(RefCell::new(move |v| f(v)));
        let weak = Weak::new(self.stepper.ns_stepper());
        let c = cb.clone();
        self.field = self.field.on_change(move |v| {
            if let Some(s) = weak.load() {
                s.setDoubleValue(v);
            }
            (c.borrow_mut())(v);
        });
        let weak = Weak::new(self.field.ns_text_field());
        self.stepper = self.stepper.on_change(move |v| {
            if let Some(s) = weak.load() {
                s.setDoubleValue(v);
            }
            (cb.borrow_mut())(v);
        });
        self
    }

    pub fn ns_text_field(&self) -> &NSTextField { self.field.ns_text_field() }

    pub fn ns_stepper(&self) -> &NSStepper { self.stepper.ns_stepper() }
}
impl NativeView for NumberStepper {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}

pub struct ColorWell {
    native: Retained<NSColorWell>,
    target: Retained<ActionTarget>,
}
impl ColorWell {
    pub fn new(ui: &Ui) -> Self {
        let native = NSColorWell::new(ui.mtm());
        native.setContinuous(false);
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    /// Show an sRGB `[red, green, blue, alpha]` color.
    pub fn set_value(&self, [r, g, b, a]: [f64; 4]) {
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
        if !self.native.color().isEqual(Some(&color)) {
            self.native.setColor(&color);
        }
    }

    /// Receive the chosen color as sRGB `[red, green, blue, alpha]`.
    pub fn on_change(self, mut f: impl FnMut([f64; 4]) + 'static) -> Self {
        self.target.set(move |sender| {
            let color = sender.downcast_ref::<NSColorWell>().and_then(|control| {
                control
                    .color()
                    .colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace())
            });
            if let Some(c) = color {
                f([
                    c.redComponent(),
                    c.greenComponent(),
                    c.blueComponent(),
                    c.alphaComponent(),
                ]);
            }
        });
        self
    }
}
native_control!(ColorWell, NSColorWell, ns_color_well);

pub struct AddRemoveControl {
    stack: HStack,
    add: IconButton,
    remove: IconButton,
}
impl AddRemoveControl {
    pub fn new(ui: &Ui) -> Self {
        let add = IconButton::new(ui, "plus", "Add");
        let remove = IconButton::new(ui, "minus", "Remove");
        for button in [&add, &remove] {
            button.ns_button().setBordered(false);
            button.width(28.0);
            button.height(24.0);
        }
        let divider = NSBox::new(ui.mtm());
        divider.setBoxType(NSBoxType::Separator);
        divider.widthAnchor().constraintEqualToConstant(1.0).setActive(true);
        divider.heightAnchor().constraintEqualToConstant(16.0).setActive(true);
        let stack = HStack::new(ui)
            .spacing(0.0)
            .push(add.ns_view().retain())
            .push(divider.into_super())
            .push(remove.ns_view().retain());
        Self { stack, add, remove }
    }

    pub fn on_add(mut self, f: impl FnMut() + 'static) -> Self {
        self.add = self.add.on_click(f);
        self
    }

    pub fn on_remove(mut self, f: impl FnMut() + 'static) -> Self {
        self.remove = self.remove.on_click(f);
        self
    }

    pub fn set_remove_enabled(&self, value: bool) { self.remove.set_enabled(value); }

    pub(crate) fn show_add(&self, visible: bool) {
        self.add.set_hidden(!visible);
        self.stack
            .ns_stack_view()
            .arrangedSubviews()
            .objectAtIndex(1)
            .setHidden(!visible);
    }

    pub(crate) fn remove_button(&self) -> &NSButton { self.remove.ns_button() }
}
impl NativeView for AddRemoveControl {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}

/// Native trailing sheet actions. Cancel is visible only while the draft differs
/// from its initial value; clients report draft changes without managing buttons.
pub struct SheetActions {
    row: HStack,
    done: std::rc::Rc<Button>,
    cancel: std::rc::Rc<Button>,
}
impl SheetActions {
    pub fn new(ui: &Ui) -> Self {
        let done = std::rc::Rc::new(Button::new(ui, "Done").key_equivalent("\r"));
        let cancel = std::rc::Rc::new(Button::new(ui, "Cancel").key_equivalent("\u{1b}"));
        cancel.set_hidden(true);
        cancel.identifier("cgs.sheet-cancel");
        done.identifier("cgs.sheet-primary");
        let row = HStack::new(ui).spacing(8.0).spacer(ui).push(cancel.clone()).push(done.clone());
        Self { row, done, cancel }
    }

    pub fn set_changed(&self, changed: bool) { self.cancel.set_hidden(!changed); }

    pub fn set_primary_title(&self, title: &str) { self.done.set_title(title); }

    pub fn set_on_done(&self, mut f: impl FnMut() + 'static) { self.done.target.set(move |_| f()); }

    pub fn set_on_cancel(&self, mut f: impl FnMut() + 'static) { self.cancel.target.set(move |_| f()); }
}
impl NativeView for SheetActions {
    fn ns_view(&self) -> &NSView { self.row.ns_view() }
}
