use cgs::controls::SheetActions;
use cgs::layout::VStack;
use cgs::settings::Section;
use cgs::text::SectionTitle;
use cgs::view::NativeView;
use cgs::{CGPoint, CGRect, CGSize, Insets, Ui};
use objc2::Message;
use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSButton, NSControl, NSPopUpButton, NSSegmentedControl,
    NSSlider, NSSwitch, NSTableView, NSView,
};

use super::*;

fn find<T: objc2::DowncastTarget + 'static>(view: &NSView) -> Option<Retained<T>> {
    if let Some(value) = view.downcast_ref::<T>() {
        return Some(value.retain());
    }
    view.subviews().iter().find_map(|child| find(&child))
}

/// Visible instances only: cached pages stay mounted but hidden.
fn count<T: objc2::DowncastTarget + 'static>(view: &NSView) -> usize {
    if view.isHidden() {
        return 0;
    }
    usize::from(view.downcast_ref::<T>().is_some())
        + view.subviews().iter().map(|child| count::<T>(&child)).sum::<usize>()
}

fn named<T: objc2::DowncastTarget + 'static>(view: &NSView, title: &str) -> Option<Retained<T>> {
    if view.isHidden() {
        return None;
    }
    if view.accessibilityLabel().is_some_and(|label| label.to_string() == title)
        && let Some(value) = view.downcast_ref::<T>()
    {
        return Some(value.retain());
    }
    if let Some(table) = view.downcast_ref::<NSTableView>() {
        for row in 0..table.numberOfRows() {
            if let Some(cell) = table.viewAtColumn_row_makeIfNecessary(0, row, true) {
                if let Some(value) = named(&cell, title) {
                    return Some(value);
                }
            }
        }
    }
    view.subviews().iter().find_map(|child| named(&child, title))
}
fn navigation_row(table: &NSTableView, title: &str) -> usize {
    (0..table.numberOfRows())
        .find(|&row| {
            table
                .viewAtColumn_row_makeIfNecessary(0, row, true)
                .and_then(|view| find::<objc2_app_kit::NSTextField>(&view))
                .is_some_and(|field| field.stringValue().to_string() == title)
        })
        .unwrap_or_else(|| panic!("missing settings destination: {title}")) as usize
}

/// A layout tile, whose accessible name is its title, plus “, default” on the default.
fn tile(view: &NSView, title: &str) -> Option<Retained<NSButton>> {
    named::<NSButton>(view, title).or_else(|| named::<NSButton>(view, &format!("{title}, default")))
}

/// Choose `index` in a native popup as a click would.
fn choose(popup: &NSPopUpButton, index: isize) {
    popup.selectItemAtIndex(index);
    let control: &NSControl = popup;
    assert!(unsafe { control.sendAction_to(control.action(), control.target().as_deref()) });
}

fn root_window(settings: &Settings) -> Retained<NSView> {
    settings.window.ns_window().contentView().unwrap()
}

fn editable(view: &NSView) -> Option<Retained<objc2_app_kit::NSTextField>> {
    if let Some(field) = view.downcast_ref::<objc2_app_kit::NSTextField>()
        && field.isEditable()
    {
        return Some(field.retain());
    }
    view.subviews().iter().find_map(|child| editable(&child))
}
fn change_text(field: &objc2_app_kit::NSTextField, text: &str) {
    field.setStringValue(&objc2_foundation::NSString::from_str(text));
    unsafe {
        let note = objc2_foundation::NSNotification::notificationWithName_object(
            &objc2_foundation::NSString::from_str("NSControlTextDidChangeNotification"),
            Some(field),
        );
        let _: () = objc2::msg_send![&*field.delegate().unwrap(), controlTextDidChange: &*note];
    }
}

pub fn run(ui: Ui) {
    let (requests, mut pending) = tokio::sync::mpsc::unbounded_channel();
    let (model, host) = autoreleasepool(|_| {
        let mut source = source();
        source.settings.layout.base.window_insertion_point =
            Some(crate::common::config::WindowInsertionPoint::NextToSelection);
        let settings = Settings::new(
            ui,
            source,
            "/tmp/native-settings-test.toml".into(),
            vec![],
            vec![],
            requests,
            || {},
        );
        // An acknowledged source is stale while either of these edits is outstanding.
        // A quick return to the original value must still reach the session queue.
        let builder = FormBuilder::new(ui, settings.model());
        let change = builder.assign(
            |s| s.settings.animate,
            |s, value| s.settings.animate = value,
            Weak::new(),
        );
        let original = settings.model().source.borrow().settings.animate;
        change(!original);
        change(original);
        let first = pending.try_recv().expect("first assignment");
        let last = pending.try_recv().expect("return to original must not be discarded");
        assert!(Rc::ptr_eq(
            first.control.as_ref().unwrap(),
            last.control.as_ref().unwrap()
        ));
        let mut edited = settings.model().source.borrow().as_ref().clone();
        for request in [first, last] {
            let Action::Edit(edit) = request.action else {
                panic!("assignment")
            };
            edit(&mut edited).unwrap();
            (request.finish)(Ok(()));
        }
        assert_eq!(edited.settings.animate, original);

        let root = settings.window.ns_window().contentView().unwrap();
        assert_eq!(
            settings
                .window
                .ns_window()
                .contentRectForFrameRect(settings.window.ns_window().frame())
                .size,
            CGSize::new(880.0, 660.0)
        );
        assert!(
            settings.router.layouts.get().is_none(),
            "startup does not construct the Layouts workspace"
        );
        late_runtime_populates_picker(&settings, &mut pending);
        let search = find::<objc2_app_kit::NSSearchField>(&root).unwrap();
        change_text(&search, "focus follows pointer");
        assert!(
            named::<NSTableView>(&root, "Settings categories").is_none(),
            "search replaces the sidebar"
        );
        change_text(&search, "   ");
        assert!(
            named::<NSTableView>(&root, "Settings categories").is_some(),
            "clearing search restores navigation"
        );
        change_text(&search, "this setting does not exist");
        assert!(named::<NSTableView>(&root, "Settings categories").is_none());
        change_text(&search, "gaps");
        assert!(
            named::<NSTableView>(&root, "Settings categories").is_none(),
            "search works after an empty result"
        );
        change_text(&search, "");
        let sidebar = named::<NSTableView>(&root, "Settings categories").unwrap();
        sidebar.selectRowIndexes_byExtendingSelection(
            &objc2_foundation::NSIndexSet::indexSetWithIndex(navigation_row(&sidebar, "Layouts")),
            false,
        );
        let toolbar = settings.window.toolbar().ns_toolbar();
        let items = toolbar.items();
        let arrows_index = items
            .iter()
            .position(|item| item.itemIdentifier().to_string() == "cgs.history")
            .unwrap();
        let title_index = items
            .iter()
            .position(|item| item.itemIdentifier().to_string() == "cgs.page-title")
            .unwrap();
        assert!(
            arrows_index < title_index,
            "history arrows precede the page title"
        );
        let arrows = items
            .objectAtIndex(arrows_index)
            .downcast::<objc2_app_kit::NSToolbarItemGroup>()
            .unwrap();
        let back = arrows.subitems().objectAtIndex(0);
        let forward = arrows.subitems().objectAtIndex(1);
        let press = |item: &objc2_app_kit::NSToolbarItem| {
            assert!(item.isEnabled());
            assert!(unsafe {
                NSApplication::sharedApplication(ui.mtm()).sendAction_to_from(
                    item.action().unwrap(),
                    item.target().as_deref(),
                    Some(item),
                )
            });
        };
        assert!(back.isEnabled());
        assert!(!forward.isEnabled());
        press(&back);
        assert_eq!(settings.router.current(), Category::General);
        assert!(forward.isEnabled());
        press(&forward);
        assert_eq!(settings.router.current(), Category::Layouts);
        // Re-selecting the current destination must not add a duplicate history entry.
        settings.router.navigate(Category::Layouts);
        press(&back);
        assert_eq!(settings.router.current(), Category::General);
        for _ in 0..16 {
            if !back.isEnabled() {
                break;
            }
            press(&back);
        }
        assert!(!back.isEnabled(), "back disables at the start of history");
        assert_eq!(
            settings.router.current(),
            Category::General,
            "history ends at the initial page"
        );
        sidebar.selectRowIndexes_byExtendingSelection(
            &objc2_foundation::NSIndexSet::indexSetWithIndex(navigation_row(&sidebar, "Keyboard")),
            false,
        );
        assert_eq!(settings.router.current(), Category::Keyboard);
        assert!(
            !forward.isEnabled(),
            "a new destination replaces the forward branch"
        );
        press(&back);
        assert_eq!(settings.router.current(), Category::General);
        sidebar.selectRowIndexes_byExtendingSelection(
            &objc2_foundation::NSIndexSet::indexSetWithIndex(navigation_row(&sidebar, "General")),
            false,
        );
        drop(sidebar);

        settings.router.navigate(Category::Keyboard);
        let keyboard = settings.router.pages.borrow()[Category::Keyboard.index()]
            .as_ref()
            .unwrap()
            .view
            .clone();
        settings.refresh_displays(vec![crate::sys::screen::ScreenInfo {
            id: crate::sys::screen::ScreenId::new(1),
            display_uuid: "test-display".into(),
            name: Some("Test display".into()),
            frame: Default::default(),
            is_builtin: false,
            backing_scale: 1.0,
            space: None,
        }]);
        let visible: Vec<_> = settings
            .router
            .host
            .ns_view()
            .subviews()
            .iter()
            .filter(|view| !view.isHidden())
            .collect();
        assert!(
            visible.len() == 1 && std::ptr::eq(&*visible[0], keyboard.ns_view()),
            "a display change off the layout pages keeps the current page"
        );
        settings.refresh_applications(vec![rift_protocol::ApplicationData {
            pid: 1,
            bundle_id: Some("dev.test.Editor".into()),
            name: "Editor".into(),
            is_frontmost: false,
            window_count: 1,
        }]);
        *settings.router.model.env.installed_applications.borrow_mut() = Some(vec![
            ("Editor".into(), "dev.test.Editor".into()),
            ("Utility".into(), "dev.test.Utility".into()),
        ]);
        settings.router.model.env.rebuild_applications();
        assert_eq!(settings.router.model.env.inventory.borrow().len(), 2);
        assert!(
            settings
                .router
                .model
                .env
                .inventory
                .borrow()
                .choices
                .iter()
                .any(|app| app.search.contains("dev.test.editor"))
        );
        // An open editor holds a draft copy, which must receive later discovery results.
        let draft = settings.router.model.draft();
        *settings.router.model.sheet_model.borrow_mut() = Some(draft.clone());
        settings.set_installed_applications(vec![("Late".into(), "dev.test.Late".into())]);
        assert!(
            draft
                .env
                .inventory
                .borrow()
                .choices
                .iter()
                .any(|app| app.search.contains("dev.test.late")),
            "installed applications reach the open editor"
        );
        settings.refresh_applications(Vec::new());
        assert_eq!(
            draft.env.inventory.borrow().len(),
            1,
            "runtime refresh reaches the open editor"
        );
        settings.router.model.sheet_model.borrow_mut().take();
        // Native visual choices select cached editors without changing persisted settings.
        autoreleasepool(|_| {
            settings.router.navigate(Category::Layouts);
            let before = settings.model().source.borrow().as_ref().clone();
            let root = settings.router.layouts().ns_view();
            assert!(
                named::<NSButton>(root, "Layouts help").is_some(),
                "help sits with the page content, not in the toolbar"
            );
            settings.window.ns_window().setContentSize(CGSize::new(880.0, 660.0));
            root.layoutSubtreeIfNeeded();
            fn instances(view: &NSView) -> usize {
                1 + view.subviews().iter().map(|view| instances(&view)).sum::<usize>()
            }
            let window = settings.window.ns_window();
            let content = window.contentView().unwrap();
            // Switching layouts never resizes the window, even at its smallest useful size.
            for size in [CGSize::new(700.0, 420.0), CGSize::new(760.0, 500.0)] {
                window.setContentSize(size);
                content.layoutSubtreeIfNeeded();
                let frame = window.frame();
                for (title, _) in pages::layouts().iter().chain(pages::layouts()) {
                    unsafe { tile(root, title).unwrap().performClick(None) };
                    content.layoutSubtreeIfNeeded();
                    assert_eq!(
                        window.frame(),
                        frame,
                        "{title} keeps the window size at {size:?}"
                    );
                }
            }
            window.setContentSize(CGSize::new(880.0, 660.0));
            content.layoutSubtreeIfNeeded();
            let frame = settings.window.ns_window().frame();
            let mut cached_instances = None;
            for _ in 0..3 {
                for (title, _) in pages::layouts() {
                    let button = tile(root, title).unwrap();
                    unsafe {
                        button.performClick(None);
                    }
                    content.layoutSubtreeIfNeeded();
                    assert_eq!(button.state(), 1);
                    assert_eq!(
                        settings.window.ns_window().frame(),
                        frame,
                        "changing the inspected layout preserves the user’s window size"
                    );
                    assert!(**settings.model().source.borrow() == before);
                    assert!(pending.try_recv().is_err());
                }
                let count = instances(root);
                if let Some(expected) = cached_instances {
                    assert_eq!(count, expected, "switching reuses native views");
                }
                cached_instances = Some(count);
            }
            let spacing = named::<NSButton>(root, "Spacing").unwrap();
            let displays = named::<NSButton>(root, "Display overrides").unwrap();
            assert!(
                named::<NSControl>(root, "Screen edges").is_none(),
                "spacing starts collapsed and lazy"
            );
            root.layoutSubtreeIfNeeded();
            // Labels, the icon, and empty trailing space all hit the native disclosure button.
            let header = unsafe { spacing.superview() }.unwrap();
            for fraction in [0.05, 0.5, 0.95] {
                let rect = header.frame();
                let hit = header
                    .hitTest(cgs::CGPoint::new(
                        rect.origin.x + rect.size.width * fraction,
                        rect.origin.y
                            + spacing.frame().origin.y
                            + spacing.frame().size.height * 0.5,
                    ))
                    .expect("the whole disclosure header is clickable");
                let button = hit.downcast::<NSButton>().expect("native button hit target");
                assert!(std::ptr::eq(&*button, &*spacing));
                unsafe {
                    button.performClick(None);
                }
                assert_eq!(spacing.state(), 1);
                unsafe {
                    button.performClick(None);
                }
                assert_eq!(spacing.state(), 0);
            }
            unsafe {
                spacing.performClick(None);
            }
            assert_eq!(
                settings.window.ns_window().frame(),
                frame,
                "expanding spacing preserves window size"
            );
            let gap = named::<NSControl>(root, "Screen edges").unwrap();
            unsafe {
                displays.performClick(None);
            }
            assert_eq!(
                spacing.state(),
                1,
                "disclosures open independently, without jumps"
            );
            assert_eq!(displays.state(), 1);
            assert_eq!(
                settings.window.ns_window().frame(),
                frame,
                "expanding displays preserves window size"
            );
            unsafe {
                spacing.performClick(None);
                displays.performClick(None);
            }
            assert!(
                named::<NSControl>(root, "Screen edges").is_none(),
                "collapsed editors hide"
            );
            unsafe {
                spacing.performClick(None);
            }
            assert!(
                std::ptr::eq(&*gap, &*named::<NSControl>(root, "Screen edges").unwrap()),
                "reopening reuses the editor"
            );
            assert_eq!(displays.state(), 0);
            assert!(**settings.model().source.borrow() == before);
            assert!(
                pending.try_recv().is_err(),
                "expansion never writes configuration"
            );
            for query in ["Master width", "Screen edges", "Display overrides"] {
                let entry =
                    search::results(query).into_iter().find(|entry| entry.title == query).unwrap();
                settings.router.open_search(entry);
                assert!(
                    named::<NSControl>(root, "Master width").is_some(),
                    "shared settings remain separate from the visible layout inspector"
                );
                if query == "Master width" || query == "Screen edges" {
                    let focused = settings.window.ns_window().firstResponder().unwrap();
                    assert_eq!(
                        focused
                            .downcast_ref::<NSControl>()
                            .and_then(|control| control.accessibilityLabel())
                            .map(|label| label.to_string())
                            .as_deref(),
                        Some(query),
                        "search focuses the revealed native control"
                    );
                }
                assert_eq!(settings.router.current(), Category::Layouts);
                let sidebar =
                    named::<NSTableView>(&root_window(&settings), "Settings categories").unwrap();
                assert_eq!(
                    sidebar.selectedRow(),
                    navigation_row(&sidebar, "Layouts") as isize
                );
            }
            settings.router.layouts().select_target(layouts::Target::Layout(
                crate::common::config::LayoutMode::Traditional,
            ));
            let position = named::<NSPopUpButton>(root, "New window position").unwrap();
            assert_eq!(
                position.titleOfSelectedItem().unwrap().to_string(),
                "Use shared setting",
                "following the shared setting is an explicit choice"
            );
            assert_eq!(
                position
                    .itemAtIndex(2)
                    .unwrap()
                    .badge()
                    .unwrap()
                    .stringValue()
                    .unwrap()
                    .to_string(),
                "Shared",
                "the menu marks the shared value"
            );
            assert!(
                named::<NSButton>(root, "Traditional, default").is_some()
                    && named::<NSButton>(root, "Traditional").is_none(),
                "the default tile says so to assistive apps"
            );
            let toggle = named::<NSSwitch>(root, "Keep windows equally sized").unwrap();
            let original_state = toggle.state();
            let pending_state = if original_state == 0 { 1 } else { 0 };
            toggle.setState(pending_state);
            assert!(unsafe { toggle.sendAction_to(toggle.action(), toggle.target().as_deref()) });
            let request = pending.try_recv().expect("native assignment is queued");
            let mut unrelated = before.clone();
            unrelated.settings.animate = !unrelated.settings.animate;
            settings.synchronize(unrelated.clone());
            assert_eq!(
                toggle.state(),
                pending_state,
                "unrelated revisions preserve immediate native feedback"
            );
            let Action::Edit(edit) = request.action else {
                panic!("native assignment")
            };
            edit(&mut unrelated).unwrap();
            (request.finish)(Ok(()));
            settings.synchronize(unrelated);
            assert_eq!(
                toggle.state(),
                pending_state,
                "acknowledgment keeps the final native value"
            );
            settings.synchronize(before.clone());
            assert_eq!(toggle.state(), original_state);
            assert!(
                pending.try_recv().is_err(),
                "synchronization never submits edits"
            );
            let mut reloaded = before.clone();
            reloaded.settings.layout.traditional.equalize_nodes = false;
            settings.synchronize(reloaded);
            assert_eq!(toggle.state(), 0, "external reload updates visible controls");
            settings.synchronize(before.clone());
            // BSP’s aspect ratio is Automatic or Custom, never a keyword typed into a field.
            settings
                .router
                .layouts()
                .select_target(layouts::Target::Layout(crate::common::config::LayoutMode::Bsp));
            let shape = named::<NSPopUpButton>(root, "Single window aspect ratio").unwrap();
            assert!(named::<objc2_app_kit::NSTextField>(root, "Width-to-height ratio").is_none());
            choose(&shape, 2);
            let request = pending.try_recv().expect("choosing Custom seeds a ratio");
            let Action::Edit(edit) = request.action else {
                panic!("source edit")
            };
            let mut custom = before.clone();
            edit(&mut custom).unwrap();
            assert_eq!(
                custom.settings.layout.bsp.single_window_aspect_ratio,
                Some(16.0 / 9.0)
            );
            settings.synchronize(custom.clone());
            // Switching must commit an actual AppKit field editor before hiding its client.
            let ratio = named::<objc2_app_kit::NSTextField>(root, "Width-to-height ratio").unwrap();
            assert!(settings.window.ns_window().makeFirstResponder(Some(&ratio)));
            ratio
                .currentEditor()
                .unwrap()
                .setString(&objc2_foundation::NSString::from_str("1.5"));
            unsafe {
                tile(root, "Traditional").unwrap().performClick(None);
            }
            let request = pending.try_recv().expect("switch commits the field’s source edit");
            let Action::Edit(edit) = request.action else {
                panic!("field uses source edits");
            };
            let mut edited = custom.clone();
            edit(&mut edited).unwrap();
            assert_eq!(edited.settings.layout.bsp.single_window_aspect_ratio, Some(1.5));
            assert!(ratio.currentEditor().is_none());
            settings.synchronize(before.clone());
            unsafe {
                tile(root, "Scrolling").unwrap().performClick(None);
            }
            // The default is chosen in the toolbar, separately from the edited layout.
            let control = settings.router.layouts().default_control();
            let default = named::<NSPopUpButton>(control.ns_view(), "Default layout").unwrap();
            choose(
                &default,
                crate::common::config::LayoutMode::Scrolling.config_choice_index() as isize,
            );
            let request = pending.try_recv().unwrap();
            let Action::Edit(edit) = request.action else {
                panic!("default submits a source edit");
            };
            let mut external = before.clone();
            edit(&mut external).unwrap();
            assert_eq!(
                external.settings.layout.mode,
                crate::common::config::LayoutMode::Scrolling
            );
            settings.synchronize(external.clone());
            let scrolling = named::<NSButton>(root, "Scrolling, default").unwrap();
            assert_eq!(scrolling.state(), 1, "making a layout default keeps editing it");
            assert_eq!(tile(root, "Traditional").unwrap().state(), 0);
            external.settings.layout.mode = before.settings.layout.mode;
            assert!(
                external == before,
                "default changes preserve individual layout settings"
            );
            settings.synchronize(before.clone());
            assert_eq!(
                default.titleOfSelectedItem().unwrap().to_string(),
                "Traditional",
                "external changes update the default popup"
            );
            // Reset only the inspected algorithm, preserving defaults and display overrides.
            let mut customized = before.clone();
            customized.settings.layout.scrolling.column_width_ratio = 0.61;
            customized.settings.layout.scrolling.per_display.insert(
                "saved-display".into(),
                crate::common::config::ScrollingWidthOverride {
                    column_width_ratio: Some(0.7),
                    ..Default::default()
                },
            );
            settings.synchronize(customized.clone());
            assert!(
                named::<NSPopUpButton>(root, "Scrolling actions").is_some(),
                "restoring defaults lives in the edited layout’s actions menu"
            );
            layouts::restore(
                &Rc::downgrade(&settings.router.model),
                crate::common::config::LayoutMode::Scrolling,
                Weak::new(),
            );
            let request = pending.try_recv().unwrap();
            let Action::Edit(edit) = request.action else {
                panic!("reset uses the source mutation pipeline");
            };
            let mut expected = before.clone();
            expected.settings.layout.scrolling = Default::default();
            expected.settings.layout.scrolling.per_display =
                customized.settings.layout.scrolling.per_display.clone();
            edit(&mut customized).unwrap();
            assert!(
                customized == expected,
                "reset preserves all other settings and display overrides"
            );
            settings.synchronize(before.clone());
            // Width presets are one chip per percentage, plus an add button.
            for title in ["33.33%", "50%", "66.67%"] {
                assert!(
                    named::<NSButton>(root, &format!("Width preset {title}")).is_some(),
                    "each preset is its own control"
                );
                assert!(named::<NSButton>(root, &format!("Remove {title}")).is_some());
            }
            assert!(named::<NSButton>(root, "Add width preset").is_some());
            settings.synchronize(before);
        });
        // A normal collection row's first click opens its editor without changing configuration.
        autoreleasepool(|_| {
            let page = editors::workspaces(ui, &settings.router.model);
            page.synchronize(&settings.router.model);
            let table = find::<NSTableView>(page.view.ns_view()).unwrap();
            table.selectRowIndexes_byExtendingSelection(
                &objc2_foundation::NSIndexSet::indexSetWithIndex(0),
                false,
            );
            assert!(unsafe { table.sendAction_to(table.action(), table.target().as_deref()) });
            assert!(
                settings.router.model.sheet.borrow().is_some(),
                "first click opens a workspace sheet"
            );
            let draft = settings.router.model.sheet_model.borrow().as_ref().unwrap().clone();
            let sheet = settings.router.model.sheet.borrow().as_ref().unwrap().ns_window().retain();
            change_text(&editable(&sheet.contentView().unwrap()).unwrap(), "Draft name");
            assert_eq!(
                draft.source.borrow().virtual_workspaces.workspace_names[0],
                "Draft name"
            );
            assert!(
                *draft.source.borrow() != *settings.router.model.source.borrow(),
                "draft changes remain transactional"
            );
            settings.router.model.close_sheet();
            assert!(settings.router.model.sheet.borrow().is_none());
            assert_ne!(
                settings
                    .router
                    .model
                    .source
                    .borrow()
                    .virtual_workspaces
                    .workspace_names
                    .first()
                    .map(String::as_str),
                Some("Draft name")
            );
            assert!(
                pending.try_recv().is_err(),
                "opening and cancelling an editor must not change config"
            );
        });
        // Numeric sliders edit through the same draft and retain exact text values.
        autoreleasepool(|_| {
            let draft = settings.router.model.draft();
            let mut form = FormBuilder::new(ui, &draft);
            let count = form.field(
                "master_count",
                |s| &s.settings.layout.master_stack,
                |s| &mut s.settings.layout.master_stack,
            );
            let page = Rc::new(form.finish(Section::new(&ui, "").row(count)));
            *draft.draft.as_ref().unwrap().page.borrow_mut() = Some(page.clone());
            page.synchronize(&draft);
            let slider = find::<NSSlider>(page.view.ns_view()).unwrap();
            assert!(slider.allowsTickMarkValuesOnly(), "counts use a stepped slider");
            slider.setDoubleValue(3.4);
            assert!(unsafe { slider.sendAction_to(slider.action(), slider.target().as_deref()) });
            assert_eq!(
                draft.source.borrow().settings.layout.master_stack.master_count,
                3
            );
            assert_eq!(
                editable(page.view.ns_view()).unwrap().stringValue().to_string(),
                "3"
            );
            // The convenient count range must not become a config limit.
            change_text(&editable(page.view.ns_view()).unwrap(), "12");
            assert_eq!(
                draft.source.borrow().settings.layout.master_stack.master_count,
                12
            );
            page.synchronize(&draft);
            assert_eq!(
                editable(page.view.ns_view()).unwrap().stringValue().to_string(),
                "12"
            );

            let mut form = FormBuilder::new(ui, &draft);
            let offset = form.field(
                "stack_offset",
                |s| &s.settings.layout.stack,
                |s| &mut s.settings.layout.stack,
            );
            let page = Rc::new(form.finish(Section::new(&ui, "").row(offset)));
            *draft.draft.as_ref().unwrap().page.borrow_mut() = Some(page.clone());
            page.synchronize(&draft);
            change_text(&editable(page.view.ns_view()).unwrap(), "123.456");
            assert_eq!(draft.source.borrow().settings.layout.stack.stack_offset, 123.456);
            page.synchronize(&draft);
            assert_eq!(
                editable(page.view.ns_view()).unwrap().stringValue().to_string(),
                "123.456"
            );
            let mut form = FormBuilder::new(ui, &draft);
            let minimum = form.field(
                "min_column_width_ratio",
                |s| &s.settings.layout.scrolling,
                |s| &mut s.settings.layout.scrolling,
            );
            let page = Rc::new(form.finish(Section::new(&ui, "").row(minimum)));
            *draft.draft.as_ref().unwrap().page.borrow_mut() = Some(page.clone());
            let mut source = draft.source.borrow().as_ref().clone();
            source.settings.layout.scrolling.max_column_width_ratio = 0.373456;
            draft.replace_source(source);
            page.synchronize(&draft);
            let slider = find::<NSSlider>(page.view.ns_view()).unwrap();
            slider.setDoubleValue(100.0);
            assert!(unsafe { slider.sendAction_to(slider.action(), slider.target().as_deref()) });
            assert!(
                (draft.source.borrow().settings.layout.scrolling.min_column_width_ratio - 0.373456)
                    .abs()
                    < 1e-9,
                "percentage scaling and rounding must respect the exact upper bound"
            );
            let mut source = draft.source.borrow().as_ref().clone();
            source.settings.layout.scrolling.max_column_width_ratio = 0.8;
            draft.replace_source(source);
            page.synchronize(&draft);
            slider.setDoubleValue(75.0);
            assert!(unsafe { slider.sendAction_to(slider.action(), slider.target().as_deref()) });
            assert_eq!(
                draft.source.borrow().settings.layout.scrolling.min_column_width_ratio,
                0.75,
                "slider ranges must follow changes to the other width bound"
            );
            assert!(
                pending.try_recv().is_err(),
                "draft slider edits do not persist before Done"
            );
        });
        autoreleasepool(|_| keyboard_controls(ui, &settings, &mut pending));
        // The primary Spacing page edits global values and preserves individual display overrides.
        autoreleasepool(|_| {
            use crate::sys::screen::NSScreenExt;
            let screen = settings.window.ns_window().screen().unwrap();
            settings.refresh_displays(vec![crate::sys::screen::ScreenInfo {
                id: screen.get_number().unwrap(),
                display_uuid: "focused-display".into(),
                name: Some("Focused display".into()),
                frame: Default::default(),
                is_builtin: false,
                backing_scale: 1.0,
                space: None,
            }]);
            let mut source = settings.router.model.source.borrow().as_ref().clone();
            let global = source.settings.layout.gaps.outer.clone();
            source
                .settings
                .layout
                .gaps
                .per_display
                .entry("focused-display".into())
                .or_default()
                .outer = Some(crate::common::config::OuterGaps {
                top: 12.0,
                right: 12.0,
                bottom: 12.0,
                left: 12.0,
            });
            settings.router.model.replace_source(source);
            let page = pages::layout_spacing(ui, &settings.router.model);
            page.synchronize(&settings.router.model);
            let slider = find::<NSSlider>(page.view.ns_view()).unwrap();
            assert_eq!(
                slider.doubleValue(),
                global.top,
                "show global spacing even on a display with an override"
            );
            slider.setDoubleValue(24.0);
            assert!(unsafe { slider.sendAction_to(slider.action(), slider.target().as_deref()) });
            let request = pending.try_recv().unwrap();
            let Action::Edit(edit) = request.action else {
                panic!("expected spacing edit")
            };
            let mut source = settings.router.model.source.borrow().as_ref().clone();
            edit(&mut source).unwrap();
            assert_eq!(
                [
                    source.settings.layout.gaps.outer.top,
                    source.settings.layout.gaps.outer.right,
                    source.settings.layout.gaps.outer.bottom,
                    source.settings.layout.gaps.outer.left
                ],
                [24.0; 4],
                "edit global spacing"
            );
            let effective =
                source.settings.layout.gaps.effective_for_display(Some("focused-display"));
            assert_eq!(
                [
                    effective.outer.top,
                    effective.outer.right,
                    effective.outer.bottom,
                    effective.outer.left
                ],
                [12.0; 4],
                "preserve this display’s explicit override"
            );
            settings.router.model.replace_source(source.clone());
            page.synchronize(&settings.router.model);
            let mode =
                named::<NSSegmentedControl>(page.view.ns_view(), "Spacing controls").unwrap();
            mode.setSelectedSegment(1);
            assert!(unsafe { mode.sendAction_to(mode.action(), mode.target().as_deref()) });
            assert_eq!(
                count::<NSSlider>(page.view.ns_view()),
                0,
                "Custom uses numerical fields, not sliders"
            );
            let names = pages::EDGES.map(|edge| format!("Screen edges, {edge}"));
            let axes = pages::AXES.map(|axis| format!("Between windows, {axis}"));
            for name in names.iter().chain(&axes) {
                assert!(
                    named::<objc2_app_kit::NSTextField>(page.view.ns_view(), name)
                        .unwrap()
                        .isEditable()
                );
            }
            let top = named::<objc2_app_kit::NSTextField>(page.view.ns_view(), "Screen edges, Top")
                .unwrap();
            settings.router.host.set_cached_page(page.view.clone());
            assert!(settings.window.ns_window().makeFirstResponder(Some(&top)));
            top.currentEditor()
                .unwrap()
                .setString(&objc2_foundation::NSString::from_str("7"));
            assert!(settings.window.ns_window().makeFirstResponder(None));
            let request = pending.try_recv().unwrap();
            let Action::Edit(edit) = request.action else {
                panic!("expected numerical spacing edit")
            };
            edit(&mut source).unwrap();
            assert_eq!(source.settings.layout.gaps.outer.top, 7.0);
            assert_eq!(source.settings.layout.gaps.outer.right, 24.0);
            assert_eq!(
                source
                    .settings
                    .layout
                    .gaps
                    .effective_for_display(Some("focused-display"))
                    .outer
                    .top,
                12.0
            );
            source.settings.layout.gaps.outer.top = 14.0;
            settings.router.model.replace_source(source);
            page.synchronize(&settings.router.model);
            assert_eq!(
                top.stringValue().to_string(),
                "14",
                "external changes update custom fields"
            );
            assert!(pending.try_recv().is_err());
        });
        // Display overrides edit inline: inherited values show as placeholders, spacing groups
        // and each Scrolling width override independently, and displays can be added by UUID.
        autoreleasepool(|_| {
            use objc2_app_kit::NSTextField;
            settings.refresh_displays(vec![crate::sys::screen::ScreenInfo {
                id: crate::sys::screen::ScreenId::new(7),
                display_uuid: "disp-a".into(),
                name: Some("Display A".into()),
                frame: Default::default(),
                is_builtin: false,
                backing_scale: 1.0,
                space: None,
            }]);
            let mut source = settings.router.model.source.borrow().as_ref().clone();
            let layout = &mut source.settings.layout;
            layout.gaps.outer = crate::common::config::OuterGaps {
                top: 5.0,
                right: 5.0,
                bottom: 5.0,
                left: 5.0,
            };
            layout.gaps.per_display.clear();
            layout.scrolling.per_display.clear();
            settings.router.model.replace_source(source.clone());
            let page = editors::display_inspector(ui, &settings.router.model);
            page.synchronize(&settings.router.model);
            settings.router.host.set_cached_page(page.view.clone());
            let view = page.view.ns_view();
            let window = settings.window.ns_window();
            let top = named::<NSTextField>(view, "Screen edges, Top").unwrap();
            assert_eq!(
                top.stringValue().to_string(),
                "",
                "inherited values leave the field empty"
            );
            assert_eq!(top.placeholderString().unwrap().to_string(), "5");
            assert!(named::<NSButton>(view, "Use Shared for screen edges").is_none());
            let type_into = |pending: &mut tokio::sync::mpsc::UnboundedReceiver<Request>,
                             field: &NSTextField,
                             text: &str| {
                assert!(window.makeFirstResponder(Some(field)));
                field
                    .currentEditor()
                    .unwrap()
                    .setString(&objc2_foundation::NSString::from_str(text));
                assert!(window.makeFirstResponder(None));
                let request = pending.try_recv().expect("a committed value is one source edit");
                let Action::Edit(edit) = request.action else {
                    panic!("source edit")
                };
                let mut source = settings.router.model.source.borrow().as_ref().clone();
                edit(&mut source).unwrap();
                settings.router.model.replace_source(source);
                page.synchronize(&settings.router.model);
            };
            let current = || settings.router.model.source.borrow().as_ref().clone();
            type_into(&mut pending, &top, "9");
            let custom = current().settings.layout.gaps.per_display["disp-a"].clone();
            assert_eq!(custom.outer.as_ref().map(|o| (o.top, o.right)), Some((9.0, 5.0)));
            assert!(custom.inner.is_none(), "between-window spacing still inherits");
            assert_eq!(
                named::<NSTextField>(view, "Screen edges, Right")
                    .unwrap()
                    .stringValue()
                    .to_string(),
                "5"
            );
            let width = named::<NSTextField>(view, "Default column width").unwrap();
            type_into(&mut pending, &width, "50");
            let widths = current().settings.layout.scrolling.per_display["disp-a"].clone();
            assert_eq!(widths.column_width_ratio, Some(0.5));
            assert_eq!(
                widths.min_column_width_ratio, None,
                "each width inherits on its own"
            );
            type_into(&mut pending, &width, "");
            assert!(
                !current().settings.layout.scrolling.per_display.contains_key("disp-a"),
                "clearing the only override removes the display’s entry"
            );
            unsafe {
                named::<NSButton>(view, "Use Shared for screen edges")
                    .unwrap()
                    .performClick(None);
            }
            let request = pending.try_recv().unwrap();
            let Action::Edit(edit) = request.action else {
                panic!("source edit")
            };
            let mut source = current();
            edit(&mut source).unwrap();
            assert!(
                source.settings.layout.gaps.per_display.is_empty(),
                "Use Shared inherits again"
            );
            settings.router.model.replace_source(source.clone());
            page.synchronize(&settings.router.model);
            // Add Display… prepares a display that isn’t connected, without writing anything.
            let popup = named::<NSPopUpButton>(view, "Display").unwrap();
            choose(&popup, popup.numberOfItems() - 1);
            assert_eq!(popup.titleOfSelectedItem().unwrap().to_string(), "Display A");
            named::<NSTextField>(view, "Display UUID")
                .unwrap()
                .setStringValue(&objc2_foundation::NSString::from_str("manual-display"));
            unsafe {
                named::<NSButton>(view, "Add").unwrap().performClick(None);
            }
            assert_eq!(
                popup.titleOfSelectedItem().unwrap().to_string(),
                "Display manual-d…"
            );
            assert!(
                pending.try_recv().is_err(),
                "adding a display writes nothing until edited"
            );
            type_into(
                &mut pending,
                &named::<NSTextField>(view, "Screen edges, Left").unwrap(),
                "3",
            );
            assert!(current().settings.layout.gaps.per_display.contains_key("manual-display"));
            settings.router.host.clear();
        });
        let model = Rc::downgrade(&settings.router.model);
        let host = objc2::rc::Weak::new(settings.router.host.ns_view());
        let mut form = FormBuilder::new(ui, &settings.router.model);
        let row = form.inherited_popup(
            "Position",
            vec![
                (
                    "Next to selection",
                    crate::common::config::WindowInsertionPoint::NextToSelection,
                ),
                (
                    "End of layout",
                    crate::common::config::WindowInsertionPoint::EndOfTree,
                ),
            ],
            |s| s.settings.layout.base.window_insertion_point,
            |s| {
                s.settings
                    .layout
                    .stack
                    .base
                    .window_insertion_point
                    .unwrap_or(crate::common::config::WindowInsertionPoint::NextToSelection)
            },
            |s, value| s.settings.layout.base.window_insertion_point = value,
        );
        let popup = find::<NSPopUpButton>(row.control_view()).unwrap();
        let page = form.finish(row);
        page.synchronize(&settings.router.model);
        assert_eq!(
            popup.titleOfSelectedItem().unwrap().to_string(),
            "Next to selection"
        );
        assert_eq!(popup.numberOfItems(), 2);
        assert_eq!(
            popup
                .itemAtIndex(0)
                .unwrap()
                .badge()
                .unwrap()
                .stringValue()
                .unwrap()
                .to_string(),
            "Default"
        );
        assert!(popup.itemAtIndex(1).unwrap().badge().is_none());
        for (index, expected) in [
            (0, None),
            (1, Some(crate::common::config::WindowInsertionPoint::EndOfTree)),
            (0, None),
        ] {
            popup.selectItemAtIndex(index);
            let control: &NSControl = &popup;
            assert!(unsafe {
                control.sendAction_to(control.action(), control.target().as_deref())
            });
            let request = pending.try_recv().unwrap();
            let Action::Edit(edit) = request.action else {
                panic!("expected config edit")
            };
            let mut source = settings.router.model.source.borrow().as_ref().clone();
            edit(&mut source).unwrap();
            assert_eq!(source.settings.layout.base.window_insertion_point, expected);
            settings.router.model.replace_source(source);
            page.synchronize(&settings.router.model);
        }
        let mut source = settings.router.model.source.borrow().as_ref().clone();
        source.settings.layout.stack.base.window_insertion_point =
            Some(crate::common::config::WindowInsertionPoint::EndOfTree);
        settings.router.model.replace_source(source);
        page.synchronize(&settings.router.model);
        assert_eq!(popup.titleOfSelectedItem().unwrap().to_string(), "End of layout");
        assert!(popup.itemAtIndex(0).unwrap().badge().is_none());
        assert_eq!(
            popup
                .itemAtIndex(1)
                .unwrap()
                .badge()
                .unwrap()
                .stringValue()
                .unwrap()
                .to_string(),
            "Default"
        );
        assert!(
            pending.try_recv().is_err(),
            "synchronization must not write config"
        );
        interface_controls(&settings, &mut pending);
        drop(page);
        drop(popup);
        drop(keyboard);
        drop(settings);
        (model, host)
    });
    assert!(
        model.upgrade().is_none(),
        "closing must release the Settings model"
    );
    autoreleasepool(|_| {
        objc2_foundation::NSRunLoop::currentRunLoop()
            .runUntilDate(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.05))
    });
    assert!(host.load().is_none(), "closing must release the page host");
}

/// Exercises the real edit boundary for the keyboard manager, with no running daemon.
fn keyboard_controls(
    ui: Ui,
    settings: &Settings,
    pending: &mut tokio::sync::mpsc::UnboundedReceiver<Request>,
) {
    use rift_protocol::{Direction, DisplaySelector, LayoutCommand as L, ReactorCommand as R};

    use crate::actor::reactor::Command;
    use crate::actor::wm_controller::{WmCmd, WmCommand};
    let focus = |direction| WmCommand::ReactorCommand(Command::Layout(L::MoveFocus(direction)));
    let balance = WmCommand::ReactorCommand(Command::Layout(L::Balance));
    let mut source = settings.model().source.borrow().as_ref().clone();
    source.keys.clear();
    source.keys.insert("Alt + H".into(), focus(Direction::Left));
    source.keys.insert("Alt + L".into(), focus(Direction::Right));
    source.keys.insert(
        "Alt + F".into(),
        WmCommand::ReactorCommand(Command::Layout(L::ToggleFullscreen)),
    );
    source.binding_modes.insert("move".into(), Default::default());
    settings.router.model.replace_source(source);
    let capturing = Rc::new(Cell::new(false));
    let available = Rc::new(Cell::new(true));
    let (active, ready) = (capturing.clone(), available.clone());
    settings.set_shortcut_capture(move |recording| {
        if recording && !ready.get() {
            return Err("Recording is unavailable".into());
        }
        active.set(recording);
        Ok(())
    });
    let page = commands::keyboard(ui, &settings.router.model);
    page.synchronize(settings.model());
    let root = page.view.ns_view();
    let shown = |name: &str| named::<NSButton>(root, &format!("Shortcut for {name}"));
    let click = |button: &NSButton| {
        let table = find::<NSTableView>(root).unwrap();
        for row in 0..table.numberOfRows() {
            if table
                .viewAtColumn_row_makeIfNecessary(0, row, true)
                .is_some_and(|cell| button.isDescendantOf(&cell))
            {
                table.scrollRowToVisible(row);
                root.layoutSubtreeIfNeeded();
                break;
            }
        }
        assert!(unsafe { button.sendAction_to(button.action(), button.target().as_deref()) })
    };
    let press = |button: &NSButton, code: u16, key: &str, flags| {
        let key = objc2_foundation::NSString::from_str(key);
        button.keyDown(&objc2_app_kit::NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            objc2_app_kit::NSEventType::KeyDown, CGPoint::ZERO, flags, 0.0, 0, None, &key, &key, false, code,
        ).unwrap());
    };
    let apply = |pending: &mut tokio::sync::mpsc::UnboundedReceiver<Request>| {
        let request = pending.try_recv().expect("a shortcut edit");
        let Action::Edit(edit) = request.action else {
            panic!("shortcut edit")
        };
        let mut source = settings.model().source.borrow().as_ref().clone();
        edit(&mut source).unwrap();
        settings.router.model.replace_source(source.clone());
        (request.finish)(Ok(()));
        page.synchronize(settings.model());
        source
    };
    let option = objc2_app_kit::NSEventModifierFlags::Option;
    let none = objc2_app_kit::NSEventModifierFlags::empty();
    assert!(
        action(root, "Shortcut for Focus window Left").is_none(),
        "the page must defer row controls until AppKit requests their cells"
    );
    // Grouping is visual only: both independent recorders remain available.
    fn action_label(button: &NSButton) -> String {
        let mut view = unsafe { button.superview() };
        while let Some(parent) = view {
            if parent.downcast_ref::<objc2_app_kit::NSTableCellView>().is_some() {
                return find::<objc2_app_kit::NSTextField>(&parent)
                    .unwrap()
                    .stringValue()
                    .to_string();
            }
            view = unsafe { parent.superview() };
        }
        panic!("shortcut must belong to a native list cell")
    }
    assert_eq!(
        action_label(&shown("Focus window Left").unwrap()),
        "Focus window"
    );
    assert_eq!(action_label(&shown("Focus window Right").unwrap()), "");
    assert!(shown("Balance").is_none(), "Assigned is the quiet default");
    assert!(
        action(root, "Shortcut for Balance").is_none(),
        "opening Assigned must not allocate hidden catalog rows"
    );
    assert!(
        action(root, "Clear shortcut").is_none(),
        "idle rows must not allocate recording-only buttons"
    );
    let _host = cgs::window::Window::new(&ui).content(page.view.clone());
    _host.ns_window().setContentSize(CGSize::new(700.0, 600.0));
    _host.show();
    root.layoutSubtreeIfNeeded();
    fn options<T: objc2::DowncastTarget + 'static>(ui: Ui, label: &str) -> Retained<T> {
        NSApplication::sharedApplication(ui.mtm())
            .windows()
            .iter()
            .filter_map(|window| window.contentView())
            .find_map(|view| named::<T>(&view, label))
            .unwrap_or_else(|| panic!("missing contextual option {label}"))
    }
    settings.window.toolbar().set_page_controls(&ui, page.header.as_deref());
    let header = settings
        .window
        .toolbar()
        .ns_toolbar()
        .items()
        .iter()
        .find(|item| item.itemIdentifier().to_string() == "cgs.page-filter")
        .unwrap()
        .view()
        .unwrap();
    let visibility = named::<NSSegmentedControl>(root, "Shortcut visibility").unwrap();
    visibility.setSelectedSegment(1);
    assert!(unsafe {
        visibility.sendAction_to(visibility.action(), visibility.target().as_deref())
    });
    assert!(shown("Balance").is_some());
    settings.window.toolbar().set_page_controls(&ui, page.header.as_deref());
    let search = named::<objc2_app_kit::NSSearchField>(root, "Filter shortcuts").unwrap();
    let table = find::<NSTableView>(root).unwrap();
    let scroll = table.enclosingScrollView().unwrap();
    let search_rect = search.convertRect_toView(search.bounds(), Some(root));
    let scroll_rect = scroll.convertRect_toView(scroll.bounds(), Some(root));
    assert_eq!(
        scroll_rect.origin.x, 0.0,
        "the native scroll edge must reach the pane's left edge"
    );
    assert_eq!(
        scroll_rect.size.width,
        root.bounds().size.width,
        "the native scroll edge must span the full pane, with padding inside the rows"
    );
    assert!(
        scroll_rect.origin.y < search_rect.origin.y + search_rect.size.height
            && scroll_rect.origin.y + scroll_rect.size.height > search_rect.origin.y,
        "the list must extend behind search for the native scroll-edge blur"
    );
    let first = table.convertRect_toView(table.rectOfRow(0), Some(root));
    let clear = if root.isFlipped() {
        first.origin.y >= search_rect.origin.y + search_rect.size.height
    } else {
        first.origin.y + first.size.height <= search_rect.origin.y
    };
    assert!(
        clear,
        "the first section must start below search at the scroll origin"
    );
    let position = scroll.contentView().bounds().origin;
    scroll.contentView().scrollToPoint(CGPoint::new(0.0, 240.0));
    scroll.reflectScrolledClipView(&scroll.contentView());
    root.layoutSubtreeIfNeeded();
    assert!(scroll.contentView().bounds().origin.y > 100.0);
    assert_eq!(
        search.convertRect_toView(search.bounds(), Some(root)),
        search_rect,
        "search must stay fixed while bindings scroll beneath it"
    );
    scroll.contentView().scrollToPoint(position);
    scroll.reflectScrolledClipView(&scroll.contentView());
    for query in ["alt+h", "option + h", "⌥H"] {
        change_text(&search, query);
        assert!(shown("Focus window Left").is_some(), "{query}");
        assert!(shown("Focus window Right").is_none(), "{query}");
    }
    change_text(&search, "right");
    assert!(shown("Focus window Left").is_none());
    assert_eq!(
        action_label(&shown("Focus window Right").unwrap()),
        "Focus window"
    );
    change_text(&search, "");
    assert_eq!(action_label(&shown("Focus window Right").unwrap()), "");
    available.set(false);
    let left = shown("Focus window Left").unwrap();
    click(&left);
    assert_eq!(left.title().to_string(), "⌥H");
    assert!(!capturing.get());
    assert!(named::<objc2_app_kit::NSTextField>(root, "Recording is unavailable").is_some());
    available.set(true);
    let mut ancestor = unsafe { left.superview() };
    let menu = loop {
        let view = ancestor.expect("binding context menu");
        if let Some(menu) = view.menu() {
            break menu;
        }
        ancestor = unsafe { view.superview() };
    };
    assert_eq!(
        menu.itemAtIndex(0).unwrap().title().to_string(),
        "Change Shortcut…"
    );
    menu.performActionForItemAtIndex(0);
    assert!(capturing.get());
    press(&left, 53, "\u{1b}", none);
    assert!(!capturing.get());
    assert!(pending.try_recv().is_err());
    let free = shown("Balance").unwrap();
    click(&free);
    assert!(capturing.get());
    press(&free, 4, "h", option);
    assert!(pending.try_recv().is_err());
    let explanation = "⌥H is used by Focus window Left. Your existing shortcuts have not changed.";
    assert!(named::<objc2_app_kit::NSTextField>(root, explanation).is_some());
    // Escape abandons the pending replacement as well as the recorded keys.
    press(&free, 53, "\u{1b}", none);
    assert!(named::<NSButton>(root, "Replace Existing").is_none());
    assert!(pending.try_recv().is_err());
    click(&free);
    press(&free, 4, "h", option);
    click(&named::<NSButton>(root, "Choose Different").unwrap());
    let stable_right = shown("Focus window Right").unwrap();
    press(&free, 11, "b", option);
    assert!(!capturing.get());
    let edited = apply(pending);
    assert_eq!(edited.keys["Alt + B"], balance);
    assert_eq!(edited.keys["Alt + H"], focus(Direction::Left));
    assert!(
        std::ptr::eq(&*stable_right, &*shown("Focus window Right").unwrap()),
        "unrelated rows survive a committed edit"
    );
    // Replacement is a single edit and does not discard unrelated bindings.
    let existing_balance = shown("Balance").unwrap();
    click(&existing_balance);
    press(&existing_balance, 4, "h", option);
    click(&named::<NSButton>(root, "Replace Existing").unwrap());
    let edited = apply(pending);
    assert_eq!(edited.keys["Alt + H"], balance);
    assert!(!edited.keys.contains_key("Alt + B"));
    assert_eq!(edited.keys["Alt + L"], focus(Direction::Right));
    // Modified Backspace records a combination rather than clearing the binding.
    let right = shown("Focus window Right").unwrap();
    click(&right);
    press(&right, 51, "\u{7f}", option);
    let edited = apply(pending);
    assert_eq!(edited.keys["Alt + Backspace"], focus(Direction::Right));
    assert!(!edited.keys.contains_key("Alt + L"));
    click(&shown("Focus window Right").unwrap());
    click(&named::<NSButton>(root, "Cancel recording and restore previous shortcut").unwrap());
    assert!(!capturing.get());
    assert!(pending.try_recv().is_err());
    assert_eq!(shown("Focus window Right").unwrap().title().to_string(), "⌥⌫");
    // A failed clear restores the displayed shortcut and leaves the source untouched.
    let right = shown("Focus window Right").unwrap();
    click(&right);
    press(&right, 51, "\u{7f}", none);
    let failed = pending.try_recv().unwrap();
    (failed.finish)(Err("Disk is full".into()));
    assert_eq!(right.title().to_string(), "⌥⌫");
    assert!(settings.model().source.borrow().keys.contains_key("Alt + Backspace"));
    click(&right);
    press(&right, 53, "\u{1b}", none);
    assert!(!capturing.get());
    assert!(pending.try_recv().is_err());
    click(&right);
    click(&named::<NSButton>(root, "Clear shortcut").unwrap());
    let edited = apply(pending);
    assert!(!edited.keys.contains_key("Alt + Backspace"));
    // Mutually exclusive modes can reuse the same combination.
    visibility.setSelectedSegment(0);
    assert!(unsafe {
        visibility.sendAction_to(visibility.action(), visibility.target().as_deref())
    });
    let keymaps = named::<NSPopUpButton>(&header, "Shortcut Set Actions")
        .unwrap()
        .menu()
        .unwrap()
        .itemArray()
        .iter()
        .find_map(|item| item.submenu())
        .unwrap();
    assert_eq!(keymaps.itemAtIndex(0).unwrap().state(), 1);
    keymaps.performActionForItemAtIndex(1);
    assert_eq!(keymaps.itemAtIndex(0).unwrap().state(), 0);
    assert_eq!(keymaps.itemAtIndex(1).unwrap().state(), 1);
    assert!(named::<NSButton>(root, "Browse Actions").is_some());
    assert!(named::<NSButton>(root, "Use Default Shortcuts…").is_none());
    click(&named::<NSButton>(root, "Browse Actions").unwrap());
    assert_eq!(visibility.selectedSegment(), 1);
    assert!(
        pending.try_recv().is_err(),
        "browsing must not install any bindings"
    );
    let free = shown("Balance").unwrap();
    click(&free);
    press(&free, 4, "h", option);
    let edited = apply(pending);
    assert_eq!(edited.binding_modes["move"]["Alt + H"], balance);
    assert_eq!(edited.keys["Alt + H"], balance);
    keymaps.performActionForItemAtIndex(0);
    assert_eq!(keymaps.itemAtIndex(0).unwrap().state(), 1);
    // Existing extended command arguments are exposed inline and round-trip losslessly.
    let advanced = WmCommand::ReactorCommand(Command::Reactor(R::MoveWindowToDisplay {
        selector: DisplaySelector::Direction(Direction::Left),
        window_id: Some(123),
    }));
    let mut source = settings.model().source.borrow().as_ref().clone();
    source.keys.insert("Alt + D".into(), advanced.clone());
    settings.router.model.replace_source(source);
    page.synchronize(settings.model());
    click(&named::<NSButton>(root, "Edit Move window to display options").unwrap());
    assert!(
        settings.model().sheet.borrow().is_none(),
        "existing command options edit contextually without a sheet"
    );
    let field = options::<objc2_app_kit::NSTextField>(ui, "System window ID");
    assert_eq!(field.stringValue().to_string(), "123");
    choose(&options::<NSPopUpButton>(ui, "Display"), 1);
    let edited = apply(pending);
    assert_eq!(
        edited.keys["Alt + D"],
        WmCommand::ReactorCommand(Command::Reactor(R::MoveWindowToDisplay {
            selector: DisplaySelector::Direction(Direction::Right),
            window_id: Some(123)
        }))
    );
    // Multi-parameter drafts preserve both choices when recorded directly from All Actions.
    click(&named::<NSButton>(root, "Edit Toggle floating options").unwrap());
    choose(&options::<NSPopUpButton>(ui, "Floating placement"), 1);
    choose(&options::<NSPopUpButton>(ui, "Floating size"), 1);
    let floating = shown("Toggle floating").unwrap();
    click(&floating);
    press(&floating, 5, "g", option);
    let edited = apply(pending);
    assert_eq!(
        edited.keys["Alt + G"],
        WmCommand::ConfiguredLayout(
            crate::actor::wm_controller::ConfiguredLayoutCommand::ToggleWindowFloating(
                rift_protocol::ToggleWindowFloatingOptions {
                    center: true,
                    size: Some(rift_protocol::FloatingWindowSize::Preset(
                        rift_protocol::FloatingWindowSizePreset::Smart
                    ))
                }
            )
        )
    );
    // A single numeric argument edits directly in the row and is included when recorded.
    let amount = named::<objc2_app_kit::NSTextField>(root, "Resize window by 10").unwrap();
    change_text(&amount, "25");
    let resize = shown("Resize window by 10").unwrap();
    click(&resize);
    press(&resize, 28, "8", option);
    let edited = apply(pending);
    assert_eq!(
        edited.keys["Alt + 8"],
        WmCommand::ReactorCommand(Command::Layout(L::ResizeWindowBy { amount: 25.0 }))
    );
    // Catalog search never changes a selection to a different action at the same index.
    click(&named::<NSButton>(&header, "Add Shortcut…").unwrap());
    let catalog = settings
        .model()
        .sheet
        .borrow()
        .as_ref()
        .unwrap()
        .ns_window()
        .contentView()
        .unwrap();
    let action_search = named::<objc2_app_kit::NSSearchField>(&catalog, "Search actions").unwrap();
    let table = named::<NSTableView>(&catalog, "Available shortcut actions").unwrap();
    change_text(&action_search, "workspace");
    table.selectRowIndexes_byExtendingSelection(
        &objc2_foundation::NSIndexSet::indexSetWithIndex(navigation_row(
            &table,
            "Switch to workspace",
        )),
        false,
    );
    change_text(&action_search, "no such action");
    assert_eq!(table.selectedRow(), -1);
    assert!(!named::<NSButton>(&catalog, "Continue").unwrap().isEnabled());
    change_text(&action_search, "workspace");
    table.selectRowIndexes_byExtendingSelection(
        &objc2_foundation::NSIndexSet::indexSetWithIndex(navigation_row(
            &table,
            "Switch to workspace",
        )),
        false,
    );
    click(&named::<NSButton>(&catalog, "Continue").unwrap());
    let editor = settings
        .model()
        .sheet
        .borrow()
        .as_ref()
        .unwrap()
        .ns_window()
        .contentView()
        .unwrap();
    assert_eq!(
        named::<NSPopUpButton>(&editor, "Action")
            .unwrap()
            .titleOfSelectedItem()
            .unwrap()
            .to_string(),
        "Switch to workspace"
    );
    assert!(named::<NSButton>(&editor, "Add").is_none());
    choose(&named::<NSPopUpButton>(&editor, "Options").unwrap(), 2);
    let recorder = named::<NSButton>(&editor, "Record shortcut").unwrap();
    click(&recorder);
    press(&recorder, 25, "9", option);
    let edited = apply(pending);
    assert_eq!(
        edited.keys["Alt + 9"],
        WmCommand::Wm(WmCmd::SwitchToWorkspace(
            crate::common::config::WorkspaceSelector::Index(2)
        ))
    );
    assert!(
        settings.model().sheet.borrow().is_none(),
        "recording completes the add without Save"
    );
    // Structured commands edit identifiers directly; raw configuration is not the default UI.
    click(&named::<NSButton>(&header, "Add Shortcut…").unwrap());
    let catalog = settings
        .model()
        .sheet
        .borrow()
        .as_ref()
        .unwrap()
        .ns_window()
        .contentView()
        .unwrap();
    change_text(
        &named::<objc2_app_kit::NSSearchField>(&catalog, "Search actions").unwrap(),
        "Focus specific window",
    );
    click(&named::<NSButton>(&catalog, "Continue").unwrap());
    let editor = settings
        .model()
        .sheet
        .borrow()
        .as_ref()
        .unwrap()
        .ns_window()
        .contentView()
        .unwrap();
    let pid = named::<objc2_app_kit::NSTextField>(&editor, "Process ID")
        .expect("Focus specific window must expose a Process ID field");
    change_text(&pid, "invalid");
    change_text(
        &named::<objc2_app_kit::NSTextField>(&editor, "Window index").unwrap(),
        "7",
    );
    assert!(
        named::<objc2_app_kit::NSTextField>(&editor, "Enter a valid value for Process ID.")
            .is_some(),
        "editing another field must not clear an unresolved identifier error"
    );
    change_text(&pid, "345");
    let system_id = named::<objc2_app_kit::NSTextField>(&editor, "System window ID").unwrap();
    change_text(&system_id, "1234");
    change_text(&system_id, "");
    change_text(&system_id, "42");
    assert!(
        pending.try_recv().is_err(),
        "editing a new command only updates its draft"
    );
    let recorder = named::<NSButton>(&editor, "Record shortcut").unwrap();
    click(&recorder);
    press(
        &recorder,
        25,
        "9",
        option | objc2_app_kit::NSEventModifierFlags::Control,
    );
    let edited = apply(pending);
    assert!(
        edited.keys.values().any(|command| *command
            == WmCommand::ReactorCommand(Command::Reactor(R::FocusWindow {
                window_id: rift_protocol::WindowId { pid: 345, idx: 7 },
                window_server_id: Some(42)
            }))),
        "recording must retain every field of the structured command"
    );
    // Hover-only duplication preserves the originating command, while labels don't reveal it.
    fn action(view: &NSView, title: &str) -> Option<Retained<NSButton>> {
        if let Some(button) = view.downcast_ref::<NSButton>()
            && button.accessibilityLabel().is_some_and(|label| label.to_string() == title)
        {
            return Some(button.retain());
        }
        view.subviews().iter().find_map(|view| action(&view, title))
    }
    fn table_action(view: &NSView, title: &str) -> Option<Retained<NSButton>> {
        if let Some(table) = view.downcast_ref::<NSTableView>() {
            for row in 0..table.numberOfRows() {
                if let Some(cell) = table.viewAtColumn_row_makeIfNecessary(0, row, true) {
                    if let Some(button) = action(&cell, title) {
                        return Some(button);
                    }
                }
            }
        }
        view.subviews().iter().find_map(|child| table_action(&child, title))
    }
    let extra = action(root, "Add another Balance shortcut")
        .or_else(|| table_action(root, "Add another Balance shortcut"))
        .unwrap();
    assert!(extra.isHidden());
    let slot = unsafe { extra.superview() }.unwrap();
    let controls = unsafe { slot.superview() }.unwrap();
    let hover = unsafe { controls.superview() }.unwrap();
    let event = unsafe { objc2_app_kit::NSEvent::enterExitEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_trackingNumber_userData(
        objc2_app_kit::NSEventType::MouseEntered, CGPoint::ZERO, none, 0.0, 0, None, 0, 0, std::ptr::null_mut(),
    ) }.unwrap();
    unsafe { hover.superview() }.unwrap().mouseEntered(&event);
    assert!(extra.isHidden());
    hover.mouseEntered(&event);
    assert!(!extra.isHidden());
    click(&extra);
    let editor = settings
        .model()
        .sheet
        .borrow()
        .as_ref()
        .unwrap()
        .ns_window()
        .contentView()
        .unwrap();
    assert_eq!(
        named::<NSPopUpButton>(&editor, "Action")
            .unwrap()
            .titleOfSelectedItem()
            .unwrap()
            .to_string(),
        "Balance"
    );
    click(&named::<NSButton>(&editor, "Cancel").unwrap());
    hover.mouseExited(&event);
    assert!(extra.isHidden());
    root.setFrameSize(CGSize::new(cgs::view::Metrics::DETAIL_MIN_WIDTH, 600.0));
    root.layoutSubtreeIfNeeded();
    assert!(
        find::<objc2_app_kit::NSScrollView>(root)
            .unwrap()
            .documentView()
            .unwrap()
            .frame()
            .size
            .width
            <= cgs::view::Metrics::DETAIL_MIN_WIDTH
    );
    assert!(pending.try_recv().is_err());
    let mut empty = settings.model().source.borrow().as_ref().clone();
    empty.keys.clear();
    settings.router.model.replace_source(empty);
    page.synchronize(settings.model());
    visibility.setSelectedSegment(0);
    assert!(unsafe {
        visibility.sendAction_to(visibility.action(), visibility.target().as_deref())
    });
    assert!(named::<NSButton>(root, "Browse Actions").is_some());
    assert!(named::<NSButton>(root, "Use Default Shortcuts…").is_some());
    assert!(settings.model().source.borrow().keys.is_empty());
    assert!(pending.try_recv().is_err());
    // Two bindings with identical parameters remain independently editable/removable.
    let mut duplicates = settings.model().source.borrow().as_ref().clone();
    duplicates.keys.insert("Alt + H".into(), focus(Direction::Left));
    duplicates.keys.insert("Alt + J".into(), focus(Direction::Left));
    duplicates.keys.insert("Alt + L".into(), focus(Direction::Right));
    settings.router.model.replace_source(duplicates);
    page.synchronize(settings.model());
    let table = find::<NSTableView>(root).unwrap();
    let left_fields: Vec<_> = (0..table.numberOfRows())
        .filter_map(|row| table.viewAtColumn_row_makeIfNecessary(0, row, true))
        .filter_map(|cell| named::<NSButton>(&cell, "Shortcut for Focus window Left"))
        .collect();
    assert_eq!(left_fields.len(), 2);
    assert_eq!(action_label(&left_fields[0]), "Focus window");
    assert_eq!(action_label(&left_fields[1]), "");
    choose(&named::<NSPopUpButton>(root, "Focus window Left").unwrap(), 1);
    let edited = apply(pending);
    assert_eq!(edited.keys["Alt + H"], focus(Direction::Right));
    assert_eq!(edited.keys["Alt + J"], focus(Direction::Left));
    click(&left_fields[1]);
    press(&left_fields[1], 51, "\u{7f}", none);
    let edited = apply(pending);
    assert!(!edited.keys.contains_key("Alt + J"));
    assert_eq!(edited.keys["Alt + H"], focus(Direction::Right));
    assert_eq!(edited.keys["Alt + L"], focus(Direction::Right));
    settings.window.toolbar().set_page_controls(&ui, None);
    settings.router.model.env.shortcut_capture.borrow_mut().take();
}

pub fn run_keyboard(ui: Ui) {
    let (requests, mut pending) = tokio::sync::mpsc::unbounded_channel();
    let settings = Settings::new(
        ui,
        source(),
        "/tmp/native-keyboard-test.toml".into(),
        vec![],
        vec![],
        requests,
        || {},
    );
    keyboard_controls(ui, &settings, &mut pending);
}

/// Exercises the native controls at the edit boundary: each destination must still
/// submit its existing config key, and acknowledged changes drive dependent controls.
fn interface_controls(
    settings: &Settings,
    pending: &mut tokio::sync::mpsc::UnboundedReceiver<Request>,
) {
    use crate::common::config::{ConfigDocument, HorizontalPlacement, MenuBarDisplayMode};
    let original = settings.model().source.borrow().as_ref().clone();
    let mut source = original.clone();
    let root = root_window(settings);
    let commit = |source: &mut ConfigSource,
                  pending: &mut tokio::sync::mpsc::UnboundedReceiver<Request>| {
        let request = pending.try_recv().expect("a native control submits its edit");
        let Action::Edit(edit) = request.action else {
            panic!("settings edit")
        };
        edit(source).unwrap();
        (request.finish)(Ok(()));
        settings.synchronize(source.clone());
        assert!(
            pending.try_recv().is_err(),
            "acknowledgement never submits another edit"
        );
    };
    let toggle = |title: &str, value: bool| {
        let control = named::<NSSwitch>(&root, title).unwrap();
        control.setState(if value { 1 } else { 0 });
        assert!(unsafe { control.sendAction_to(control.action(), control.target().as_deref()) });
    };
    settings.router.navigate(Category::MenuBar);
    toggle("Show menu bar indicator", true);
    commit(&mut source, pending);
    let mode = named::<NSPopUpButton>(&root, "Workspaces to show").unwrap();
    assert!(mode.isEnabled());
    choose(&mode, 1);
    commit(&mut source, pending);
    assert_eq!(source.settings.ui.menu_bar.mode, MenuBarDisplayMode::Active);
    toggle("Show menu bar indicator", false);
    commit(&mut source, pending);
    assert!(!mode.isEnabled());

    settings.router.navigate(Category::Overview);
    assert!(named::<NSButton>(&root, "Check Screen Recording Access…").is_some());
    toggle("Enabled", true);
    commit(&mut source, pending);
    toggle("Window previews", false);
    commit(&mut source, pending);
    assert!(!source.settings.ui.mission_control.window_previews);
    toggle("Fade transitions", true);
    commit(&mut source, pending);
    let duration = named::<NSSlider>(&root, "Fade duration").unwrap();
    assert!(duration.isEnabled());
    toggle("Fade transitions", false);
    commit(&mut source, pending);
    assert!(!duration.isEnabled());

    let windows = NSApplication::sharedApplication(settings.router.ui.mtm()).windows().len();
    settings.router.navigate(Category::StackTabs);
    assert_eq!(
        NSApplication::sharedApplication(settings.router.ui.mtm()).windows().len(),
        windows,
        "the actual tab component embeds without allocating any preview windows"
    );
    toggle("Enabled", true);
    commit(&mut source, pending);
    let placement = named::<NSPopUpButton>(&root, "Horizontal placement").unwrap();
    choose(&placement, 1);
    commit(&mut source, pending);
    assert_eq!(
        source.settings.ui.stack_line.horiz_placement,
        HorizontalPlacement::Bottom
    );
    let vertical = named::<NSPopUpButton>(&root, "Vertical placement").unwrap();
    choose(&vertical, 1);
    commit(&mut source, pending);
    assert_eq!(
        source.settings.ui.stack_line.vert_placement,
        crate::common::config::VerticalPlacement::Right
    );
    for (title, value) in [("Thickness", 44.0), ("Spacing", 12.0)] {
        let slider = named::<NSSlider>(&root, title).unwrap();
        slider.setDoubleValue(value);
        assert!(unsafe { slider.sendAction_to(slider.action(), slider.target().as_deref()) });
        commit(&mut source, pending);
    }
    assert_eq!(source.settings.ui.stack_line.thickness, 44.0);
    assert_eq!(source.settings.ui.stack_line.spacing, 12.0);
    root.layoutSubtreeIfNeeded();
    let tab = named::<NSButton>(&root, "Terminal").unwrap();
    unsafe {
        tab.performClick(None);
    }
    assert_eq!(
        tab.state(),
        1,
        "selecting a preview tab updates its active state"
    );
    assert!(
        pending.try_recv().is_err(),
        "example tabs never edit real settings or select real windows"
    );
    // A failed save must discard the transient slider preview, even with no new
    // source revision arriving from ConfigActor.
    let bar = named::<NSView>(&root, "Horizontal stack example").unwrap();
    let thickness = named::<NSSlider>(&root, "Thickness").unwrap();
    thickness.setDoubleValue(48.0);
    assert!(unsafe { thickness.sendAction_to(thickness.action(), thickness.target().as_deref()) });
    root.layoutSubtreeIfNeeded();
    assert_eq!(bar.frame().size.height, 48.0);
    let rejected = pending.try_recv().unwrap();
    (rejected.finish)(Err("The configuration could not be saved.".into()));
    root.layoutSubtreeIfNeeded();
    assert_eq!(bar.frame().size.height, source.settings.ui.stack_line.thickness);
    let reset = named::<NSButton>(&root, "Reset preview").unwrap();
    unsafe {
        reset.performClick(None);
    }
    assert_eq!(tab.state(), 0, "reset restores the first example window");
    toggle("Enabled", false);
    commit(&mut source, pending);
    assert!(!placement.isEnabled());
    assert!(named::<NSButton>(&root, "Terminal").is_none());
    // Use the real configuration document serializer/parser, not a preview state.
    let mut document = ConfigDocument::default();
    document
        .update(|current| current.settings.ui = source.settings.ui.clone())
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rift.toml");
    document.save(&path).unwrap();
    let loaded = ConfigDocument::read(&path).unwrap().source().unwrap();
    assert_eq!(loaded.settings.ui, source.settings.ui);
    settings.synchronize(original);
    settings.router.navigate(Category::General);
}

/// Exercise the real About controls without network access, including a completion
/// delivered after teardown. This catches lost/strong callbacks and stuck checking UI.
pub fn run_about(ui: Ui) {
    use objc2_app_kit::NSTextField;
    let (model, env, page, button, status, pending_update) = autoreleasepool(|_| {
        let (requests, mut pending) = tokio::sync::mpsc::unbounded_channel();
        let settings = Settings::new(
            ui,
            source(),
            "/tmp/native-about-test.toml".into(),
            vec![],
            vec![],
            requests,
            || {},
        );
        settings.router.navigate(Category::About);
        assert!(
            pending.try_recv().is_err(),
            "opening About never checks the network"
        );
        let page = settings.router.pages.borrow()[Category::About.index()]
            .as_ref()
            .unwrap()
            .view
            .clone();
        let button = named::<NSButton>(page.ns_view(), "Check for Updates…").unwrap();
        let status = named::<NSTextField>(page.ns_view(), "Update status").unwrap();
        assert_eq!(status.stringValue().to_string(), "Not checked yet");
        for (result, expected) in [
            (Ok(updates::Release::Current), "Rift is up to date."),
            (
                Ok(updates::Release::Available("0.7.0".into())),
                "Version 0.7.0 is available.",
            ),
            (
                Err("Couldn’t connect. Try again.".into()),
                "Couldn’t connect. Try again.",
            ),
        ] {
            assert!(unsafe { button.sendAction_to(button.action(), button.target().as_deref()) });
            assert!(!button.isEnabled(), "one check at a time");
            assert_eq!(status.stringValue().to_string(), "Checking for updates…");
            let request = pending.try_recv().expect("a user-initiated update request");
            let Action::CheckUpdates(done) = request.action else {
                panic!("update action")
            };
            done(result);
            (request.finish)(Ok(()));
            assert!(button.isEnabled(), "success and failure both allow retry");
            assert_eq!(status.stringValue().to_string(), expected);
            assert!(
                pending.try_recv().is_err(),
                "completion never starts another check"
            );
        }
        // A second request stays pending while its entire Settings hierarchy closes.
        assert!(unsafe { button.sendAction_to(button.action(), button.target().as_deref()) });
        let request = pending.try_recv().unwrap();
        let weak = (
            Rc::downgrade(settings.model()),
            Rc::downgrade(&settings.model().env),
            objc2::rc::Weak::new(page.ns_view()),
            objc2::rc::Weak::new(&*button),
            objc2::rc::Weak::new(&*status),
        );
        settings.window.ns_window().close();
        drop(settings);
        (weak.0, weak.1, weak.2, weak.3, weak.4, request)
    });
    assert!(model.upgrade().is_none());
    assert!(
        env.upgrade().is_none(),
        "pending update work must not retain the environment"
    );
    autoreleasepool(|_| {
        objc2_foundation::NSRunLoop::currentRunLoop()
            .runUntilDate(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.05));
    });
    assert!(
        page.load().is_none(),
        "pending update work must not retain the page"
    );
    assert!(button.load().is_none());
    assert!(status.load().is_none());
    let Action::CheckUpdates(done) = pending_update.action else {
        panic!("update action")
    };
    done(Err("A late result after Settings closed".into()));
    (pending_update.finish)(Ok(()));
}

/// Profiling only: repeatedly constructs and closes hidden Settings windows.
/// Its entry point is commented out in tests/settings_native.rs. Keep it disabled
/// for routine development; enable only for startup, memory, or teardown profiling.
pub fn run_lifecycle(ui: Ui) {
    let app = NSApplication::sharedApplication(ui.mtm());
    let policy = app.activationPolicy();
    for cycle in 0..20 {
        let (requests, mut pending) = tokio::sync::mpsc::unbounded_channel();
        let closed = Rc::new(Cell::new(false));
        let on_close = closed.clone();
        let mut completions = Vec::new();
        let (model, router, env, window, toolbar, pages, sheet, draft) = autoreleasepool(|_| {
            let settings = Settings::new(
                ui,
                source(),
                "/tmp/native-settings-test.toml".into(),
                vec![],
                vec![],
                requests,
                move || on_close.set(true),
            );
            settings.activation.activate(); // Exercise lifecycle without presenting over the user’s work.
            let presented_frame = settings.window.ns_window().frame();
            settings.window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
            assert_eq!(
                settings.window.ns_window().frame(),
                presented_frame,
                "initial construction completes layout before another layout pass"
            );
            assert!(pending.try_recv().is_err(), "General does not need discovery");
            for &(category, _, _) in &CATEGORIES {
                settings.router.navigate(category);
            }
            for &(_, mode) in pages::layouts() {
                settings.router.layouts().select_target(layouts::Target::Layout(mode));
            }
            for target in [layouts::Target::Spacing, layouts::Target::Displays] {
                settings.router.layouts().select_target(target);
            }
            let discovery = pending.try_recv().unwrap();
            assert!(matches!(discovery.action, Action::RefreshRuntime));
            completions.push(discovery);
            settings.router.navigate(Category::About);
            let about = settings.router.pages.borrow()[Category::About.index()]
                .as_ref()
                .unwrap()
                .view
                .clone();
            let check =
                named::<objc2_app_kit::NSButton>(about.ns_view(), "Check for Updates…").unwrap();
            assert_eq!(check.title().to_string(), "Check for Updates…");
            assert!(unsafe { check.sendAction_to(check.action(), check.target().as_deref()) });
            completions.push(pending.try_recv().unwrap());
            // Leave discovery and update completions pending and a sheet open during teardown.
            settings.router.navigate(Category::Workspaces);
            let page = settings.router.pages.borrow()[Category::Workspaces.index()]
                .as_ref()
                .unwrap()
                .view
                .clone();
            let table = find::<NSTableView>(page.ns_view()).unwrap();
            table.selectRowIndexes_byExtendingSelection(
                &objc2_foundation::NSIndexSet::indexSetWithIndex(0),
                false,
            );
            assert!(unsafe { table.sendAction_to(table.action(), table.target().as_deref()) });
            let draft = Rc::downgrade(settings.router.model.sheet_model.borrow().as_ref().unwrap());
            let sheet = objc2::rc::Weak::new(
                settings.router.model.sheet.borrow().as_ref().unwrap().ns_window(),
            );
            let model = Rc::downgrade(&settings.router.model);
            let router = Rc::downgrade(&settings.router);
            let env = Rc::downgrade(&settings.router.model.env);
            let window = objc2::rc::Weak::new(settings.window.ns_window());
            let toolbar = objc2::rc::Weak::new(settings.window.toolbar().ns_toolbar());
            let pages: Vec<_> = settings
                .router
                .pages
                .borrow()
                .iter()
                .flatten()
                .map(|page| objc2::rc::Weak::new(page.view.ns_view()))
                .collect();
            if cycle == 0 {
                settings.model().close_sheet();
                app.mainMenu()
                    .unwrap()
                    .itemAtIndex(0)
                    .unwrap()
                    .submenu()
                    .unwrap()
                    .performActionForItemAtIndex(0);
            } else {
                settings.window.ns_window().close();
            }
            assert!(closed.get(), "native close notifies the deferred owner");
            assert!(model.upgrade().is_some(), "owner survives windowWillClose");
            assert_eq!(
                app.activationPolicy(),
                policy,
                "close restores activation policy"
            );
            autoreleasepool(|_| drop(settings));
            (model, router, env, window, toolbar, pages, sheet, draft)
        });
        assert!(model.upgrade().is_none());
        assert!(router.upgrade().is_none());
        assert!(
            env.upgrade().is_none(),
            "pending completions must not retain the environment"
        );
        assert!(draft.upgrade().is_none(), "release the open sheet's draft");
        // Late asynchronous completions must remain safe after native and Rust teardown.
        for request in completions {
            if let Action::CheckUpdates(done) = request.action {
                done(Ok(updates::Release::Current));
            }
            (request.finish)(Ok(()));
        }
        autoreleasepool(|_| {
            objc2_foundation::NSRunLoop::currentRunLoop()
                .runUntilDate(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.05));
        });
        assert!(window.load().is_none(), "release the native window");
        assert!(toolbar.load().is_none(), "release the native toolbar");
        assert!(sheet.load().is_none(), "release the attached sheet");
        assert!(
            pages.iter().all(|page| page.load().is_none()),
            "release cached native pages"
        );
    }
}

fn source() -> ConfigSource {
    ConfigSource {
        settings: crate::common::config::Config::default().settings,
        keys: Default::default(),
        binding_modes: Default::default(),
        virtual_workspaces: Default::default(),
        modifier_combinations: Default::default(),
    }
}

// Open the real Add Rule sheet before runtime discovery completes: late inventory must
// redraw its native list without changing the draft or requiring another search keystroke.
fn late_runtime_populates_picker(
    settings: &Settings,
    pending: &mut tokio::sync::mpsc::UnboundedReceiver<Request>,
) {
    fn add_button(view: &NSView) -> Option<Retained<objc2_app_kit::NSButton>> {
        if let Some(button) = view.downcast_ref::<objc2_app_kit::NSButton>()
            && button.title().to_string() == "Add"
        {
            return Some(button.retain());
        }
        view.subviews().iter().find_map(|child| add_button(&child))
    }
    settings.router.navigate(Category::Rules);
    let pages = settings.router.pages.borrow();
    let add = add_button(pages[Category::Rules.index()].as_ref().unwrap().view.ns_view()).unwrap();
    drop(pages);
    assert!(unsafe { add.sendAction_to(add.action(), add.target().as_deref()) });
    let window = settings.model().sheet.borrow().as_ref().unwrap().ns_window().retain();
    let list = find::<NSTableView>(&window.contentView().unwrap()).unwrap();
    assert_eq!(list.numberOfRows(), 0);
    settings.refresh_applications(vec![rift_protocol::ApplicationData {
        pid: 1,
        bundle_id: Some("dev.test.Late".into()),
        name: "Late".into(),
        is_frontmost: false,
        window_count: 1,
    }]);
    assert_eq!(list.numberOfRows(), 1, "late runtime populates an open picker");
    list.selectRowIndexes_byExtendingSelection(
        &objc2_foundation::NSIndexSet::indexSetWithIndex(0),
        false,
    );
    fn bundle_input(view: &NSView) -> Option<Retained<objc2_app_kit::NSTextField>> {
        if let Some(field) = view.downcast_ref::<objc2_app_kit::NSTextField>()
            && field
                .placeholderString()
                .is_some_and(|value| value.to_string() == "com.apple.Safari")
        {
            return Some(field.retain());
        }
        view.subviews().iter().find_map(|child| bundle_input(&child))
    }
    let input = bundle_input(&window.contentView().unwrap()).unwrap();
    assert_eq!(input.stringValue().to_string(), "dev.test.Late");
    settings.refresh_applications(vec![]);
    assert_eq!(list.numberOfRows(), 0, "removed apps leave the picker");
    assert_eq!(
        input.stringValue().to_string(),
        "dev.test.Late",
        "inventory refresh preserves the chosen matcher"
    );
    settings.model().close_sheet();
    settings.router.navigate(Category::General);
    while let Ok(request) = pending.try_recv() {
        assert!(
            matches!(request.action, Action::RefreshRuntime),
            "discovery must not edit config"
        );
    }
}

/// Hidden-window visual fixture: captures actual AppKit content without activation or presentation.
pub fn render(
    ui: Ui,
    dark: bool,
    small: bool,
    scope: Option<usize>,
    page: Option<usize>,
    viewport: Option<CGSize>,
    path: &str,
) {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSBitmapImageFileType,
    };
    use objc2_foundation::{NSDictionary, NSString};
    let (requests, _pending) = tokio::sync::mpsc::unbounded_channel();
    let mut snapshot = source();
    if page == Some(Category::Keyboard.index()) && scope == Some(35) {
        snapshot.keys.clear();
        snapshot.binding_modes.clear();
        snapshot.modifier_combinations.clear();
    }
    if page == Some(Category::MenuBar.index()) {
        snapshot.settings.ui.menu_bar.enabled = true;
        snapshot.settings.ui.menu_bar.show_empty = true;
        if scope == Some(9) {
            snapshot.settings.ui.menu_bar.display_style =
                crate::common::config::WorkspaceDisplayStyle::Label;
            snapshot.settings.ui.menu_bar.active_label =
                crate::common::config::ActiveWorkspaceLabel::Name;
        }
    }
    if page == Some(Category::StackTabs.index()) {
        if scope == Some(10) {
            snapshot.settings.ui.stack_line.enabled = false;
        }
        if scope == Some(11) {
            let tabs = &mut snapshot.settings.ui.stack_line;
            tabs.horiz_placement = crate::common::config::HorizontalPlacement::Bottom;
            tabs.vert_placement = crate::common::config::VerticalPlacement::Right;
            tabs.thickness = 40.0;
            tabs.spacing = 12.0;
        }
    }
    if page == Some(Category::MenuBar.index()) && scope == Some(12) {
        snapshot.settings.ui.menu_bar.enabled = false;
    }
    if page == Some(Category::Keyboard.index()) && scope != Some(35) {
        snapshot.keys = defaults().keys.clone();
        snapshot.modifier_combinations = defaults().modifier_combinations.clone();
    }
    if page == Some(Category::Overview.index()) && scope == Some(13) {
        snapshot.settings.ui.mission_control.enabled = true;
        snapshot.settings.ui.mission_control.fade_enabled = true;
    }
    if scope == Some(7) {
        snapshot.settings.layout.gaps.outer.top = 5.0;
    }
    let mut displays = vec![];
    // 20..=23: a workspace sheet's Display row, per-display, global, pinned to a
    // disconnected display, and entering a display by hand.
    let workspace_sheet = matches!(scope, Some(20..=23));
    if matches!(scope, Some(21..=23)) {
        snapshot.virtual_workspaces.mode = crate::common::config::WorkspaceMode::Global;
    }
    if scope == Some(22) {
        snapshot.virtual_workspaces.workspace_rules.push(
            crate::common::config::WorkspaceLayoutRule {
                workspace: crate::common::config::WorkspaceSelector::Index(1),
                layout: None,
                display: Some(vec![
                    "builtin".into(),
                    "right".into(),
                    "37D8832A-2D66-02CA-B9F7-8F30A301B230".into(),
                ]),
            },
        );
    }
    if scope == Some(8) || workspace_sheet {
        snapshot.settings.layout.gaps.outer.top = 5.0;
        for (id, uuid, name) in [
            (1, "studio", "Studio Display"),
            (2, "builtin", "Built-in Retina Display"),
        ] {
            displays.push(ScreenInfo {
                id: crate::sys::screen::ScreenId::new(id),
                frame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1512.0, 982.0)),
                is_builtin: false,
                backing_scale: 2.0,
                display_uuid: uuid.into(),
                name: Some(name.into()),
                space: None,
            });
        }
        let gaps = &mut snapshot.settings.layout.gaps;
        let mut inner = gaps.inner.clone();
        inner.horizontal = 12.0;
        inner.vertical = 12.0;
        gaps.per_display.entry("studio".into()).or_default().inner = Some(inner);
        let widths = snapshot
            .settings
            .layout
            .scrolling
            .per_display
            .entry("studio".into())
            .or_default();
        widths.column_width_ratio = Some(0.5);
        snapshot
            .settings
            .layout
            .scrolling
            .per_display
            .entry("5A0C3D2E-OLD".into())
            .or_default()
            .max_column_width_ratio = Some(0.9);
    }
    snapshot.settings.ui.menu_bar.layout_folder = "/Users/atticus/.config/rift/layouts".into();
    snapshot.settings.run_on_start = vec!["sketchybar --reload".into()];
    snapshot
        .virtual_workspaces
        .app_rules
        .push(crate::common::config::AppWorkspaceRule {
            app_id: Some("com.example.Tool".into()),
            floating: true,
            ..Default::default()
        });
    snapshot
        .virtual_workspaces
        .app_rules
        .push(crate::common::config::AppWorkspaceRule {
            app_id: Some("org.mozilla.firefox".into()),
            workspace: Some(crate::common::config::WorkspaceSelector::Name(
                "Communication".into(),
            )),
            focus: true,
            ..Default::default()
        });
    let settings = Settings::new(
        ui,
        snapshot,
        "/tmp/native-settings-preview.toml".into(),
        displays,
        vec![],
        requests,
        || {},
    );
    settings
        .router
        .navigate(page.map_or(Category::Layouts, |page| CATEGORIES[page].0));
    if let Some(scope) = scope {
        match scope {
            7 => settings.router.layouts().select_target(layouts::Target::Spacing),
            8 => settings.router.layouts().select_target(layouts::Target::Displays),
            1..=6 => settings.router.layouts().select_target(layouts::Target::Layout(
                crate::common::config::LayoutMode::from_config_choice(scope - 1).unwrap(),
            )),
            _ => {}
        }
    }
    let window = settings.window.ns_window();
    window.setAppearance(
        NSAppearance::appearanceNamed(unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        })
        .as_deref(),
    );
    if workspace_sheet {
        // Mirror record_editor's sheet chrome in an unpresented window.
        let page = super::editors::workspace_detail(ui, settings.model(), 1);
        page.synchronize(settings.model());
        let content = VStack::new(&ui)
            .insets(Insets {
                top: 24.0,
                left: 24.0,
                bottom: 20.0,
                right: 24.0,
            })
            .spacing(16.0)
            .push(SectionTitle::new(&ui, "Workspace 2"))
            .push(page.view.clone())
            .push(SheetActions::new(&ui));
        let sheet = cgs::window::Window::new(&ui).title("Workspace 2").content(content);
        let sheet_window = sheet.ns_window();
        sheet_window.setAppearance(window.appearance().as_deref());
        let view = sheet_window.contentView().unwrap();
        if scope == Some(23) {
            let add = named::<NSButton>(&view, "Add Display Preference").unwrap();
            let menu = add.menu().unwrap();
            menu.performActionForItemAtIndex(menu.numberOfItems() - 1);
        }
        view.layoutSubtreeIfNeeded();
        sheet_window.setContentSize(CGSize::new(
            viewport.map_or(480.0, |size| size.width),
            view.fittingSize().height,
        ));
        // Capture the frame view so the window background is drawn, as below.
        let view = unsafe { view.superview() }.unwrap_or(view);
        view.layoutSubtreeIfNeeded();
        let bitmap = view.bitmapImageRepForCachingDisplayInRect(view.bounds()).unwrap();
        view.cacheDisplayInRect_toBitmapImageRep(view.bounds(), &bitmap);
        let png = unsafe {
            bitmap.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }
        .unwrap();
        assert!(png.writeToFile_atomically(&NSString::from_str(path), true));
        return;
    }
    // Hidden AppKit windows can adjust their fitting size as controls are inserted.
    // Capture the requested viewport after the selected editor has been mounted.
    window.setContentSize(viewport.unwrap_or(if small {
        CGSize::new(700.0, 420.0)
    } else {
        CGSize::new(880.0, 660.0)
    }));
    let content = window.contentView().unwrap();
    fn invalidate_layout(view: &NSView) {
        view.setNeedsUpdateConstraints(true);
        view.setNeedsLayout(true);
        for child in view.subviews() {
            invalidate_layout(&child);
        }
    }
    for _ in 0..3 {
        invalidate_layout(&content);
        content.updateConstraintsForSubtreeIfNeeded();
        content.layoutSubtreeIfNeeded();
    }
    assert!(
        content.bounds().size.width
            <= window.contentRectForFrameRect(window.frame()).size.width + 1.0,
        "the native content must fit the requested viewport width"
    );
    if small {
        let sidebar = named::<NSTableView>(&content, "Settings categories").unwrap();
        let current = settings.router.current().title();
        sidebar.scrollRowToVisible(navigation_row(&sidebar, current) as isize);
    }
    if page == Some(Category::About.index()) && matches!(scope, Some(14 | 15)) {
        let emblem = named::<NSButton>(&content, "Rearrange windows").unwrap();
        for _ in 0..scope.unwrap() - 13 {
            assert!(unsafe { emblem.sendAction_to(emblem.action(), emblem.target().as_deref()) });
        }
        objc2_foundation::NSRunLoop::currentRunLoop()
            .runUntilDate(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.35));
        content.layoutSubtreeIfNeeded();
    }
    if scope == Some(36) {
        let pages = settings.router.pages.borrow();
        let root = pages[settings.router.current().index()].as_ref().unwrap().view.ns_view();
        let scroll = find::<objc2_app_kit::NSScrollView>(root).unwrap();
        let clip = scroll.contentView();
        let mut bounds = clip.bounds();
        bounds.origin.y += 240.0;
        clip.scrollToPoint(clip.constrainBoundsRect(bounds).origin);
        scroll.reflectScrolledClipView(&clip);
        content.layoutSubtreeIfNeeded();
    }
    // Keyboard recording, catalog, key search, add editor and inline options.
    if page == Some(Category::Keyboard.index()) && matches!(scope, Some(30..=34 | 37 | 38)) {
        let header = settings
            .window
            .toolbar()
            .ns_toolbar()
            .items()
            .iter()
            .find(|item| item.itemIdentifier().to_string() == "cgs.page-filter")
            .unwrap()
            .view()
            .unwrap();
        let click = |title: &str| {
            let button = named::<NSButton>(&content, title).unwrap();
            assert!(unsafe { button.sendAction_to(button.action(), button.target().as_deref()) });
        };
        match scope {
            Some(30) => click("Shortcut for Move window Left"),
            Some(31) | Some(33) => {
                named::<NSPopUpButton>(&header, "Shortcut Set Actions")
                    .unwrap()
                    .menu()
                    .unwrap()
                    .performActionForItemAtIndex(1);
                if scope == Some(33) {
                    let catalog = settings
                        .model()
                        .sheet
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .ns_window()
                        .contentView()
                        .unwrap();
                    let table = find::<NSTableView>(&catalog).unwrap();
                    table.selectRowIndexes_byExtendingSelection(
                        &objc2_foundation::NSIndexSet::indexSetWithIndex(navigation_row(
                            &table,
                            "Toggle floating",
                        )),
                        false,
                    );
                    let next = named::<NSButton>(&catalog, "Continue").unwrap();
                    assert!(unsafe { next.sendAction_to(next.action(), next.target().as_deref()) });
                }
            }
            Some(34 | 37 | 38) => {
                window.orderFront(None);
                if scope == Some(38) {
                    let search =
                        named::<objc2_app_kit::NSSearchField>(&content, "Filter shortcuts")
                            .unwrap();
                    change_text(&search, "Run command");
                    content.layoutSubtreeIfNeeded();
                }
                click(if scope == Some(38) {
                    "Edit Run command options"
                } else {
                    "Edit Toggle floating options"
                });
                if scope == Some(37) {
                    let size = NSApplication::sharedApplication(ui.mtm())
                        .windows()
                        .iter()
                        .filter_map(|window| window.contentView())
                        .find_map(|view| named::<NSPopUpButton>(&view, "Floating size"))
                        .unwrap();
                    choose(&size, 2);
                }
            }
            _ => {
                let search =
                    named::<objc2_app_kit::NSSearchField>(&content, "Filter shortcuts").unwrap();
                change_text(&search, "alt+h");
            }
        }
        for _ in 0..3 {
            invalidate_layout(&content);
            content.layoutSubtreeIfNeeded();
        }
    }
    let card = match scope {
        Some(7) => Some("Spacing"),
        Some(8) => Some("Display overrides"),
        _ => None,
    };
    let spacing_card = if let Some(card) = card {
        let view = unsafe { named::<NSButton>(&content, card).unwrap().superview() }.unwrap();
        view.scrollRectToVisible(view.bounds());
        Some(view)
    } else {
        None
    };
    // Include native window chrome without ordering the window on screen.
    let frame = if page == Some(Category::Keyboard.index()) && matches!(scope, Some(34 | 37 | 38)) {
        let popup = NSApplication::sharedApplication(ui.mtm())
            .windows()
            .iter()
            .filter_map(|window| window.contentView())
            .find(|view| {
                if scope == Some(38) {
                    named::<objc2_app_kit::NSTextField>(view, "Parameter").is_some()
                } else {
                    named::<NSPopUpButton>(view, "Floating placement").is_some()
                }
            })
            .expect("native options popover");
        unsafe { popup.superview() }.unwrap_or(popup)
    } else if page == Some(Category::Keyboard.index()) && matches!(scope, Some(31 | 33)) {
        let view = settings
            .model()
            .sheet
            .borrow()
            .as_ref()
            .unwrap()
            .ns_window()
            .contentView()
            .unwrap();
        view.layoutSubtreeIfNeeded();
        unsafe { view.superview() }.unwrap_or(view)
    } else {
        unsafe { content.superview() }.unwrap_or(content)
    };
    frame.layoutSubtreeIfNeeded();
    let capture = |view: &NSView, rect: CGRect, path: &str| {
        let bitmap = view.bitmapImageRepForCachingDisplayInRect(rect).unwrap();
        view.cacheDisplayInRect_toBitmapImageRep(rect, &bitmap);
        let png = unsafe {
            bitmap.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }
        .unwrap();
        assert!(png.writeToFile_atomically(&NSString::from_str(path), true));
    };
    capture(&frame, frame.bounds(), path);
    if let Some(card) = spacing_card {
        capture(
            &frame,
            card.convertRect_toView(card.bounds(), Some(&frame)),
            &format!("{path}.spacing.png"),
        );
    }
    // Keep a native fixture available briefly for live compositor/blur inspection.
    if std::env::args().any(|arg| arg == "--inspect") {
        settings.show();
        objc2_foundation::NSRunLoop::currentRunLoop()
            .runUntilDate(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(45.0));
        window.orderOut(None);
    }
    // Sheets and popovers require an ordered parent; dismiss it after capture.
    if page == Some(Category::Keyboard.index()) && matches!(scope, Some(31 | 33 | 34 | 37 | 38)) {
        settings.model().close_sheet();
        window.orderOut(None);
    }
    assert!(!window.isVisible(), "rendering leaves no test window visible");
}
