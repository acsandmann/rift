//! Grouped settings navigation backed by AppKit's source-list selection.
use cgs::*;
use objc2::rc::Retained;
use objc2_app_kit::{NSAccessibility, NSColor, NSLayoutConstraint, NSTableCellView, NSView};
use objc2_foundation::NSArray;

use super::{CATEGORIES, Category};

#[derive(Clone)]
enum Row {
    Spacer,
    Destination(Category),
}

pub(super) struct SettingsSidebar {
    table: Table<Row>,
}

impl SettingsSidebar {
    pub(super) fn new(ui: &Ui, mut select: impl FnMut(Category) + 'static) -> Self {
        let mut rows = Vec::new();
        for (category, _, _) in CATEGORIES {
            if matches!(category, Category::Keyboard | Category::Interface) {
                rows.push(Row::Spacer);
            }
            rows.push(Row::Destination(category));
        }
        let ui_copy = *ui;
        let table = Table::new(ui)
            .source_list()
            .column("destination", "", 0.0)
            .cells(move |row, _| match row {
                Row::Spacer => Box::new(View::new(&ui_copy)),
                Row::Destination(category) => Box::new(destination_cell(&ui_copy, *category)),
            })
            .selectable(|row| matches!(row, Row::Destination(_)))
            .row_heights(|row| {
                if matches!(row, Row::Spacer) {
                    12.0
                } else {
                    32.0
                }
            })
            .rows(rows)
            .on_select_item(move |row| {
                if let Some(Row::Destination(category)) = row {
                    select(category);
                }
            });
        table.ns_table_view().setIntercellSpacing(CGSize::new(0.0, 0.0));
        table.ns_table_view().setBackgroundColor(&NSColor::clearColor());
        table.min_width(210.0);
        // Prefer a 212-point search field plus its side insets; yield in compact windows.
        let width = table.ns_view().widthAnchor().constraintEqualToConstant(232.0);
        width.setPriority(499.0);
        width.setActive(true);
        table.max_width(280.0);
        table.identifier("rift.settings.navigation");
        table
            .ns_table_view()
            .setAccessibilityLabel(Some(&objc2_foundation::NSString::from_str(
                "Settings categories",
            )));
        Self { table }
    }

    pub(super) fn set_selected(&self, category: Category) {
        let index = (0..self.table.ns_table_view().numberOfRows() as usize).find(
            |&index| matches!(self.table.row(index), Some(Row::Destination(id)) if id == category),
        );
        self.table.set_selected(index);
    }
}

impl NativeView for SettingsSidebar {
    fn ns_view(&self) -> &NSView { self.table.ns_view() }
}

struct DestinationCell(Retained<NSTableCellView>);
impl NativeView for DestinationCell {
    fn ns_view(&self) -> &NSView { &self.0 }
}

fn destination_cell(ui: &Ui, category: Category) -> DestinationCell {
    let (_, title, symbol) = CATEGORIES[category.index()];
    let native = NSTableCellView::new(ui.mtm());
    let label = Label::new(ui, title);
    label.ns_view().setTranslatesAutoresizingMaskIntoConstraints(false);
    native.addSubview(label.ns_view());
    // Let AppKit manage text contrast in selected and inactive rows.
    unsafe {
        native.setTextField(Some(label.ns_text_field()));
    }
    let badge = View::new(ui);
    badge.width(20.0);
    badge.height(20.0);
    badge.ns_view().setWantsLayer(true);
    // Keep category cues subtle; the native selection is the sidebar's strong accent.
    let (red, green, blue) = match category {
        Category::General | Category::Advanced | Category::About => (0.38, 0.40, 0.43),
        Category::Layouts => (0.32, 0.42, 0.52),
        Category::Workspaces => (0.40, 0.41, 0.51),
        Category::Rules => (0.49, 0.43, 0.35),
        Category::Keyboard | Category::Mouse => (0.40, 0.41, 0.46),
        Category::Interface => (0.34, 0.44, 0.46),
    };
    let color = NSColor::colorWithSRGBRed_green_blue_alpha(red, green, blue, 1.0);
    if let Some(layer) = badge.ns_view().layer() {
        layer.setBackgroundColor(Some(&color.CGColor()));
        layer.setCornerRadius(5.0);
        layer.setBorderWidth(0.5);
        layer.setBorderColor(Some(&NSColor::colorWithWhite_alpha(1.0, 0.10).CGColor()));
    }
    if let Some(image) = ImageView::symbol(ui, symbol) {
        image
            .ns_image_view()
            .setContentTintColor(Some(&NSColor::colorWithWhite_alpha(0.94, 1.0)));
        image.width(14.0);
        image.height(14.0);
        badge.ns_view().addSubview(image.ns_view());
        image.ns_view().setTranslatesAutoresizingMaskIntoConstraints(false);
        image
            .ns_view()
            .centerXAnchor()
            .constraintEqualToAnchor(&badge.ns_view().centerXAnchor())
            .setActive(true);
        image
            .ns_view()
            .centerYAnchor()
            .constraintEqualToAnchor(&badge.ns_view().centerYAnchor())
            .setActive(true);
    }
    native.addSubview(badge.ns_view());
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        badge
            .ns_view()
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&native.leadingAnchor(), 2.0),
        badge.ns_view().centerYAnchor().constraintEqualToAnchor(&native.centerYAnchor()),
        label
            .ns_view()
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&badge.ns_view().trailingAnchor(), 8.0),
        label
            .ns_view()
            .trailingAnchor()
            .constraintEqualToAnchor_constant(&native.trailingAnchor(), -4.0),
        label.ns_view().centerYAnchor().constraintEqualToAnchor(&native.centerYAnchor()),
    ]));
    DestinationCell(native)
}
