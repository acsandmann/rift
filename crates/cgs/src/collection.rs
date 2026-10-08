use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{
    NSArray, NSIndexSet, NSNotification, NSNumber, NSObject, NSObjectProtocol, NSString,
};

use crate::bridge::{ActionTarget, callback};
use crate::{AddRemoveControl, Label, NativeControl, NativeView, ScrollView, Ui, VStack};

// The native reuse pool owns the Rust content and its action targets, not the datasource.
// A legacy arbitrary cell factory can replace content; reusable factories configure it in place.
define_class!(
    #[unsafe(super(NSTableCellView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiCollectionCell"]
    #[ivars = RefCell<Option<Box<dyn NativeView>>>]
    struct CollectionCell;
);
impl CollectionCell {
    fn new(ui: &Ui) -> Retained<Self> {
        unsafe {
            msg_send![
                super(Self::alloc(ui.mtm()).set_ivars(RefCell::new(None))),
                init
            ]
        }
    }

    fn set_content(&self, content: Box<dyn NativeView>) {
        unsafe {
            self.setTextField(None);
            self.setImageView(None);
        }
        if let Some(old) = self.ivars().borrow_mut().take() {
            old.ns_view().removeFromSuperview();
        }
        self.addSubview(content.ns_view());
        crate::view::pin(self, content.ns_view(), crate::Insets {
            top: 0.0,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });
        // Expose outlets to AppKit's native selected-cell appearance.
        let cell = content.ns_view().downcast_ref::<NSTableCellView>();
        unsafe {
            self.setTextField(cell.and_then(|cell| cell.textField()).as_deref());
            self.setImageView(cell.and_then(|cell| cell.imageView()).as_deref());
        }
        *self.ivars().borrow_mut() = Some(content);
    }
}

fn drag_type() -> Retained<NSString> { NSString::from_str("org.cgs.local-row") }
type CellFactory = Box<dyn FnMut(usize, usize) -> Box<dyn NativeView>>;
type NativeCellFactory = Box<dyn FnMut(&NSTableView, usize, usize) -> Retained<NSView>>;
type Selection = Box<dyn FnMut(Option<usize>)>;
type Reorder = Box<dyn FnMut(usize, usize)>;
struct Node {
    object: Retained<NSNumber>,
    children: Vec<usize>,
}
struct CollectionState {
    count: Box<dyn Fn() -> usize>,
    cell: RefCell<CellFactory>,
    native_cell: RefCell<Option<NativeCellFactory>>,
    selection: RefCell<Option<Selection>>,
    selectable: RefCell<Box<dyn Fn(usize) -> bool>>,
    reorder: RefCell<Option<Reorder>>,
    reorderable: Cell<bool>,
    group_parents: Cell<bool>,
    table_groups: RefCell<Box<dyn Fn(usize) -> bool>>,
    row_height: RefCell<Option<Box<dyn Fn(usize) -> f64>>>,
    roots: RefCell<Vec<usize>>,
    nodes: RefCell<Vec<Node>>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiCollectionBridge"]
    #[ivars = CollectionState]
    struct CollectionBridge;
    unsafe impl NSObjectProtocol for CollectionBridge {}
    unsafe impl NSControlTextEditingDelegate for CollectionBridge {}
    unsafe impl NSTableViewDataSource for CollectionBridge {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn count(&self, _table: &NSTableView) -> isize { (self.ivars().count)() as isize }

        #[unsafe(method_id(tableView:pasteboardWriterForRow:))]
        fn writer(
            &self,
            _table: &NSTableView,
            row: isize,
        ) -> Option<Retained<ProtocolObject<dyn NSPasteboardWriting>>> {
            if !self.ivars().reorderable.get() {
                None
            } else {
                let item = NSPasteboardItem::new();
                item.setString_forType(&NSString::from_str(&row.to_string()), &drag_type());
                Some(ProtocolObject::from_retained(item))
            }
        }

        #[unsafe(method(tableView:validateDrop:proposedRow:proposedDropOperation:))]
        fn validate_drop(
            &self,
            table: &NSTableView,
            info: &ProtocolObject<dyn NSDraggingInfo>,
            row: isize,
            operation: NSTableViewDropOperation,
        ) -> NSDragOperation {
            if operation == NSTableViewDropOperation::Above
                && row >= 0
                && row as usize <= (self.ivars().count)()
                && self.local_drag(table, info)
            {
                NSDragOperation::Move
            } else {
                NSDragOperation::None
            }
        }

        #[unsafe(method(tableView:acceptDrop:row:dropOperation:))]
        fn accept_drop(
            &self,
            table: &NSTableView,
            info: &ProtocolObject<dyn NSDraggingInfo>,
            row: isize,
            operation: NSTableViewDropOperation,
        ) -> bool {
            self.do_accept_drop(table, info, row, operation)
        }
    }
    unsafe impl NSTableViewDelegate for CollectionBridge {
        #[unsafe(method(tableView:isGroupRow:))]
        fn table_group(&self, _table: &NSTableView, row: isize) -> bool {
            (self.ivars().table_groups.borrow())(row as usize)
        }
        #[unsafe(method(tableView:heightOfRow:))]
        fn table_height(&self, table: &NSTableView, row: isize) -> f64 {
            self.ivars().row_height.borrow().as_ref().map_or(table.rowHeight(), |height| height(row as usize))
        }

        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn table_cell(
            &self,
            table: &NSTableView,
            column: Option<&NSTableColumn>,
            row: isize,
        ) -> Option<Retained<NSView>> {
            usize::try_from(row).ok().and_then(|row| {
                self.make_cell(
                    table,
                    row,
                    column
                        .map(|c| {
                            table
                                .tableColumns()
                                .iter()
                                .position(|v| std::ptr::eq(&*v, c))
                                .unwrap_or(0)
                        })
                        .unwrap_or(0),
                )
            })
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, note: &NSNotification) {
            if let Some(table) = note.object().and_then(|o| o.downcast::<NSTableView>().ok()) {
                self.select(usize::try_from(table.selectedRow()).ok());
            }
        }

        #[unsafe(method(tableView:shouldSelectRow:))]
        fn should_select_row(&self, _table: &NSTableView, row: isize) -> bool {
            let _keep_alive = self.retain();
            let mut result = false;
            callback(|| {
                result = usize::try_from(row)
                    .ok()
                    .is_some_and(|row| (self.ivars().selectable.borrow())(row))
            });
            result
        }
    }
    unsafe impl NSOutlineViewDataSource for CollectionBridge {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        unsafe fn children_count(&self, _view: &NSOutlineView, item: Option<&AnyObject>) -> isize {
            self.children(item).len() as isize
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        unsafe fn child(
            &self,
            _view: &NSOutlineView,
            index: isize,
            item: Option<&AnyObject>,
        ) -> Retained<AnyObject> {
            let child = self.children(item)[index as usize];
            self.ivars().nodes.borrow()[child].object.clone().into()
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        unsafe fn expandable(&self, _view: &NSOutlineView, item: &AnyObject) -> bool {
            !self.children(Some(item)).is_empty()
        }
    }
    unsafe impl NSOutlineViewDelegate for CollectionBridge {
        #[unsafe(method(outlineView:isGroupItem:))]
        fn is_group(&self, _view: &NSOutlineView, item: &AnyObject) -> bool {
            self.ivars().group_parents.get() && node_index(item).is_some_and(|index| {
                self.ivars().nodes.borrow().get(index).is_some_and(|node| !node.children.is_empty())
            })
        }

        #[unsafe(method(outlineView:shouldSelectItem:))]
        fn should_select_item(&self, _view: &NSOutlineView, item: &AnyObject) -> bool {
            !self.ivars().group_parents.get() || node_index(item).is_some_and(|index| {
                self.ivars().nodes.borrow().get(index).is_some_and(|node| node.children.is_empty())
            })
        }

        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        unsafe fn outline_cell(
            &self,
            view: &NSOutlineView,
            _column: Option<&NSTableColumn>,
            item: &AnyObject,
        ) -> Option<Retained<NSView>> {
            node_index(item).and_then(|row| self.make_cell(view, row, 0))
        }

        #[unsafe(method(outlineViewSelectionDidChange:))]
        fn outline_selection(&self, note: &NSNotification) {
            if let Some(view) = note.object().and_then(|o| o.downcast::<NSOutlineView>().ok()) {
                let row = view.selectedRow();
                let index = if row < 0 {
                    None
                } else {
                    view.itemAtRow(row).as_deref().and_then(node_index)
                };
                self.select(index);
            }
        }
    }
);
fn node_index(item: &AnyObject) -> Option<usize> {
    item.downcast_ref::<NSNumber>()
        .and_then(|n| usize::try_from(n.unsignedLongLongValue()).ok())
}
impl CollectionBridge {
    fn do_accept_drop(
        &self,
        table: &NSTableView,
        info: &ProtocolObject<dyn NSDraggingInfo>,
        row: isize,
        operation: NSTableViewDropOperation,
    ) -> bool {
        let _keep_alive = self.retain();
        if operation != NSTableViewDropOperation::Above || !self.local_drag(table, info) {
            return false;
        }
        let Some(from) = info
            .draggingPasteboard()
            .stringForType(&drag_type())
            .and_then(|s| s.to_string().parse::<usize>().ok())
        else {
            return false;
        };
        let count = (self.ivars().count)();
        if from >= count || row < 0 || row as usize > count {
            return false;
        }
        let to = row as usize - usize::from(from < row as usize);
        if from == to {
            return true;
        }
        if let Ok(mut cb) = self.ivars().reorder.try_borrow_mut() {
            if let Some(cb) = cb.as_mut() {
                callback(|| cb(from, to));
            }
        }
        self.reload(table);
        table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(to), false);
        true
    }

    fn new(
        ui: &Ui,
        count: impl Fn() -> usize + 'static,
        cell: impl FnMut(usize, usize) -> Box<dyn NativeView> + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(ui.mtm()).set_ivars(CollectionState {
            count: Box::new(count),
            cell: RefCell::new(Box::new(cell)),
            native_cell: RefCell::new(None),
            selection: RefCell::new(None),
            selectable: RefCell::new(Box::new(|_| true)),
            reorder: RefCell::new(None),
            reorderable: Cell::new(false),
            group_parents: Cell::new(false),
            table_groups: RefCell::new(Box::new(|_| false)),
            row_height: RefCell::new(None),
            roots: RefCell::new(Vec::new()),
            nodes: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn make_cell(
        &self,
        table: &NSTableView,
        row: usize,
        column: usize,
    ) -> Option<Retained<NSView>> {
        let _keep_alive = self.retain();
        if row >= (self.ivars().count)() {
            return None;
        }
        let mut result = None;
        callback(|| {
            if let Some(make_cell) = self.ivars().native_cell.borrow_mut().as_mut() {
                result = Some(make_cell(table, row, column));
            } else {
                let identifier = NSString::from_str(&format!("cgs.cell.{column}"));
                let native = unsafe { table.makeViewWithIdentifier_owner(&identifier, None) }
                    .and_then(|view| view.downcast::<CollectionCell>().ok())
                    .unwrap_or_else(|| {
                        let cell = CollectionCell::new(&Ui::new(table.mtm()));
                        cell.setIdentifier(Some(&identifier));
                        cell
                    });
                native.set_content((self.ivars().cell.borrow_mut())(row, column));
                result = Some(native.into_super().into_super());
            }
        });
        result
    }

    fn reload(&self, table: &NSTableView) { table.reloadData(); }

    fn select(&self, index: Option<usize>) {
        let _keep_alive = self.retain();
        if let Ok(mut f) = self.ivars().selection.try_borrow_mut() {
            if let Some(f) = f.as_mut() {
                callback(|| f(index));
            }
        }
    }

    fn local_drag(&self, table: &NSTableView, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
        self.ivars().reorderable.get()
            && info
                .draggingSource()
                .is_some_and(|source| std::ptr::eq(&*source, table as &AnyObject))
    }

    fn children(&self, item: Option<&AnyObject>) -> Vec<usize> {
        match item {
            None => self.ivars().roots.borrow().clone(),
            Some(item) => node_index(item)
                .and_then(|i| self.ivars().nodes.borrow().get(i).map(|n| n.children.clone()))
                .unwrap_or_default(),
        }
    }
}

pub struct Table<T: 'static> {
    native: Retained<NSTableView>,
    scroll: ScrollView,
    bridge: Retained<CollectionBridge>,
    rows: Rc<RefCell<Vec<T>>>,
    double: Retained<ActionTarget>,
    menu: RefCell<Option<crate::Menu>>,
}
impl<T: 'static> Table<T> {
    pub fn new(ui: &Ui) -> Self {
        let native = NSTableView::new(ui.mtm());
        native.setAllowsEmptySelection(true);
        native.setAllowsMultipleSelection(false);
        native.setUsesAutomaticRowHeights(false);
        native.setStyle(NSTableViewStyle::Inset);
        native.setRowSizeStyle(NSTableViewRowSizeStyle::Custom);
        native.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
        );
        let rows = Rc::new(RefCell::new(Vec::new()));
        let r = rows.clone();
        let ui_copy = *ui;
        let bridge = CollectionBridge::new(
            ui,
            move || r.borrow().len(),
            move |_, _| Box::new(Label::new(&ui_copy, "")),
        );
        unsafe {
            native.setDataSource(Some(ProtocolObject::from_ref(&*bridge)));
            native.setDelegate(Some(ProtocolObject::from_ref(&*bridge)));
        }
        let double = ActionTarget::new(ui);
        unsafe {
            native.setTarget(Some(&double));
            native.setDoubleAction(Some(objc2::sel!(invoke:)));
        }
        let view: Retained<NSView> = native.clone().into_super().into_super();
        let scroll = ScrollView::new(ui, view);
        scroll.ns_scroll_view().setDrawsBackground(true);
        scroll.ns_scroll_view().setBorderType(NSBorderType::NoBorder);
        {
            let r = rows.clone();
            *bridge.ivars().reorder.borrow_mut() = Some(Box::new(move |from, to| {
                let value = r.borrow_mut().remove(from);
                r.borrow_mut().insert(to, value);
            }));
            Self {
                native,
                scroll,
                bridge,
                rows,
                double,
                menu: RefCell::new(None),
            }
        }
    }

    pub fn column(self, id: &str, title: &str, width: f64) -> Self {
        let column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(self.native.mtm()),
            &NSString::from_str(id),
        );
        column.setTitle(&NSString::from_str(title));
        if width > 0.0 {
            column.setWidth(width);
        }
        self.native.addTableColumn(&column);
        self
    }

    pub fn cells(self, mut f: impl FnMut(&T, usize) -> Box<dyn NativeView> + 'static) -> Self {
        self.cells_with_index(move |item, column, _| f(item, column))
    }

    pub fn cells_with_index(
        self,
        mut f: impl FnMut(&T, usize, usize) -> Box<dyn NativeView> + 'static,
    ) -> Self {
        let rows = self.rows.clone();
        *self.bridge.ivars().native_cell.borrow_mut() = None;
        *self.bridge.ivars().cell.borrow_mut() =
            Box::new(move |row, column| f(&rows.borrow()[row], column, row));
        self.bridge.reload(&self.native);
        self
    }

    pub fn set_rows_if_changed(&self, rows: Vec<T>)
    where T: PartialEq {
        if *self.rows.borrow() != rows {
            self.set_rows(rows);
        }
    }

    pub fn rows(self, rows: Vec<T>) -> Self {
        self.set_rows(rows);
        self
    }

    pub fn set_rows(&self, rows: Vec<T>) {
        *self.rows.borrow_mut() = rows;
        self.bridge.reload(&self.native);
    }

    pub fn selection(&self) -> Option<usize> { usize::try_from(self.native.selectedRow()).ok() }

    pub fn set_selected(&self, index: Option<usize>) {
        if self.selection() == index {
            return;
        }
        if let Some(index) = index.filter(|i| *i < self.rows.borrow().len()) {
            self.native.selectRowIndexes_byExtendingSelection(
                &NSIndexSet::indexSetWithIndex(index),
                false,
            );
        } else {
            unsafe {
                self.native.deselectAll(None);
            }
        }
    }

    pub fn on_select_item(self, mut f: impl FnMut(Option<T>) + 'static) -> Self
    where T: Clone {
        let rows = self.rows.clone();
        *self.bridge.ivars().selection.borrow_mut() = Some(Box::new(move |index| {
            let item = index.and_then(|i| rows.borrow().get(i).cloned());
            f(item);
        }));
        self
    }

    pub fn on_select(self, f: impl FnMut(Option<usize>) + 'static) -> Self {
        self.set_on_select(f);
        self
    }

    pub fn set_on_select(&self, f: impl FnMut(Option<usize>) + 'static) {
        *self.bridge.ivars().selection.borrow_mut() = Some(Box::new(f));
    }

    /// Use AppKit source-list section rows and intrinsic per-item heights.
    pub fn group_rows(self, group: impl Fn(&T) -> bool + 'static) -> Self {
        let rows = self.rows.clone();
        *self.bridge.ivars().table_groups.borrow_mut() = Box::new(move |row| group(&rows.borrow()[row]));
        self
    }
    pub fn row_heights(self, height: impl Fn(&T) -> f64 + 'static) -> Self {
        let rows = self.rows.clone();
        *self.bridge.ivars().row_height.borrow_mut() = Some(Box::new(move |row| height(&rows.borrow()[row])));
        self
    }

    pub fn selectable(self, f: impl Fn(&T) -> bool + 'static) -> Self {
        let rows = self.rows.clone();
        *self.bridge.ivars().selectable.borrow_mut() =
            Box::new(move |row| rows.borrow().get(row).is_some_and(&f));
        self
    }

    pub fn on_double_click(self, mut f: impl FnMut(usize) + 'static) -> Self {
        self.double.set(move |sender| {
            if let Some(table) = sender.downcast_ref::<NSTableView>() {
                if let Ok(row) = usize::try_from(if table.clickedRow() >= 0 {
                    table.clickedRow()
                } else {
                    table.selectedRow()
                }) {
                    f(row);
                }
            }
        });
        self
    }

    pub fn context_menu(self, menu: crate::Menu) -> Self {
        self.set_context_menu(menu);
        self
    }

    pub fn set_context_menu(&self, menu: crate::Menu) {
        let table = objc2::rc::Weak::new(&*self.native);
        let native_menu = objc2::rc::Weak::new(menu.ns_menu());
        let menu = menu.on_tracking(move |opening| {
            if !opening {
                return;
            }
            if let (Some(table), Some(menu)) = (table.load(), native_menu.load()) {
                let row = table.clickedRow();
                if row >= 0 {
                    table.selectRowIndexes_byExtendingSelection(
                        &NSIndexSet::indexSetWithIndex(row as usize),
                        false,
                    );
                }
                for item in menu.itemArray() {
                    item.setEnabled(row >= 0);
                }
            }
        });
        unsafe {
            self.native.setMenu(Some(menu.ns_menu()));
        }
        *self.menu.borrow_mut() = Some(menu);
    }

    pub fn reorderable(self, value: bool) -> Self {
        self.bridge.ivars().reorderable.set(value);
        self.native.registerForDraggedTypes(&NSArray::from_slice(&[&*drag_type()]));
        self.native.setDraggingSourceOperationMask_forLocal(NSDragOperation::Move, true);
        self
    }

    pub fn on_reorder(self, mut f: impl FnMut(usize, usize) + 'static) -> Self {
        let rows = self.rows.clone();
        *self.bridge.ivars().reorder.borrow_mut() = Some(Box::new(move |from, to| {
            let value = rows.borrow_mut().remove(from);
            rows.borrow_mut().insert(to, value);
            f(from, to);
        }));
        self
    }

    pub fn ns_table_view(&self) -> &NSTableView { &self.native }

    pub fn ns_scroll_view(&self) -> &NSScrollView { self.scroll.ns_scroll_view() }
}
impl<T: 'static> NativeView for Table<T> {
    fn ns_view(&self) -> &NSView { self.scroll.ns_view() }
}
impl<T: 'static> Drop for Table<T> {
    fn drop(&mut self) {
        unsafe {
            self.native.setDataSource(None);
            self.native.setDelegate(None);
            self.native.setTarget(None);
            self.native.setDoubleAction(None);
        }
    }
}

pub struct List<T: 'static>(Table<T>);
impl<T: 'static> List<T> {
    pub fn new(ui: &Ui, label: impl Fn(&T) -> String + 'static) -> Self {
        let ui_copy = *ui;
        let table = Table::new(ui)
            .column("item", "", 240.0)
            .cells(move |item, _| Box::new(Label::new(&ui_copy, &label(item))));
        table.native.setHeaderView(None);
        Self(table)
    }

    pub fn items(self, items: Vec<T>) -> Self { Self(self.0.rows(items)) }

    pub fn set_items(&self, items: Vec<T>) { self.0.set_rows(items); }

    pub fn on_select(self, f: impl FnMut(Option<usize>) + 'static) -> Self {
        Self(self.0.on_select(f))
    }

    pub fn selection(&self) -> Option<usize> { self.0.selection() }

    pub fn set_selected(&self, index: Option<usize>) { self.0.set_selected(index); }

    pub fn ns_table_view(&self) -> &NSTableView { self.0.ns_table_view() }

    pub fn defer_scrolling(&self) { self.0.scroll.defer_scrolling(); }
}
impl<T: 'static> NativeView for List<T> {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}

pub struct EditableList<T: 'static> {
    stack: VStack,
    list: SettingsList<T>,
    controls: AddRemoveControl,
    selection: Rc<RefCell<Option<Selection>>>,
}
impl<T: 'static> EditableList<T> {
    pub fn new(ui: &Ui, label: impl Fn(&T) -> String + 'static) -> Self {
        let controls = AddRemoveControl::new(ui);
        controls.set_remove_enabled(false);
        let selection: Rc<RefCell<Option<Selection>>> = Rc::new(RefCell::new(None));
        let cb = selection.clone();
        let remove = Weak::new(controls.remove_button());
        let list = SettingsList::new(ui, label, |_| String::new()).fit_content(220.0).on_select(
            move |index| {
                if let Some(remove) = remove.load() {
                    remove.setEnabled(index.is_some());
                }
                if let Ok(mut f) = cb.try_borrow_mut() {
                    if let Some(f) = f.as_mut() {
                        f(index);
                    }
                }
            },
        );
        let stack = VStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(list.ns_view());
        crate::view::prepare(list.ns_view());
        list.ns_view()
            .widthAnchor()
            .constraintEqualToAnchor(&stack.ns_view().widthAnchor())
            .setActive(true);

        stack.ns_stack_view().addArrangedSubview(controls.ns_view());
        Self {
            stack,
            list,
            controls,
            selection,
        }
    }

    /// Let the enclosing settings page scroll the entire collection.
    pub fn full_length(mut self) -> Self {
        self.list = self.list.full_length();
        self
    }

    pub fn items(self, items: Vec<T>) -> Self {
        self.list.set_rows(items);
        self
    }

    pub fn set_items(&self, items: Vec<T>) {
        self.list.set_rows(items);
        self.controls.set_remove_enabled(self.list.selection().is_some());
    }

    pub fn reorderable(mut self, value: bool) -> Self {
        self.list = self.list.reorderable(value);
        self
    }

    pub fn on_reorder(mut self, f: impl FnMut(usize, usize) + 'static) -> Self {
        self.list = self.list.on_reorder(f);
        self
    }

    pub fn on_add(mut self, f: impl FnMut() + 'static) -> Self {
        self.controls = self.controls.on_add(f);
        self
    }

    pub fn on_remove(mut self, mut f: impl FnMut(usize) + 'static) -> Self {
        let table = Weak::new(self.list.ns_table_view());
        self.controls = self.controls.on_remove(move || {
            if let Some(t) = table.load() {
                if let Ok(i) = usize::try_from(t.selectedRow()) {
                    f(i);
                }
            }
        });
        self
    }

    pub fn on_select(self, f: impl FnMut(Option<usize>) + 'static) -> Self {
        *self.selection.borrow_mut() = Some(Box::new(f));
        self
    }

    pub fn set_selected(&self, index: Option<usize>) {
        self.list.set_selected(index);
        self.controls.set_remove_enabled(self.list.selection().is_some());
    }

    pub fn selection(&self) -> Option<usize> { self.list.selection() }

    pub fn ns_table_view(&self) -> &NSTableView { self.list.ns_table_view() }
}
impl<T: 'static> NativeView for EditableList<T> {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}

struct SettingsCellContent {
    content: crate::HStack,
    title: Rc<Label>,
    summary: Option<Rc<crate::Caption>>,
    trailing: Option<Rc<Label>>,
    icon: Option<Retained<NSImageView>>,
    button: Option<Rc<crate::Button>>,
    divider: Rc<crate::Divider>,
    index: Rc<Cell<usize>>,
}
define_class!(
    #[unsafe(super(NSTableCellView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiSettingsListCell"]
    #[ivars = SettingsCellContent]
    struct SettingsListCell;
);
impl SettingsListCell {
    fn new(
        ui: &Ui,
        open: Rc<RefCell<Option<Box<dyn FnMut(usize)>>>>,
        inline: bool,
        has_icon: bool,
        navigation: bool,
        opens: bool,
    ) -> Retained<Self> {
        let title = Rc::new(Label::new(ui, ""));
        let summary = (!inline).then(|| {
            let label = Rc::new(crate::Caption::new(ui, ""));
            label.ns_text_field().setMaximumNumberOfLines(1);
            label
        });
        let trailing = inline.then(|| Rc::new(Label::new(ui, "")));
        let icon = has_icon.then(|| {
            let image = NSImageView::new(ui.mtm());
            image.widthAnchor().constraintEqualToConstant(22.0).setActive(true);
            image.heightAnchor().constraintEqualToConstant(22.0).setActive(true);
            image.setContentTintColor(Some(&crate::Color::secondary_label()));
            image
        });
        let chevron = navigation.then(|| {
            let image = NSImageView::new(ui.mtm());
            image.setImage(crate::Symbol::named("chevron.forward").as_deref());
            image.widthAnchor().constraintEqualToConstant(12.0).setActive(true);
            image.setContentTintColor(Some(&crate::Color::secondary_label()));
            image.setAccessibilityElement(false);
            image
        });
        let index = Rc::new(Cell::new(0));
        let action_index = index.clone();
        let button = (opens && !navigation).then(|| {
            let button = Rc::new(
                crate::Button::new(ui, "Open").symbol("chevron.forward").borderless().on_click(
                    move || {
                        if let Some(open) = open.borrow_mut().as_mut() {
                            open(action_index.get());
                        }
                    },
                ),
            );
            button.ns_button().setContentTintColor(Some(&crate::Color::secondary_label()));
            button.control_size(NSControlSize::Small);
            button.width(16.0);
            button
        });
        let mut row = crate::HStack::new(ui).insets(crate::Insets {
            top: 8.0,
            left: 12.0,
            bottom: 8.0,
            right: 12.0,
        });
        if let Some(icon) = &icon {
            row = row.push(icon.clone().into_super().into_super());
        }
        row = if let Some(summary) = &summary {
            row.push(VStack::new(ui).spacing(2.0).push(title.clone()).push(summary.clone()))
        } else {
            row.push(title.clone())
        };
        row = row.spacer(ui);
        if let Some(trailing) = &trailing {
            row = row.push(trailing.clone());
        }
        if let Some(chevron) = &chevron {
            row = row.push(chevron.clone().into_super().into_super());
        }
        if let Some(button) = &button {
            row = row.push(button.clone());
        }
        let divider = Rc::new(crate::Divider::new(ui));
        let content = row;
        let native: Retained<Self> = unsafe {
            msg_send![
                super(Self::alloc(ui.mtm()).set_ivars(SettingsCellContent {
                    content,
                    title,
                    summary,
                    trailing,
                    icon,
                    button,
                    divider,
                    index,
                })),
                init
            ]
        };
        native.addSubview(native.ivars().content.ns_view());
        native.addSubview(native.ivars().divider.ns_view());
        let line = native.ivars().divider.ns_view();
        crate::view::prepare(line);
        line.leadingAnchor()
            .constraintEqualToAnchor_constant(&native.leadingAnchor(), 12.0)
            .setActive(true);
        line.trailingAnchor()
            .constraintEqualToAnchor_constant(&native.trailingAnchor(), -12.0)
            .setActive(true);
        line.bottomAnchor()
            .constraintEqualToAnchor(&native.bottomAnchor())
            .setActive(true);
        line.heightAnchor().constraintEqualToConstant(1.0).setActive(true);
        crate::view::pin(&native, native.ivars().content.ns_view(), crate::Insets {
            top: 0.0,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });
        unsafe {
            native.setTextField(Some(native.ivars().title.ns_text_field()));
            native.setImageView(native.ivars().icon.as_deref());
        }
        native
    }

    fn configure(
        &self,
        name: &str,
        summary: &str,
        symbol: Option<&str>,
        index: usize,
        count: usize,
    ) {
        let cell = self.ivars();
        cell.index.set(index);
        cell.title.set_text(name);
        cell.title.tooltip(name);
        if let Some(caption) = &cell.summary {
            caption.set_text(summary);
            caption.tooltip(summary);
            caption.set_hidden(summary.is_empty());
        }
        if let Some(trailing) = &cell.trailing {
            trailing.set_text(summary);
            trailing.tooltip(summary);
            trailing.set_hidden(summary.is_empty());
        }
        if let Some(icon) = &cell.icon {
            icon.setContentTintColor(Some(&crate::Color::secondary_label()));
            icon.setImage(symbol.and_then(crate::Symbol::named).as_deref());
            icon.setHidden(symbol.is_none());
        }
        if let Some(button) = &cell.button {
            button.accessibility_label(&format!("Open {name}"));
        }
        cell.divider.set_hidden(index + 1 == count);
    }
}
impl NativeView for Retained<SettingsListCell> {
    fn ns_view(&self) -> &NSView { self }
}

/// A native settings collection with primary text, a summary, and optional disclosure.
/// NSTableView owns selection, keyboard navigation, scrolling, and drag reordering.
pub struct SettingsList<T: 'static> {
    table: Table<T>,
    surface: crate::GroupBox,
    open: Rc<RefCell<Option<Box<dyn FnMut(usize)>>>>,
    symbol: Rc<RefCell<Option<Box<dyn Fn(&T) -> String>>>>,
    image: Rc<RefCell<Option<Box<dyn Fn(&T) -> Option<Retained<NSImage>>>>>>,
    fitted_height: Option<(Retained<NSLayoutConstraint>, f64)>,
    row_count: Rc<Cell<usize>>,
    empty: crate::Caption,
    trailing_summary: Rc<Cell<bool>>,
    navigation: Rc<Cell<bool>>,
}
impl<T: 'static> SettingsList<T> {
    pub fn new(
        ui: &Ui,
        title: impl Fn(&T) -> String + 'static,
        summary: impl Fn(&T) -> String + 'static,
    ) -> Self {
        let open: Rc<RefCell<Option<Box<dyn FnMut(usize)>>>> = Rc::new(RefCell::new(None));
        let symbol: Rc<RefCell<Option<Box<dyn Fn(&T) -> String>>>> = Rc::new(RefCell::new(None));
        let image: Rc<RefCell<Option<Box<dyn Fn(&T) -> Option<Retained<NSImage>>>>>> =
            Rc::new(RefCell::new(None));
        let row_image = image.clone();
        let row_open = open.clone();
        let row_symbol = symbol.clone();
        let ui_copy = *ui;
        let double_open = open.clone();
        let row_count = Rc::new(Cell::new(0));
        let cell_count = row_count.clone();
        let trailing_summary = Rc::new(Cell::new(false));
        let inline = trailing_summary.clone();
        let navigation = Rc::new(Cell::new(false));
        let row_navigation = navigation.clone();
        let table = Table::new(ui).column("item", "", 0.0).on_double_click(move |index| {
            if let Some(open) = double_open.borrow_mut().as_mut() {
                open(index);
            }
        });
        let rows = table.rows.clone();
        let opens = open.clone();
        *table.bridge.ivars().native_cell.borrow_mut() = Some(Box::new(move |table, row, _| {
            let identifier = NSString::from_str("cgs.settings-list");
            let cell = unsafe { table.makeViewWithIdentifier_owner(&identifier, None) }
                .and_then(|view| view.downcast::<SettingsListCell>().ok())
                .unwrap_or_else(|| {
                    let cell = SettingsListCell::new(
                        &ui_copy,
                        row_open.clone(),
                        inline.get(),
                        row_symbol.borrow().is_some() || row_image.borrow().is_some(),
                        row_navigation.get(),
                        opens.borrow().is_some(),
                    );
                    cell.setIdentifier(Some(&identifier));
                    cell
                });
            let rows = rows.borrow();
            let item = &rows[row];
            let symbol = row_symbol.borrow().as_ref().map(|symbol| symbol(item));
            cell.configure(
                &title(item),
                &summary(item),
                symbol.as_deref(),
                row,
                cell_count.get(),
            );
            if let Some(provider) = row_image.borrow().as_ref() {
                if let Some(image) = provider(item) {
                    if let Some(icon) = &cell.ivars().icon {
                        icon.setContentTintColor(None);
                        icon.setImage(Some(&image));
                        icon.setHidden(false);
                    }
                }
            }
            cell.into_super().into_super()
        }));
        // The enclosing group supplies the surface; avoid NSTableView's extra inset margins.
        table.ns_table_view().setStyle(NSTableViewStyle::Plain);
        table.ns_table_view().setHeaderView(None);
        table.ns_table_view().setRowHeight(44.0);
        table.ns_table_view().setBackgroundColor(&NSColor::clearColor());
        table.ns_scroll_view().setDrawsBackground(false);
        let surface = crate::GroupBox::with_insets(ui, table.ns_view().retain(), crate::Insets {
            top: 0.0,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });
        let empty = crate::Caption::new(ui, "No items");
        surface.ns_view().addSubview(empty.ns_view());
        crate::view::prepare(empty.ns_view());
        empty
            .ns_view()
            .centerXAnchor()
            .constraintEqualToAnchor(&surface.ns_view().centerXAnchor())
            .setActive(true);
        empty
            .ns_view()
            .centerYAnchor()
            .constraintEqualToAnchor(&surface.ns_view().centerYAnchor())
            .setActive(true);
        Self {
            table,
            surface,
            open,
            symbol,
            image,
            fitted_height: None,
            row_count,
            empty,
            trailing_summary,
            navigation,
        }
    }

    /// Expand to every row so an enclosing settings page owns scrolling.
    pub fn full_length(self) -> Self {
        self.table.scroll.defer_scrolling();
        self.fit_content(f64::MAX)
    }

    /// Small inventories size to their rows; larger inventories scroll within the cap.

    pub fn fit_content(mut self, maximum_height: f64) -> Self {
        if let Some((_, maximum)) = &mut self.fitted_height {
            *maximum = maximum_height.max(64.0);
        } else {
            let height = self.ns_view().heightAnchor().constraintEqualToConstant(64.0);
            height.setActive(true);
            self.fitted_height = Some((height, maximum_height.max(64.0)));
        }
        self
    }

    pub fn set_rows_if_changed(&self, rows: Vec<T>)
    where T: PartialEq {
        if *self.table.rows.borrow() != rows {
            self.set_rows(rows);
        }
    }

    pub fn set_rows(&self, rows: Vec<T>) {
        let count = rows.len();
        self.row_count.set(count);
        self.empty.set_hidden(count != 0);
        self.table.set_rows(rows);
        if let Some((height, maximum)) = &self.fitted_height {
            let table = self.table.ns_table_view();
            // Inset tables add native padding before the first and after the last row.
            let native_padding = if count == 0 {
                0.0
            } else {
                table.rectOfRow(0).origin.y * 2.0
            };
            let total = (count as f64 * (table.rowHeight() + table.intercellSpacing().height)
                + native_padding)
                .max(if self.navigation.get() && count > 0 { table.rowHeight() } else { 64.0 });
            let fitted = total.min(*maximum);
            if height.constant() != fitted {
                height.setConstant(fitted);
            }
            self.table.ns_scroll_view().setHasVerticalScroller(total > *maximum);
        }
    }

    /// Open destinations with one click, without a persistent selection highlight.
    pub fn navigation(self) -> Self {
        self.navigation.set(true);
        let table = self.table.ns_table_view();
        table.setUsesAutomaticRowHeights(false);
        table.setRowHeight(56.0);
        table.setSelectionHighlightStyle(NSTableViewSelectionHighlightStyle::None);
        unsafe {
            table.setAction(Some(objc2::sel!(invoke:)));
        }
        self
    }

    /// Display a short value beside the title, as in a native keyboard shortcuts list.
    pub fn trailing_summary(self) -> Self {
        self.trailing_summary.set(true);
        self.table.ns_table_view().setRowHeight(44.0);
        self
    }

    pub fn empty_message(self, message: &str) -> Self {
        self.empty.set_text(message);
        self
    }

    pub fn on_select(self, f: impl FnMut(Option<usize>) + 'static) -> Self {
        self.table.set_on_select(f);
        self
    }

    pub fn on_open(self, f: impl FnMut(usize) + 'static) -> Self {
        *self.open.borrow_mut() = Some(Box::new(f));
        // Use AppKit's click action, preserving selection and drag handling.
        unsafe {
            self.table.ns_table_view().setAction(Some(objc2::sel!(invoke:)));
            self.table.ns_table_view().setDoubleAction(None);
        }
        self
    }

    pub fn symbol(self, name: &str) -> Self {
        let name = name.to_owned();
        *self.symbol.borrow_mut() = Some(Box::new(move |_| name.clone()));
        self
    }

    /// Provide a native image, with the configured symbol as a fallback.
    pub fn images(self, image: impl Fn(&T) -> Option<Retained<NSImage>> + 'static) -> Self {
        *self.image.borrow_mut() = Some(Box::new(image));
        self
    }

    pub fn symbols(self, symbol: impl Fn(&T) -> String + 'static) -> Self {
        *self.symbol.borrow_mut() = Some(Box::new(symbol));
        self
    }

    pub fn reorderable(mut self, value: bool) -> Self {
        self.table = self.table.reorderable(value);
        self
    }

    pub fn on_reorder(mut self, f: impl FnMut(usize, usize) + 'static) -> Self {
        self.table = self.table.on_reorder(f);
        self
    }
}
impl<T: 'static> std::ops::Deref for SettingsList<T> {
    type Target = Table<T>;

    fn deref(&self) -> &Self::Target { &self.table }
}
impl<T: 'static> NativeView for SettingsList<T> {
    fn ns_view(&self) -> &NSView { self.surface.ns_view() }
}

pub struct OutlineItem<T> {
    pub id: T,
    pub title: String,
    pub children: Vec<OutlineItem<T>>,
}
pub struct Outline<T: 'static> {
    native: Retained<NSOutlineView>,
    scroll: ScrollView,
    bridge: Retained<CollectionBridge>,
    items: Rc<RefCell<Vec<(T, String)>>>,
    column: Retained<NSTableColumn>,
}
impl<T: 'static> Outline<T> {
    pub fn new(ui: &Ui) -> Self {
        let native = NSOutlineView::new(ui.mtm());
        let column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(ui.mtm()),
            &NSString::from_str("item"),
        );
        native.addTableColumn(&column);
        unsafe {
            native.setOutlineTableColumn(Some(&column));
        }
        native.setHeaderView(None);
        native.setStyle(NSTableViewStyle::SourceList);
        let items: Rc<RefCell<Vec<(T, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let i = items.clone();
        let j = items.clone();
        let ui_copy = *ui;
        let bridge = CollectionBridge::new(
            ui,
            move || i.borrow().len(),
            move |row, _| Box::new(Label::new(&ui_copy, &j.borrow()[row].1)),
        );
        unsafe {
            native.setDataSource(Some(ProtocolObject::from_ref(&*bridge)));
            native.setDelegate(Some(ProtocolObject::from_ref(&*bridge)));
        }
        let view: Retained<NSView> = native.clone().into_super().into_super().into_super();
        let scroll = ScrollView::new(ui, view);
        Self {
            native,
            scroll,
            bridge,
            items,
            column,
        }
    }

    pub fn items(self, items: Vec<OutlineItem<T>>) -> Self {
        self.set_items(items);
        self
    }

    pub fn set_items(&self, items: Vec<OutlineItem<T>>) {
        fn flatten<T>(
            items: Vec<OutlineItem<T>>,
            values: &mut Vec<(T, String)>,
            nodes: &mut Vec<Node>,
        ) -> Vec<usize> {
            items
                .into_iter()
                .map(|item| {
                    let index = values.len();
                    values.push((item.id, item.title));
                    nodes.push(Node {
                        object: NSNumber::new_usize(index),
                        children: Vec::new(),
                    });
                    let children = flatten(item.children, values, nodes);
                    nodes[index].children = children;
                    index
                })
                .collect()
        }
        let mut values = Vec::new();
        let mut nodes = Vec::new();
        let roots = flatten(items, &mut values, &mut nodes);
        *self.items.borrow_mut() = values;
        *self.bridge.ivars().nodes.borrow_mut() = nodes;
        *self.bridge.ivars().roots.borrow_mut() = roots;
        self.native.reloadData();
    }

    pub fn on_select(self, mut f: impl FnMut(Option<T>) + 'static) -> Self
    where T: Clone {
        let items = self.items.clone();
        *self.bridge.ivars().selection.borrow_mut() = Some(Box::new(move |i| {
            let id = i.and_then(|i| items.borrow().get(i).map(|(id, _)| id.clone()));
            f(id);
        }));
        self
    }

    pub fn cells(self, mut cell: impl FnMut(&T, &str) -> Box<dyn NativeView> + 'static) -> Self {
        let items = self.items.clone();
        *self.bridge.ivars().cell.borrow_mut() = Box::new(move |index, _| {
            let items = items.borrow();
            let (item, title) = &items[index];
            cell(item, title)
        });
        self.native.reloadData();
        self
    }

    /// Select a stable item index, revealing its parent if collapsed.
    pub fn set_selected(&self, index: usize) {
        let nodes = self.bridge.ivars().nodes.borrow();
        let Some(node) = nodes.get(index) else {
            return;
        };
        for parent in nodes.iter().filter(|node| node.children.contains(&index)) {
            unsafe {
                self.native.expandItem(Some(&parent.object));
            }
        }
        let row = unsafe { self.native.rowForItem(Some(&node.object)) };
        if row >= 0 {
            self.native.selectRowIndexes_byExtendingSelection(
                &NSIndexSet::indexSetWithIndex(row as usize),
                false,
            );
        }
    }

    /// Treat parent rows as native, nonselectable source-list section headings.
    pub fn group_parents(&self) {
        self.bridge.ivars().group_parents.set(true);
        self.native.reloadData();
        self.expand_all();
    }

    pub fn expand_all(&self) {
        unsafe {
            self.native.expandItem_expandChildren(None, true);
        }
    }

    pub fn ns_outline_view(&self) -> &NSOutlineView { &self.native }

    pub fn ns_table_column(&self) -> &NSTableColumn { &self.column }
}
impl<T: 'static> NativeView for Outline<T> {
    fn ns_view(&self) -> &NSView { self.scroll.ns_view() }
}
impl<T: 'static> Drop for Outline<T> {
    fn drop(&mut self) {
        unsafe {
            self.native.setDelegate(None);
            self.native.setDataSource(None);
            self.native.setOutlineTableColumn(None);
        }
    }
}

#[derive(Clone)]
pub struct PickerItem<T> {
    pub id: T,
    pub title: String,
    pub group: Option<String>,
}
#[derive(Clone)]
enum PickerRow<T> {
    Group(String),
    Item(PickerItem<T>),
}
fn picker_rows<T: Clone>(items: &[PickerItem<T>], query: &str) -> Vec<PickerRow<T>> {
    let query = query.to_lowercase();
    let mut groups: Vec<(Option<String>, Vec<PickerItem<T>>)> = Vec::new();
    for item in items.iter().filter(|i| i.title.to_lowercase().contains(&query)) {
        if let Some((_, items)) = groups.iter_mut().find(|(g, _)| *g == item.group) {
            items.push(item.clone());
        } else {
            groups.push((item.group.clone(), vec![item.clone()]));
        }
    }
    groups
        .into_iter()
        .flat_map(|(group, items)| {
            group
                .map(PickerRow::Group)
                .into_iter()
                .chain(items.into_iter().map(PickerRow::Item))
        })
        .collect()
}
pub struct SearchablePicker<T: Clone + 'static> {
    stack: VStack,
    search: crate::SearchField,
    table: Table<PickerRow<T>>,
    items: Rc<RefCell<Vec<PickerItem<T>>>>,
}
impl<T: Clone + 'static> SearchablePicker<T> {
    pub fn new(ui: &Ui, items: Vec<PickerItem<T>>) -> Self {
        let items = Rc::new(RefCell::new(items));
        let ui_copy = *ui;
        let table = Table::new(ui)
            .column("item", "", 300.0)
            .cells(move |row, _| match row {
                PickerRow::Group(title) => Box::new(crate::SectionTitle::new(&ui_copy, title)),
                PickerRow::Item(item) => Box::new(Label::new(&ui_copy, &item.title)),
            })
            .selectable(|row| matches!(row, PickerRow::Item(_)))
            .rows(picker_rows(&items.borrow(), ""));
        table.native.setHeaderView(None);
        let all = items.clone();
        let rows = table.rows.clone();
        let native = Weak::new(&*table.native);
        let bridge = Weak::new(&*table.bridge);
        let search = crate::SearchField::new(ui).placeholder("Search").on_change(move |query| {
            *rows.borrow_mut() = picker_rows(&all.borrow(), &query);
            if let (Some(table), Some(bridge)) = (native.load(), bridge.load()) {
                bridge.reload(&table);
            }
        });
        let stack = VStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(search.ns_view());
        stack.ns_stack_view().addArrangedSubview(table.ns_view());
        Self { stack, search, table, items }
    }

    pub fn on_select(mut self, mut f: impl FnMut(T) + 'static) -> Self {
        let rows = self.table.rows.clone();
        self.table = self.table.on_select(move |index| {
            let id = index.and_then(|i| match rows.borrow().get(i) {
                Some(PickerRow::Item(item)) => Some(item.id.clone()),
                _ => None,
            });
            if let Some(id) = id {
                f(id);
            }
        });
        self
    }

    pub fn set_items(&self, items: Vec<PickerItem<T>>) {
        *self.items.borrow_mut() = items;
        self.table.set_rows(picker_rows(&self.items.borrow(), &self.search.get_value()));
    }

    pub fn set_selected(&self, id: &T)
    where T: PartialEq {
        let index = self
            .table
            .rows
            .borrow()
            .iter()
            .position(|row| matches!(row,PickerRow::Item(item) if &item.id==id));
        self.table.set_selected(index);
    }

    pub fn ns_search_field(&self) -> &NSSearchField { self.search.ns_search_field() }

    pub fn ns_table_view(&self) -> &NSTableView { self.table.ns_table_view() }
}
impl<T: Clone + 'static> NativeView for SearchablePicker<T> {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
