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
