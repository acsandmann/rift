use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use super::*;
use crate::actor::app::WindowId;
use crate::actor::drag::{DropTarget, DropZone, preview_frame};
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

fn verify_actions<S: LayoutSystem>(
    baseline: &S,
    layout: LayoutId,
    source: WindowId,
    target: WindowId,
    clone_system: impl Fn(&S) -> S,
) {
    for (action, zone) in ACTIONS {
        let mut preview_copy = clone_system(baseline);
        let mut committed = clone_system(baseline);
        let preview_available = preview_copy.apply_window_drop(layout, source, target, action);
        let committed_available = committed.apply_window_drop(layout, source, target, action);
        assert_eq!(
            preview_available, committed_available,
            "availability differs for {action:?}"
        );
        if !preview_available {
            continue;
        }
        let advertised = calculated_source(&preview_copy, layout, source);
        let actual = calculated_source(&committed, layout, source);
        assert_eq!(advertised, actual, "preview differs for {action:?}");

        let visual = preview_frame(DropTarget {
            window: target,
            space: SpaceId::new(1),
            frame: actual,
            preview_area: advertised,
            zone,
            action,
        });
        assert_eq!(visual, inset(actual), "overlay differs for {action:?}");
    }
}

fn populate<S: LayoutSystem>(system: &mut S) -> LayoutId {
    let layout = system.create_layout();
    for window in [w(1), w(2), w(3), w(4)] {
        system.add_window_after_selection(layout, window);
    }
    layout
}

#[test]
fn traditional_previews_match_committed_frames_for_every_action() {
    let mut system = TraditionalLayoutSystem::default();
    let layout = populate(&mut system);
    verify_actions(
        &system,
        layout,
        w(4),
        w(2),
        TraditionalLayoutSystem::preview_clone,
    );

    assert!(system.apply_window_drop(layout, w(3), w(2), WindowDropAction::Stack));
    verify_actions(
        &system,
        layout,
        w(3),
        w(1),
        TraditionalLayoutSystem::preview_clone,
    );

    let mut nested = TraditionalLayoutSystem::default();
    let nested_layout = populate(&mut nested);
    assert!(nested.apply_window_drop(
        nested_layout,
        w(4),
        w(3),
        WindowDropAction::Insert(Direction::Down),
    ));
    verify_actions(
        &nested,
        nested_layout,
        w(4),
        w(3),
        TraditionalLayoutSystem::preview_clone,
    );
}

#[test]
fn traditional_target_relative_down_stays_in_the_target_container() {
    let mut system = TraditionalLayoutSystem::default();
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
    let mut system = BspLayoutSystem::default();
    let layout = populate(&mut system);
    verify_actions(&system, layout, w(4), w(2), BspLayoutSystem::preview_clone);

    assert!(system.apply_window_drop(layout, w(3), w(2), WindowDropAction::Stack));
    verify_actions(&system, layout, w(3), w(1), BspLayoutSystem::preview_clone);
    assert!(system.apply_window_drop(layout, w(3), w(2), WindowDropAction::Swap));
    assert_eq!(system.stack_members(layout, w(3)), vec![w(2), w(3)]);
    let _ = calculated_source(&system, layout, w(3));

    let mut nested = BspLayoutSystem::default();
    let nested_layout = populate(&mut nested);
    assert!(nested.apply_window_drop(
        nested_layout,
        w(4),
        w(3),
        WindowDropAction::Insert(Direction::Down),
    ));
    verify_actions(
        &nested,
        nested_layout,
        w(4),
        w(3),
        BspLayoutSystem::preview_clone,
    );
}

#[test]
fn master_stack_previews_match_committed_frames_for_every_action() {
    let mut system = MasterStackLayoutSystem::default();
    let layout = populate(&mut system);
    verify_actions(
        &system,
        layout,
        w(4),
        w(2),
        MasterStackLayoutSystem::preview_clone,
    );
}

#[test]
fn scrolling_previews_match_committed_frames_for_every_action() {
    let mut system = ScrollingLayoutSystem::default();
    let layout = populate(&mut system);
    verify_actions(&system, layout, w(4), w(2), ScrollingLayoutSystem::preview_clone);
}

#[test]
fn stack_previews_match_committed_frames_for_every_action() {
    let mut system = StackLayoutSystem::default();
    let layout = populate(&mut system);
    verify_actions(&system, layout, w(4), w(2), StackLayoutSystem::preview_clone);

    assert!(system.apply_window_drop(layout, w(4), w(2), WindowDropAction::Swap));
    assert_eq!(system.selected_window(layout), Some(w(4)));
}
