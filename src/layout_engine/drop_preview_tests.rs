use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use super::*;
use crate::actor::app::WindowId;
use crate::common::collections::HashMap;
use crate::common::config::GapSettings;
use crate::layout_engine::systems::{
    Focus, LayoutEdit, WindowChange, WindowLayoutConstraints, WindowQuery,
};

const ACTIONS: [WindowDropAction; 6] = [
    WindowDropAction::Swap,
    WindowDropAction::Stack,
    WindowDropAction::Insert(Direction::Left),
    WindowDropAction::Insert(Direction::Right),
    WindowDropAction::Insert(Direction::Up),
    WindowDropAction::Insert(Direction::Down),
];

fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

fn screen() -> CGRect { CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0)) }

fn calculated_source<S: LayoutSystem>(system: &S, layout: LayoutId, source: WindowId) -> CGRect {
    system
        .calculate_layout(
            layout,
            screen(),
            0.0,
            &HashMap::<WindowId, WindowLayoutConstraints>::default(),
            &GapSettings::default(),
            Default::default(),
        )
        .into_iter()
        .find_map(|(window, frame)| (window == source).then_some(frame))
        .expect("source frame after successful drop")
}

fn verify_actions(
    baseline: &LayoutSystemKind,
    layout: LayoutId,
    source: WindowId,
    target: WindowId,
) {
    let before = format!("{baseline:?}");
    for action in ACTIONS {
        let mut preview_copy = baseline.preview_clone().unwrap();
        let mut committed = baseline.preview_clone().unwrap();
        let preview_available =
            preview_copy.edit(layout, LayoutEdit::Drop { source, target, action }).changed();
        assert_eq!(
            preview_available,
            committed
                .edit(layout, LayoutEdit::Drop {
                    source: source,
                    target: target,
                    action: action
                })
                .changed(),
            "availability differs for {action:?}"
        );
        if !preview_available {
            continue;
        }
        assert_eq!(
            calculated_source(&preview_copy, layout, source),
            calculated_source(&committed, layout, source),
            "preview differs for {action:?}"
        );
    }
    assert_eq!(format!("{baseline:?}"), before, "preview mutated live layout");
}

fn populate(system: &mut LayoutSystemKind) -> LayoutId {
    let layout = system.create_layout();
    for window in [w(1), w(2), w(3), w(4)] {
        system.update(WindowChange::Add(layout, window, None));
    }
    layout
}

#[test]
fn previews_match_committed_frames_for_every_layout_and_action() {
    for mut system in [
        LayoutSystemKind::Traditional(TraditionalLayoutSystem::default()),
        LayoutSystemKind::Bsp(BspLayoutSystem::default()),
        LayoutSystemKind::MasterStack(MasterStackLayoutSystem::default()),
        LayoutSystemKind::Scrolling(ScrollingLayoutSystem::default()),
        LayoutSystemKind::Stack(StackLayoutSystem::default()),
    ] {
        let layout = populate(&mut system);
        verify_actions(&system, layout, w(4), w(2));
    }
}

#[test]
fn traditional_previews_preserve_group_and_nested_layouts() {
    let mut system = LayoutSystemKind::Traditional(TraditionalLayoutSystem::default());
    let layout = populate(&mut system);
    assert!(
        system
            .edit(layout, LayoutEdit::Drop {
                source: w(3),
                target: w(2),
                action: WindowDropAction::Stack
            })
            .changed()
    );
    verify_actions(&system, layout, w(3), w(1));

    let mut nested = LayoutSystemKind::Traditional(TraditionalLayoutSystem::default());
    let nested_layout = populate(&mut nested);
    assert!(
        nested
            .edit(nested_layout, LayoutEdit::Drop {
                source: w(4),
                target: w(3),
                action: WindowDropAction::Insert(Direction::Down)
            })
            .changed()
    );
    verify_actions(&nested, nested_layout, w(4), w(3));
}

#[test]
fn traditional_target_relative_down_stays_in_the_target_container() {
    let mut system = LayoutSystemKind::Traditional(TraditionalLayoutSystem::default());
    let layout = system.create_layout();
    for window in [w(1), w(2), w(3)] {
        system.update(WindowChange::Add(layout, window, None));
    }
    assert!(
        system
            .edit(layout, LayoutEdit::Drop {
                source: w(3),
                target: w(2),
                action: WindowDropAction::Insert(Direction::Down)
            })
            .changed()
    );
    system.edit(layout, LayoutEdit::ToggleOrientation).discard();
    assert!(
        system
            .edit(layout, LayoutEdit::Drop {
                source: w(3),
                target: w(2),
                action: WindowDropAction::Insert(Direction::Down)
            })
            .changed()
    );
    assert_eq!(
        calculated_source(&system, layout, w(1)),
        CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(600.0, 800.0)),
    );
    assert_eq!(
        calculated_source(&system, layout, w(2)),
        CGRect::new(CGPoint::new(600.0, 0.0), CGSize::new(600.0, 400.0)),
    );
    assert_eq!(
        calculated_source(&system, layout, w(3)),
        CGRect::new(CGPoint::new(600.0, 400.0), CGSize::new(600.0, 400.0)),
    );
}

#[test]
fn move_drop_matches_keyboard_move_without_mutating_preview_source() {
    for mut system in [
        LayoutSystemKind::Traditional(TraditionalLayoutSystem::default()),
        LayoutSystemKind::Bsp(BspLayoutSystem::default()),
        LayoutSystemKind::MasterStack(MasterStackLayoutSystem::default()),
        LayoutSystemKind::Scrolling(ScrollingLayoutSystem::default()),
        LayoutSystemKind::Stack(StackLayoutSystem::default()),
    ] {
        let layout = populate(&mut system);
        let before = format!("{system:?}");
        let mut preview = system.preview_clone().unwrap();
        let mut keyboard = system.preview_clone().unwrap();
        let moved = preview
            .edit(layout, LayoutEdit::Drop {
                source: w(2),
                target: w(2),
                action: WindowDropAction::Move(Direction::Right),
            })
            .changed();
        assert!(keyboard.focus(layout, Focus::Select(w(2))).handled);
        assert_eq!(
            moved,
            keyboard
                .edit(layout, LayoutEdit::Move {
                    direction: Direction::Right,
                    has_display_neighbor: false
                })
                .changed()
        );
        assert_eq!(format!("{preview:?}"), format!("{keyboard:?}"));
        assert_eq!(format!("{system:?}"), before);
    }
}

#[test]
fn bsp_previews_preserve_stacked_and_nested_layouts() {
    let mut system = LayoutSystemKind::Bsp(BspLayoutSystem::default());
    let layout = populate(&mut system);
    assert!(
        system
            .edit(layout, LayoutEdit::Drop {
                source: w(3),
                target: w(2),
                action: WindowDropAction::Stack
            })
            .changed()
    );
    verify_actions(&system, layout, w(3), w(1));
    assert!(
        system
            .edit(layout, LayoutEdit::Drop {
                source: w(3),
                target: w(2),
                action: WindowDropAction::Swap
            })
            .changed()
    );
    assert_eq!(system.windows(layout, WindowQuery::Stack(w(3))), vec![
        w(2),
        w(3)
    ]);
    let _ = calculated_source(&system, layout, w(3));

    let mut nested = LayoutSystemKind::Bsp(BspLayoutSystem::default());
    let nested_layout = populate(&mut nested);
    assert!(
        nested
            .edit(nested_layout, LayoutEdit::Drop {
                source: w(4),
                target: w(3),
                action: WindowDropAction::Insert(Direction::Down)
            })
            .changed()
    );
    verify_actions(&nested, nested_layout, w(4), w(3));
}

#[test]
fn stack_drop_actions_are_arbitrary_swaps() {
    let mut system = LayoutSystemKind::Stack(StackLayoutSystem::default());
    let layout = populate(&mut system);
    let before = system.windows(layout, WindowQuery::All);
    let source = before[0];
    let target = *before.last().unwrap();
    assert!(system.focus(layout, Focus::Select(source)).handled);
    assert!(
        system
            .edit(layout, LayoutEdit::Drop {
                source: source,
                target: target,
                action: WindowDropAction::Stack
            })
            .changed()
    );
    let mut expected = before;
    let last = expected.len() - 1;
    expected.swap(0, last);
    assert_eq!(system.windows(layout, WindowQuery::All), expected);
    assert_eq!(system.selected_window(layout), Some(source));

    let source = expected[1];
    let target = expected[2];
    assert!(
        system
            .edit(layout, LayoutEdit::Drop {
                source: source,
                target: target,
                action: WindowDropAction::Insert(Direction::Left)
            })
            .changed()
    );
    expected.swap(1, 2);
    assert_eq!(system.windows(layout, WindowQuery::All), expected);
    assert_eq!(system.selected_window(layout), Some(source));
    assert!(
        !system
            .edit(layout, LayoutEdit::Drop {
                source: source,
                target: source,
                action: WindowDropAction::Swap
            })
            .changed()
    );
}

#[test]
fn drop_slot_identity_uses_topology_in_traditional_and_bsp() {
    let mut traditional = LayoutSystemKind::Traditional(TraditionalLayoutSystem::default());
    let layout = populate(&mut traditional);
    let source_slot = traditional.window_slot(layout, w(2)).unwrap();

    let mut same = traditional.preview_clone().unwrap();
    assert!(
        same.edit(layout, LayoutEdit::Drop {
            source: w(2),
            target: w(1),
            action: WindowDropAction::Insert(Direction::Right)
        })
        .changed()
    );
    assert_eq!(same.window_slot(layout, w(2)), Some(source_slot.clone()));

    let mut reordered = traditional.preview_clone().unwrap();
    assert!(
        reordered
            .edit(layout, LayoutEdit::Drop {
                source: w(2),
                target: w(1),
                action: WindowDropAction::Insert(Direction::Left)
            })
            .changed()
    );
    assert_ne!(reordered.window_slot(layout, w(2)), Some(source_slot));

    let mut bsp = LayoutSystemKind::Bsp(BspLayoutSystem::default());
    let layout = bsp.create_layout();
    bsp.update(WindowChange::Add(layout, w(1), None));
    bsp.update(WindowChange::Add(layout, w(2), None));
    let old_frame = calculated_source(&bsp, layout, w(2));
    bsp.update(WindowChange::Resize {
        layout,
        window: w(2),
        old: old_frame,
        new: CGRect::new(CGPoint::new(960.0, 0.0), CGSize::new(240.0, 800.0)),
        screen: screen(),
        gaps: &GapSettings::default(),
    });
    let resized_frame = calculated_source(&bsp, layout, w(2));
    assert!(resized_frame.size.width < old_frame.size.width / 2.0);
    let source_slot = bsp.window_slot(layout, w(2)).unwrap();
    let mut same = bsp.preview_clone().unwrap();
    assert!(
        same.edit(layout, LayoutEdit::Drop {
            source: w(2),
            target: w(1),
            action: WindowDropAction::Insert(Direction::Right)
        })
        .changed()
    );
    assert_eq!(same.window_slot(layout, w(2)), Some(source_slot));
    let new_frame = calculated_source(&same, layout, w(2));
    assert!(new_frame.size.width > resized_frame.size.width * 2.0);
}

#[test]
fn same_slot_edges_use_the_configured_center_action() {
    use crate::common::config::MouseDropAction;
    use crate::layout_engine::engine::DropPreview;

    let same_slot = DropPreview {
        frame: screen(),
        same_slot: true,
    };
    let insert = WindowDropAction::Insert(Direction::Right);
    assert_eq!(
        same_slot.action(insert, MouseDropAction::Swap),
        WindowDropAction::Swap
    );
    assert_eq!(
        same_slot.action(insert, MouseDropAction::Stack),
        WindowDropAction::Stack
    );
    assert_eq!(
        DropPreview {
            frame: screen(),
            same_slot: false
        }
        .action(insert, MouseDropAction::Swap),
        insert
    );
}
