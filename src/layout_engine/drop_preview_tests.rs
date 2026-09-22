use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use super::*;
use crate::actor::app::WindowId;
use crate::common::collections::HashMap;
use crate::common::config::GapSettings;
use crate::layout_engine::systems::WindowLayoutConstraints;

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
            0.0,
            Default::default(),
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
        let preview_available = preview_copy.apply_window_drop(layout, source, target, action);
        assert_eq!(
            preview_available,
            committed.apply_window_drop(layout, source, target, action),
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
        system.add_window_after_selection(layout, window);
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
    assert!(system.apply_window_drop(layout, w(3), w(2), WindowDropAction::Stack));
    verify_actions(&system, layout, w(3), w(1));

    let mut nested = LayoutSystemKind::Traditional(TraditionalLayoutSystem::default());
    let nested_layout = populate(&mut nested);
    assert!(nested.apply_window_drop(
        nested_layout,
        w(4),
        w(3),
        WindowDropAction::Insert(Direction::Down),
    ));
    verify_actions(&nested, nested_layout, w(4), w(3));
}

#[test]
fn traditional_target_relative_down_stays_in_the_target_container() {
    let mut system = LayoutSystemKind::Traditional(TraditionalLayoutSystem::default());
    let layout = system.create_layout();
    for window in [w(1), w(2), w(3)] {
        system.add_window_after_selection(layout, window);
    }
    assert!(system.apply_window_drop(
        layout,
        w(3),
        w(2),
        WindowDropAction::Insert(Direction::Down),
    ));
    system.toggle_tile_orientation(layout);
    assert!(system.apply_window_drop(
        layout,
        w(3),
        w(2),
        WindowDropAction::Insert(Direction::Down),
    ));
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
        let moved =
            preview.apply_window_drop(layout, w(2), w(2), WindowDropAction::Move(Direction::Right));
        assert!(keyboard.select_window(layout, w(2)));
        assert_eq!(moved, keyboard.move_selection(layout, Direction::Right));
        assert_eq!(format!("{preview:?}"), format!("{keyboard:?}"));
        assert_eq!(format!("{system:?}"), before);
    }
}

#[test]
fn bsp_previews_preserve_stacked_and_nested_layouts() {
    let mut system = LayoutSystemKind::Bsp(BspLayoutSystem::default());
    let layout = populate(&mut system);
    assert!(system.apply_window_drop(layout, w(3), w(2), WindowDropAction::Stack));
    verify_actions(&system, layout, w(3), w(1));
    assert!(system.apply_window_drop(layout, w(3), w(2), WindowDropAction::Swap));
    assert_eq!(system.stack_members(layout, w(3)), vec![w(2), w(3)]);
    let _ = calculated_source(&system, layout, w(3));

    let mut nested = LayoutSystemKind::Bsp(BspLayoutSystem::default());
    let nested_layout = populate(&mut nested);
    assert!(nested.apply_window_drop(
        nested_layout,
        w(4),
        w(3),
        WindowDropAction::Insert(Direction::Down),
    ));
    verify_actions(&nested, nested_layout, w(4), w(3));
}

#[test]
fn stack_drop_actions_are_arbitrary_swaps() {
    let mut system = LayoutSystemKind::Stack(StackLayoutSystem::default());
    let layout = populate(&mut system);
    let before = system.all_windows_in_layout(layout);
    let source = before[0];
    let target = *before.last().unwrap();
    assert!(system.select_window(layout, source));
    assert!(system.apply_window_drop(layout, source, target, WindowDropAction::Stack));
    let mut expected = before;
    let last = expected.len() - 1;
    expected.swap(0, last);
    assert_eq!(system.all_windows_in_layout(layout), expected);
    assert_eq!(system.selected_window(layout), Some(source));

    let source = expected[1];
    let target = expected[2];
    assert!(system.apply_window_drop(
        layout,
        source,
        target,
        WindowDropAction::Insert(Direction::Left),
    ));
    expected.swap(1, 2);
    assert_eq!(system.all_windows_in_layout(layout), expected);
    assert_eq!(system.selected_window(layout), Some(source));
    assert!(!system.apply_window_drop(layout, source, source, WindowDropAction::Swap));
}
