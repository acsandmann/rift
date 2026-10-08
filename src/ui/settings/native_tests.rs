use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::{
    NSControl, NSMenuToolbarItem, NSOutlineView, NSPopUpButton, NSSlider, NSSwitch, NSTableView, NSView,
};

use super::*;

fn find<T: objc2::DowncastTarget + 'static>(view: &NSView) -> Option<Retained<T>> {
    if let Some(value) = view.downcast_ref::<T>() {
        return Some(value.retain());
    }
    view.subviews().iter().find_map(|child| find(&child))
}

fn count<T: objc2::DowncastTarget + 'static>(view: &NSView) -> usize {
    usize::from(view.downcast_ref::<T>().is_some())
        + view.subviews().iter().map(|child| count::<T>(&child)).sum::<usize>()
}

pub fn run(ui: Ui) {
    let (requests, mut pending) = tokio::sync::mpsc::unbounded_channel();
    let (model, host) = autoreleasepool(|_| {
        let mut source = ConfigSource {
            settings: crate::common::config::Config::default().settings,
            keys: Default::default(),
            binding_modes: Default::default(),
            virtual_workspaces: Default::default(),
            modifier_combinations: Default::default(),
        };
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
        assert_eq!(find::<NSOutlineView>(&root).unwrap().numberOfRows(), 9);
        let sidebar = find::<NSOutlineView>(&root).unwrap();
        sidebar.selectRowIndexes_byExtendingSelection(
            &objc2_foundation::NSIndexSet::indexSetWithIndex(1),
            false,
        );
        let toolbar = settings.window.toolbar().ns_toolbar();
        let back = toolbar.items().iter().find(|item| item.itemIdentifier().to_string() == "cgs.back").unwrap();
        let forward = toolbar.items().iter().find(|item| item.itemIdentifier().to_string() == "cgs.forward").unwrap();
        assert!(back.isEnabled(), "enable Back after visiting a section");
        assert!(!forward.isEnabled(), "disable Forward at the latest destination");
        sidebar.selectRowIndexes_byExtendingSelection(
            &objc2_foundation::NSIndexSet::indexSetWithIndex(0),
            false,
        );
        drop(sidebar);

        Settings::select(ui, &settings.model, &settings._host, &settings.pages, 4);
        settings.history.borrow_mut().select(4);
        let keyboard = settings.pages.borrow()[4].as_ref().unwrap().view.clone();
        settings.refresh_displays(vec![crate::sys::screen::ScreenInfo {
            id: crate::sys::screen::ScreenId::new(1),
            display_uuid: "test-display".into(),
            name: Some("Test display".into()),
            frame: Default::default(),
            backing_scale: 1.0,
            space: None,
        }]);
        assert!(std::ptr::eq(
            &*settings._host.ns_view().subviews().firstObject().unwrap(),
            keyboard.ns_view()
        ));
        settings.refresh_applications(vec![rift_protocol::ApplicationData {
            pid: 1,
            bundle_id: Some("dev.test.Editor".into()),
            name: "Editor".into(),
            is_frontmost: false,
            window_count: 1,
        }]);
        *settings.model.installed_applications.borrow_mut() = Some(vec![
            ("Editor".into(), "dev.test.Editor".into()),
            ("Utility".into(), "dev.test.Utility".into()),
        ]);
        settings.model.rebuild_applications();
        assert_eq!(settings.model.application_inventory.borrow().len(), 2);
        assert!(
            settings
                .model
                .application_inventory
                .borrow()
                .iter()
                .any(|app| app.search.contains("dev.test.editor"))
        );
        // The native menu switches layouts, history returns to the overview, and no secondary sidebar remains.
        autoreleasepool(|_| {
            let navigate = settings.model.navigate.borrow().as_ref().unwrap().clone();
            navigate(1);
            assert_eq!(count::<NSOutlineView>(&root), 1, "one persistent sidebar");
            let menu = toolbar
                .items()
                .iter()
                .find_map(|item| item.downcast::<NSMenuToolbarItem>().ok())
                .unwrap()
                .menu();
            autoreleasepool(|_| {
                menu.performActionForItemAtIndex(
                    menu.indexOfItemWithTitle(&objc2_foundation::NSString::from_str("Traditional")),
                )
            });
            let old =
                objc2::rc::Weak::new(settings.pages.borrow()[1].as_ref().unwrap().view.ns_view());
            assert_eq!(
                autoreleasepool(|_| count::<NSSwitch>(settings._host.ns_view())),
                1
            );
            autoreleasepool(|_| {
                menu.performActionForItemAtIndex(
                    menu.indexOfItemWithTitle(&objc2_foundation::NSString::from_str("BSP")),
                )
            });
            assert_eq!(count::<NSSwitch>(settings._host.ns_view()), 0);
            autoreleasepool(|_| {
                root.layoutSubtreeIfNeeded();
                objc2_foundation::NSRunLoop::currentRunLoop()
                    .runUntilDate(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.05));
            });
            assert!(old.load().is_none(), "release replaced subpages");
            assert_eq!(
                settings.history.borrow().history[settings.history.borrow().cursor],
                11
            );
            assert!(unsafe {
                NSApplication::sharedApplication(ui.mtm()).sendAction_to_from(
                    back.action().unwrap(),
                    back.target().as_deref(),
                    Some(&back),
                )
            });
            assert_eq!(
                count::<NSSwitch>(settings._host.ns_view()),
                1,
                "Back restores Traditional"
            );
            assert!(unsafe {
                NSApplication::sharedApplication(ui.mtm()).sendAction_to_from(
                    back.action().unwrap(),
                    back.target().as_deref(),
                    Some(&back),
                )
            });
            assert_eq!(
                count::<NSOutlineView>(settings.pages.borrow()[1].as_ref().unwrap().view.ns_view()),
                0
            );
            assert!(forward.isEnabled());
            assert!(unsafe {
                NSApplication::sharedApplication(ui.mtm()).sendAction_to_from(
                    forward.action().unwrap(),
                    forward.target().as_deref(),
                    Some(&forward),
                )
            });
            assert_eq!(
                count::<NSSwitch>(settings._host.ns_view()),
                1,
                "Forward restores Traditional"
            );
            assert_eq!(
                find::<NSOutlineView>(&root).unwrap().selectedRow(),
                1,
                "Layouts remains selected"
            );
            assert!(pending.try_recv().is_err(), "navigation must not change config");
        });
        // A normal collection row's first click opens its editor without changing configuration.
        autoreleasepool(|_| {
            let page = editors::workspaces(ui, &settings.model);
            page.synchronize(&settings.model);
            let table = find::<NSTableView>(page.view.ns_view()).unwrap();
            table.selectRowIndexes_byExtendingSelection(&objc2_foundation::NSIndexSet::indexSetWithIndex(0), false);
            assert!(unsafe { table.sendAction_to(table.action(), table.target().as_deref()) });
            assert!(settings.model.sheet.borrow().is_some(), "first-click action opens the workspace editor");
            settings.model.close_sheet();
            assert!(pending.try_recv().is_err(), "opening an editor must not change config");
        });
        // A spacing slider must edit the visible display override, not an ineffective global value.
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
            let mut source = settings.model.source.borrow().clone();
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
            settings.model.replace_source(source);
            let page = pages::layout_scope(ui, &settings.model, 7);
            page.synchronize(&settings.model);
            let slider = find::<NSSlider>(page.view.ns_view()).unwrap();
            assert_eq!(
                slider.doubleValue(),
                12.0,
                "show the display's effective spacing"
            );
            slider.setDoubleValue(24.0);
            assert!(unsafe { slider.sendAction_to(slider.action(), slider.target().as_deref()) });
            let request = pending.try_recv().unwrap();
            let Action::Edit(edit) = request.action else {
                panic!("expected spacing edit")
            };
            let mut source = settings.model.source.borrow().clone();
            edit(&mut source).unwrap();
            assert_eq!(
                source.settings.layout.gaps.outer, global,
                "preserve global spacing"
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
                [24.0; 4]
            );
        });
        let model = Rc::downgrade(&settings.model);
        let host = objc2::rc::Weak::new(settings._host.ns_view());
        let mut form = FormBuilder::new(ui, &settings.model);
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
        page.synchronize(&settings.model);
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
            let mut source = settings.model.source.borrow().clone();
            edit(&mut source).unwrap();
            assert_eq!(source.settings.layout.base.window_insertion_point, expected);
            settings.model.replace_source(source);
            page.synchronize(&settings.model);
        }
        let mut source = settings.model.source.borrow().clone();
        source.settings.layout.stack.base.window_insertion_point =
            Some(crate::common::config::WindowInsertionPoint::EndOfTree);
        settings.model.replace_source(source);
        page.synchronize(&settings.model);
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
