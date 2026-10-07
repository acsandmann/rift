use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::{NSControl, NSOutlineView, NSPopUpButton, NSSwitch, NSTableView, NSView};

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
            &objc2_foundation::NSIndexSet::indexSetWithIndex(1), false,
        );
        let navigation = settings._navigation.ns_segmented_control();
        assert!(navigation.isEnabledForSegment(0), "enable Back after visiting a section");
        assert!(!navigation.isEnabledForSegment(1), "disable Forward at the latest destination");
        sidebar.selectRowIndexes_byExtendingSelection(
            &objc2_foundation::NSIndexSet::indexSetWithIndex(0), false,
        );
        drop(sidebar);

        Settings::select(ui, &settings.model, &settings._host, &settings.pages, 4);
        settings.selected.set(4);
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
        // Selecting a peer layout replaces its controls without writing configuration.
        autoreleasepool(|_| {
            Settings::select(ui, &settings.model, &settings._host, &settings.pages, 1);
            settings.selected.set(1);
            let pages = settings.pages.borrow();
            let view = pages[1].as_ref().unwrap().view.ns_view();
            let browser = find::<NSOutlineView>(view).unwrap();
            browser.selectRowIndexes_byExtendingSelection(
                &objc2_foundation::NSIndexSet::indexSetWithIndex(3), false,
            );
            assert_eq!(count::<NSSwitch>(view), 1, "mount the selected layout once");
            browser.selectRowIndexes_byExtendingSelection(
                &objc2_foundation::NSIndexSet::indexSetWithIndex(4), false,
            );
            assert_eq!(count::<NSSwitch>(view), 0, "replace the old layout form");
            assert!(
                pending.try_recv().is_err(),
                "layout selection must not change config"
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
