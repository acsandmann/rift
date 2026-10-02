//! Paced consumption of cumulative physical motion; no layout preparation here.
use std::time::{Duration, Instant};

use super::{Reactor, command_workflow};
use crate::actor::gesture::{Context, Control, Lifecycle, Motion};
use crate::layout_engine::{
    EventResponse, LayoutCommand, LayoutId, LayoutSystem, LayoutSystemKind, VirtualWorkspaceId,
};

pub(super) struct ViewportSession {
    pub(super) context: Context,
    pub(super) control: Control,
    pub(super) workspace: VirtualWorkspaceId,
    pub(super) layout: LayoutId,
    pub(super) released: bool,
    pub(super) applied: f64,
    pub(super) timestamp: Duration,
}
impl ViewportSession {
    fn visible(&self, r: &Reactor) -> bool {
        r.is_space_active(self.context.space)
            && r.layout_manager
                .layout_engine
                .workspaces()
                .active_layout_for_space(self.context.space)
                == Some((self.workspace, self.layout))
            && r.layout_manager
                .layout_engine
                .workspaces()
                .workspaces
                .get(self.workspace)
                .is_some_and(|ws| matches!(&ws.layout_system, LayoutSystemKind::Scrolling(system) if system.contains_layout(self.layout)))
    }

    fn valid(&self, r: &Reactor) -> bool {
        self.visible(r)
            && self.control.valid(self.context.epoch)
            && r.gesture_space_active(self.context.space)
    }
}
impl Reactor {
    fn gesture_space_active(&self, space: crate::sys::screen::SpaceId) -> bool {
        self.is_space_active(space)
            && matches!(
                self.mission_control_manager.mission_control_state,
                super::MissionControlState::Inactive
            )
    }

    pub(super) fn gesture_event(&mut self, event: Lifecycle) {
        match event {
            Lifecycle::Begin { context, control } => {
                if self
                    .viewport_gesture
                    .as_ref()
                    .is_some_and(|s| s.context.session == context.session)
                {
                    return;
                }
                if !control.valid(context.epoch) || !self.gesture_space_active(context.space) {
                    return;
                }
                let target = self
                    .layout_manager
                    .layout_engine
                    .workspaces()
                    .active_layout_for_space(context.space);
                if let Some(s) = &self.viewport_gesture
                    && s.released
                    && target != Some((s.workspace, s.layout))
                {
                    let (workspace, layout, visible) = (s.workspace, s.layout, s.visible(self));
                    if let Some(ws) = self
                        .layout_manager
                        .layout_engine
                        .workspaces_mut()
                        .workspaces
                        .get_mut(workspace)
                        && let LayoutSystemKind::Scrolling(system) = &mut ws.layout_system
                    {
                        system.finish_viewport_animation(layout);
                    }
                    if visible {
                        self.apply_viewport_frames();
                    }
                }
                let cancel = self.viewport_gesture.as_ref().is_some_and(|s| !s.released);
                self.finish_gesture(None, cancel);
                let engine = &mut self.layout_manager.layout_engine;
                let Some((workspace, layout)) =
                    engine.workspaces().active_layout_for_space(context.space)
                else {
                    return;
                };
                let LayoutSystemKind::Scrolling(system) =
                    &mut engine.workspaces_mut()[workspace].layout_system
                else {
                    return;
                };
                if !system.begin_viewport_gesture(layout, Instant::now()) {
                    return;
                }
                system.update_viewport_gesture(layout, 0.0, context.started);
                if let Some(tx) = &self.animation_tx {
                    _ = tx.send(super::animation::Message::Stop(
                        system.viewport_frames(layout).map(|(wid, _)| wid).collect(),
                        None,
                    ));
                }
                self.viewport_gesture = Some(ViewportSession {
                    timestamp: context.started,
                    context,
                    control,
                    workspace,
                    layout,
                    released: false,
                    applied: 0.0,
                });
                self.start_gesture_presentation();
            }
            Lifecycle::End { sample, cancelled } => {
                if self
                    .viewport_gesture
                    .as_ref()
                    .is_some_and(|s| !s.released && s.context.session == sample.session)
                {
                    self.finish_gesture(Some(sample), cancelled);
                }
            }
            Lifecycle::Workspace { context, next, control } => {
                if control.valid(context.epoch) {
                    self.gesture_workspace(&context, next);
                }
            }
            Lifecycle::Reset => self.finish_gesture(None, true),
        }
    }

    fn start_gesture_presentation(&mut self) {
        if let Some(s) = &self.viewport_gesture {
            let space = s.context.space;
            let gesture = (!s.released)
                .then(|| (s.context.clone(), s.control.clone(), s.applied, s.timestamp));
            self.present_camera(space, true, gesture, None);
        }
    }

    fn apply_gesture_sample(&mut self, sample: Motion) {
        let Some(s) = self.viewport_gesture.as_mut() else {
            return;
        };
        if !s.control.valid(s.context.epoch)
            || sample.session != s.context.session
            || sample.timestamp <= s.timestamp
        {
            return;
        }
        let delta = sample.total_x - s.applied;
        s.timestamp = sample.timestamp;
        s.applied = sample.total_x;
        let LayoutSystemKind::Scrolling(system) =
            &mut self.layout_manager.layout_engine.workspaces_mut()[s.workspace].layout_system
        else {
            return;
        };
        system.update_viewport_gesture_normalized(s.layout, delta, sample.timestamp);
    }

    fn apply_viewport_frames(&mut self) {
        if let Some(s) = &self.viewport_gesture {
            let space = s.context.space;
            self.present_camera(space, false, None, None);
            self.reconcile_presentations();
        }
    }

    fn finish_gesture(&mut self, final_sample: Option<Motion>, cancelled: bool) {
        let Some(s) = &self.viewport_gesture else { return };
        if let Some(camera) = self.presentations.get(&s.context.space) {
            camera.state.lock().paused = true;
        }
        self.reconcile_presentations();
        let Some(s) = &self.viewport_gesture else { return };
        if !self
            .layout_manager
            .layout_engine
            .workspaces()
            .workspaces
            .get(s.workspace)
            .is_some_and(|ws| matches!(ws.layout_system, LayoutSystemKind::Scrolling(_)))
        {
            self.retire_viewport_session();
            return;
        }
        if s.released {
            if cancelled {
                let (workspace, layout) = (s.workspace, s.layout);
                if let LayoutSystemKind::Scrolling(system) =
                    &mut self.layout_manager.layout_engine.workspaces_mut()[workspace].layout_system
                {
                    system.cancel_viewport_gesture(layout);
                }
                if s.visible(self) {
                    self.apply_viewport_frames();
                }
            }
            self.retire_viewport_session();
            return;
        }
        let valid = s.valid(self);
        if valid && let Some(sample) = final_sample.or_else(|| s.control.latest(s.context.session))
        {
            self.apply_gesture_sample(sample);
        }
        let s = self.viewport_gesture.as_ref().unwrap();
        let visible = valid || s.visible(self);
        let cancelled = cancelled || !valid;
        if cancelled {
            s.control.retire_session(s.context.epoch, s.context.session);
        }
        // Like a fluid page swipe, an edge transition remains reversible until
        // lift. Require deliberate travel beyond the boundary, measured directly
        // in working-area widths, before committing a workspace switch.
        let excess =
            match &self.layout_manager.layout_engine.workspaces()[s.workspace].layout_system {
                LayoutSystemKind::Scrolling(system) => system.gesture_overscroll(s.layout),
                _ => 0.0,
            };
        let next = (excess > 0.0) != s.context.action.invert;
        if !cancelled
            && s.context.action.propagate
            && excess != 0.0
            && excess.abs() >= s.context.action.boundary_threshold
        {
            let context = s.context.clone();
            if self.gesture_workspace(&context, next) {
                return;
            }
        }
        let s = self.viewport_gesture.take().unwrap();
        let animate = s
            .context
            .action
            .animate
            .or(self
                .layout_manager
                .layout_engine
                .layout_specific_animate_settings(s.context.space))
            .unwrap_or(self.config.settings.animate);
        let LayoutSystemKind::Scrolling(system) =
            &mut self.layout_manager.layout_engine.workspaces_mut()[s.workspace].layout_system
        else {
            return;
        };
        let release = if cancelled {
            system.cancel_viewport_gesture(s.layout);
            None
        } else {
            system.end_viewport_gesture(s.layout, s.timestamp, animate)
        };
        if !visible {
            self.viewport_gesture = Some(s);
            self.retire_viewport_session();
            return;
        }
        if let Some(release) = release {
            let space = s.context.space;
            self.viewport_gesture = Some(ViewportSession { released: true, ..s });
            if animate {
                self.start_gesture_presentation();
            } else {
                self.apply_viewport_frames();
            }
            if !animate {
                self.retire_viewport_session();
            }
            if self.main_window() != Some(release.window) {
                self.handle_layout_response(
                    EventResponse {
                        changed: true,
                        focus_window: Some(release.window),
                        ..Default::default()
                    },
                    Some(space),
                );
            }
        } else {
            self.viewport_gesture = Some(s);
            self.apply_viewport_frames();
            self.retire_viewport_session();
        }
    }

    pub(super) fn retire_viewport_session(&mut self) {
        if let Some(s) = self.viewport_gesture.take() {
            if let Some(camera) = self.presentations.get(&s.context.space) {
                camera.state.lock().stop();
            }
        }
    }

    fn gesture_workspace(&mut self, context: &Context, next: bool) -> bool {
        if !self.gesture_space_active(context.space) {
            return false;
        }
        let (visible_spaces, visible_space_centers) = self.visible_spaces_for_layout(false);
        let result = command_workflow::handle_command_layout(
            &mut self.state,
            &mut self.layout_manager,
            &mut self.workspace_switch_manager,
            command_workflow::LayoutCommandPayload {
                command: if next {
                    LayoutCommand::NextWorkspace(context.skip_empty)
                } else {
                    LayoutCommand::PrevWorkspace(context.skip_empty)
                },
                command_space: Some(context.space),
                visible_spaces,
                visible_space_centers,
                post_arrange_mouse_warp: None,
            },
        );
        let Ok(outcome) = result else { return false };
        if !outcome.layout_responses.iter().any(|(response, _)| response.changed) {
            return false;
        }
        // Reconcile cached viewport frames before the switch parks outgoing windows.
        // A late EndWindowAnimation would otherwise reapply the old visible frame.
        self.finish_gesture(None, true);
        self.apply_event_outcome(outcome);
        if let Some(pattern) = context.haptic {
            let _ = crate::sys::haptics::perform_haptic(pattern);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::*;
    use crate::actor::app::{AppThreadHandle, FrameSource, Request, WindowId};
    use crate::sys::geometry::SameAs;

    impl Reactor {
        fn gesture_tick(&mut self) {
            if self.viewport_gesture.as_ref().is_some_and(|s| !s.valid(self)) {
                self.finish_gesture(None, true);
                return;
            }
            if let Some(s) = &self.viewport_gesture
                && let Some(camera) = self.presentations.get(&s.context.space)
            {
                camera.state.lock().sample(Instant::now(), 1.0);
            }
            self.reconcile_presentations();
            if self.viewport_gesture.as_ref().is_some_and(|s| {
                s.released
                    && self
                        .presentations
                        .get(&s.context.space)
                        .is_none_or(|c| !c.state.lock().active)
            }) {
                self.retire_viewport_session();
            }
        }
    }
    use crate::actor::gesture::Settings;
    use crate::actor::reactor::testing::*;
    use crate::common::config::{Config, LayoutMode};
    use crate::layout_engine::LayoutSystem;
    use crate::sys::screen::{CoordinateConverter, SpaceId};

    fn sample(session: u64, total_x: f64, millis: u64) -> Motion {
        Motion {
            session,
            total_x: total_x / 1000.0,
            timestamp: Duration::from_millis(millis),
        }
    }

    fn setup_options_threshold(
        propagate: bool,
        skip_empty: bool,
        invert: bool,
        threshold: f64,
    ) -> (Reactor, Context, Control, Control) {
        let mut r = test_reactor_with_workspace_count(3);
        let space = SpaceId::new(1);
        r.handle_loop_event(space_state_event(
            vec![CGRect::new(CGPoint::ZERO, CGSize::new(1000.0, 1000.0))],
            vec![Some(space)],
        ));
        r.handle_test_layout_command(LayoutCommand::NextWorkspace(Some(false)));
        r.handle_test_layout_command(LayoutCommand::SetWorkspaceLayout {
            workspace: None,
            mode: LayoutMode::Scrolling,
        });
        for i in 1..=4 {
            r.send_layout_event(crate::layout_engine::LayoutEvent::WindowAdded(
                space,
                crate::actor::app::WindowId::new(1, i),
            ));
        }
        // Prepare through the real engine boundary; no native app is needed to
        // exercise viewport focus, cached frames and workspace ownership.
        let (ws, layout) = r
            .layout_manager
            .layout_engine
            .workspaces()
            .active_layout_for_space(space)
            .unwrap();
        let LayoutSystemKind::Scrolling(system) =
            &mut r.layout_manager.layout_engine.workspaces_mut()[ws].layout_system
        else {
            panic!("scrolling layout");
        };
        system.select_window(layout, crate::actor::app::WindowId::new(1, 1));
        system.prepare_layout(
            layout,
            CGRect::new(CGPoint::ZERO, CGSize::new(1000.0, 1000.0)),
            &Default::default(),
            &Default::default(),
        );
        let mut cfg = Config::default();
        cfg.settings.layout.scrolling.gestures.enabled = true;
        cfg.settings.layout.scrolling.gestures.animate = Some(false);
        cfg.settings.layout.scrolling.gestures.invert_horizontal = invert;
        cfg.settings.layout.scrolling.gestures.propagate_to_workspace_swipe = propagate;
        cfg.settings.layout.scrolling.gestures.workspace_switch_threshold = threshold;
        cfg.settings.gestures.haptics_enabled = false;
        cfg.settings.gestures.skip_empty = skip_empty;
        let (context, c, motion) = begin(&mut r, &cfg, space);
        (r, context, c, motion)
    }
    fn begin(r: &mut Reactor, config: &Config, space: SpaceId) -> (Context, Control, Control) {
        let settings = Settings::new(config);
        let c = Control::new(config);
        c.configure(settings, true, Vec::new(), CoordinateConverter::default());
        let context = Context::new(
            1,
            0,
            space,
            (
                LayoutMode::Scrolling,
                config.settings.layout.scrolling.gestures.fingers,
            ),
            settings,
            Duration::ZERO,
        )
        .unwrap();
        let motion = c.clone();
        r.gesture_event(Lifecycle::Begin {
            context: context.clone(),
            control: c.clone(),
        });
        (context, c, motion)
    }

    #[test]
    fn workspace_gesture_with_one_workspace_leaves_switch_inactive() {
        for next in [true, false] {
            let settings = crate::common::config::VirtualWorkspaceSettings {
                default_workspace_count: 1,
                prevent_wrapping: false,
                ..Default::default()
            };
            let mut r = test_reactor_with_workspace_settings(&settings);
            let space = SpaceId::new(1);
            r.handle_loop_event(space_state_event(
                vec![CGRect::new(CGPoint::ZERO, CGSize::new(1000.0, 1000.0))],
                vec![Some(space)],
            ));
            let workspace = r.layout_manager.layout_engine.workspaces().active_workspace(space);
            assert!(workspace.is_some());
            let mut config = Config::default();
            config.settings.layout.scrolling.gestures.enabled = true;
            config.settings.layout.scrolling.gestures.animate = Some(false);
            config.settings.gestures.haptics_enabled = false;
            config.settings.gestures.skip_empty = false;
            let (context, _, _) = begin(&mut r, &config, space);

            assert!(!r.gesture_workspace(&context, next));
            assert_eq!(
                r.layout_manager.layout_engine.workspaces().active_workspace(space),
                workspace
            );
            assert_eq!(
                r.workspace_switch_manager.workspace_switch_state,
                super::super::WorkspaceSwitchState::Inactive
            );
            assert_eq!(r.workspace_switch_manager.pending_workspace_switch_origin, None);
            assert_eq!(r.workspace_switch_manager.active_workspace_switch, None);
            assert!(!r.workspace_switch_manager.manual_switch_in_progress());
        }
    }

    #[test]
    fn live_scroll_coalesces_app_writes_and_reconciles_once_at_lift() {
        for (cancelled, final_x) in [(false, 40.0), (false, 60.0), (true, 40.0)] {
            let (mut apps, mut r) = test_context();
            let space = SpaceId::new(1);
            r.handle_loop_event(space_state_event(
                vec![CGRect::new(CGPoint::ZERO, CGSize::new(1000.0, 1000.0))],
                vec![Some(space)],
            ));
            r.handle_test_layout_command(LayoutCommand::SetWorkspaceLayout {
                workspace: None,
                mode: LayoutMode::Scrolling,
            });
            apps.make_app_and_settle(&mut r, 1, make_windows(4));
            r.send_layout_event(crate::layout_engine::LayoutEvent::WindowFocused(
                space,
                WindowId::new(1, 1),
            ));
            apps.simulate_until_quiet(&mut r);
            let initial = r.state.windows.window(WindowId::new(1, 1)).unwrap().frame_monotonic;

            let mut config = Config::default();
            config.settings.layout.scrolling.gestures.enabled = true;
            config.settings.layout.scrolling.gestures.animate = Some(false);
            let (_, _, motion) = begin(&mut r, &config, space);
            for (total_x, time) in [(20.0, 10), (40.0, 20)] {
                motion.publish(sample(1, total_x, time));
                r.gesture_tick();
            }
            let mut requests = apps.requests();
            assert_eq!(
                requests.len(),
                1,
                "live motion must coalesce into one app wake: {requests:?}"
            );
            let Request::InteractiveFramesPending(queue) = requests.pop().unwrap() else {
                panic!("live scrolling must use the interactive transport");
            };
            let mut written = crate::common::collections::HashSet::default();
            queue.drain_with(|wid, frame, set_size, _, source, _| {
                assert_eq!(source, crate::actor::app::FrameSource::Viewport);
                assert!(!set_size, "scrolling must only move windows");
                assert!(frame.same_as(r.state.windows.window(wid).unwrap().frame_monotonic));
                written.insert(wid);
            });
            assert_eq!(written.len(), 4);
            assert!(
                !initial
                    .same_as(r.state.windows.window(WindowId::new(1, 1)).unwrap().frame_monotonic)
            );

            // Reconcile the last interactive frame before the release snap (or
            // cancellation), even if no additional under-finger motion arrived.
            r.gesture_event(Lifecycle::End {
                sample: sample(1, final_x, 200),
                cancelled,
            });
            let requests = apps.requests();
            if !cancelled && final_x != 40.0 {
                assert!(
                    matches!(requests.first(), Some(Request::InteractiveFramesPending(_))),
                    "the final motion must be queued before terminal reconciliation: {requests:?}"
                );
            }
            let reconciled: Vec<_> = requests
                .iter()
                .filter_map(|request| match request {
                    Request::EndWindowAnimation(wid) => Some(*wid),
                    _ => None,
                })
                .collect();
            assert_eq!(
                reconciled.len(),
                written.len(),
                "each moved window needs one terminal reconciliation"
            );
            assert_eq!(
                reconciled.into_iter().collect::<crate::common::collections::HashSet<_>>(),
                written
            );
        }
    }
    #[test]
    fn gesture_flushes_final_sample_and_ignores_stale_motion() {
        let (mut r, ctx, _, m) = setup_options_threshold(false, false, false, 0.25);
        assert!(r.viewport_gesture.is_some());
        let (workspace, layout) = r
            .layout_manager
            .layout_engine
            .workspaces()
            .active_layout_for_space(ctx.space)
            .unwrap();
        let initial = r.layout_manager.layout_engine.workspaces()[workspace]
            .layout_system
            .selected_window(layout);
        for (total, time) in [(4.0, 10), (9.0, 20), (15.0, 30)] {
            m.publish(sample(1, total, time));
        }
        r.gesture_tick();
        assert_eq!(r.viewport_gesture.as_ref().unwrap().applied, 0.015);
        assert_eq!(
            r.layout_manager.layout_engine.workspaces()[workspace]
                .layout_system
                .selected_window(layout),
            initial
        );
        m.publish(sample(99, 9000.0, 40));
        r.gesture_tick();
        assert_eq!(r.viewport_gesture.as_ref().unwrap().applied, 0.015);
        r.gesture_event(Lifecycle::End {
            sample: sample(1, 900.0, 50),
            cancelled: false,
        });
        assert!(r.viewport_gesture.is_none());
        let selected = r.layout_manager.layout_engine.workspaces()[workspace]
            .layout_system
            .selected_window(layout);
        assert_ne!(selected, initial);
        r.gesture_event(Lifecycle::End {
            sample: sample(1, 5000.0, 60),
            cancelled: false,
        });
        assert_eq!(
            r.layout_manager.layout_engine.workspaces()[workspace]
                .layout_system
                .selected_window(layout),
            selected
        );
    }
    #[test]
    fn boundary_requires_configured_overscroll_and_propagates_once() {
        for (enabled, invert, threshold, skip_empty) in [
            (false, false, 0.25, false),
            (true, false, 0.1, false),
            (true, false, 0.25, false),
            (true, false, 0.5, false),
            (true, true, 0.25, false),
            (true, false, 0.25, true),
        ] {
            for (fraction, reverse, cancelled) in [
                (1.01, false, false),
                (0.48, false, false),
                (0.96, false, false),
                (1.2, true, false),
                (1.2, false, true),
            ] {
                let (mut r, ctx, _, m) =
                    setup_options_threshold(enabled, skip_empty, invert, threshold);
                let travel = threshold * fraction;
                let store = r.layout_manager.layout_engine.workspaces();
                let before = store.active_workspace(ctx.space);
                let expected = if !enabled || skip_empty || fraction < 1.0 || reverse || cancelled {
                    before
                } else if invert {
                    store.next_workspace(&r.state.windows, ctx.space, before.unwrap(), Some(false))
                } else {
                    store.prev_workspace(&r.state.windows, ctx.space, before.unwrap(), Some(false))
                };
                for (total, time) in [(-travel * 0.2, 10), (-travel * 0.5, 20), (-travel, 30)] {
                    m.publish(sample(1, total * 1000.0, time));
                    r.gesture_tick();
                    assert_eq!(
                        r.layout_manager.layout_engine.workspaces().active_workspace(ctx.space),
                        before,
                        "workspace handoff waits until lift"
                    );
                }
                for total in [
                    if reverse {
                        -travel + threshold * 0.5
                    } else {
                        -travel
                    },
                    -1000.0,
                ] {
                    r.gesture_event(Lifecycle::End {
                        sample: sample(1, total * 1000.0, 40),
                        cancelled,
                    });
                    assert_eq!(
                        r.layout_manager.layout_engine.workspaces().active_workspace(ctx.space),
                        expected,
                        "release must require deliberate edge travel and propagate only once"
                    );
                }
                assert!(r.viewport_gesture.is_none());
            }
        }
    }

    #[test]
    fn reset_and_workspace_change_retire_camera_without_applying_late_motion() {
        for reset in [true, false] {
            let (mut r, ctx, c, m) = setup_options_threshold(false, false, false, 0.25);
            m.publish(sample(1, 100.0, 10));
            r.gesture_tick();
            let session = r.viewport_gesture.as_ref().unwrap();
            let (workspace, layout) = (session.workspace, session.layout);
            let frames = |r: &Reactor| {
                let LayoutSystemKind::Scrolling(system) =
                    &r.layout_manager.layout_engine.workspaces()[workspace].layout_system
                else {
                    panic!("scrolling layout");
                };
                system.viewport_frames(layout).collect::<Vec<_>>()
            };
            let before = frames(&r);
            if reset {
                let (tx, _) = crate::actor::channel();
                c.reset(&tx);
            } else {
                r.handle_test_layout_command(LayoutCommand::NextWorkspace(Some(false)));
            }
            m.publish(sample(1, 9000.0, 20));
            r.gesture_tick();
            assert!(r.viewport_gesture.is_none());
            assert_eq!(
                frames(&r),
                before,
                "late motion must not move an invalid viewport"
            );
            assert!(c.valid(ctx.epoch) != reset);
        }
    }
    #[test]
    fn empty_scrolling_workspace_routes_to_workspace_swipe() {
        let (mut r, ctx, _, _) = setup_options_threshold(false, false, false, 0.25);
        r.gesture_event(Lifecycle::Reset);
        r.handle_test_layout_command(LayoutCommand::NextWorkspace(Some(false)));
        r.handle_test_layout_command(LayoutCommand::SetWorkspaceLayout {
            workspace: None,
            mode: LayoutMode::Scrolling,
        });
        let (tx, mut rx) = crate::actor::channel();
        r.communication_manager.input_tx = Some(tx);
        r.notification_manager.last_layout_modes_by_space.clear();
        r.update_event_tap_layout_mode();
        let crate::actor::input::Request::LayoutModesChanged(modes) = rx.try_recv().unwrap().1
        else {
            panic!("gesture routing");
        };
        assert_eq!(
            modes,
            vec![(ctx.space, LayoutMode::Traditional)],
            "empty strips must not acquire viewport strokes"
        );
        assert_eq!(
            r.layout_manager.layout_engine.active_layout_mode_at(ctx.space),
            LayoutMode::Scrolling
        );
    }
    #[test]
    fn workspace_swipes_reconcile_viewport_before_hiding_outgoing_windows() {
        for (released, final_x) in [(false, -400.0), (false, 4000.0), (true, 300.0)] {
            let (mut r, ctx, control, motion) = setup_options_threshold(true, false, false, 0.25);
            let (workspace, layout) = {
                let s = r.viewport_gesture.as_ref().unwrap();
                (s.workspace, s.layout)
            };
            let LayoutSystemKind::Scrolling(system) =
                &r.layout_manager.layout_engine.workspaces()[workspace].layout_system
            else {
                panic!("scrolling")
            };
            let initial: Vec<_> = system.viewport_frames(layout).collect();
            r.add_test_app(1);
            let (tx, mut rx) = crate::actor::channel();
            r.app_manager.apps.get_mut(&1).unwrap().handle = AppThreadHandle::new_for_test(tx);
            for (wid, frame) in initial {
                r.insert_test_window_state(wid, frame, None, true);
            }
            r.start_gesture_presentation();
            r.viewport_gesture.as_mut().unwrap().context.action.animate = Some(released);
            motion.publish(sample(1, 300.0, 100));
            r.gesture_tick();
            if released {
                r.gesture_event(Lifecycle::End {
                    sample: sample(1, 300.0, 100),
                    cancelled: false,
                });
                assert!(r.viewport_gesture.as_ref().unwrap().released);
                r.gesture_event(Lifecycle::Workspace {
                    context: ctx.clone(),
                    next: true,
                    control,
                });
            } else {
                r.gesture_event(Lifecycle::End {
                    sample: sample(1, final_x, 200),
                    cancelled: false,
                });
            }
            assert_ne!(
                r.layout_manager.layout_engine.workspaces().active_workspace(ctx.space),
                Some(workspace)
            );
            assert!(r.viewport_gesture.is_none());
            let mut written = crate::common::collections::HashSet::default();
            let mut reconciled = crate::common::collections::HashSet::default();
            let mut hidden = crate::common::collections::HashSet::default();
            while let Ok((_, request)) = rx.try_recv() {
                match request {
                    Request::InteractiveFramesPending(queue) => {
                        queue.drain_with(|wid, _, _, _, _, _| {
                            written.insert(wid);
                        });
                    }
                    Request::AnimationFrame { wid, .. } => {
                        written.insert(wid);
                    }
                    Request::EndWindowAnimation(wid) => {
                        assert!(
                            !hidden.contains(&wid),
                            "terminal viewport frames must precede workspace hiding"
                        );
                        assert!(reconciled.insert(wid), "reconcile each window once");
                    }
                    Request::SetWorkspaceSwitchPositions(positions, _, _) => {
                        for (wid, position) in positions {
                            if written.contains(&wid) {
                                assert!(
                                    reconciled.contains(&wid),
                                    "flush viewport frames before parking"
                                );
                            }
                            assert_eq!(
                                position,
                                r.state.windows.window(wid).unwrap().frame_monotonic.origin
                            );
                            hidden.insert(wid);
                        }
                    }
                    _ => {}
                }
            }
            assert_eq!(hidden.len(), 4);
            assert!(!written.is_empty());
            assert_eq!(reconciled, written);
        }
    }

    #[test]
    fn release_uses_coalesced_viewport_transport_and_invalidation_retires_it() {
        for invalidate in [false, true] {
            let (mut r, ctx, control, motion) = setup_options_threshold(false, false, false, 0.25);
            r.viewport_gesture.as_mut().unwrap().context.action.animate = Some(true);
            r.config.settings.animate = false;
            let (workspace, layout) = {
                let s = r.viewport_gesture.as_ref().unwrap();
                (s.workspace, s.layout)
            };
            let LayoutSystemKind::Scrolling(system) =
                &r.layout_manager.layout_engine.workspaces()[workspace].layout_system
            else {
                panic!("scrolling")
            };
            let initial: Vec<_> = system.viewport_frames(layout).collect();
            r.add_test_app(1);
            let (app_tx, mut app_rx) = crate::actor::channel();
            r.app_manager.apps.get_mut(&1).unwrap().handle = AppThreadHandle::new_for_test(app_tx);
            for (wid, frame) in initial {
                r.insert_test_window_state(wid, frame, None, true);
            }
            r.start_gesture_presentation();
            let (animation_tx, animation_rx) = crossbeam_channel::unbounded();
            r.animation_tx = Some(animation_tx.into());
            motion.publish(sample(1, 300.0, 100));
            r.gesture_tick();
            let before = r.state.windows.window(WindowId::new(1, 1)).unwrap().frame_monotonic;
            r.gesture_event(Lifecycle::End {
                sample: sample(1, 300.0, 100),
                cancelled: false,
            });
            assert!(
                r.viewport_gesture.as_ref().unwrap().released,
                "gesture override must animate even with global animation disabled"
            );
            assert_eq!(
                r.state.windows.window(WindowId::new(1, 1)).unwrap().frame_monotonic,
                before
            );
            assert!(
                animation_rx
                    .try_iter()
                    .all(|message| matches!(message, super::super::animation::Message::Camera(_))),
                "camera release must not start generic window animation"
            );
            // Duplicate lift and late physical samples cannot retarget the release.
            r.gesture_event(Lifecycle::End {
                sample: sample(1, 900.0, 110),
                cancelled: false,
            });
            motion.publish(sample(1, 900.0, 120));
            let LayoutSystemKind::Scrolling(system) =
                &mut r.layout_manager.layout_engine.workspaces_mut()[workspace].layout_system
            else {
                panic!("scrolling")
            };
            if invalidate {
                system.remove_layout(layout);
                // Drain the preceding gesture wake before checking for stale release writes.
                while let Ok((_, request)) = app_rx.try_recv() {
                    if let Request::InteractiveFramesPending(queue) = request {
                        queue.drain_with(|_, _, _, _, _, _| {});
                    }
                }
                r.gesture_tick();
                assert!(r.viewport_gesture.is_none());
                while let Ok((_, request)) = app_rx.try_recv() {
                    assert!(
                        !matches!(request, Request::InteractiveFramesPending(_)),
                        "invalidated release must not overwrite structural layout frames"
                    );
                }
            } else {
                assert_eq!(
                    system.advance_viewport_animation(
                        layout,
                        Instant::now() + Duration::from_millis(50)
                    ),
                    Some(true)
                );
                r.start_gesture_presentation();
                let camera = &r.presentations.get(&ctx.space).unwrap().state;
                camera.lock().sample(Instant::now() + Duration::from_millis(50), 1.0);
                r.reconcile_presentations();
                let mut moved = 0;
                while let Ok((_, request)) = app_rx.try_recv() {
                    if let Request::InteractiveFramesPending(queue) = request {
                        queue.drain_with(|wid, frame, set_size, _, source, _| {
                            assert_eq!(source, FrameSource::Viewport);
                            assert!(!set_size);
                            assert!(
                                frame.same_as(r.state.windows.window(wid).unwrap().frame_monotonic)
                            );
                            moved += 1;
                        });
                    }
                }
                assert!(moved > 0);
                // Epoch invalidation also stops a released camera; lift alone doesn't invalidate it.
                let (tx, _) = crate::actor::channel();
                control.reset(&tx);
                r.gesture_tick();
                assert!(r.viewport_gesture.is_none());
            }
            assert_eq!(
                r.layout_manager.layout_engine.workspaces().active_workspace(ctx.space),
                Some(workspace)
            );
        }
    }
}
