use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadOnly, define_class, msg_send};
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
pub struct Sidebar<T: 'static> {
    table: Table<SidebarItem<T>>,
}
impl<T: Clone + 'static> Sidebar<T> {
    pub fn new(ui: &Ui, items: Vec<SidebarItem<T>>) -> Self {
        let ui_copy = *ui;
        // Let system fonts and symbol sizes determine the content minimum, so
        // navigation labels remain readable even at the platform's narrow sidebar size.
        let content_width = items
            .iter()
            .map(|item| {
                let label = Label::new(ui, &item.title).ns_view().fittingSize().width;
                let icon = ImageView::symbol(ui, &item.symbol)
                    .map_or(0.0, |image| image.ns_view().fittingSize().width);
                label + icon + Metrics::CONTROL_SPACING * 4.0
            })
            .fold(0.0, f64::max);
        let table = Table::<SidebarItem<T>>::new(ui)
            .column("page", "", 0.0)
            .cells(move |item, _| {
                let row = HStack::new(&ui_copy);
                let row = if let Some(image) = ImageView::symbol(&ui_copy, &item.symbol) {
                    row.push(image)
                } else {
                    row
                };
                Box::new(row.push(Label::new(&ui_copy, &item.title)))
            })
            .rows(items);
        table.ns_table_view().setHeaderView(None);
        table.ns_table_view().setStyle(NSTableViewStyle::SourceList);
        table.ns_table_view().setUsesAutomaticRowHeights(false);
        table.ns_table_view().setRowHeight(26.0);
        table.ns_table_view().setBackgroundColor(&NSColor::clearColor());
        table.ns_scroll_view().setDrawsBackground(false);
        table.ns_scroll_view().setBorderType(NSBorderType::NoBorder);
        table.min_width(content_width);
        table.max_width(content_width + Metrics::PAGE_INSET * 2.0);
        Self { table }
    }

    pub fn on_select(mut self, mut f: impl FnMut(T) + 'static) -> Self {
        self.table = self.table.on_select_item(move |item| {
            if let Some(item) = item {
                f(item.id);
            }
        });
        self
    }

    pub fn set_selected(&self, index: usize) { self.table.set_selected(Some(index)); }

    pub fn ns_table_view(&self) -> &NSTableView { self.table.ns_table_view() }
}
impl<T: 'static> NativeView for Sidebar<T> {
    fn ns_view(&self) -> &NSView { self.table.ns_view() }
}
pub struct NavigationSplitView(SplitView);
impl NavigationSplitView {
    pub fn new(ui: &Ui, sidebar: impl NativeView, detail: impl NativeView) -> Self {
        Self(SplitView::new(ui).pane(ui, sidebar, true).pane(ui, detail, false))
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

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsNavigationToolbarDelegate"]
    struct NavigationToolbarDelegate;
    unsafe impl NSObjectProtocol for NavigationToolbarDelegate {}
    unsafe impl NSToolbarDelegate for NavigationToolbarDelegate {
        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            NSArray::from_slice(&[unsafe { NSToolbarToggleSidebarItemIdentifier }, unsafe {
                NSToolbarFlexibleSpaceItemIdentifier
            }])
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn defaults(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            NSArray::from_slice(&[unsafe { NSToolbarToggleSidebarItemIdentifier }, unsafe {
                NSToolbarFlexibleSpaceItemIdentifier
            }])
        }
    }
);

pub struct Toolbar {
    native: Retained<NSToolbar>,
    delegate: Option<Retained<NavigationToolbarDelegate>>,
}
impl Toolbar {
    pub fn new(ui: &Ui, id: &str) -> Self {
        Self {
            native: NSToolbar::initWithIdentifier(
                NSToolbar::alloc(ui.mtm()),
                &NSString::from_str(id),
            ),
            delegate: None,
        }
    }

    pub fn navigation(ui: &Ui, id: &str) -> Self {
        let mut toolbar = Self::new(ui, id);
        let delegate: Retained<NavigationToolbarDelegate> = unsafe {
            msg_send![
                super(NavigationToolbarDelegate::alloc(ui.mtm()).set_ivars(())),
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

    pub fn ns_toolbar(&self) -> &NSToolbar { &self.native }
}
