use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString};

use crate::*;

pub struct SplitView {
    native: Retained<NSSplitViewController>,
    view: Retained<NSView>,
    children: Vec<ViewController>,
}
impl SplitView {
    pub fn new(ui: &Ui) -> Self {
        let native = NSSplitViewController::new(ui.mtm());
        let view = native.view();
        Self {
            native,
            view,
            children: Vec::new(),
        }
    }

    pub fn pane(mut self, ui: &Ui, content: impl NativeView, sidebar: bool) -> Self {
        let controller = ViewController::new(ui, content);
        let item = if sidebar {
            NSSplitViewItem::sidebarWithViewController(controller.ns_view_controller())
        } else {
            NSSplitViewItem::splitViewItemWithViewController(controller.ns_view_controller())
        };
        if sidebar {
            item.setAllowsFullHeightLayout(true);
            item.setHoldingPriority(750.0);
        }
        self.native.addSplitViewItem(&item);
        self.children.push(controller);
        self
    }

    pub fn ns_split_view_controller(&self) -> &NSSplitViewController { &self.native }
}
impl NativeView for SplitView {
    fn ns_view(&self) -> &NSView { &self.view }

    fn view_controller(&self) -> Option<&NSViewController> { Some(&self.native) }
}
#[derive(Clone)]
pub struct SidebarItem<T> {
    pub id: T,
    pub title: String,
    pub symbol: String,
}
struct SidebarCell {
    native: Retained<NSTableCellView>,
    _content: HStack,
}
impl NativeView for SidebarCell {
    fn ns_view(&self) -> &NSView { &self.native }
}

pub struct Sidebar<T: 'static> {
    outline: Outline<SidebarItem<T>>,
}
impl<T: Clone + 'static> Sidebar<T> {
    pub fn new(ui: &Ui, items: Vec<SidebarItem<T>>) -> Self {
        Self::with_children(ui, items, |_| Vec::new())
    }

    pub fn with_children(
        ui: &Ui,
        items: Vec<SidebarItem<T>>,
        children: impl Fn(&T) -> Vec<SidebarItem<T>>,
    ) -> Self {
        let ui_copy = *ui;
        let content_width = items
            .iter()
            .map(|item| {
                Label::new(ui, &item.title).ns_view().fittingSize().width
                    + Metrics::CONTROL_SPACING * 6.0
            })
            .fold(0.0, f64::max);
        let items = items
            .into_iter()
            .map(|item| OutlineItem {
                children: children(&item.id)
                    .into_iter()
                    .map(|child| OutlineItem {
                        title: child.title.clone(),
                        id: child,
                        children: Vec::new(),
                    })
                    .collect(),
                title: item.title.clone(),
                id: item,
            })
            .collect();
        let outline = Outline::new(ui).items(items).cells(move |item, title| {
            let native = NSTableCellView::new(ui_copy.mtm());
            let label = Label::new(&ui_copy, title);
            unsafe {
                native.setTextField(Some(label.ns_text_field()));
            }
            let row = HStack::new(&ui_copy).spacing(8.0);
            let row = if let Some(image) = ImageView::symbol(&ui_copy, &item.symbol) {
                image.width(16.0);
                image.height(16.0);
                unsafe {
                    native.setImageView(Some(image.ns_image_view()));
                }
                row.push(image)
            } else {
                row
            };
            let content = row.push(label);
            native.addSubview(content.ns_view());
            crate::view::prepare(content.ns_view());
            content
                .ns_view()
                .leadingAnchor()
                .constraintEqualToAnchor(&native.leadingAnchor())
                .setActive(true);
            content
                .ns_view()
                .trailingAnchor()
                .constraintEqualToAnchor(&native.trailingAnchor())
                .setActive(true);
            content
                .ns_view()
                .centerYAnchor()
                .constraintEqualToAnchor(&native.centerYAnchor())
                .setActive(true);
            Box::new(SidebarCell { native, _content: content })
        });
        let native = outline.ns_outline_view();
        native.setUsesAutomaticRowHeights(false);
        native.setRowSizeStyle(NSTableViewRowSizeStyle::Default);
        native.setBackgroundColor(&NSColor::clearColor());
        outline.min_width(content_width);
        outline.max_width(content_width + Metrics::PAGE_INSET * 2.0);
        Self { outline }
    }

    pub fn on_select(mut self, mut f: impl FnMut(T) + 'static) -> Self {
        let native = objc2::rc::Weak::new(self.outline.ns_outline_view());
        self.outline = self.outline.on_select(move |item| {
            if let Some(item) = item {
                unsafe {
                    if let Some(native) = native.load() {
                        if let Some(selected) = native.itemAtRow(native.selectedRow()) {
                            native.expandItem(Some(&selected));
                        }
                    }
                }
                f(item.id);
            }
        });
        self
    }

    pub fn set_items(&self, items: Vec<SidebarItem<T>>) {
        self.outline.set_items(items.into_iter().map(|item| OutlineItem {
            title: item.title.clone(), id: item, children: Vec::new(),
        }).collect());
    }

    pub fn group_parents(&self) { self.outline.group_parents(); }

    pub fn set_selected(&self, index: usize) { self.outline.set_selected(index); }

    pub fn ns_table_view(&self) -> &NSTableView { self.outline.ns_outline_view() }
}
impl<T: 'static> NativeView for Sidebar<T> {
    fn ns_view(&self) -> &NSView { self.outline.ns_view() }
}
pub struct NavigationSplitView(SplitView);
impl NavigationSplitView {
    pub fn new(ui: &Ui, sidebar: impl NativeView, detail: impl NativeView) -> Self {
        let split = SplitView::new(ui).pane(ui, sidebar, true).pane(ui, detail, false);
        let items = split.native.splitViewItems();
        items.objectAtIndex(0).setMinimumThickness(Metrics::SIDEBAR_MIN_WIDTH);
        items.objectAtIndex(1).setMinimumThickness(Metrics::DETAIL_MIN_WIDTH);
        Self(split)
    }

    pub fn ns_split_view_controller(&self) -> &NSSplitViewController {
        self.0.ns_split_view_controller()
    }
}
impl NativeView for NavigationSplitView {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }

    fn view_controller(&self) -> Option<&NSViewController> { self.0.view_controller() }
}
pub struct MasterDetail(SplitView);
impl MasterDetail {
    pub fn new(ui: &Ui, master: impl NativeView, detail: impl NativeView) -> Self {
        let split = SplitView::new(ui).pane(ui, master, false).pane(ui, detail, false);
        let items = split.native.splitViewItems();
        let list = items.objectAtIndex(0);
        list.setMinimumThickness(140.0);
        list.setMaximumThickness(190.0);
        list.setPreferredThicknessFraction(0.28);
        Self(split)
    }

    pub fn ns_split_view_controller(&self) -> &NSSplitViewController {
        self.0.ns_split_view_controller()
    }
}
impl NativeView for MasterDetail {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }

    fn view_controller(&self) -> Option<&NSViewController> { self.0.view_controller() }
}

struct NavigationToolbarItems {
    title: Option<Rc<Label>>,
    back: Retained<NSToolbarItem>,
    navigation: std::cell::RefCell<Vec<Retained<NSToolbarItem>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsNavigationToolbarDelegate"]
    #[ivars = NavigationToolbarItems]
    struct NavigationToolbarDelegate;
    unsafe impl NSObjectProtocol for NavigationToolbarDelegate {}
    unsafe impl NSToolbarDelegate for NavigationToolbarDelegate {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(
            &self,
            _toolbar: &NSToolbar,
            identifier: &NSToolbarItemIdentifier,
            _insert: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            if let Some(item) = self
                .ivars()
                .navigation
                .borrow()
                .iter()
                .find(|item| &*item.itemIdentifier() == identifier)
            {
                Some(item.clone())
            } else if identifier.to_string() == "cgs.back" {
                Some(self.ivars().back.clone())
            } else {
                self.ivars()
                    .title
                    .as_ref()
                    .filter(|_| identifier.to_string() == "cgs.page-title")
                    .map(|title| {
                        let item = NSToolbarItem::initWithItemIdentifier(
                            NSToolbarItem::alloc(self.mtm()),
                            identifier,
                        );
                        let label = HStack::new(&Ui::new(self.mtm()))
                            .insets(Insets { top: 0.0, left: 3.0, bottom: 0.0, right: 0.0 })
                            .push(title.clone());
                        item.setView(Some(label.ns_view()));
                        item.setBordered(false);
                        item
                    })
            }
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            let mut ids: Vec<_> = self.item_identifiers().iter().collect();
            ids.push(NSString::from_str("cgs.back"));
            ids.extend(self.ivars().navigation.borrow().iter().map(|item| item.itemIdentifier()));
            NSArray::from_retained_slice(&ids)
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn defaults(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            self.item_identifiers()
        }
    }
);

impl NavigationToolbarDelegate {
    fn item_identifiers(&self) -> Retained<NSArray<NSToolbarItemIdentifier>> {
        if self.ivars().title.is_some() {
            NSArray::from_retained_slice(&[
                unsafe { NSToolbarFlexibleSpaceItemIdentifier }.retain(),
                unsafe { NSToolbarToggleSidebarItemIdentifier }.retain(),
                unsafe { NSToolbarSidebarTrackingSeparatorItemIdentifier }.retain(),
                NSString::from_str("cgs.page-title"),
                unsafe { NSToolbarFlexibleSpaceItemIdentifier }.retain(),
            ])
        } else {
            NSArray::from_slice(&[unsafe { NSToolbarToggleSidebarItemIdentifier }, unsafe {
                NSToolbarFlexibleSpaceItemIdentifier
            }])
        }
    }
}

pub struct Toolbar {
    native: Retained<NSToolbar>,
    delegate: Option<Retained<NavigationToolbarDelegate>>,
    back_target: Retained<crate::bridge::ActionTarget>,
    forward_target: Retained<crate::bridge::ActionTarget>,
    menu: std::cell::RefCell<Option<(Rc<crate::Menu>, Retained<NSPopUpButton>)>>,
}
impl Toolbar {
    pub fn new(ui: &Ui, id: &str) -> Self {
        Self {
            native: NSToolbar::initWithIdentifier(
                NSToolbar::alloc(ui.mtm()),
                &NSString::from_str(id),
            ),
            delegate: None,
            back_target: crate::bridge::ActionTarget::new(ui),
            forward_target: crate::bridge::ActionTarget::new(ui),
            menu: std::cell::RefCell::new(None),
        }
    }

    pub fn navigation(ui: &Ui, id: &str) -> Self { Self::navigation_title(ui, id, None) }

    pub(crate) fn navigation_title(ui: &Ui, id: &str, title: Option<Rc<Label>>) -> Self {
        let mut toolbar = Self::new(ui, id);
        let back = NSToolbarItem::initWithItemIdentifier(
            NSToolbarItem::alloc(ui.mtm()),
            &NSString::from_str("cgs.back"),
        );
        back.setLabel(&NSString::from_str("Back"));
        back.setPaletteLabel(&NSString::from_str("Back"));
        back.setNavigational(true);
        back.setImage(crate::Symbol::named("chevron.backward").as_deref());
        back.setEnabled(false);
        back.setAutovalidates(false);
        unsafe {
            back.setTarget(Some(&toolbar.back_target));
            back.setAction(Some(objc2::sel!(invoke:)));
        }
        let delegate: Retained<NavigationToolbarDelegate> = unsafe {
            msg_send![
                super(NavigationToolbarDelegate::alloc(ui.mtm()).set_ivars(
                    NavigationToolbarItems {
                        title,
                        back,
                        navigation: std::cell::RefCell::new(Vec::new())
                    }
                )),
                init
            ]
        };
        toolbar.native.setDisplayMode(NSToolbarDisplayMode::IconOnly);
        toolbar.native.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        toolbar.delegate = Some(delegate);
        toolbar
    }

    pub fn attach(&self, window: &NSWindow) {
        window.setToolbar(Some(&self.native));
        if self.native.items().is_empty() {
            self.native.insertItemWithItemIdentifier_atIndex(
                unsafe { NSToolbarToggleSidebarItemIdentifier },
                0,
            );
            self.native.insertItemWithItemIdentifier_atIndex(
                unsafe { NSToolbarFlexibleSpaceItemIdentifier },
                1,
            );
        }
        self.native.setVisible(true);
    }

    /// Show one native navigation item without replacing the toolbar.
    pub fn set_back(&self, back: Option<(&str, Box<dyn FnMut()>)>) {
        let Some(delegate) = &self.delegate else {
            return;
        };
        let item = &delegate.ivars().back;
        let existing = self
            .native
            .items()
            .iter()
            .position(|item| item.itemIdentifier().to_string() == "cgs.back");
        if let Some((label, mut action)) = back {
            self.back_target.set(move |_| action());
            item.setToolTip(Some(&NSString::from_str(label)));
            item.setEnabled(true);
            if existing.is_none() {
                let index = self
                    .native
                    .items()
                    .iter()
                    .position(|item| {
                        item.itemIdentifier().to_string()
                            == unsafe { NSToolbarSidebarTrackingSeparatorItemIdentifier }
                                .to_string()
                    })
                    .map_or(1, |index| index + 1);
                self.native.insertItemWithItemIdentifier_atIndex(
                    &NSString::from_str("cgs.back"),
                    index as isize,
                );
            }
        } else {
            item.setEnabled(false);
            if let Some(index) = existing {
                self.native.removeItemAtIndex(index as isize);
            }
        }
    }

    /// Standard toolbar arrows and a native title menu; AppKit owns their appearance.
    pub fn set_navigation(
        &self,
        ui: &Ui,
        mut back: impl FnMut() + 'static,
        mut forward: impl FnMut() + 'static,
        menu: Rc<crate::Menu>,
    ) {
        let Some(delegate) = &self.delegate else {
            return;
        };
        self.back_target.set(move |_| back());
        self.forward_target.set(move |_| forward());
        let next = NSToolbarItem::initWithItemIdentifier(
            NSToolbarItem::alloc(ui.mtm()),
            &NSString::from_str("cgs.forward"),
        );
        next.setLabel(&NSString::from_str("Forward"));
        next.setToolTip(Some(&NSString::from_str("Forward")));
        next.setImage(Symbol::named("chevron.forward").as_deref());
        next.setNavigational(true);
        next.setAutovalidates(false);
        next.setBordered(false);
        unsafe {
            next.setTarget(Some(&self.forward_target));
            next.setAction(Some(objc2::sel!(invoke:)));
        }
        let arrows = NSToolbarItemGroup::initWithItemIdentifier(
            NSToolbarItemGroup::alloc(ui.mtm()), &NSString::from_str("cgs.history"));
        arrows.setLabel(&NSString::from_str("Back and Forward"));
        arrows.setNavigational(true);
        arrows.setBordered(true);
        arrows.setSelectionMode(NSToolbarItemGroupSelectionMode::Momentary);
        arrows.setControlRepresentation(NSToolbarItemGroupControlRepresentation::Expanded);
        arrows.setSubitems(&NSArray::from_retained_slice(&[delegate.ivars().back.clone(), next.clone()]));
        let title = NSMenuToolbarItem::initWithItemIdentifier(
            NSMenuToolbarItem::alloc(ui.mtm()),
            &NSString::from_str("cgs.page-menu"),
        );
        title.setMenu(menu.ns_menu());
        // Keep an independent display label while the native menu tracks its selection.
        let placeholder = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(ui.mtm()),
                &NSString::from_str(""),
                None,
                &NSString::from_str(""),
            )
        };
        placeholder.setHidden(true);
        placeholder.setImage(Symbol::named("line.3.horizontal.decrease").as_deref());
        menu.ns_menu().insertItem_atIndex(&placeholder, 0);
        let popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(ui.mtm()),
            CGRect::ZERO,
            false,
        );
        popup.setBordered(true);
        popup.setBezelStyle(crate::control::action_button_bezel());
        popup.setImagePosition(NSCellImagePosition::ImageLeading);
        popup.setFont(Some(&crate::Font::body()));
        popup.setMenu(Some(menu.ns_menu()));
        if let Some(cell) = popup.cell().and_then(|cell| cell.downcast::<NSPopUpButtonCell>().ok()) {
            cell.setUsesItemFromMenu(false);
            cell.setMenuItem(Some(&placeholder));
            cell.setAltersStateOfSelectedItem(false);
            cell.setImage(Symbol::named("line.3.horizontal.decrease").as_deref());
        }
        unsafe { menu.ns_menu().setFont(Some(&crate::Font::body())); }
        title.setView(Some(&popup));
        title.setBordered(true);
        title.setAutovalidates(false);
        delegate.ivars().back.setBordered(false);
        delegate.ivars().back.setToolTip(Some(&NSString::from_str("Back")));
        *delegate.ivars().navigation.borrow_mut() = vec![next, title.into_super(), arrows.into_super()];
        *self.menu.borrow_mut() = Some((menu, popup));
        let index = self
            .native
            .items()
            .iter()
            .position(|item| item.itemIdentifier().to_string() == "cgs.page-title")
            .unwrap_or(0);
        self.native.insertItemWithItemIdentifier_atIndex(
            &NSString::from_str("cgs.history"), index as isize);

    }

    pub fn update_navigation(&self, back: bool, forward: bool, title: &str, has_menu: bool) {
        let Some(delegate) = &self.delegate else {
            return;
        };
        delegate.ivars().back.setEnabled(back);
        let items = delegate.ivars().navigation.borrow();
        if let Some(item) = items.first() {
            item.setEnabled(forward);
        }
        if let Some(item) = items.get(1).and_then(|item| item.downcast_ref::<NSMenuToolbarItem>()) {
            item.setLabel(&NSString::from_str(title));
            if let Some((menu, popup)) = self.menu.borrow().as_ref() {
                for entry in menu.ns_menu().itemArray() {
                    if entry.tag() == 1 || entry.tag() >= 9 {
                        entry.setState(if entry.title().to_string() == title {
                            NSControlStateValueOn
                        } else {
                            NSControlStateValueOff
                        });
                    }
                }
                if let Some(cell) = popup.cell().and_then(|cell| cell.downcast::<NSPopUpButtonCell>().ok()) {
                    if let Some(display) = cell.menuItem() { display.setTitle(&NSString::from_str(title)); }
                }
                popup.sizeToFit();
                item.setView(Some(popup));
            }
            item.setShowsIndicator(true);
        }
        let index = self.native.items().iter().position(|item| item.itemIdentifier().to_string() == "cgs.page-menu");
        if has_menu && index.is_none() {
            self.native.insertItemWithItemIdentifier_atIndex(&NSString::from_str("cgs.page-menu"), self.native.items().len() as isize);
        } else if !has_menu {
            if let Some(index) = index { self.native.removeItemAtIndex(index as isize); }
        }
    }
    /// Native trailing filter and search controls owned by the selected page.
    pub fn set_page_controls(&self, ui: &Ui, controls: Option<(&NSPopUpButton, &NSSearchField)>) {
        let Some(delegate) = &self.delegate else { return; };
        for id in ["cgs.page-filter", "cgs.page-search"] {
            let id = NSString::from_str(id);
            if let Some(index) = self.native.items().iter().position(|item| item.itemIdentifier() == id) {
                if let Some(glass) = self.native.items().objectAtIndex(index).view().and_then(|view| view.downcast::<NSGlassEffectView>().ok()) {
                    glass.setContentView(None);
                }
                self.native.removeItemAtIndex(index as isize);
            }
            delegate.ivars().navigation.borrow_mut().retain(|item| item.itemIdentifier() != id);
        }
        if let Some((filter, search)) = controls {
            let item = NSMenuToolbarItem::initWithItemIdentifier(
                NSMenuToolbarItem::alloc(ui.mtm()), &NSString::from_str("cgs.page-filter"));
            item.setLabel(&NSString::from_str("Filter rules"));
            let content: Retained<NSView> = unsafe { Retained::retain(filter as *const NSPopUpButton as *mut NSView).unwrap() };
            let glass = GlassEffectView::new(ui, content).corner_radius(18.0);
            glass.width(36.0);
            glass.height(36.0);
            item.setView(Some(glass.ns_view()));
            if let Some(menu) = filter.menu() { item.setMenu(&menu); }
            item.setShowsIndicator(false);
            item.setBordered(false);
            let search_item = NSSearchToolbarItem::initWithItemIdentifier(
                NSSearchToolbarItem::alloc(ui.mtm()), &NSString::from_str("cgs.page-search"));
            search_item.setLabel(&NSString::from_str("Search rules"));
            search_item.setSearchField(search);
            search_item.setPreferredWidthForSearchField(216.0);
            delegate.ivars().navigation.borrow_mut().extend([item.into_super(), search_item.into_super()]);
            for id in ["cgs.page-filter", "cgs.page-search"] {
                self.native.insertItemWithItemIdentifier_atIndex(&NSString::from_str(id), self.native.items().len() as isize);
            }
        }
    }

    pub fn ns_toolbar(&self) -> &NSToolbar { &self.native }
}

impl Drop for Toolbar {
    fn drop(&mut self) {
        if let Some(delegate) = &self.delegate {
            unsafe {
                delegate.ivars().back.setTarget(None);
                delegate.ivars().back.setAction(None);
                for item in delegate.ivars().navigation.borrow().iter() {
                    if item.itemIdentifier().to_string() == "cgs.page-filter" {
                        if let Some(glass) = item.view().and_then(|view| view.downcast::<NSGlassEffectView>().ok()) { glass.setContentView(None); }
                        item.setView(None);
                    }
                    item.setTarget(None);
                    item.setAction(None);
                }
            }
        }
        self.native.setDelegate(None);
    }
}
