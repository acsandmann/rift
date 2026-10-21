use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use super::*;
use crate::actor::reactor::Reactor;

fn screen(id: u32, display: &str, space: u64) -> ScreenInfo {
    ScreenInfo {
        id: crate::sys::screen::ScreenId::new(id),
        display_uuid: display.into(),
        name: None,
        frame: CGRect::new(
            CGPoint::new((id - 1) as f64 * 1000.0, 0.0),
            CGSize::new(1000.0, 800.0),
        ),
        backing_scale: 1.0,
        space: Some(SpaceId::new(space)),
    }
}

fn sample(screens: Vec<ScreenInfo>) -> NativeSnapshot {
    let inventory = screens
        .iter()
        .map(|s| (s.display_uuid.clone(), s.space.into_iter().collect()))
        .collect();
    NativeSnapshot {
        screens,
        inventory,
        converter: CoordinateConverter::from_height(800.0),
        quiet: true,
    }
}

/// Fake native query data, with automatic cleanup; all acceptance, quarantine,
/// settling, retry and deduplication decisions remain in the production actor.
struct Rig {
    actor: SpacesActor,
    snapshots: actor::Receiver<wm_controller::WmEvent>,
    events: actor::Receiver<reactor::Event>,
    spaces: HashSet<SpaceId>,
    windows: HashSet<WindowServerId>,
}

impl Rig {
    fn new() -> Self {
        let (wm, snapshots) = actor::channel();
        let (reactor, events) = actor::channel();
        let (actor, _) = SpacesActor::new_for_tests(reactor, wm);
        Self {
            actor,
            snapshots,
            events,
            spaces: HashSet::default(),
            windows: HashSet::default(),
        }
    }

    fn membership(&mut self, space: u64, ids: &[u32]) {
        let space = SpaceId::new(space);
        self.spaces.insert(space);
        window_server::set_space_window_list_for_space_override(space.get(), Some(ids.to_vec()));
    }

    fn native_home(&mut self, window: u32, space: Option<u64>) {
        let id = WindowServerId::new(window);
        self.windows.insert(id);
        window_server::set_window_spaces_override(id, space.map(|s| vec![s]));
    }

    fn tick(
        &mut self,
        attempt: u8,
        snapshot: Option<NativeSnapshot>,
    ) -> Option<ForwardedSpaceState> {
        if let Some(snapshot) = &snapshot {
            for space in snapshot.inventory.values().flatten() {
                if self.spaces.insert(*space) {
                    window_server::set_space_window_list_for_space_override(
                        space.get(),
                        Some(vec![]),
                    );
                }
            }
        }
        self.actor.process_screen_refresh(attempt, snapshot);
        let result = self.snapshots.try_recv().ok().map(|(_, event)| match event {
            wm_controller::WmEvent::SpaceStateUpdated(state, converter) => {
                assert_eq!(
                    converter.convert_point(CGPoint::ZERO),
                    Some(CGPoint::new(0.0, 800.0))
                );
                for screen in &state.screens {
                    if let Some(space) = screen.space {
                        assert!(
                            state
                                .display_space_ids
                                .get(&screen.display_uuid)
                                .is_some_and(|spaces| spaces.contains(&space)),
                            "forwarded current Space must belong to its display: {state:?}"
                        );
                    }
                }
                state
            }
            other => panic!("unexpected controller event: {other:?}"),
        });
        assert!(
            self.snapshots.try_recv().is_err(),
            "one native sample must produce at most one commit"
        );
        result
    }

    fn event(&mut self, event: Event) { self.actor.handle_event(event); }

    fn notifications(&mut self) -> Vec<reactor::Event> {
        std::iter::from_fn(|| self.events.try_recv().ok().map(|(_, event)| event)).collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        for space in &self.spaces {
            window_server::set_space_window_list_for_space_override(space.get(), None);
            window_server::set_space_membership_query_failed((*space).into(), false);
        }
        for window in &self.windows {
            window_server::set_window_spaces_override(*window, None);
        }
    }
}

#[test]
fn active_display_resolution_prefers_uuid_then_active_space_then_screen_order() {
    let screens = vec![screen(1, "left", 1), screen(2, "right", 2)];
    for (uuid, active, expected) in [
        (Some("right"), Some(1), 2),
        (Some("missing"), Some(2), 2),
        (None, Some(2), 2),
        (Some("missing"), None, 1),
        (None, Some(99), 1),
    ] {
        assert_eq!(
            SpacesActor::resolve_active_display_space(&screens, uuid, active.map(SpaceId::new)),
            Some(SpaceId::new(expected))
        );
    }
}

#[test]
fn active_display_change_updates_command_context_once_without_a_layout_refresh() {
    let mut rig = Rig::new();
    rig.tick(
        0,
        Some(sample(vec![screen(1, "left", 1), screen(2, "right", 2)])),
    )
    .unwrap();
    rig.actor.handle_active_display_changed_for(Some("right"));
    assert!(
        matches!(rig.notifications().as_slice(), [reactor::Event::ActiveDisplayChanged {
        menu_bar_space: Some(space), command_space: Some(command),
    }] if *space == SpaceId::new(2) && space == command)
    );
    rig.actor.handle_active_display_changed_for(Some("right"));
    assert!(rig.notifications().is_empty());
    assert!(rig.snapshots.try_recv().is_err());
}

#[test]
fn every_physical_reconfiguration_revokes_authority_but_begin_only_does_not() {
    use DisplayReconfigFlags as F;
    for (flags, revokes) in [
        (F::empty(), false),
        (F::BEGIN_CONFIGURATION, false),
        (F::ADD, true),
        (F::REMOVE, true),
        (F::MOVED, true),
        (F::SET_MAIN, true),
        (F::SET_MODE, true),
        (F::ENABLED, true),
        (F::DISABLED, true),
        (F::MIRROR, true),
        (F::UNMIRROR, true),
        (F::DESKTOP_SHAPE_CHANGED, true),
        (
            F::BEGIN_CONFIGURATION | F::SET_MAIN | F::DESKTOP_SHAPE_CHANGED,
            true,
        ),
    ] {
        let mut rig = Rig::new();
        let before = rig.tick(0, Some(sample(vec![screen(1, "laptop", 1)]))).unwrap();
        rig.event(Event::DisplayReconfigured { display_id: 1, flags });
        assert_eq!(
            rig.actor.topology_guard().invalidated_revision() > before.revision,
            revokes,
            "{flags:?}"
        );
        assert_eq!(rig.notifications().len(), usize::from(revokes), "{flags:?}");
        assert!(rig.snapshots.try_recv().is_err());
    }
}

#[test]
fn sleep_lock_and_overlapping_holds_require_a_fresh_settled_sample_after_every_release() {
    for (holds, releases) in [
        (vec![Event::SystemWillSleep], vec![Event::SystemDidWake]),
        (vec![Event::SessionDidResignActive], vec![
            Event::SessionDidBecomeActive,
        ]),
        (
            vec![Event::SystemWillSleep, Event::SessionDidResignActive],
            vec![Event::SystemDidWake, Event::SessionDidBecomeActive],
        ),
        (
            vec![Event::SessionDidResignActive, Event::SystemWillSleep],
            vec![Event::SessionDidBecomeActive, Event::SystemDidWake],
        ),
    ] {
        let mut rig = Rig::new();
        rig.membership(1, &[7]);
        let before = rig.tick(0, Some(sample(vec![screen(1, "laptop", 1)]))).unwrap();
        for hold in holds {
            rig.event(hold);
        }
        rig.notifications();
        rig.event(Event::WindowServerAppeared(
            WindowServerId::new(8),
            SpaceId::new(99),
        ));
        rig.event(Event::WindowServerDestroyed(
            WindowServerId::new(7),
            SpaceId::new(1),
        ));
        rig.event(Event::SpaceCreated(SpaceId::new(99)));
        rig.event(Event::SpaceDestroyed(SpaceId::new(1)));
        // Even an apparently coherent login/display report cannot release a lifecycle hold.
        for _ in 0..2 {
            assert!(
                rig.tick(REFRESH_MAX_RETRIES, Some(sample(vec![screen(1, "login", 99)])))
                    .is_none()
            );
        }
        assert!(rig.notifications().is_empty());
        assert_eq!(rig.actor.state.screens, before.screens);
        assert_eq!(
            rig.actor.state.visible_window_spaces,
            before.active_window_spaces
        );
        for (index, release) in releases.iter().enumerate() {
            rig.event(release.clone());
            if index + 1 < releases.len() {
                assert!(rig.tick(0, Some(sample(before.screens.clone()))).is_none());
            }
        }
        let fresh = sample(before.screens.clone());
        assert!(rig.tick(0, Some(fresh.clone())).is_none());
        let after = rig.tick(1, Some(fresh)).unwrap();
        assert!(after.authoritative && after.membership_complete);
        assert_eq!(after.active_window_spaces, before.active_window_spaces);
        assert_eq!(
            after.last_user_space_by_display,
            before.last_user_space_by_display
        );
    }
}

#[test]
fn duplicate_session_notifications_do_not_repeat_revocation_or_resume() {
    let mut rig = Rig::new();
    rig.event(Event::SessionDidBecomeActive);
    assert!(rig.notifications().is_empty());
    for event in [Event::SessionDidResignActive, Event::SessionDidBecomeActive] {
        rig.event(event.clone());
        assert_eq!(rig.notifications().len(), 1);
        rig.event(event);
        assert!(rig.notifications().is_empty());
    }
}

#[test]
fn inconclusive_samples_never_erase_accepted_topology_or_history_and_can_recover() {
    let good = sample(vec![screen(1, "left", 1), screen(2, "right", 2)]);
    let mut invalid = vec![("unavailable", None), ("no displays", Some(sample(vec![])))];
    for kind in [
        "duplicate Space",
        "duplicate UUID",
        "missing UUID",
        "zero width",
        "negative height",
        "NaN origin",
        "infinite scale",
        "zero scale",
        "unresolved Space",
        "missing inventory",
        "wrong current Space",
        "shared inventory",
    ] {
        let mut bad = good.clone();
        match kind {
            "duplicate Space" => bad.screens[1].space = bad.screens[0].space,
            "duplicate UUID" => bad.screens[1].display_uuid = "left".into(),
            "missing UUID" => bad.screens[0].display_uuid.clear(),
            "zero width" => bad.screens[0].frame.size.width = 0.0,
            "negative height" => bad.screens[0].frame.size.height = -1.0,
            "NaN origin" => bad.screens[0].frame.origin.y = f64::NAN,
            "infinite scale" => bad.screens[0].backing_scale = f64::INFINITY,
            "zero scale" => bad.screens[0].backing_scale = 0.0,
            "unresolved Space" => bad.screens[0].space = None,
            "missing inventory" => {
                bad.inventory.remove("right");
            }
            "wrong current Space" => {
                bad.inventory.insert("left".into(), vec![SpaceId::new(99)]);
            }
            "shared inventory" => {
                bad.inventory.get_mut("right").unwrap().push(SpaceId::new(1));
            }
            _ => unreachable!(),
        }
        invalid.push((kind, Some(bad)));
    }
    for (case, invalid) in invalid {
        let mut rig = Rig::new();
        let before = rig.tick(0, Some(good.clone())).unwrap();
        assert!(rig.tick(0, invalid).is_none(), "{case}");
        assert_eq!(rig.actor.state.screens, before.screens, "{case}");
        assert_eq!(
            rig.actor.state.last_forwarded.as_ref().unwrap().revision,
            before.revision,
            "{case}"
        );
        let revoked = rig.actor.topology_guard().invalidated_revision();
        assert!(
            revoked > before.revision,
            "{case}: rejected native data revokes writes"
        );
        rig.notifications();
        rig.event(Event::WindowServerAppeared(
            WindowServerId::new(99),
            SpaceId::new(2),
        ));
        rig.event(Event::WindowServerDestroyed(
            WindowServerId::new(99),
            SpaceId::new(1),
        ));
        assert!(
            rig.notifications().is_empty(),
            "{case}: stale window events are quarantined"
        );
        assert!(rig.tick(1, Some(good.clone())).is_none(), "{case}");
        let recovered = rig.tick(2, Some(good.clone())).unwrap();
        assert_eq!(recovered.screens, before.screens, "{case}");
        assert_eq!(
            recovered.last_user_space_by_display, before.last_user_space_by_display,
            "{case}"
        );
    }
}

#[test]
fn recovery_requires_consecutive_full_samples_and_a_quiet_window_server() {
    let mut base = sample(vec![screen(1, "left", 1), screen(2, "right", 2)]);
    base.inventory.get_mut("left").unwrap().push(SpaceId::new(4));
    for change in [
        "origin",
        "size",
        "scale",
        "native ID",
        "inactive inventory",
        "enumeration",
        "inventory enumeration",
        "busy",
    ] {
        let mut rig = Rig::new();
        rig.tick(0, Some(base.clone())).unwrap();
        rig.event(Event::DisplayReconfigured {
            display_id: 1,
            flags: DisplayReconfigFlags::MOVED,
        });
        assert!(rig.tick(0, Some(base.clone())).is_none());
        let mut next = base.clone();
        match change {
            "origin" => next.screens[0].frame.origin.y = -800.0,
            "size" => next.screens[0].frame.size.width = 1200.0,
            "scale" => next.screens[0].backing_scale = 2.0,
            "native ID" => next.screens[0].id = crate::sys::screen::ScreenId::new(9),
            "inactive inventory" => next.inventory.get_mut("left").unwrap().push(SpaceId::new(3)),
            "enumeration" => next.screens.reverse(),
            "inventory enumeration" => next.inventory.get_mut("left").unwrap().reverse(),
            "busy" => next.quiet = false,
            _ => unreachable!(),
        }
        let observed = rig.tick(1, Some(next));
        if matches!(change, "enumeration" | "inventory enumeration") {
            assert!(observed.is_some());
            continue;
        }
        assert!(observed.is_none(), "{change}");
        let last = rig.tick(2, Some(base.clone()));
        if change == "busy" {
            assert!(last.is_some());
        } else {
            assert!(
                last.is_none(),
                "{change}: an older matching sample is not consecutive"
            );
            assert!(rig.tick(3, Some(base.clone())).is_some(), "{change}");
        }
    }
}

#[test]
fn retry_exhaustion_keeps_recovery_owed_and_stale_timers_cannot_release_it() {
    let mut rig = Rig::new();
    let good = sample(vec![screen(1, "laptop", 1)]);
    let before = rig.tick(0, Some(good.clone())).unwrap();
    // Membership can discover an unavailable native sample before the physical callback.
    // No callback is needed to retain retry debt or revoke stale write authority.
    for attempt in 0..=REFRESH_MAX_RETRIES + 1 {
        assert!(rig.tick(attempt, None).is_none());
        assert!(rig.actor.topology_guard().invalidated_revision() > before.revision);
        assert!(rig.actor.state.refresh_pending);
        assert_eq!(
            rig.actor.topology_guard().invalidated_revision(),
            before.revision + 1
        );
    }
    assert!(rig.tick(0, Some(good.clone())).is_none());
    rig.event(Event::ProcessScreenRefresh {
        revision: before.revision,
        attempt: 0,
    });
    assert!(rig.actor.state.refresh_pending);
    assert!(rig.snapshots.try_recv().is_err());
    assert!(rig.tick(1, Some(good)).is_some());
}

#[test]
fn topology_changes_report_only_the_spaces_that_need_resizing() {
    let base = sample(vec![screen(1, "left", 1), screen(2, "right", 2)]);
    for change in ["attach", "detach", "resize", "move", "main display"] {
        let mut rig = Rig::new();
        rig.tick(0, Some(base.clone())).unwrap();
        let mut next = base.clone();
        match change {
            "attach" => {
                let s = screen(3, "projector", 3);
                next.inventory.insert("projector".into(), vec![SpaceId::new(3)]);
                next.screens.push(s);
            }
            "detach" => {
                next.screens.pop();
                next.inventory.remove("right");
            }
            "resize" => next.screens[0].frame.size.width = 1200.0,
            "move" => next.screens[0].frame.origin.x = -1000.0,
            "main display" => {
                next.screens.reverse();
                rig.event(Event::DisplayReconfigured {
                    display_id: 2,
                    flags: DisplayReconfigFlags::SET_MAIN,
                });
            }
            _ => unreachable!(),
        }
        assert!(rig.tick(0, Some(next.clone())).is_none(), "{change}");
        let after = rig.tick(1, Some(next)).unwrap();
        assert_eq!(after.display_set_changed, matches!(change, "attach" | "detach"));
        assert!(after.should_force_refresh_layout);
        let expected = match change {
            "attach" => vec![(SpaceId::new(3), CGSize::new(1000.0, 800.0))],
            "resize" => vec![(SpaceId::new(1), CGSize::new(1200.0, 800.0))],
            _ => vec![],
        };
        assert_eq!(after.resized_spaces, expected, "{change}");
    }
}

#[test]
fn desktop_replacement_switch_and_migration_follow_full_inventory_identity() {
    let base = sample(vec![screen(1, "left", 1), screen(2, "right", 2)]);
    for case in [
        "new replacement",
        "ordinary switch",
        "known target",
        "old migrated hidden",
        "old migrated visible",
        "incomplete then replacement",
        "ambiguous replacement",
        "inactive replacement",
        "inactive replacement after disconnect",
    ] {
        let mut rig = Rig::new();
        let mut initial = base.clone();
        initial.inventory.get_mut("left").unwrap().push(SpaceId::new(3));
        let before = rig.tick(0, Some(initial)).unwrap();
        let mut next = base.clone();
        next.screens[0].space = Some(SpaceId::new(4));
        next.inventory.insert("left".into(), vec![SpaceId::new(3), SpaceId::new(4)]);
        match case {
            "ordinary switch" => {
                next.screens[0].space = Some(SpaceId::new(3));
                next.inventory.insert("left".into(), vec![SpaceId::new(1), SpaceId::new(3)]);
            }
            "known target" => {
                next.screens[0].space = Some(SpaceId::new(3));
                next.inventory.insert("left".into(), vec![SpaceId::new(3)]);
            }
            "old migrated hidden" => {
                next.inventory.insert("left".into(), vec![SpaceId::new(4)]);
                next.inventory.get_mut("right").unwrap().push(SpaceId::new(1));
            }
            "old migrated visible" => {
                next.inventory.insert("left".into(), vec![SpaceId::new(4)]);
                next.screens[1].space = Some(SpaceId::new(1));
                next.inventory.insert("right".into(), vec![SpaceId::new(1), SpaceId::new(2)]);
            }
            "ambiguous replacement" => {
                next.inventory.insert("left".into(), vec![SpaceId::new(4)]);
            }
            "inactive replacement" | "inactive replacement after disconnect" => {
                next.screens[0].space = Some(SpaceId::new(1));
                next.inventory.insert("left".into(), vec![SpaceId::new(1), SpaceId::new(4)]);
            }
            "incomplete then replacement" => {
                let mut invalid = next.clone();
                invalid.inventory.remove("left");
                assert!(rig.tick(0, Some(invalid)).is_none());
                assert_eq!(
                    rig.actor.state.last_user_space_by_display,
                    before.last_user_space_by_display
                );
            }
            _ => {}
        }
        if case == "inactive replacement after disconnect" {
            let detached = sample(vec![screen(2, "right", 2)]);
            assert!(rig.tick(0, Some(detached.clone())).is_none());
            rig.tick(1, Some(detached)).unwrap();
        }
        // Coalesce with a queued native query; the notification carries no remap hint.
        rig.event(Event::ScreenRefreshRequested);
        rig.event(Event::SpaceInventoryChanged);
        let first = rig.tick(0, Some(next.clone()));
        if case == "ambiguous replacement" {
            assert!(first.is_none());
            for attempt in 1..=REFRESH_MAX_RETRIES {
                assert!(rig.tick(attempt, Some(next.clone())).is_none());
            }
            assert_eq!(
                rig.actor.state.last_forwarded.as_ref().unwrap().revision,
                before.revision
            );
            assert_eq!(
                rig.actor.state.last_known_display_space_ids,
                before.display_space_ids
            );
            assert!(rig.tick(0, Some(base.clone())).is_none());
            assert!(rig.tick(1, Some(base.clone())).is_some());
            continue;
        }
        let after = if case == "ordinary switch" {
            first.unwrap()
        } else {
            assert!(first.is_none(), "{case}");
            rig.tick(1, Some(next)).unwrap()
        };
        let expected = if matches!(case, "new replacement" | "incomplete then replacement") {
            vec![(SpaceId::new(1), SpaceId::new(4))]
        } else if case.starts_with("inactive replacement") {
            vec![(SpaceId::new(3), SpaceId::new(4))]
        } else {
            vec![]
        };
        assert_eq!(after.space_remaps, expected, "{case}");
    }
}

#[test]
fn partial_return_does_not_steal_an_absent_displays_desktop_history() {
    let mut rig = Rig::new();
    rig.tick(
        0,
        Some(sample(vec![screen(1, "builtin", 1), screen(2, "external", 2)])),
    )
    .unwrap();
    let transient = sample(vec![screen(1, "external", 1)]);
    assert!(rig.tick(0, Some(transient.clone())).is_none());
    let displaced = rig.tick(1, Some(transient)).unwrap();
    assert!(displaced.space_remaps.is_empty());
    assert_eq!(
        displaced.last_user_space_by_display.get("external"),
        Some(&SpaceId::new(2))
    );
    let returned = sample(vec![screen(1, "builtin", 1), screen(2, "external", 4)]);
    assert!(rig.tick(0, Some(returned.clone())).is_none());
    assert_eq!(rig.tick(1, Some(returned)).unwrap().space_remaps, vec![(
        SpaceId::new(2),
        SpaceId::new(4)
    )]);
}

#[test]
fn fullscreen_changes_preserve_other_displays_user_context_in_any_enumeration_order() {
    let fullscreen = SpaceId::new(0x400000002);
    for (current_left, reversed, expected_left) in [(3, false, 3), (3, true, 3), (2, false, 1)] {
        let mut rig = Rig::new();
        let mut base = sample(vec![screen(1, "left", 1), screen(2, "right", 2)]);
        base.inventory.get_mut("left").unwrap().push(SpaceId::new(3));
        rig.tick(0, Some(base.clone())).unwrap();
        let mut next = sample(vec![
            screen(1, "left", current_left),
            screen(2, "right", fullscreen.get()),
        ]);
        // Model a coherent native inventory, independent from the temporary current-space callback.
        next.inventory = base.inventory.clone();
        next.inventory.get_mut("right").unwrap().push(fullscreen);
        if current_left == 2 {
            next.inventory.insert("left".into(), vec![SpaceId::new(2)]);
            next.inventory.insert("right".into(), vec![fullscreen]);
        }
        if reversed {
            next.screens.reverse();
        }
        if current_left == 2 {
            assert!(rig.tick(0, Some(next.clone())).is_none());
            assert!(rig.tick(1, Some(next)).is_none());
            assert_eq!(rig.actor.state.screens, base.screens);
            assert_eq!(rig.actor.state.last_known_display_space_ids, base.inventory);
            // A fresh inventory/current-space pair resolves the contamination.
            next = base.clone();
            next.screens[1].space = Some(fullscreen);
            next.inventory.get_mut("right").unwrap().push(fullscreen);
            if reversed {
                next.screens.reverse();
            }
        }
        let after = match rig.tick(0, Some(next.clone())) {
            Some(s) => s,
            None => rig.tick(1, Some(next)).unwrap(),
        };
        assert_eq!(
            after.screens.iter().find(|s| s.display_uuid == "left").unwrap().space,
            Some(SpaceId::new(expected_left))
        );
        assert_eq!(
            after.screens.iter().find(|s| s.display_uuid == "right").unwrap().space,
            None
        );
        assert!(after.fullscreen_spaces.contains(&fullscreen));
        assert!(!after.active_window_spaces.values().any(|s| *s == fullscreen));
    }
}

#[test]
fn duplicate_window_listings_use_previous_active_home_then_live_native_home() {
    for (previous, native, expected) in [
        (Some(1), None, 1),
        (Some(1), Some(2), 1),
        (Some(3), Some(2), 2),
        (None, Some(2), 2),
    ] {
        for reversed in [false, true] {
            let mut rig = Rig::new();
            let mut initial = sample(vec![
                screen(1, "left", previous.unwrap_or(1)),
                screen(2, "right", 2),
            ]);
            initial.inventory.insert("left".into(), vec![SpaceId::new(1), SpaceId::new(3)]);
            if let Some(previous) = previous {
                rig.membership(previous, &[7]);
            }
            rig.tick(0, Some(initial.clone())).unwrap();
            rig.membership(1, &[7]);
            rig.membership(2, &[7]);
            rig.native_home(7, native);
            let mut next = sample(vec![screen(1, "left", 1), screen(2, "right", 2)]);
            next.inventory = initial.inventory;
            if reversed {
                next.screens.reverse();
            }
            let after = rig.tick(0, Some(next)).unwrap();
            assert_eq!(
                after.active_window_spaces.get(&WindowServerId::new(7)),
                Some(&SpaceId::new(expected))
            );
        }
    }
}

#[test]
fn failed_membership_and_first_empty_wake_sample_preserve_windows_until_complete() {
    let mut rig = Rig::new();
    let base = sample(vec![screen(1, "left", 1), screen(2, "right", 2)]);
    rig.membership(1, &[7]);
    rig.membership(2, &[8]);
    rig.tick(0, Some(base.clone())).unwrap();
    window_server::set_space_membership_query_failed(1, true);
    rig.membership(2, &[]);
    let failed = rig.tick(0, Some(base.clone())).unwrap();
    assert!(!failed.membership_complete);
    assert_eq!(
        failed.active_window_spaces,
        HashMap::from_iter([(WindowServerId::new(7), SpaceId::new(1))])
    );
    window_server::set_space_membership_query_failed(1, false);
    rig.event(Event::SystemWillSleep);
    rig.event(Event::SystemDidWake);
    rig.membership(1, &[]);
    assert!(rig.tick(0, Some(base.clone())).is_none());
    let wake = rig.tick(1, Some(base.clone())).unwrap();
    assert!(!wake.membership_complete);
    assert_eq!(wake.active_window_spaces, failed.active_window_spaces);
    let complete = rig.tick(2, Some(base)).unwrap();
    assert!(complete.membership_complete);
    assert!(
        complete.active_window_spaces.is_empty(),
        "a real close, including the last window, must eventually be observed"
    );
}

#[test]
fn native_membership_notifications_coalesce_with_a_queued_unchanged_topology_sample() {
    for notification in [
        Event::SpaceInventoryChanged,
        Event::ReconcileWindowSpaces,
        Event::WindowServerDestroyed(WindowServerId::new(7), SpaceId::new(1)),
    ] {
        let mut rig = Rig::new();
        let base = sample(vec![screen(1, "left", 1), screen(2, "right", 2)]);
        rig.membership(1, &[7]);
        rig.membership(2, &[]);
        rig.tick(0, Some(base.clone())).unwrap();
        rig.native_home(7, Some(2));
        rig.membership(1, &[]);
        rig.membership(2, &[7]);
        rig.event(Event::ScreenRefreshRequested);
        rig.event(notification);
        let after = rig.tick(0, Some(base)).unwrap();
        assert_eq!(
            after.active_window_spaces.get(&WindowServerId::new(7)),
            Some(&SpaceId::new(2))
        );
        assert!(!after.display_set_changed && !after.should_force_refresh_layout);
        assert!(
            rig.notifications().is_empty(),
            "a native move requests a full snapshot rather than forwarding a stale destroy"
        );
    }
}

#[test]
fn resizing_during_a_native_space_switch_preserves_both_workspace_catalogs() {
    let mut rig = Rig::new();
    let mut base = sample(vec![screen(1, "left", 1)]);
    base.inventory.get_mut("left").unwrap().push(SpaceId::new(2));
    let initial = rig.tick(0, Some(base.clone())).unwrap();
    let mut reactor = Reactor::new_for_test(crate::layout_engine::LayoutEngine::new(
        &Default::default(),
        &Default::default(),
        None,
    ));
    reactor.handle_events(vec![reactor::Event::SpaceStateChanged(initial)]);
    let a = reactor.test_workspace_ids(SpaceId::new(1));
    let b = reactor.test_workspace_ids(SpaceId::new(2));
    base.screens[0].space = Some(SpaceId::new(2));
    base.screens[0].frame.size.width = 1200.0;
    assert!(rig.tick(0, Some(base.clone())).is_none());
    let after = rig.tick(1, Some(base)).unwrap();
    assert!(after.space_remaps.is_empty());
    reactor.handle_events(vec![reactor::Event::SpaceStateChanged(after)]);
    assert_eq!(reactor.test_workspace_ids(SpaceId::new(1)), a);
    assert_eq!(reactor.test_workspace_ids(SpaceId::new(2)), b);
}

#[test]
fn queued_active_space_notification_waits_for_a_real_switch_or_bounded_membership_refresh() {
    for changed_space in [true, false] {
        let mut rig = Rig::new();
        let mut base = sample(vec![screen(1, "laptop", 1)]);
        base.inventory.get_mut("laptop").unwrap().push(SpaceId::new(2));
        rig.membership(1, &[7]);
        let before = rig.tick(0, Some(base.clone())).unwrap();
        rig.event(Event::ScreenRefreshRequested);
        rig.event(Event::ActiveSpaceChanged);
        assert!(matches!(rig.notifications().as_slice(), [
            reactor::Event::TopologyInvalidated(_)
        ]));
        rig.membership(1, &[]);
        rig.membership(2, &[7]);
        for attempt in 0..REFRESH_MAX_RETRIES {
            assert!(rig.tick(attempt, Some(base.clone())).is_none());
        }
        let mut next = base;
        if changed_space {
            next.screens[0].space = Some(SpaceId::new(2));
        }
        let after = rig
            .tick(
                if changed_space {
                    1
                } else {
                    REFRESH_MAX_RETRIES
                },
                Some(next),
            )
            .unwrap();
        assert!(after.space_remaps.is_empty());
        assert!(!after.display_set_changed && !after.should_force_refresh_layout);
        assert_eq!(
            after.last_user_space_by_display.get("laptop"),
            Some(&SpaceId::new(if changed_space { 2 } else { 1 }))
        );
        assert_eq!(
            after.active_window_spaces.get(&WindowServerId::new(7)),
            changed_space.then_some(&SpaceId::new(2))
        );
        assert_eq!(after.screens[0].display_uuid, before.screens[0].display_uuid);
        assert!(rig.notifications().is_empty());
    }
}

#[test]
fn a_space_switch_revokes_queued_geometry_before_the_reactor_receives_it() {
    use crate::actor::app::{AppThreadHandle, Request, WindowId};
    for notify in [false, true] {
        let mut rig = Rig::new();
        let mut native = sample(vec![screen(1, "display", 1)]);
        native.inventory.get_mut("display").unwrap().push(SpaceId::new(2));
        let before = rig.tick(0, Some(native.clone())).unwrap();
        let (mut producer, mut requests) = AppThreadHandle::channel();
        producer.set_topology_authority(&rig.actor.topology_guard(), before.revision);
        let old = producer.clone();
        let request = || {
            Request::set_window_frame(WindowId::new(1, 1), CGRect::ZERO, Default::default(), false)
        };
        old.send(request()).unwrap();
        let queued = requests.try_recv().unwrap().1;
        if notify {
            rig.event(Event::ActiveSpaceChanged);
            assert!(
                !producer.can_write_frames(),
                "the switch signal revokes old producers before sampling"
            );
        }
        native.screens[0].space = Some(SpaceId::new(2));
        let after = rig.tick(0, Some(native.clone())).unwrap();
        assert!(
            queued.authorized().is_none(),
            "a published Space switch must revoke already queued geometry"
        );
        old.send(request()).unwrap();
        assert!(requests.try_recv().unwrap().1.authorized().is_none());
        producer.set_topology_authority(&rig.actor.topology_guard(), after.revision);
        producer.send(request()).unwrap();
        assert!(requests.try_recv().unwrap().1.authorized().is_some());
        let unchanged = rig.tick(0, Some(native)).unwrap();
        assert_eq!(after.revision, unchanged.revision);
        assert!(
            producer.can_write_frames(),
            "unchanged samples must preserve presentation authority"
        );
    }
}
