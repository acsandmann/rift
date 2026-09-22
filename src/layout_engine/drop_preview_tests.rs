use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use super::*;
use crate::actor::app::WindowId;
use crate::actor::drag::{DropIntent, DropTarget, DropZone, preview_frame};
use crate::common::collections::HashMap;
use crate::common::config::GapSettings;
use crate::layout_engine::systems::WindowLayoutConstraints;
use crate::sys::screen::SpaceId;

const ACTIONS: [(WindowDropAction, DropZone); 6] = [
    (WindowDropAction::Swap, DropZone::Center),
    (WindowDropAction::Stack, DropZone::Center),
    (WindowDropAction::Insert(Direction::Left), DropZone::West),
    (WindowDropAction::Insert(Direction::Right), DropZone::East),
    (WindowDropAction::Insert(Direction::Up), DropZone::North),
    (WindowDropAction::Insert(Direction::Down), DropZone::South),
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

fn inset(frame: CGRect) -> CGRect {
    CGRect::new(
        CGPoint::new(frame.origin.x + 4.0, frame.origin.y + 4.0),
        CGSize::new(
            (frame.size.width - 8.0).max(0.0),
            (frame.size.height - 8.0).max(0.0),
        ),
    )
}

fn verify_actions(
    baseline: &LayoutSystemKind,
    layout: LayoutId,
    source: WindowId,
    target: WindowId,
) {
    for (action, zone) in ACTIONS {
        let baseline_before = format!("{baseline:?}");
        let mut preview_copy = baseline.preview_clone().unwrap();
        let mut committed = baseline.preview_clone().unwrap();
        let preview_available = preview_copy.apply_window_drop(layout, source, target, action);
        let committed_available = committed.apply_window_drop(layout, source, target, action);
        assert_eq!(
            preview_available, committed_available,
            "availability differs for {action:?}"
        );
        assert_eq!(
            format!("{baseline:?}"),
            baseline_before,
            "preview mutated live layout"
        );
        if !preview_available {
            continue;
        }
        let advertised = calculated_source(&preview_copy, layout, source);
        assert_eq!(
            format!("{baseline:?}"),
            baseline_before,
            "preview mutated live layout"
        );
        let actual = calculated_source(&committed, layout, source);
        assert_eq!(advertised, actual, "preview differs for {action:?}");

        let visual = preview_frame(DropTarget {
            intent: DropIntent {
                window: target,
                space: SpaceId::new(1),
                frame: actual,
                zone,
                action,
            },
            preview_area: advertised,
        });
        assert_eq!(visual, inset(actual), "overlay differs for {action:?}");
    }
}

fn populate(system: &mut LayoutSystemKind) -> LayoutId {
    let layout = system.create_layout();
    for window in [w(1), w(2), w(3), w(4)] {
        system.add_window_after_selection(layout, window);
    }
    layout
}

#[test]
fn traditional_previews_match_committed_frames_for_every_action() {
    let mut system = LayoutSystemKind::Traditional(TraditionalLayoutSystem::default());
    let layout = populate(&mut system);
    verify_actions(&system, layout, w(4), w(2));

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
fn bsp_previews_match_committed_frames_for_every_action() {
    let mut system = LayoutSystemKind::Bsp(BspLayoutSystem::default());
    let layout = populate(&mut system);
    verify_actions(&system, layout, w(4), w(2));

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
fn master_stack_previews_match_committed_frames_for_every_action() {
    let mut system = LayoutSystemKind::MasterStack(MasterStackLayoutSystem::default());
    let layout = populate(&mut system);
    verify_actions(&system, layout, w(4), w(2));
}

#[test]
fn scrolling_previews_match_committed_frames_for_every_action() {
    let mut system = LayoutSystemKind::Scrolling(ScrollingLayoutSystem::default());
    let layout = populate(&mut system);
    verify_actions(&system, layout, w(4), w(2));
}

#[test]
fn stack_previews_match_committed_frames_for_every_action() {
    let mut system = LayoutSystemKind::Stack(StackLayoutSystem::default());
    let layout = populate(&mut system);
    verify_actions(&system, layout, w(4), w(2));

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
