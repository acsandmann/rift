use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cgs::*;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSControl, NSControlStateValueOn, NSControlTextEditingDelegate,
    NSDragOperation, NSDraggingInfo, NSEvent, NSEventType, NSPasteboard, NSPasteboardWriting,
    NSTableViewDataSource, NSTableViewDropOperation,
};
use objc2_foundation::{NSArray, NSNotification, NSObject, NSObjectProtocol, NSString};

fn action(control: &NSControl) {
    assert!(unsafe { control.sendAction_to(control.action(), control.target().as_deref()) });
}

fn callbacks_survive_composition_and_release_with_the_page(ui: &Ui) {
    let changes = Rc::new(RefCell::new(Vec::new()));
    let host = PageHost::new(ui);
    let native =
        autoreleasepool(|_| {
            let received = changes.clone();
            let switch = Switch::new(ui).on_change(move |value| received.borrow_mut().push(value));
            let native = Weak::new(switch.ns_switch());
            host.set_page(SettingsPage::new(ui, "General").section(
                Section::new(ui, "Behavior").row(SwitchRow::new(ui, "Animations", switch)),
            ));
            native
        });
    autoreleasepool(|_| {
        let switch = native.load().unwrap();
        switch.setState(NSControlStateValueOn);
        action(&switch);
        assert_eq!(*changes.borrow(), [true]);
    });
    autoreleasepool(|_| host.set_page(Label::new(ui, "Another page")));
    assert!(
        native.load().is_none(),
        "replacing a page must release its controls"
    );
    assert_eq!(Rc::strong_count(&changes), 1, "action closure must be released");

    let clicks = Rc::new(Cell::new(0));
    let c = clicks.clone();
    let menu = Menu::new(ui).item(MenuItem::new(ui, "Reset").on_click(move || c.set(c.get() + 1)));
    autoreleasepool(|_| {
        menu.ns_menu().performActionForItemAtIndex(0);
        assert_eq!(clicks.get(), 1);
        menu.clear();
    });
    assert_eq!(Rc::strong_count(&clicks), 1);
}

fn callbacks_can_remove_their_own_controls(ui: &Ui) {
    let slot = Rc::new(RefCell::new(None::<Button>));
    let owner = Rc::downgrade(&slot);
    let button = Button::new(ui, "Remove").on_click(move || {
        owner.upgrade().unwrap().borrow_mut().take();
    });
    let native = button.ns_button().retain();
    let target = autoreleasepool(|_| Weak::new(&*native.target().unwrap()));
    *slot.borrow_mut() = Some(button);
    autoreleasepool(|_| action(&native));
    assert!(slot.borrow().is_none());
    assert!(
        target.load().is_none(),
        "target must finish dispatch before deallocating"
    );
}

fn numeric_fields_reject_invalid_commits(ui: &Ui) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let copy = values.clone();
    let number = NumberField::new(ui)
        .integer()
        .range(1.0, 10.0)
        .on_change(move |value| copy.borrow_mut().push(value));
    let field = number.ns_text_field();
    for text in ["4", "11", "not a number", "2.5", "-1"] {
        field.setStringValue(&NSString::from_str(text));
        let note = unsafe {
            NSNotification::notificationWithName_object(
                &NSString::from_str("NSControlTextDidEndEditingNotification"),
                Some(field),
            )
        };
        field.delegate().unwrap().controlTextDidEndEditing(&note);
    }
    assert_eq!(*values.borrow(), [4.0]);
    let number = NumberField::new(ui).range(0.0, 1.0).value(0.25);
    assert_eq!(number.get_value(), Some(0.25));
}

fn delegates_and_selection_use_current_data(ui: &Ui) {
    let selected = Rc::new(RefCell::new(Vec::new()));
    let s = selected.clone();
    let ui_copy = *ui;
    let table = Table::new(ui)
        .column("name", "Name", 240.0)
        .cells(move |value: &String, _| Box::new(Label::new(&ui_copy, value)))
        .rows(vec!["one".into(), "two".into()])
        .on_select_item(move |value| s.borrow_mut().push(value));
    table.set_selected(Some(1));
    assert_eq!(selected.borrow().last(), Some(&Some("two".to_string())));
    table.set_rows(vec!["new".into()]);
    table.set_selected(Some(0));
    assert_eq!(selected.borrow().last(), Some(&Some("new".to_string())));

    let edits = Rc::new(RefCell::new(Vec::new()));
    let e = edits.clone();
    let commits = Rc::new(Cell::new(0));
    let committed = commits.clone();
    let field = TextField::new(ui)
        .on_change(move |text| e.borrow_mut().push(text))
        .on_commit(move |_| committed.set(committed.get() + 1));
    field.set_value("edited");
    let note = unsafe {
        NSNotification::notificationWithName_object(
            &NSString::from_str("NSControlTextDidChangeNotification"),
            Some(field.ns_text_field()),
        )
    };
    field.ns_text_field().delegate().unwrap().controlTextDidChange(&note);
    assert_eq!(*edits.borrow(), ["edited"]);
    field.ns_text_field().delegate().unwrap().controlTextDidEndEditing(&note);
    assert_eq!(commits.get(), 1);

    let outline = Outline::new(ui).items(vec![OutlineItem {
        id: 1,
        title: "Group".into(),
        children: vec![OutlineItem {
            id: 2,
            title: "Child".into(),
            children: vec![],
        }],
    }]);
    outline.expand_all();
    assert_eq!(outline.ns_outline_view().numberOfRows(), 2);
}

fn event(code: u16, chars: &str, modifiers: Modifiers) -> Retained<NSEvent> {
    NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyDown, CGPoint::ZERO, modifiers, 0.0, 0, None,
        &NSString::from_str(chars), &NSString::from_str(chars), false, code,
    ).unwrap()
}

fn recording_is_scoped_and_cancellable(ui: &Ui) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let v = values.clone();
    let recorder = KeyRecorder::new(ui).on_change(move |value| v.borrow_mut().push(value));
    recorder.begin_recording();
    assert!(recorder.ns_button().performKeyEquivalent(&event(
        4,
        "h",
        Modifiers::Control | Modifiers::Option | Modifiers::Shift | Modifiers::Command,
    )));
    assert!(!recorder.ns_button().performKeyEquivalent(&event(4, "h", Modifiers::Command)));
    let saved = recorder.get_value().unwrap();
    assert_eq!(saved.to_string(), "⌃⌥⇧⌘H");
    assert_eq!(values.borrow().len(), 1);
    recorder.begin_recording();
    recorder.ns_button().keyDown(&event(53, "\u{1b}", Modifiers::empty()));
    assert_eq!(recorder.get_value(), Some(saved));
    assert_eq!(
        values.borrow().len(),
        1,
        "Escape cancels without publishing a change"
    );
    recorder.begin_recording();
    recorder.ns_button().keyDown(&event(51, "\u{7f}", Modifiers::empty()));
    assert!(recorder.get_value().is_none());
    assert_eq!(values.borrow().last(), Some(&None));
}

struct DragState {
    source: Retained<AnyObject>,
    pasteboard: Retained<NSPasteboard>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiTestDragInfo"]
    #[ivars = DragState]
    struct DragInfo;
    unsafe impl NSObjectProtocol for DragInfo {}
    unsafe impl NSDraggingInfo for DragInfo {
        #[unsafe(method(animatesToDestination))]
        fn animates(&self) -> bool { false }

        #[unsafe(method(setAnimatesToDestination:))]
        fn set_animates(&self, _value: bool) {}

        #[unsafe(method_id(draggingDestinationWindow))]
        fn destination(&self) -> Option<Retained<objc2_app_kit::NSWindow>> { None }

        #[unsafe(method(draggingSourceOperationMask))]
        fn operations(&self) -> NSDragOperation { NSDragOperation::Move }

        #[unsafe(method(draggingLocation))]
        fn location(&self) -> CGPoint { CGPoint::ZERO }

        #[unsafe(method(draggedImageLocation))]
        fn image_location(&self) -> CGPoint { CGPoint::ZERO }

        #[unsafe(method_id(draggedImage))]
        fn image(&self) -> Option<Retained<objc2_app_kit::NSImage>> { None }

        #[unsafe(method(draggingSequenceNumber))]
        fn sequence(&self) -> isize { 0 }

        #[unsafe(method(slideDraggedImageTo:))]
        fn slide(&self, _point: CGPoint) {}

        #[unsafe(method_id(namesOfPromisedFilesDroppedAtDestination:))]
        fn promised_files(
            &self,
            _url: &objc2_foundation::NSURL,
        ) -> Option<Retained<NSArray<NSString>>> {
            None
        }

        #[unsafe(method(draggingFormation))]
        fn formation(&self) -> objc2_app_kit::NSDraggingFormation {
            objc2_app_kit::NSDraggingFormation::None
        }

        #[unsafe(method(setDraggingFormation:))]
        fn set_formation(&self, _value: objc2_app_kit::NSDraggingFormation) {}

        #[unsafe(method(numberOfValidItemsForDrop))]
        fn valid_items(&self) -> isize { 1 }

        #[unsafe(method(setNumberOfValidItemsForDrop:))]
        fn set_valid_items(&self, _value: isize) {}

        #[unsafe(method(springLoadingHighlight))]
        fn highlight(&self) -> objc2_app_kit::NSSpringLoadingHighlight {
            objc2_app_kit::NSSpringLoadingHighlight::None
        }

        #[unsafe(method(resetSpringLoading))]
        fn reset_spring_loading(&self) {}

        #[unsafe(method(enumerateDraggingItemsWithOptions:forView:classes:searchOptions:usingBlock:))]
        unsafe fn enumerate(
            &self,
            _opts: objc2_app_kit::NSDraggingItemEnumerationOptions,
            _view: Option<&objc2_app_kit::NSView>,
            _classes: &NSArray<objc2::runtime::AnyClass>,
            _options: &objc2_foundation::NSDictionary<NSString, AnyObject>,
            _block: &block2::DynBlock<
                dyn Fn(
                    std::ptr::NonNull<objc2_app_kit::NSDraggingItem>,
                    isize,
                    std::ptr::NonNull<objc2::runtime::Bool>,
                ),
            >,
        ) {
        }

        #[unsafe(method_id(draggingSource))]
        fn source(&self) -> Option<Retained<AnyObject>> { Some(self.ivars().source.clone()) }

        #[unsafe(method_id(draggingPasteboard))]
        fn pasteboard(&self) -> Retained<NSPasteboard> { self.ivars().pasteboard.clone() }
    }
);

fn local_reordering_uses_final_indices_and_rejects_other_tables(ui: &Ui) {
    let moved = Rc::new(RefCell::new(Vec::new()));
    let m = moved.clone();
    let selected = Rc::new(RefCell::new(None));
    let selected_copy = selected.clone();
    let table = Table::new(ui)
        .column("item", "", 200.0)
        .rows(vec!["a", "b", "c"])
        .reorderable(true)
        .on_reorder(move |from, to| m.borrow_mut().push((from, to)))
        .on_select_item(move |value| *selected_copy.borrow_mut() = value);
    let data_source = unsafe { table.ns_table_view().dataSource() }.unwrap();
    let item = data_source.tableView_pasteboardWriterForRow(table.ns_table_view(), 0).unwrap();
    let pasteboard = NSPasteboard::pasteboardWithUniqueName();
    assert!(pasteboard.writeObjects(
        &NSArray::<ProtocolObject<dyn NSPasteboardWriting>>::from_slice(&[&*item])
    ));
    let drag = DragInfo::alloc(ui.mtm()).set_ivars(DragState {
        source: table.ns_table_view().retain().into(),
        pasteboard,
    });
    let drag: Retained<DragInfo> = unsafe { msg_send![super(drag), init] };
    let info = ProtocolObject::from_ref(&*drag);
    assert_eq!(
        data_source.tableView_validateDrop_proposedRow_proposedDropOperation(
            table.ns_table_view(),
            info,
            3,
            NSTableViewDropOperation::Above
        ),
        NSDragOperation::Move
    );
    assert!(data_source.tableView_acceptDrop_row_dropOperation(
        table.ns_table_view(),
        info,
        3,
        NSTableViewDropOperation::Above
    ));
    assert_eq!(*moved.borrow(), [(0, 2)]);
    assert_eq!(*selected.borrow(), Some("a"));
    table.set_selected(Some(0));
    assert_eq!(*selected.borrow(), Some("b"));
    let other = Table::<&str>::new(ui).reorderable(true);
    let other_source = unsafe { other.ns_table_view().dataSource() }.unwrap();
    assert_eq!(
        other_source.tableView_validateDrop_proposedRow_proposedDropOperation(
            other.ns_table_view(),
            info,
            0,
            NSTableViewDropOperation::Above
        ),
        NSDragOperation::None
    );
}

fn pages_start_at_top_and_controllers_leave_the_host(ui: &Ui) {
    let mut section = Section::new(ui, "Long page");
    for _ in 0..30 {
        section = section.row(SwitchRow::new(ui, "Option", Switch::new(ui)));
    }
    let page = SettingsPage::new(ui, "Settings").section(section);
    let scroll = Weak::new(page.ns_scroll_view());
    let window = Window::new(ui).size(CGSize::new(640.0, 400.0)).content(page);
    window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
    let scroll = scroll.load().unwrap();
    let document = scroll.documentView().unwrap();
    assert!(document.isFlipped());
    assert!(document.frame().size.height > scroll.contentView().bounds().size.height);
    assert!(
        scroll.documentVisibleRect().origin.y.abs() < 1.0,
        "new pages must begin at the top"
    );
    let host = Rc::new(PageHost::new(ui));
    let navigation = NavigationSplitView::new(
        ui,
        Sidebar::new(ui, vec![SidebarItem {
            id: 0,
            title: "General".into(),
            symbol: "gearshape".into(),
        }]),
        host.clone(),
    );
    let _window = SettingsWindow::new(ui, "cgs tests").content(navigation);
    let controller = ViewController::new(ui, Label::new(ui, "Child"));
    let child = Weak::new(controller.ns_view_controller());
    host.set_page(controller);
    autoreleasepool(|_| {
        assert!(child.load().unwrap().parentViewController().is_some());
        host.set_page(Label::new(ui, "Replacement"));
        assert!(host.ns_view_controller().childViewControllers().is_empty());
    });
}

fn settings_lists_do_not_materialize_offscreen_rows(ui: &Ui) {
    let configured = Rc::new(Cell::new(0));
    let count = configured.clone();
    let list = SettingsList::new(
        ui,
        move |row: &usize| {
            count.set(count.get() + 1);
            format!("Layout {row}")
        },
        |_| "Description".into(),
    )
    .navigation()
    .fit_content(120.0);
    list.set_rows((0..200).collect());
    assert_eq!(list.ns_table_view().rectOfRow(0).size.height, 56.0);
    assert!(
        configured.get() < 32,
        "sizing a capped list created {} row hierarchies",
        configured.get()
    );
    let configured = Rc::new(Cell::new(0));
    let count = configured.clone();
    let list = Rc::new(
        SettingsList::new(
            ui,
            move |row: &usize| {
                count.set(count.get() + 1);
                format!("Rule {row}")
            },
            |_| "Description".into(),
        )
        .navigation()
        .full_length(),
    );
    list.set_rows((0..200).collect());
    let page = SettingsPage::new(ui, "").section(list.clone());
    let window = Window::new(ui).size(CGSize::new(600.0, 300.0)).content(page);
    window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
    let clip = list.ns_scroll_view().contentView().bounds();
    let first = list.ns_table_view().rectOfRow(0);
    let last = list.ns_table_view().rectOfRow(199);
    assert_eq!(clip.origin.y, 0.0, "full-length lists must stay at the top");
    assert!(
        clip.size.height >= last.origin.y + last.size.height + first.origin.y,
        "the viewport must include the final row and symmetric native padding"
    );
    assert!(
        !list.ns_scroll_view().hasVerticalScroller(),
        "the page must own scrolling"
    );
    assert!(
        list.ns_view().frame().size.height > 10_000.0,
        "all rows must contribute to page height"
    );
    assert!(
        configured.get() < 32,
        "full-length page materialized {} offscreen rows",
        configured.get()
    );
}

fn unchanged_popup_items_preserve_selection_and_native_items(ui: &Ui) {
    let popup = Popup::new(ui).items(["One", "Two"]);
    popup.set_selected(1);
    let first = popup.ns_popup_button().itemAtIndex(0).unwrap();
    popup.set_items(["One", "Two"]);
    assert_eq!(popup.selected(), Some(1));
    assert!(std::ptr::eq(
        &*first,
        &*popup.ns_popup_button().itemAtIndex(0).unwrap()
    ));
}

fn page_headings_preserve_window_identity(ui: &Ui) {
    let title = Rc::new(Label::new(ui, "General"));
    let window = SettingsWindow::new(ui, "Rift Settings").page_title(ui, title.clone());
    let items = window.ns_window().toolbar().unwrap().items();
    let item = items
        .iter()
        .find(|item| item.itemIdentifier().to_string() == "cgs.page-title")
        .expect("the page heading must be installed in the native toolbar");
    let heading = item.view().unwrap();
    title.set_text("Keyboard");
    assert_eq!(
        heading
            .downcast_ref::<objc2_app_kit::NSTextField>()
            .unwrap()
            .stringValue()
            .to_string(),
        "Keyboard"
    );
    assert_eq!(window.ns_window().title().to_string(), "Rift Settings");
}

fn cached_pages_keep_their_mount_and_release_on_clear(ui: &Ui) {
    let host = PageHost::new(ui);
    let (first, second) = autoreleasepool(|_| {
        let first = Rc::new(Label::new(ui, "First"));
        let second = Rc::new(Label::new(ui, "Second"));
        host.set_cached_page(first.clone());
        let original = host.ns_view().constraints().firstObject().unwrap();
        host.set_cached_page(second.clone());
        assert!(first.ns_view().isHidden());
        host.set_cached_page(first.clone());
        assert!(!first.ns_view().isHidden());
        assert!(second.ns_view().isHidden());
        assert!(
            std::ptr::eq(&*original, &*host.ns_view().constraints().firstObject().unwrap()),
            "returning to a cached page must preserve its mount"
        );
        host.set_cached_page(first.clone());
        assert!(std::ptr::eq(
            &*original,
            &*host.ns_view().constraints().firstObject().unwrap()
        ));
        (Weak::new(first.ns_view()), Weak::new(second.ns_view()))
    });
    autoreleasepool(|_| host.clear());
    assert!(first.load().is_none(), "clear must release the active page");
    assert!(
        second.load().is_none(),
        "clear must release inactive cached pages"
    );
    assert!(host.ns_view().subviews().is_empty());
}

fn main() {
    let ui =
        Ui::new(MainThreadMarker::new().expect("native tests must run on the macOS main thread"));
    let app = Application::shared(&ui);
    app.ns_application()
        .setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    autoreleasepool(|_| {
        page_headings_preserve_window_identity(&ui);
        cached_pages_keep_their_mount_and_release_on_clear(&ui);
        callbacks_survive_composition_and_release_with_the_page(&ui);
        callbacks_can_remove_their_own_controls(&ui);
        settings_lists_do_not_materialize_offscreen_rows(&ui);
        unchanged_popup_items_preserve_selection_and_native_items(&ui);
        numeric_fields_reject_invalid_commits(&ui);
        delegates_and_selection_use_current_data(&ui);
        recording_is_scoped_and_cancellable(&ui);
        local_reordering_uses_final_indices_and_rejects_other_tables(&ui);
        pages_start_at_top_and_controllers_leave_the_host(&ui);
    });
    println!("native ownership, callbacks, delegates, selection and shortcut recording passed");
}
