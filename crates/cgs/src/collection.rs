use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{
    NSArray, NSIndexSet, NSNotification, NSNumber, NSObject, NSObjectProtocol, NSString,
};

use crate::bridge::{ActionTarget, callback};
use crate::{AddRemoveControl, Label, NativeView, ScrollView, Ui, VStack};

fn drag_type() -> Retained<NSString> { NSString::from_str("org.cgs.local-row") }
type CellFactory = Box<dyn FnMut(usize, usize) -> Box<dyn NativeView>>;
type Selection = Box<dyn FnMut(Option<usize>)>;
type Reorder = Box<dyn FnMut(usize, usize)>;
struct Node {
    object: Retained<NSNumber>,
    children: Vec<usize>,
}
struct CollectionState {
    count: Box<dyn Fn() -> usize>,
    cell: RefCell<CellFactory>,
    cells: RefCell<HashMap<(usize, usize), Box<dyn NativeView>>>,
    selection: RefCell<Option<Selection>>,
    selectable: RefCell<Box<dyn Fn(usize) -> bool>>,
    reorder: RefCell<Option<Reorder>>,
    reorderable: Cell<bool>,
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
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn table_cell(
            &self,
            table: &NSTableView,
            column: Option<&NSTableColumn>,
            row: isize,
        ) -> Option<Retained<NSView>> {
            usize::try_from(row).ok().and_then(|row| {
                self.make_cell(
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
        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        unsafe fn outline_cell(
            &self,
            _view: &NSOutlineView,
            _column: Option<&NSTableColumn>,
            item: &AnyObject,
        ) -> Option<Retained<NSView>> {
            node_index(item).and_then(|row| self.make_cell(row, 0))
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
            cells: RefCell::new(HashMap::new()),
            selection: RefCell::new(None),
            selectable: RefCell::new(Box::new(|_| true)),
            reorder: RefCell::new(None),
            reorderable: Cell::new(false),
            roots: RefCell::new(Vec::new()),
            nodes: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn make_cell(&self, row: usize, column: usize) -> Option<Retained<NSView>> {
        let _keep_alive = self.retain();
        if row >= (self.ivars().count)() {
            return None;
        }
        if let Some(view) = self.ivars().cells.borrow().get(&(row, column)) {
            return Some(view.ns_view().retain());
        }
        let mut view = None;
        callback(|| view = Some((self.ivars().cell.borrow_mut())(row, column)));
        let view = view?;
        let native = view.ns_view().retain();
        self.ivars().cells.borrow_mut().insert((row, column), view);
        Some(native)
    }

    fn reload(&self, table: &NSTableView) {
        self.ivars().cells.borrow_mut().clear();
        table.reloadData();
    }

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
        native.setUsesAutomaticRowHeights(true);
        native.setStyle(NSTableViewStyle::Inset);
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
        let rows = self.rows.clone();
        *self.bridge.ivars().cell.borrow_mut() =
            Box::new(move |row, column| f(&rows.borrow()[row], column));
        self.bridge.reload(&self.native);
        self
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

    pub fn selectable(self, f: impl Fn(&T) -> bool + 'static) -> Self {
        let rows = self.rows.clone();
        *self.bridge.ivars().selectable.borrow_mut() =
            Box::new(move |row| rows.borrow().get(row).is_some_and(&f));
        self
    }

    pub fn on_double_click(self, mut f: impl FnMut(usize) + 'static) -> Self {
        self.double.set(move |sender| {
            if let Some(table) = sender.downcast_ref::<NSTableView>() {
                if let Ok(row) = usize::try_from(table.clickedRow()) {
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
}
impl<T: 'static> NativeView for List<T> {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}

pub struct EditableList<T: 'static> {
    stack: VStack,
    list: List<T>,
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
        let list = List::new(ui, label).on_select(move |index| {
            if let Some(remove) = remove.load() {
                remove.setEnabled(index.is_some());
            }
            if let Ok(mut f) = cb.try_borrow_mut() {
                if let Some(f) = f.as_mut() {
                    f(index);
                }
            }
        });
        let stack = VStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(list.ns_view());
        stack.ns_stack_view().addArrangedSubview(controls.ns_view());
        Self {
            stack,
            list,
            controls,
            selection,
        }
    }

    pub fn items(self, items: Vec<T>) -> Self {
        self.list.set_items(items);
        self
    }

    pub fn set_items(&self, items: Vec<T>) {
        self.list.set_items(items);
        self.controls.set_remove_enabled(self.list.selection().is_some());
    }

    pub fn reorderable(mut self, value: bool) -> Self {
        self.list.0 = self.list.0.reorderable(value);
        self
    }

    pub fn on_reorder(mut self, f: impl FnMut(usize, usize) + 'static) -> Self {
        self.list.0 = self.list.0.on_reorder(f);
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
        self.bridge.ivars().cells.borrow_mut().clear();
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
