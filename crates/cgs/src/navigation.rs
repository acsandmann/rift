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
            if identifier.to_string() == "cgs.back" {
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
                        item.setView(Some(title.ns_view()));
                        item.setBordered(false);
                        item
                    })
            }
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            let mut ids: Vec<_> = self.item_identifiers().iter().collect();
            ids.push(NSString::from_str("cgs.back"));
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
                super(
                    NavigationToolbarDelegate::alloc(ui.mtm())
                        .set_ivars(NavigationToolbarItems { title, back })
                ),
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

    pub fn ns_toolbar(&self) -> &NSToolbar { &self.native }
}

impl Drop for Toolbar {
    fn drop(&mut self) {
        if let Some(delegate) = &self.delegate {
            unsafe {
                delegate.ivars().back.setTarget(None);
                delegate.ivars().back.setAction(None);
            }
        }
        self.native.setDelegate(None);
    }
}
