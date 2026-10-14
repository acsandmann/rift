use objc2::Message;
use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSButton, NSControl, NSOutlineView, NSPopUpButton,
    NSSegmentedControl, NSSlider, NSSwitch, NSTableView, NSView,
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
    view.subviews().iter().find_map(|child| named(&child, title))
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
        assert_eq!(count::<NSOutlineView>(&root), 0, "search replaces the sidebar");
        change_text(&search, "   ");
        assert_eq!(
            count::<NSOutlineView>(&root),
            1,
            "clearing search restores navigation"
        );
        change_text(&search, "this setting does not exist");
        assert_eq!(count::<NSOutlineView>(&root), 0);
        change_text(&search, "gaps");
        assert_eq!(
            count::<NSOutlineView>(&root),
            0,
            "search works after an empty result"
        );
        change_text(&search, "");
        assert_eq!(find::<NSOutlineView>(&root).unwrap().numberOfRows(), 9);
        let sidebar = find::<NSOutlineView>(&root).unwrap();
        sidebar.selectRowIndexes_byExtendingSelection(
            &objc2_foundation::NSIndexSet::indexSetWithIndex(1),
            false,
        );
        let toolbar = settings.window.toolbar().ns_toolbar();
        assert!(
            toolbar.items().iter().all(|item| {
                !matches!(
                    item.itemIdentifier().to_string().as_str(),
                    "cgs.back" | "cgs.history" | "cgs.page-menu"
                )
            }),
            "preferences toolbar has no browser navigation"
        );
        sidebar.selectRowIndexes_byExtendingSelection(
            &objc2_foundation::NSIndexSet::indexSetWithIndex(0),
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
            let before = settings.model().source.borrow().clone();
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
            let frame = settings.window.ns_window().frame();
            let mut cached_instances = None;
            for _ in 0..3 {
                for (title, _) in pages::layouts() {
                    let button = named::<NSButton>(root, &format!("Configure {title}")).unwrap();
                    unsafe {
                        button.performClick(None);
                    }
                    assert_eq!(button.state(), 1);
                    assert_eq!(
                        settings.window.ns_window().frame(),
                        frame,
                        "changing the inspected layout preserves the user’s window size"
                    );
                    assert!(*settings.model().source.borrow() == before);
                    assert!(pending.try_recv().is_err());
                }
                let count = instances(root);
                if let Some(expected) = cached_instances {
                    assert_eq!(count, expected, "switching reuses native views");
                }
                cached_instances = Some(count);
            }
            let spacing = named::<NSButton>(root, "Spacing").unwrap();
            let displays = named::<NSButton>(root, "Displays").unwrap();
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
            assert_eq!(spacing.state(), 0, "opening Displays collapses Spacing");
            assert_eq!(displays.state(), 1);
            assert_eq!(
                settings.window.ns_window().frame(),
                frame,
                "expanding displays preserves window size"
            );
            assert!(
                named::<NSControl>(root, "Screen edges").is_none(),
                "the other editor is hidden"
            );
            assert_eq!(displays.state(), 1);
            unsafe {
                spacing.performClick(None);
            }
            assert!(
                std::ptr::eq(&*gap, &*named::<NSControl>(root, "Screen edges").unwrap()),
                "reopening reuses the editor"
            );
            assert_eq!(displays.state(), 0, "opening Spacing collapses Displays");
            assert!(*settings.model().source.borrow() == before);
            assert!(
                pending.try_recv().is_err(),
                "expansion never writes configuration"
            );
            for query in ["Master width", "Screen edges", "Display settings"] {
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
                assert_eq!(
                    find::<NSOutlineView>(&root_window(&settings)).unwrap().selectedRow(),
                    Category::Layouts.index() as isize
                );
            }
            settings.router.layouts().select_target(layouts::Target::Layout(
                crate::common::config::LayoutMode::Traditional,
            ));
            let position = named::<NSPopUpButton>(root, "New window position").unwrap();
            assert_eq!(
                position.titleOfSelectedItem().unwrap().to_string(),
                "Shared · Next to selection",
                "an inherited choice names its effective value"
            );
            let toggle = named::<NSSwitch>(root, "Keep windows equally sized").unwrap();
            let mut reloaded = before.clone();
            reloaded.settings.layout.traditional.equalize_nodes = false;
            settings.synchronize(reloaded);
            assert_eq!(toggle.state(), 0, "external reload updates visible controls");
            settings.synchronize(before.clone());
            // Switching must commit an actual AppKit field editor before hiding its client.
            settings
                .router
                .layouts()
                .select_target(layouts::Target::Layout(crate::common::config::LayoutMode::Bsp));
            let ratio =
                named::<objc2_app_kit::NSTextField>(root, "Single window aspect ratio").unwrap();
            assert!(settings.window.ns_window().makeFirstResponder(Some(&ratio)));
            ratio
                .currentEditor()
                .unwrap()
                .setString(&objc2_foundation::NSString::from_str("1.78"));
            unsafe {
                named::<NSButton>(root, "Configure Traditional").unwrap().performClick(None);
            }
            let request = pending.try_recv().expect("switch commits the field’s source edit");
            let Action::Edit(edit) = request.action else {
                panic!("field uses source edits");
            };
            let mut edited = before.clone();
            edit(&mut edited).unwrap();
            assert_eq!(edited.settings.layout.bsp.single_window_aspect_ratio, Some(1.78));
            assert!(ratio.currentEditor().is_none());
            unsafe {
                named::<NSButton>(root, "Configure Scrolling").unwrap().performClick(None);
            }
            unsafe {
                named::<NSButton>(root, "Set Scrolling as Default")
                    .expect("a nondefault layout offers Set as Default beside its title")
                    .performClick(None);
            }
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
            assert_eq!(
                named::<NSButton>(root, "Configure Scrolling").unwrap().state(),
                1,
                "making a layout default keeps editing it"
            );
            assert!(
                named::<NSButton>(root, "Set Scrolling as Default").is_none(),
                "the default layout offers no Set as Default action"
            );
            external.settings.layout.mode = before.settings.layout.mode;
            assert!(
                external == before,
                "default changes preserve individual layout settings"
            );
            settings.synchronize(before.clone());
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
            settings.router.layouts().select_target(layouts::Target::Layout(
                crate::common::config::LayoutMode::Scrolling,
            ));
            assert!(
                named::<NSButton>(root, "Restore Scrolling Defaults…").is_some(),
                "restoring defaults lives in the page footer"
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
            let mut source = draft.source.borrow().clone();
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
            let mut source = draft.source.borrow().clone();
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
        // Filtering must not change which command the selected visible row edits.
        autoreleasepool(|_| {
            use rift_protocol::LayoutCommand;

            use crate::actor::reactor::Command;
            use crate::actor::wm_controller::WmCommand;
            let mut source = settings.router.model.source.borrow().clone();
            source.keys.insert(
                "Alt+a".into(),
                WmCommand::ReactorCommand(Command::Layout(LayoutCommand::ToggleWindowFloating)),
            );
            source.keys.insert(
                "Alt+f".into(),
                WmCommand::ReactorCommand(Command::Layout(LayoutCommand::ToggleFullscreen)),
            );
            settings.router.model.replace_source(source);
            let page = commands::keyboard(ui, &settings.router.model);
            page.synchronize(&settings.router.model);
            settings.window.toolbar().set_page_controls(&ui, page.header.as_deref());
            let search = toolbar
                .items()
                .iter()
                .find_map(|item| item.downcast::<objc2_app_kit::NSSearchToolbarItem>().ok())
                .unwrap()
                .searchField();
            change_text(&search, "fullscreen");
            let table = find::<NSTableView>(page.view.ns_view()).unwrap();
            assert_eq!(table.numberOfRows(), 1);
            table.selectRowIndexes_byExtendingSelection(
                &objc2_foundation::NSIndexSet::indexSetWithIndex(0),
                false,
            );
            assert!(unsafe { table.sendAction_to(table.action(), table.target().as_deref()) });
            let window =
                settings.router.model.sheet.borrow().as_ref().unwrap().ns_window().retain();
            let action = find::<NSPopUpButton>(&window.contentView().unwrap()).unwrap();
            assert_eq!(
                action.titleOfSelectedItem().unwrap().to_string(),
                "Window · Fullscreen"
            );
            settings.router.model.close_sheet();
            settings.window.toolbar().set_page_controls(&ui, None);
            assert!(pending.try_recv().is_err());
        });
        // The primary Spacing page edits global values and preserves individual display overrides.
        autoreleasepool(|_| {
            use crate::sys::screen::NSScreenExt;
            let screen = settings.window.ns_window().screen().unwrap();
            settings.refresh_displays(vec![crate::sys::screen::ScreenInfo {
                id: screen.get_number().unwrap(),
                display_uuid: "focused-display".into(),
                name: Some("Focused display".into()),
                frame: Default::default(),
                backing_scale: 1.0,
                space: None,
            }]);
            let mut source = settings.router.model.source.borrow().clone();
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
            let mut source = settings.router.model.source.borrow().clone();
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
            for name in pages::EDGES.into_iter().chain(pages::AXES) {
                assert!(
                    named::<objc2_app_kit::NSTextField>(page.view.ns_view(), name)
                        .unwrap()
                        .isEditable()
                );
            }
            let top = named::<objc2_app_kit::NSTextField>(page.view.ns_view(), "Top").unwrap();
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
            let mut source = settings.router.model.source.borrow().clone();
            edit(&mut source).unwrap();
            assert_eq!(source.settings.layout.base.window_insertion_point, expected);
            settings.router.model.replace_source(source);
            page.synchronize(&settings.router.model);
        }
        let mut source = settings.router.model.source.borrow().clone();
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
            let check = find::<objc2_app_kit::NSButton>(about.ns_view()).unwrap();
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
                done(Ok("Up to date".into()));
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
pub fn render(ui: Ui, dark: bool, small: bool, scope: Option<usize>, path: &str) {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSBitmapImageFileType,
    };
    use objc2_foundation::{NSDictionary, NSString};
    let (requests, _pending) = tokio::sync::mpsc::unbounded_channel();
    let mut snapshot = source();
    if scope == Some(7) {
        snapshot.settings.layout.gaps.outer.top = 5.0;
    }
    let settings = Settings::new(
        ui,
        snapshot,
        "/tmp/native-settings-preview.toml".into(),
        vec![],
        vec![],
        requests,
        || {},
    );
    settings.router.navigate(Category::Layouts);
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
    // Hidden AppKit windows can adjust their fitting size as controls are inserted.
    // Capture the requested viewport after the selected editor has been mounted.
    window.setContentSize(if small {
        CGSize::new(700.0, 420.0)
    } else {
        CGSize::new(880.0, 660.0)
    });
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
        let sidebar = find::<NSOutlineView>(&content).unwrap();
        assert!(
            sidebar.rectOfRow(8).origin.y + sidebar.rectOfRow(8).size.height
                <= sidebar.visibleRect().origin.y + sidebar.visibleRect().size.height + 1.0,
            "all primary destinations fit in the smaller viewport"
        );
    }
    let spacing_card = if scope == Some(7) {
        let view = unsafe { named::<NSButton>(&content, "Spacing").unwrap().superview() }.unwrap();
        view.scrollRectToVisible(view.bounds());
        Some(view)
    } else {
        None
    };
    // Include native window chrome without ordering the window on screen.
    let frame = unsafe { content.superview() }.unwrap_or(content);
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
    assert!(!window.isVisible(), "rendering never presents a test window");
}
