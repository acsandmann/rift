use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use super::*;
use crate::actor::app::WindowId;
use crate::common::collections::HashMap;
use crate::common::config::GapSettings;
use crate::layout_engine::systems::{LayoutEdit, WindowChange, WindowLayoutConstraints};

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
