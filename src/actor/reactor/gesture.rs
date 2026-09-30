//! Paced consumption of cumulative physical motion; no layout preparation here.
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use super::animation::AnimationManager;
use super::{Reactor, command_workflow};
use crate::actor::app::{AppThreadHandle, Request, WindowId};
use crate::actor::gesture::{Context, Control, Lifecycle, Motion, MotionPublisher};
use crate::common::collections::HashMap;
use crate::layout_engine::{
    EventResponse, LayoutCommand, LayoutId, LayoutSystemKind, VirtualWorkspaceId,
};

pub(super) struct ViewportSession {
    context: Context,
    control: Control,
    motion: MotionPublisher,
    workspace: VirtualWorkspaceId,
    layout: LayoutId,
    applied: f64,
    timestamp: Duration,
    updated_windows: HashMap<WindowId, AppThreadHandle>,
    pub(super) interval: Duration,
    pub(super) refresh: Option<Arc<Notify>>,
    pub(super) display_link: Option<crate::sys::display_link::DisplayLink>,
}
impl ViewportSession {
    fn visible(&self, r: &Reactor) -> bool {
        r.is_space_active(self.context.space)
            && r.layout_manager
                .layout_engine
                .workspaces()
                .active_layout_for_space(self.context.space)
                == Some((self.workspace, self.layout))
    }

    fn valid(&self, r: &Reactor) -> bool {
        self.visible(r)
            && self.control.valid(self.context.epoch)
            && matches!(
                r.mission_control_manager.mission_control_state,
                super::MissionControlState::Inactive
            )
    }
}
impl Reactor {
    pub(super) fn gesture_event(&mut self, event: Lifecycle) {
        match event {
            Lifecycle::Begin { context, motion, control } => {
                if self
                    .viewport_gesture
                    .as_ref()
                    .is_some_and(|s| s.context.session == context.session)
                {
                    return;
                }
                self.finish_gesture(None, true);
                if !control.valid(context.epoch)
                    || !self.is_space_active(context.space)
                    || !matches!(
                        self.mission_control_manager.mission_control_state,
                        super::MissionControlState::Inactive
                    )
                {
                    return;
                }
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
                if !system.begin_viewport_gesture(layout) {
                    return;
                }
                system.update_viewport_gesture(layout, 0.0, context.started);
                if let Some(tx) = &self.animation_tx {
                    _ = tx.send(super::animation::Message::Stop(
                        system.viewport_frames(layout).into_iter().map(|(wid, _)| wid).collect(),
                    ));
                }
                let hz = self
                    .space_state
                    .screen_by_space(context.space)
                    .and_then(|s| objc2_core_graphics::CGDisplayCopyDisplayMode(s.id.as_u32()))
                    .map(|mode| objc2_core_graphics::CGDisplayMode::refresh_rate(Some(&mode)))
                    .filter(|hz| hz.is_finite() && *hz > 0.0)
                    .unwrap_or(120.0)
                    .clamp(30.0, 240.0);
                let notify = Arc::new(Notify::new());
                // A display callback is useful only once the native animation
                // actor is installed; otherwise the timer can drive the model.
                let link = self
                    .space_state
                    .screen_by_space(context.space)
                    .filter(|_| self.animation_tx.is_some())
                    .map(|screen| {
                        crate::sys::display_link::DisplayLink::for_display(
                            screen.id.as_u32(),
                            notify.clone(),
                        )
                    });
                let refresh = link.as_ref().map(|_| notify);
                self.viewport_gesture = Some(ViewportSession {
                    updated_windows: HashMap::default(),
                    timestamp: context.started,
                    context,
                    control,
                    motion,
                    workspace,
                    layout,
                    applied: 0.0,
                    interval: Duration::from_secs_f64(1.0 / hz),
                    refresh,
                    display_link: link,
                });
            }
            Lifecycle::End { sample, cancelled } => {
                if self
                    .viewport_gesture
                    .as_ref()
                    .is_some_and(|s| s.context.session == sample.session)
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

    pub(super) fn gesture_tick(&mut self) {
        let Some(session) = &self.viewport_gesture else {
            return;
        };
        if !session.valid(self) {
            self.finish_gesture(None, true);
            return;
        }
        if let Some(sample) = session.motion.latest(session.context.session) {
            self.apply_gesture_sample(sample);
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
        let moved = system.update_viewport_gesture(s.layout, delta, sample.timestamp);
        if moved.is_some_and(|x| x != 0.0) {
            let frames = system.viewport_frames(s.layout);
            let space = s.context.space;
            let frames = self.bound_viewport_frames(space, frames);
            if AnimationManager::interactive_layout(self, space, &frames) {
                let session = self.viewport_gesture.as_mut().unwrap();
                for (wid, _) in frames {
                    if let Some(app) = self.app_manager.apps.get(&wid.pid) {
                        session.updated_windows.entry(wid).or_insert_with(|| app.handle.clone());
                    }
                }
            }
        }
    }

    fn finish_gesture(&mut self, final_sample: Option<Motion>, cancelled: bool) {
        if let Some(sample) = final_sample {
            self.apply_gesture_sample(sample);
        } else if let Some(s) = &self.viewport_gesture
            && let Some(sample) = s.motion.latest(s.context.session)
        {
            self.apply_gesture_sample(sample);
        }
        let Some(s) = self.viewport_gesture.take() else {
            return;
        };
        // Flush the last coalesced write and read accepted geometry once, even
        // when release leaves the camera exactly where the fingers stopped.
        for (&wid, handle) in &s.updated_windows {
            let _ = handle.send(Request::EndWindowAnimation(wid));
        }
        let valid = s.valid(self);
        let visible = valid || s.visible(self);
        if cancelled || !valid {
            s.control.retire_session(s.context.epoch, s.context.session);
        }
        // Like a fluid page swipe, an edge transition remains reversible until
        // lift. Edge travel has 40% of the strip's gain so merely reaching the
        // boundary during a fast scroll cannot commit a workspace switch.
        let excess =
            match &self.layout_manager.layout_engine.workspaces()[s.workspace].layout_system {
                LayoutSystemKind::Scrolling(system) => system.gesture_overscroll(s.layout),
                _ => 0.0,
            };
        let next = (excess > 0.0) != s.context.action.invert;
        if !cancelled
            && valid
            && s.context.action.propagate
            && excess != 0.0
            && 0.4 * excess.abs() / s.context.width >= s.context.action.boundary_threshold
            && self.gesture_workspace_available(&s.context, next)
        {
            if let LayoutSystemKind::Scrolling(system) =
                &mut self.layout_manager.layout_engine.workspaces_mut()[s.workspace].layout_system
            {
                system.cancel_viewport_gesture(s.layout);
            }
            self.gesture_workspace(&s.context, next);
            return;
        }
        let LayoutSystemKind::Scrolling(system) =
            &mut self.layout_manager.layout_engine.workspaces_mut()[s.workspace].layout_system
        else {
            return;
        };
        let release = if cancelled || !valid {
            system.cancel_viewport_gesture(s.layout);
            None
        } else {
            system.end_viewport_gesture(
                s.layout,
                final_sample.map_or(s.timestamp, |m| m.timestamp),
                None,
            )
        };
        if visible {
            let frames = system.viewport_frames(s.layout);
            if let Some(release) = release {
                let frames = self.bound_viewport_frames(s.context.space, frames);
                AnimationManager::animate_viewport_release(
                    self,
                    s.context.space,
                    &frames,
                    release,
                    s.context.action.animate,
                );
                tracing::debug!(
                    session = s.context.session,
                    velocity = release.velocity,
                    from = release.from_offset,
                    target = release.offset,
                    "gesture released"
                );
            } else {
                self.apply_viewport_frames(s.context.space, frames);
            }
            if let Some(release) = release
                && self.main_window() != Some(release.window)
            {
                self.handle_layout_response(
                    EventResponse {
                        changed: true,
                        focus_window: Some(release.window),
                        ..Default::default()
                    },
                    Some(s.context.space),
                );
            }
        }
    }

    fn apply_viewport_frames(
        &mut self,
        space: crate::sys::screen::SpaceId,
        frames: Vec<(crate::actor::app::WindowId, objc2_core_foundation::CGRect)>,
    ) {
        let frames = self.bound_viewport_frames(space, frames);
        AnimationManager::workspace_switch_layout(self, space, &frames, None);
    }

    fn bound_viewport_frames(
        &self,
        space: crate::sys::screen::SpaceId,
        mut frames: Vec<(crate::actor::app::WindowId, objc2_core_foundation::CGRect)>,
    ) -> Vec<(crate::actor::app::WindowId, objc2_core_foundation::CGRect)> {
        if self.active_spaces.len() > 1
            && let Some(screen) = self.space_state.screen_by_space(space)
        {
            for (_, frame) in &mut frames {
                *frame = super::managers::bound_frame_to_screen(*frame, screen.frame);
            }
        }
        frames
    }

    fn gesture_workspace_available(&self, context: &Context, next: bool) -> bool {
        let store = self.layout_manager.layout_engine.workspaces();
        let Some(current) = store.active_workspace(context.space) else {
            return false;
        };
        let target = if next {
            store.next_workspace(
                &self.state.windows,
                context.space,
                current,
                context.settings.skip_empty,
            )
        } else {
            store.prev_workspace(
                &self.state.windows,
                context.space,
                current,
                context.settings.skip_empty,
            )
        };
        target.is_some_and(|target| target != current)
            && self.is_space_active(context.space)
            && matches!(
                self.mission_control_manager.mission_control_state,
                super::MissionControlState::Inactive
            )
    }

    fn gesture_workspace(&mut self, context: &Context, next: bool) {
        if !self.gesture_workspace_available(context, next) {
            return;
        }
        let (visible_spaces, visible_space_centers) = self.visible_spaces_for_layout(false);
        let result = command_workflow::handle_command_layout(
            &mut self.state,
            &mut self.layout_manager,
            &mut self.workspace_switch_manager,
            command_workflow::LayoutCommandPayload {
                command: if next {
                    LayoutCommand::NextWorkspace(context.settings.skip_empty)
                } else {
                    LayoutCommand::PrevWorkspace(context.settings.skip_empty)
                },
                command_space: Some(context.space),
                visible_spaces,
                visible_space_centers,
                post_arrange_mouse_warp: None,
            },
        );
        if let Ok(outcome) = result {
            self.apply_event_outcome(outcome);
            if let Some(pattern) = context.settings.haptic {
                let _ = crate::sys::haptics::perform_haptic(pattern);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::*;
    use crate::actor::gesture::Settings;
    use crate::actor::reactor::testing::*;
    use crate::common::config::{Config, LayoutMode};
    use crate::layout_engine::LayoutSystem;
    use crate::sys::geometry::SameAs;
    use crate::sys::screen::{CoordinateConverter, SpaceId};

    fn setup(propagate: bool) -> (Reactor, Context, Control, MotionPublisher) {
        setup_options(propagate, false, false)
    }
    fn setup_options(
        propagate: bool,
        skip_empty: bool,
        invert: bool,
    ) -> (Reactor, Context, Control, MotionPublisher) {
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
        cfg.settings.layout.scrolling.gestures.invert_horizontal = invert;
        cfg.settings.layout.scrolling.gestures.propagate_to_workspace_swipe = propagate;
        cfg.settings.layout.scrolling.gestures.workspace_switch_threshold = 0.1;
        cfg.settings.gestures.haptics_enabled = false;
        cfg.settings.gestures.skip_empty = skip_empty;
        let settings = Settings::new(&cfg);
        let c = Control::new(&cfg);
        c.configure(settings, true, Vec::new(), CoordinateConverter::default());
        let context = Context::new(
            1,
            0,
            space,
            1000.0,
            LayoutMode::Scrolling,
            settings,
            Duration::ZERO,
        )
        .unwrap();
        let motion = MotionPublisher::default();
        r.gesture_event(Lifecycle::Begin {
            context: context.clone(),
            control: c.clone(),
            motion: motion.clone(),
        });
        (r, context, c, motion)
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
            let settings = Settings::new(&config);
            let control = Control::new(&config);
            control.configure(settings, true, Vec::new(), CoordinateConverter::default());
            let context = Context::new(
                1,
                0,
                space,
                1000.0,
                LayoutMode::Scrolling,
                settings,
                Duration::ZERO,
            )
            .unwrap();
            let motion = MotionPublisher::default();
            r.gesture_event(Lifecycle::Begin {
                context,
                control,
                motion: motion.clone(),
            });
            for (total_x, time) in [(20.0, 10), (40.0, 20)] {
                motion.publish(Motion {
                    session: 1,
                    total_x,
                    timestamp: Duration::from_millis(time),
                });
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
            queue.drain_with(|wid, frame, set_size, _, source| {
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

            // Pause before lift: niri leaves the camera here, so there is no new
            // layout write to implicitly acknowledge the last interactive frame.
            r.gesture_event(Lifecycle::End {
                sample: Motion {
                    session: 1,
                    total_x: final_x,
                    timestamp: Duration::from_millis(200),
                },
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
        let (mut r, ctx, _, m) = setup(false);
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
            m.publish(Motion {
                session: 1,
                total_x: total,
                timestamp: Duration::from_millis(time),
            });
        }
        r.gesture_tick();
        assert_eq!(r.viewport_gesture.as_ref().unwrap().applied, 15.0);
        assert_eq!(
            r.layout_manager.layout_engine.workspaces()[workspace]
                .layout_system
                .selected_window(layout),
            initial
        );
        m.publish(Motion {
            session: 99,
            total_x: 9000.0,
            timestamp: Duration::from_millis(40),
        });
        r.gesture_tick();
        assert_eq!(r.viewport_gesture.as_ref().unwrap().applied, 15.0);
        r.gesture_event(Lifecycle::End {
            sample: Motion {
                session: 1,
                total_x: 900.0,
                timestamp: Duration::from_millis(50),
            },
            cancelled: false,
        });
        assert!(r.viewport_gesture.is_none());
        let selected = r.layout_manager.layout_engine.workspaces()[workspace]
            .layout_system
            .selected_window(layout);
        assert_ne!(selected, initial);
        r.gesture_event(Lifecycle::End {
            sample: Motion {
                session: 1,
                total_x: 5000.0,
                timestamp: Duration::from_millis(60),
            },
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
        for (enabled, invert) in [(false, false), (true, false), (true, true)] {
            let (mut r, ctx, _, m) = setup_options(enabled, false, invert);
            let before = r.layout_manager.layout_engine.workspaces().active_workspace(ctx.space);
            let store = r.layout_manager.layout_engine.workspaces();
            let expected = if !enabled {
                before
            } else if invert {
                store.next_workspace(&r.state.windows, ctx.space, before.unwrap(), Some(false))
            } else {
                store.prev_workspace(&r.state.windows, ctx.space, before.unwrap(), Some(false))
            };
            for (total, time) in [(-50.0, 10), (-120.0, 20), (-300.0, 30)] {
                m.publish(Motion {
                    session: 1,
                    total_x: total,
                    timestamp: Duration::from_millis(time),
                });
                r.gesture_tick();
                assert_eq!(
                    r.layout_manager.layout_engine.workspaces().active_workspace(ctx.space),
                    before,
                    "edge movement must remain reversible until lift"
                );
            }
            for total in [-300.0, -1000.0] {
                r.gesture_event(Lifecycle::End {
                    sample: Motion {
                        session: 1,
                        total_x: total,
                        timestamp: Duration::from_millis(40),
                    },
                    cancelled: false,
                });
                assert_eq!(
                    r.layout_manager.layout_engine.workspaces().active_workspace(ctx.space),
                    expected,
                    "release propagates once and mirrors prior inversion"
                );
            }
            assert!(r.viewport_gesture.is_none());
        }
    }

    #[test]
    fn boundary_release_rejects_brush_reversal_and_cancellation() {
        for (travel, reverse, cancelled) in [
            (120.0, false, false),
            (240.0, false, false),
            (300.0, true, false),
            (300.0, false, true),
        ] {
            let (mut r, ctx, _, m) = setup(true);
            let before = r.layout_manager.layout_engine.workspaces().active_workspace(ctx.space);
            m.publish(Motion {
                session: 1,
                total_x: -travel,
                timestamp: Duration::from_millis(10),
            });
            r.gesture_tick();
            r.gesture_event(Lifecycle::End {
                sample: Motion {
                    session: 1,
                    total_x: if reverse { -travel + 20.0 } else { -travel },
                    timestamp: Duration::from_millis(20),
                },
                cancelled,
            });
            assert_eq!(
                r.layout_manager.layout_engine.workspaces().active_workspace(ctx.space),
                before,
                "an edge brush, inward reversal, or cancellation must not change workspaces"
            );
            assert!(r.viewport_gesture.is_none());
        }
    }
    #[test]
    fn reset_and_workspace_change_retire_camera_without_applying_late_motion() {
        for reset in [true, false] {
            let (mut r, ctx, c, m) = setup(false);
            m.publish(Motion {
                session: 1,
                total_x: 100.0,
                timestamp: Duration::from_millis(10),
            });
            r.gesture_tick();
            if reset {
                let (tx, _) = crate::actor::channel();
                c.reset(&tx);
            } else {
                r.handle_test_layout_command(LayoutCommand::NextWorkspace(Some(false)));
            }
            m.publish(Motion {
                session: 1,
                total_x: 9000.0,
                timestamp: Duration::from_millis(20),
            });
            r.gesture_tick();
            assert!(r.viewport_gesture.is_none());
            assert!(c.valid(ctx.epoch) != reset);
        }
    }
    #[test]
    fn idle_samples_preserve_velocity_for_a_flick_after_a_pause() {
        let (mut r, ctx, _, m) = setup(false);
        for (total, time) in [(0.0, 100), (0.0, 200), (0.0, 290), (200.0, 300)] {
            m.publish(Motion {
                session: 1,
                total_x: total,
                timestamp: Duration::from_millis(time),
            });
            r.gesture_tick();
        }
        let (ws, layout) = r
            .layout_manager
            .layout_engine
            .workspaces()
            .active_layout_for_space(ctx.space)
            .unwrap();
        let LayoutSystemKind::Scrolling(system) =
            &mut r.layout_manager.layout_engine.workspaces_mut()[ws].layout_system
        else {
            panic!("scrolling");
        };
        let release =
            system.end_viewport_gesture(layout, Duration::from_millis(310), None).unwrap();
        assert!(
            release.velocity > 1000.0,
            "recent idle samples must anchor velocity: {}",
            release.velocity
        );
        assert_eq!(release.window, crate::actor::app::WindowId::new(1, 4));
    }

    #[test]
    fn empty_scrolling_workspace_routes_to_workspace_swipe() {
        let (mut r, ctx, _, _) = setup(false);
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
    fn unavailable_boundary_workspace_does_not_cancel_the_camera() {
        let (mut r, ctx, _, m) = setup_options(true, true, false);
        let before = r.layout_manager.layout_engine.workspaces().active_workspace(ctx.space);
        m.publish(Motion {
            session: 1,
            total_x: -300.0,
            timestamp: Duration::from_millis(10),
        });
        r.gesture_tick();
        assert_eq!(
            r.layout_manager.layout_engine.workspaces().active_workspace(ctx.space),
            before
        );
        assert!(
            r.viewport_gesture.is_some(),
            "an unavailable workspace must not end the camera"
        );
        m.publish(Motion {
            session: 1,
            total_x: 100.0,
            timestamp: Duration::from_millis(20),
        });
        r.gesture_tick();
        assert_eq!(r.viewport_gesture.as_ref().unwrap().applied, 100.0);
    }
}
