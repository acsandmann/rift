use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use parking_lot::Mutex;
use tracing::{debug, trace};

use super::TransactionId;
use crate::actor::app::{AppThreadHandle, FrameSource, Request, WindowId, pid_t};
use crate::actor::gesture::{Context, Control};
use crate::actor::reactor::Reactor;
use crate::common::collections::{HashMap, HashSet};
use crate::layout_engine::systems::scrolling::ViewportPresentation;
use crate::layout_engine::{LayoutId, LayoutSystem, LayoutSystemKind, VirtualWorkspaceId};
use crate::model::tx_store::WindowTxStore;
use crate::sys::display_link::DisplayLink;
use crate::sys::geometry::{Round, SameAs};
use crate::sys::power;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

#[derive(Debug)]
pub struct AnimationSender {
    tx: Sender<Message>,
}
impl From<Sender<Message>> for AnimationSender {
    fn from(tx: Sender<Message>) -> Self { Self { tx } }
}
impl AnimationSender {
    pub fn send(&self, message: Message) -> Result<(), crossbeam_channel::SendError<Message>> {
        self.tx.send(message)
    }

    pub fn cancel(&self, windows: Vec<WindowId>) -> Vec<(WindowId, CGRect)> {
        let measured = tracing::enabled!(tracing::Level::TRACE).then(Instant::now);
        let (done, wait) = crossbeam_channel::bounded(1);
        if self.send(Message::Stop(windows, Some(done))).is_ok() {
            let frames = wait.recv().unwrap_or_default();
            if let Some(started) = measured {
                trace!(
                    round_trip_us = started.elapsed().as_micros(),
                    "presenter cancellation"
                );
            }
            frames
        } else {
            Vec::new()
        }
    }
}
pub type AnimationReceiver = Receiver<Message>;

#[derive(Debug)]
pub enum Message {
    Replace(Animation),
    SkipToEnd(Animation),
    Stop(Vec<WindowId>, Option<Sender<Vec<(WindowId, CGRect)>>>),
    Camera(Arc<Mutex<CameraAnimation>>),
}

#[derive(Debug, Default)]
pub struct AnimationManager {
    active: Option<ActiveAnimation>,
    camera: Option<Arc<Mutex<CameraAnimation>>>,
}

struct DisplayPresenter {
    manager: AnimationManager,
    link: DisplayLink,
    sequence: u64,
    fallback_deadline: Instant,
}

#[derive(Debug)]
struct ActiveAnimation {
    animation: Animation,
    started: Instant,
    progress: f64,
    next_sample: Instant,
}

#[derive(Debug)]
pub struct Animation {
    interval: Duration,
    duration: Duration,
    display: u32,
    windows: Vec<AnimatedWindow>,
    handled_windows: Vec<WindowId>,
}

#[derive(Debug)]
struct AnimatedWindow {
    handle: AppThreadHandle,
    wid: WindowId,
    start: CGRect,
    finish: CGRect,
    is_focus: bool,
    txid: TransactionId,
}

#[derive(Debug)]
pub(super) struct PreparedCamera {
    pub workspace: VirtualWorkspaceId,
    pub layout: LayoutId,
    pub state: Arc<Mutex<CameraAnimation>>,
}

#[derive(Debug)]
pub struct CameraAnimation {
    pub(super) presentation: ViewportPresentation,
    windows: Vec<PresentedWindow>,
    pub(super) gesture: Option<(Context, Control, f64, Duration)>,
    pub(super) active: bool,
    pub(super) paused: bool,
    display: u32,
    scale: f64,
    store: WindowTxStore,
    interval: Duration,
    bound: Option<CGRect>,
}

#[derive(Debug)]
struct PresentedWindow {
    handle: AppThreadHandle,
    wid: WindowId,
    wsid: Option<WindowServerId>,
    frame: CGRect,
}

impl CameraAnimation {
    pub(super) fn sample(&mut self, now: Instant) {
        let scale = self.scale;
        if !self.active || self.paused {
            return;
        }
        if let Some((context, control, total, time)) = &mut self.gesture {
            if !control.valid(context.epoch) {
                self.stop();
                return;
            }
            if let Some(motion) = control.latest(context.session)
                && motion.timestamp > *time
            {
                self.presentation.update(motion.total_x - *total, motion.timestamp);
                *total = motion.total_x;
                *time = motion.timestamp;
            }
        }
        let ongoing = self.presentation.sample(now, scale);
        let mut submitted = 0;
        for (window, &(_, frame, fixed)) in self.windows.iter_mut().zip(&self.presentation.frames) {
            let frame = self.presentation.frame(frame, fixed, scale);
            let frame = self.bound.map_or(frame, |screen| {
                super::managers::bound_frame_to_screen(frame, screen)
            });
            if frame.same_as(window.frame) {
                continue;
            }
            let txid = window
                .wsid
                .map_or_else(TransactionId::default, |wsid| self.store.next_frame(wsid, frame));
            window.handle.send_interactive_frame(
                window.wid,
                frame,
                !frame.size.same_as(window.frame.size),
                txid,
                FrameSource::Viewport,
            );
            window.frame = frame;
            submitted += 1;
        }
        trace!(
            frames_submitted = submitted,
            unchanged_frames = self.windows.len() - submitted,
            "camera frames"
        );
        if !ongoing {
            self.end();
        }
    }

    fn end(&mut self) {
        self.active = false;
        for window in &self.windows {
            let _ = window.handle.send(Request::EndWindowAnimation(window.wid));
        }
    }

    pub(super) fn stop(&mut self) {
        if self.active {
            self.active = false;
            for window in &self.windows {
                let through = window.handle.cancel_interactive_frame(window.wid);
                let _ = window.handle.send(Request::CancelWindowAnimation(window.wid, through));
                if let Some(wsid) = window.wsid {
                    self.store.clear_target(&wsid);
                }
            }
        }
    }
}

impl Reactor {
    pub(super) fn cancel_window_presentations(&mut self, mut windows: Vec<WindowId>) {
        if windows.is_empty() {
            return;
        }
        windows.sort_unstable();
        let mut frames = Vec::new();
        for camera in self.presentations.values() {
            let mut state = camera.state.lock();
            for window in state.windows.iter().filter(|w| windows.binary_search(&w.wid).is_ok()) {
                if state.active {
                    frames.push((window.wid, window.frame));
                }
                let through = window.handle.cancel_interactive_frame(window.wid);
                let _ = window.handle.send(Request::CancelWindowAnimation(window.wid, through));
            }
            state.windows.retain(|w| windows.binary_search(&w.wid).is_err());
            state
                .presentation
                .frames
                .retain(|(wid, _, _)| windows.binary_search(wid).is_err());
            if state.windows.is_empty() {
                state.active = false;
            }
        }
        if let Some(tx) = &self.animation_tx {
            frames.extend(tx.cancel(windows));
        }
        for (wid, frame) in frames {
            if let Some(window) = self.state.windows.window_mut(wid) {
                window.frame_monotonic = frame;
                if let Some(wsid) = window.info.sys_id {
                    self.transaction_manager.clear_target_for_window(wsid);
                }
            }
        }
    }

    /// Reconcile only at semantic boundaries, never on a display tick.
    pub(super) fn reconcile_presentations(&mut self) {
        for camera in self.presentations.values() {
            let state = camera.state.lock();
            if let Some(ws) = self
                .layout_manager
                .layout_engine
                .workspaces_mut()
                .workspaces
                .get_mut(camera.workspace)
                && let LayoutSystemKind::Scrolling(system) = &mut ws.layout_system
            {
                system.reconcile_presentation(camera.layout, &state.presentation);
            }
            for window in &state.windows {
                if let Some(model) = self.state.windows.window_mut(window.wid) {
                    model.frame_monotonic = window.frame;
                }
            }
            if let Some(session) = &mut self.viewport_gesture
                && session.workspace == camera.workspace
                && session.layout == camera.layout
                && let Some((_, _, total, time)) = &state.gesture
            {
                session.applied = *total;
                session.timestamp = *time;
            }
        }
        if self.viewport_gesture.as_ref().is_some_and(|s| {
            s.released
                && self.presentations.get(&s.context.space).is_some_and(|c| !c.state.lock().active)
        }) {
            self.retire_viewport_session();
        }
    }

    pub(super) fn retire_presentations(&mut self) {
        self.presentations.retain(|space, camera| {
            let valid =
                self.layout_manager.layout_engine.workspaces().active_layout_for_space(*space)
                    == Some((camera.workspace, camera.layout))
                    && self.active_spaces.contains(space)
                    && self.space_state.screen_by_space(*space).is_some()
                    && self
                        .layout_manager
                        .layout_engine
                        .workspaces()
                        .workspaces
                        .get(camera.workspace)
                        .is_some_and(|ws| matches!(&ws.layout_system, LayoutSystemKind::Scrolling(system) if system.contains_layout(camera.layout)))
                    && matches!(
                        self.mission_control_manager.mission_control_state,
                        super::MissionControlState::Inactive
                    );
            if !valid && let Some(tx) = &self.animation_tx {
                let _ = tx.send(Message::Stop(
                    camera.state.lock().windows.iter().map(|w| w.wid).collect(),
                    None,
                ));
            }
            if !valid {
                camera.state.lock().stop();
            }
            valid
        });
    }

    pub(super) fn present_camera(
        &mut self,
        space: SpaceId,
        animate: bool,
        gesture: Option<(Context, Control, f64, Duration)>,
        skip: Option<WindowId>,
    ) -> bool {
        if self.animation_tx.is_none() && gesture.is_none() && self.viewport_gesture.is_none() {
            return false;
        }
        let Some(screen) = self.space_state.screen_by_space(space) else {
            return false;
        };
        let display = screen.id.as_u32();
        let scale = screen.backing_scale;
        let bound = (self.active_spaces.len() > 1).then_some(screen.frame);
        let Some((workspace, layout)) =
            self.layout_manager.layout_engine.workspaces().active_layout_for_space(space)
        else {
            return false;
        };
        let LayoutSystemKind::Scrolling(system) =
            &self.layout_manager.layout_engine.workspaces()[workspace].layout_system
        else {
            return false;
        };
        let Some(mut presentation) = system.presentation(layout) else {
            return false;
        };
        let previous = self.presentations.remove(&space).filter(|previous| {
            let same = previous.workspace == workspace && previous.layout == layout;
            if !same {
                previous.state.lock().stop();
            }
            same
        });
        let gesture = gesture.or_else(|| {
            self.viewport_gesture
                .as_ref()
                .filter(|s| {
                    presentation.gesturing()
                        && !s.released
                        && s.workspace == workspace
                        && s.layout == layout
                })
                .map(|s| (s.context.clone(), s.control.clone(), s.applied, s.timestamp))
        });
        let now = Instant::now();
        if gesture.is_none()
            && animate
            && !presentation.animated()
            && let Some(previous) = &previous
        {
            let old = previous.state.lock();
            let (mut from, velocity) = old.presentation.position_velocity(now);
            // Anchor in world geometry, preserving screen position through edits.
            if let Some((wid, new, _)) = presentation
                .frames
                .iter()
                .find(|(wid, _, _)| old.presentation.frames.iter().any(|(old, _, _)| old == wid))
                && let Some((_, old_frame, _)) =
                    old.presentation.frames.iter().find(|(old, _, _)| old == wid)
            {
                from += new.origin.x - old_frame.origin.x;
            }
            presentation.retarget(from, velocity, now);
        }
        let mut windows = Vec::with_capacity(presentation.frames.len());
        presentation.frames.retain(|(wid, _, _)| {
            let Some(window) = self.state.windows.window(*wid) else {
                return false;
            };
            let Some(app) = self.app_manager.apps.get(&wid.pid) else {
                return false;
            };
            if Some(*wid) == skip {
                return false;
            }
            windows.push(PresentedWindow {
                handle: app.handle.clone(),
                wid: *wid,
                wsid: window.info.sys_id,
                frame: window.frame_monotonic,
            });
            true
        });
        if !animate {
            presentation.finish();
        }
        let next = CameraAnimation {
            presentation,
            windows,
            gesture,
            active: true,
            paused: false,
            display,
            scale,
            store: self.transaction_manager.store.clone(),
            interval: Duration::from_secs_f64(1.0 / self.config.settings.animation_fps),
            bound,
        };
        let state = if let Some(previous) = previous {
            let mut old = previous.state.lock();
            for window in &old.windows {
                if !next.windows.iter().any(|new| new.wid == window.wid) {
                    window.handle.cancel_interactive_frame(window.wid);
                    let _ = window.handle.send(Request::EndWindowAnimation(window.wid));
                }
            }
            *old = next;
            drop(old);
            previous.state
        } else {
            Arc::new(Mutex::new(next))
        };
        self.presentations.insert(space, PreparedCamera {
            workspace,
            layout,
            state: state.clone(),
        });
        let instant = !animate && state.lock().gesture.is_none();
        if instant && self.animation_tx.is_none() {
            state.lock().sample(now);
        } else if let Some(tx) = &self.animation_tx {
            let _ = tx.send(Message::Camera(state));
        }
        true
    }
}

impl AnimatedWindow {
    fn send_frame(&self, frame: CGRect, set_size: bool) {
        self.handle.send_interactive_frame(
            self.wid,
            frame,
            set_size,
            self.txid,
            FrameSource::Ordinary,
        );
    }

    fn begin(&self) {
        let _ = self.handle.send(Request::BeginWindowAnimation(self.wid));
        if self.is_focus {
            self.send_frame(self.frame_after(0.0), true);
        }
    }

    fn frame_after(&self, t: f64) -> CGRect {
        let mut rect = get_frame(self.start, self.finish, t);
        if self.is_focus || t >= 0.5 {
            rect.size = self.finish.size;
        } else {
            rect.size = self.start.size;
        }
        rect
    }
}

impl AnimationManager {
    pub fn new() -> Self { Self::default() }

    /// One runner for all displays. Commands are semantic work; display wakes
    /// are bounded to one permit and each link retains only its newest timing.
    pub fn run(rx: AnimationReceiver) {
        let (wake, ticks) = crossbeam_channel::bounded(1);
        let mut displays: HashMap<u32, DisplayPresenter> = HashMap::default();
        loop {
            let deadline = displays.values().map(|d| d.fallback_deadline).min();
            let timeout =
                deadline.map_or(Duration::MAX, |t| t.saturating_duration_since(Instant::now()));
            crossbeam_channel::select! {
                recv(rx) -> message => {
                    let Ok(message) = message else { break };
                    match message {
                        Message::Stop(mut windows, done) => {
                            windows.sort_unstable();
                            let mut frames = Vec::new();
                            for d in displays.values_mut() {
                                if done.is_some() { frames.extend(d.manager.presented_frames(&windows)); }
                                d.manager.stop_windows(&windows);
                            }
                            if let Some(done) = done { let _ = done.send(frames); }
                        }
                        message => {
                            let display = match &message {
                                Message::Replace(a) | Message::SkipToEnd(a) => a.display,
                                Message::Camera(c) => c.lock().display,
                                Message::Stop(..) => unreachable!(),
                            };
                            let entry = displays.entry(display).or_insert_with(|| DisplayPresenter {
                                manager: Self::new(), link: DisplayLink::for_display(display, wake.clone()),
                                sequence: 0, fallback_deadline: Instant::now(),
                            });
                            entry.manager.handle_message(message);
                        }
                    }
                }
                recv(ticks) -> _ => {}
                default(timeout) => {}
            }
            let now = Instant::now();
            for DisplayPresenter {
                manager,
                link,
                sequence,
                fallback_deadline: deadline,
            } in displays.values_mut()
            {
                let previous_sequence = *sequence;
                let native = link.latest(sequence);
                let sample = native
                    .map(|(tick, target)| {
                        trace!(
                            sequence = tick.sequence,
                            timestamp = tick.timestamp,
                            target_timestamp = tick.target_timestamp,
                            skipped_ticks =
                                tick.sequence.saturating_sub(previous_sequence).saturating_sub(1),
                            wake_timestamp = objc2_quartz_core::CACurrentMediaTime(),
                            lateness_us = now.saturating_duration_since(target).as_micros(),
                            "presentation sample"
                        );
                        target
                    })
                    .or_else(|| (now >= *deadline).then_some(now));
                if let Some(sample) = sample {
                    let measured = tracing::enabled!(tracing::Level::TRACE).then(Instant::now);
                    if let Some(camera) = &manager.camera {
                        camera.lock().sample(sample);
                    }
                    let delay = manager.tick_at(sample).unwrap_or_else(|| {
                        manager
                            .camera
                            .as_ref()
                            .map_or(Duration::from_secs_f64(1.0 / 60.0), |c| c.lock().interval)
                    });
                    // Resume fallback after three missing native refreshes.
                    *deadline = if let Some((tick, _)) = native {
                        sample
                            + Duration::try_from_secs_f64(
                                3.0 * (tick.target_timestamp - tick.timestamp),
                            )
                            .ok()
                            .filter(|d| !d.is_zero())
                            .unwrap_or(delay)
                            .min(Duration::from_millis(250))
                    } else {
                        now + delay
                    };
                    if let Some(started) = measured {
                        trace!(work_us = started.elapsed().as_micros(), "presentation work");
                    }
                }
            }
            displays.retain(|_, d| d.manager.has_work());
            trace!(active_displays = displays.len(), "presentation lifecycle");
        }
        for d in displays.values_mut() {
            d.manager.finish_active();
            if let Some(camera) = &d.manager.camera {
                camera.lock().stop();
            }
        }
    }

    fn has_work(&self) -> bool {
        self.active.is_some()
            || self.camera.as_ref().is_some_and(|c| {
                let c = c.lock();
                c.active && !c.paused
            })
    }

    fn stop_windows(&mut self, windows: &[WindowId]) {
        if let Some(camera) = &self.camera {
            let mut camera = camera.lock();
            for window in camera.windows.iter().filter(|w| windows.binary_search(&w.wid).is_ok()) {
                let through = window.handle.cancel_interactive_frame(window.wid);
                let _ = window.handle.send(Request::CancelWindowAnimation(window.wid, through));
            }
            camera.windows.retain(|w| windows.binary_search(&w.wid).is_err());
            camera
                .presentation
                .frames
                .retain(|(wid, _, _)| windows.binary_search(wid).is_err());
            if camera.windows.is_empty() {
                camera.active = false;
            }
        }
        if let Some(active) = &mut self.active {
            active.animation.windows.retain(|window| {
                if windows.binary_search(&window.wid).is_ok() {
                    let through = window.handle.cancel_interactive_frame(window.wid);
                    _ = window.handle.send(Request::CancelWindowAnimation(window.wid, through));
                    false
                } else {
                    true
                }
            });
            if active.animation.is_empty() {
                self.active = None;
            }
        }
    }

    fn presented_frames(&self, windows: &[WindowId]) -> Vec<(WindowId, CGRect)> {
        let mut frames = Vec::new();
        if let Some(active) = &self.active {
            frames.extend(
                active
                    .animation
                    .windows
                    .iter()
                    .filter(|w| windows.binary_search(&w.wid).is_ok())
                    .map(|w| (w.wid, w.frame_after(active.progress))),
            );
        }
        if let Some(camera) = &self.camera {
            let camera = camera.lock();
            if camera.active {
                frames.extend(
                    camera
                        .windows
                        .iter()
                        .filter(|w| windows.binary_search(&w.wid).is_ok())
                        .map(|w| (w.wid, w.frame)),
                );
            }
        }
        frames
    }

    pub fn handle_message(&mut self, message: Message) -> Option<Duration> {
        match message {
            Message::Camera(camera) => {
                let previous = self.camera.take();
                let windows = camera.lock().windows.iter().map(|w| w.wid).collect();
                self.handle_message(Message::Stop(windows, None));
                if let Some(old) = previous
                    && !Arc::ptr_eq(&old, &camera)
                {
                    old.lock().stop();
                }
                self.camera = Some(camera);
                Some(Duration::ZERO)
            }
            Message::Stop(mut windows, done) => {
                windows.sort_unstable();
                let frames = done.as_ref().map(|_| self.presented_frames(&windows));
                self.stop_windows(&windows);
                if let Some(done) = done {
                    let _ = done.send(frames.unwrap());
                }
                self.active.as_ref().map(|active| active.animation.interval)
            }
            Message::Replace(animation) => {
                self.active = match self.active.take() {
                    Some(active) => Some(active.replace_with(animation)),
                    None => ActiveAnimation::start(animation),
                };
                self.active.as_ref().map(|active| active.animation.interval)
            }
            Message::SkipToEnd(animation) => {
                self.finish_active();
                animation.skip_to_end();
                None
            }
        }
    }

    pub fn tick_at(&mut self, now: Instant) -> Option<Duration> {
        let active = self.active.as_mut()?;
        // Independently rounded refresh/FPS intervals can differ by nanoseconds.
        if now + Duration::from_micros(1) < active.next_sample
            && now.saturating_duration_since(active.started) < active.animation.duration
        {
            return Some(active.next_sample - now);
        }
        let interval = active.animation.interval;
        let missed =
            now.saturating_duration_since(active.next_sample).as_nanos() / interval.as_nanos();
        active.next_sample += interval.mul_f64((missed + 1) as f64);
        active.send_frame(now);
        if active.is_complete() {
            let active = self.active.take().expect("animation disappeared while ticking");
            active.animation.end();
            None
        } else {
            Some(active.animation.interval)
        }
    }

    fn finish_active(&mut self) {
        if let Some(active) = self.active.take() {
            active.animation.finish_all();
        }
    }

    pub fn animate_layout(
        reactor: &mut Reactor,
        space: SpaceId,
        layout: &[(WindowId, CGRect)],
        is_resize: bool,
        skip_wid: Option<WindowId>,
    ) -> bool {
        reactor.retire_presentations();
        let setting = reactor.layout_manager.layout_engine.layout_specific_animate_settings(space);
        let animate_camera = !is_resize
            && setting.unwrap_or(reactor.config.settings.animate)
            && !(setting.is_none() && power::is_low_power_mode_enabled());
        let camera = reactor.present_camera(space, animate_camera, None, skip_wid);
        let presented: HashSet<_> = reactor
            .presentations
            .get(&space)
            .filter(|_| camera)
            .map(|c| c.state.lock().windows.iter().map(|w| w.wid).collect())
            .unwrap_or_default();
        if !animate_camera {
            let windows = layout
                .iter()
                .map(|(wid, _)| *wid)
                .filter(|wid| !presented.contains(wid))
                .collect();
            reactor.cancel_window_presentations(windows);
        }
        let Some(active_ws) =
            reactor.layout_manager.layout_engine.workspaces().active_workspace(space)
        else {
            return false;
        };
        let mut anim = Animation::new(
            reactor.config.settings.animation_fps,
            reactor.config.settings.animation_duration,
        );
        let Some(screen) = reactor.space_state.screen_by_space(space) else {
            return false;
        };
        anim.display = screen.id.as_u32();
        let mut animated_count = 0;
        let mut any_frame_changed = camera;

        for &(wid, target_frame) in layout {
            if presented.contains(&wid) {
                anim.mark_handled(wid);
                continue;
            }
            if skip_wid == Some(wid) {
                anim.mark_handled(wid);
                trace!(
                    ?wid,
                    "Skipping animated layout update for window currently being dragged"
                );
                continue;
            }

            let target_frame = target_frame.round();
            let (current_frame, window_server_id, txid) = {
                let window_store = &mut reactor.state.windows;
                match window_store.window_mut(wid) {
                    Some(window) => {
                        let current_frame = window.frame_monotonic;
                        let wsid = window.info.sys_id;
                        let pending_target = wsid
                            .and_then(|wsid| reactor.transaction_manager.get_target_frame(wsid));
                        // An observed intermediate frame may already match this layout,
                        // while an older animation is still headed somewhere else.
                        if target_frame.same_as(current_frame)
                            && pending_target.is_none_or(|pending| pending.same_as(target_frame))
                        {
                            continue;
                        }
                        if pending_target.is_some_and(|pending| pending.same_as(target_frame)) {
                            trace!(?wid, ?target_frame, "Skipping redundant layout request");
                            continue;
                        }
                        any_frame_changed = true;
                        let txid = wsid
                            .map(|wsid| reactor.transaction_manager.generate_next_txid(wsid))
                            .unwrap_or_default();
                        (current_frame, wsid, txid)
                    }
                    None => {
                        debug!(?wid, "Skipping - window no longer exists");
                        continue;
                    }
                }
            };

            let Some(app_state) = &reactor.app_manager.apps.get(&wid.pid) else {
                debug!(?wid, "Skipping for window - app no longer exists");
                continue;
            };

            let is_active = reactor
                .state
                .windows
                .workspace_for_window(space, wid)
                .is_some_and(|ws| ws == active_ws);

            if is_active {
                trace!(?wid, ?current_frame, ?target_frame, "Animating visible window");
                anim.add_window(&app_state.handle, wid, current_frame, target_frame, false, txid);
                animated_count += 1;
                if let Some(wsid) = window_server_id {
                    reactor.transaction_manager.update_txid_entries([(wsid, txid, target_frame)]);
                }
            } else {
                anim.mark_handled(wid);
                trace!(
                    ?wid,
                    ?current_frame,
                    ?target_frame,
                    "Direct positioning hidden window"
                );
                if let Some(wsid) = window_server_id {
                    reactor.transaction_manager.update_txid_entries([(wsid, txid, target_frame)]);
                }
                if let Err(e) =
                    app_state.handle.send(Request::SetWindowFrame(wid, target_frame, txid, true))
                {
                    debug!(?wid, ?e, "Failed to send frame request for hidden window");
                    continue;
                }
            }

            if let Some(window) = reactor.state.windows.window_mut(wid) {
                window.frame_monotonic = target_frame;
            }
        }

        if animated_count > 0 {
            if let Some(tx) = &reactor.animation_tx {
                let message = if !animate_camera {
                    Message::SkipToEnd(anim)
                } else {
                    Message::Replace(anim)
                };
                if let Err(err) = tx.send(message) {
                    match err.0 {
                        Message::Replace(animation) => animation.skip_to_end(),
                        Message::SkipToEnd(animation) => animation.skip_to_end(),
                        Message::Stop(..) | Message::Camera(_) => {}
                    }
                }
            } else {
                anim.skip_to_end();
            }
        }

        any_frame_changed
    }

    pub fn instant_layout(
        reactor: &mut Reactor,
        space: SpaceId,
        layout: &[(WindowId, CGRect)],
        skip_wid: Option<WindowId>,
    ) -> bool {
        Self::instant_layout_inner(reactor, space, layout, skip_wid, false)
    }

    /// Apply the position-only layout used while switching virtual workspaces.
    ///
    /// Keep this entry point separate from `instant_layout`: layouts merely suppressed
    /// while a switch is in progress may still change window sizes and must use the
    /// full-frame request.
    pub fn workspace_switch_layout(
        reactor: &mut Reactor,
        space: SpaceId,
        layout: &[(WindowId, CGRect)],
        skip_wid: Option<WindowId>,
    ) -> bool {
        Self::instant_layout_inner(reactor, space, layout, skip_wid, true)
    }

    fn instant_layout_inner(
        reactor: &mut Reactor,
        _space: SpaceId,
        layout: &[(WindowId, CGRect)],
        skip_wid: Option<WindowId>,
        position_only: bool,
    ) -> bool {
        reactor.cancel_window_presentations(layout.iter().map(|(wid, _)| *wid).collect());
        let mut per_app: HashMap<pid_t, Vec<(WindowId, CGRect, bool)>> = HashMap::default();
        let mut any_frame_changed = false;

        for &(wid, target_frame) in layout {
            if skip_wid == Some(wid) {
                trace!(?wid, "Skipping layout update for window currently being dragged");
                continue;
            }

            let window_store = &mut reactor.state.windows;
            let Some(window) = window_store.window_mut(wid) else {
                debug!(?wid, "Skipping layout - window no longer exists");
                continue;
            };
            let target_frame = target_frame.round();
            let current_frame = window.frame_monotonic;
            if target_frame.same_as(current_frame) {
                continue;
            }
            if let Some(wsid) = window.info.sys_id
                && reactor
                    .transaction_manager
                    .get_target_frame(wsid)
                    .is_some_and(|pending| pending.same_as(target_frame))
            {
                trace!(?wid, ?target_frame, "Skipping redundant instant layout request");
                continue;
            }
            any_frame_changed = true;
            trace!(
                ?wid,
                ?current_frame,
                ?target_frame,
                "Instant workspace positioning"
            );

            let size_unchanged = current_frame.size.same_as(target_frame.size);
            window.frame_monotonic = target_frame;
            per_app.entry(wid.pid).or_default().push((wid, target_frame, size_unchanged));
        }

        for (pid, frames) in per_app {
            let Some(app_state) = reactor.app_manager.apps.get(&pid) else {
                debug!(?pid, "Skipping layout update for app - app no longer exists");
                continue;
            };

            let handle = &app_state.handle;

            let txid = frames
                .iter()
                .find_map(|(wid, _, _)| reactor.state.windows.window(*wid)?.info.sys_id)
                .map(|wsid| reactor.transaction_manager.generate_next_txid(wsid))
                .unwrap_or_default();
            for (wid, frame, _) in &frames {
                if let Some(wsid) = reactor.state.windows.window(*wid).and_then(|w| w.info.sys_id) {
                    reactor.transaction_manager.store_txid(wsid, txid, *frame);
                }
            }

            let requests = if position_only {
                let mut positions = Vec::new();
                let mut full_frames = Vec::new();
                for (wid, frame, size_unchanged) in frames {
                    if size_unchanged {
                        positions.push((wid, frame.origin));
                    } else {
                        full_frames.push((wid, frame));
                    }
                }

                [
                    (!positions.is_empty())
                        .then(|| Request::SetWorkspaceSwitchPositions(positions, txid, true)),
                    (!full_frames.is_empty())
                        .then(|| Request::SetBatchWindowFrame(full_frames, txid, true)),
                ]
            } else {
                [
                    Some(Request::SetBatchWindowFrame(
                        frames.into_iter().map(|(wid, frame, _)| (wid, frame)).collect(),
                        txid,
                        true,
                    )),
                    None,
                ]
            };
            for request in requests.into_iter().flatten() {
                if let Err(e) = handle.send(request) {
                    debug!(
                        ?pid,
                        ?e,
                        "Failed to send instant layout request - app may have quit"
                    );
                    break;
                }
            }
        }

        any_frame_changed
    }
}

impl ActiveAnimation {
    fn start(animation: Animation) -> Option<Self> {
        if animation.is_empty() {
            return None;
        }
        animation.begin();
        let started = Instant::now();
        let next_sample = started + animation.interval;
        Some(Self {
            animation,
            started,
            progress: 0.0,
            next_sample,
        })
    }

    fn replace_with(self, mut next: Animation) -> Self {
        for window in &mut next.windows {
            if let Some(old) = self.animation.windows.iter().find(|old| old.wid == window.wid) {
                window.start = old.frame_after(self.progress);
            } else {
                window.begin();
            }
        }
        for mut old in self.animation.windows {
            if !next.handled_windows.contains(&old.wid) {
                old.start = old.frame_after(self.progress);
                next.windows.push(old);
            } else if !next.windows.iter().any(|w| w.wid == old.wid) {
                let through = old.handle.cancel_interactive_frame(old.wid);
                let _ = old.handle.send(Request::CancelWindowAnimation(old.wid, through));
            }
        }
        let started = Instant::now();
        let next_sample = started + next.interval;
        Self {
            animation: next,
            started,
            progress: 0.0,
            next_sample,
        }
    }

    fn send_frame(&mut self, now: Instant) {
        let t = if self.animation.duration.is_zero() {
            1.0
        } else {
            (now.saturating_duration_since(self.started).as_secs_f64()
                / self.animation.duration.as_secs_f64())
            .clamp(0.0, 1.0)
        };
        let t = t.max(self.progress);
        self.animation.send_frame(t, self.progress);
        self.progress = t;
    }

    fn is_complete(&self) -> bool { self.progress >= 1.0 }
}

impl Animation {
    fn new(fps: f64, duration: f64) -> Self {
        let interval = Duration::from_secs_f64(1.0 / fps);
        Self {
            interval,
            duration: Duration::from_secs_f64(duration),
            display: 0,
            windows: vec![],
            handled_windows: vec![],
        }
    }

    pub fn add_window(
        &mut self,
        handle: &AppThreadHandle,
        wid: WindowId,
        start: CGRect,
        finish: CGRect,
        is_focus: bool,
        txid: TransactionId,
    ) {
        self.windows.push(AnimatedWindow {
            handle: handle.clone(),
            wid,
            start,
            finish,
            is_focus,
            txid,
        });
        self.mark_handled(wid);
    }

    fn mark_handled(&mut self, wid: WindowId) {
        if !self.handled_windows.contains(&wid) {
            self.handled_windows.push(wid);
        }
    }

    pub fn skip_to_end(&self) {
        for window in &self.windows {
            _ = window.handle.send(Request::SetWindowFrame(
                window.wid,
                window.finish,
                window.txid,
                true,
            ));
        }
    }

    pub fn is_empty(&self) -> bool { self.windows.is_empty() }

    fn begin(&self) {
        for window in &self.windows {
            window.begin();
        }
    }

    fn finish_all(&self) {
        for window in &self.windows {
            window.handle.cancel_interactive_frame(window.wid);
            let _ = window.handle.send(Request::AnimationFrame {
                wid: window.wid,
                frame: window.finish,
                set_size: true,
                txid: window.txid,
            });
            _ = window.handle.send(Request::EndWindowAnimation(window.wid));
        }
    }

    fn send_frame(&self, t: f64, previous: f64) {
        let mut submitted = 0;
        for window in &self.windows {
            let rect = window.frame_after(t);
            let set_size = (previous < 0.5 && t >= 0.5) || t == 1.0;
            if rect != window.frame_after(previous) || set_size {
                window.send_frame(rect, set_size);
                submitted += 1;
            }
        }
        trace!(frames_submitted = submitted, "generic frames");
    }

    fn end(&self) {
        for window in &self.windows {
            _ = window.handle.send(Request::EndWindowAnimation(window.wid));
        }
    }
}

fn get_frame(a: CGRect, b: CGRect, t: f64) -> CGRect {
    let s = ease(t);
    CGRect {
        origin: CGPoint {
            x: blend(a.origin.x, b.origin.x, s),
            y: blend(a.origin.y, b.origin.y, s),
        },
        size: CGSize {
            width: blend(a.size.width, b.size.width, s),
            height: blend(a.size.height, b.size.height, s),
        },
    }
}

fn ease(t: f64) -> f64 {
    if t < 0.5 {
        (1.0 - f64::sqrt(1.0 - f64::powi(2.0 * t, 2))) / 2.0
    } else {
        (f64::sqrt(1.0 - f64::powi(-2.0 * t + 2.0, 2)) + 1.0) / 2.0
    }
}

fn blend(a: f64, b: f64, s: f64) -> f64 { (1.0 - s) * a + s * b }

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGSize};

    use super::*;

    fn rect(origin_x: f64, origin_y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(CGPoint::new(origin_x, origin_y), CGSize::new(width, height))
    }

    fn empty_animation() -> Animation {
        let settings = crate::common::config::Config::default().settings;
        Animation::new(settings.animation_fps, settings.animation_duration)
    }

    fn animation(handle: &AppThreadHandle, wid: WindowId, from: CGRect, to: CGRect) -> Animation {
        let mut animation = empty_animation();
        animation.add_window(handle, wid, from, to, false, TransactionId::default());
        animation
    }

    fn collect_requests(rx: &mut crate::actor::Receiver<Request>) -> Vec<Request> {
        let mut requests = Vec::new();
        while let Ok((_, request)) = rx.try_recv() {
            if let Request::InteractiveFramesPending(queue) = request {
                queue.drain_with(|wid, frame, set_size, txid, _, _| {
                    requests.push(Request::AnimationFrame { wid, frame, set_size, txid })
                });
            } else {
                requests.push(request);
            }
        }
        requests
    }

    fn assert_set_window_frame(request: &Request, wid: WindowId, frame: CGRect) {
        match request {
            Request::SetWindowFrame(req_wid, req_frame, txid, eui) => {
                assert_eq!(*req_wid, wid);
                assert_eq!(*req_frame, frame);
                assert_eq!(*txid, TransactionId::default());
                assert!(*eui);
            }
            _ => panic!("expected SetWindowFrame, got {request:?}"),
        }
    }

    fn assert_animation_frame(request: &Request, wid: WindowId, frame: CGRect) {
        match request {
            Request::AnimationFrame {
                wid: req_wid,
                frame: req_frame,
                set_size,
                txid,
            } => {
                assert_eq!(*req_wid, wid);
                assert_eq!(*req_frame, frame);
                assert!(*set_size, "expected a set_size frame");
                assert_eq!(*txid, TransactionId::default());
            }
            _ => panic!("expected AnimationFrame, got {request:?}"),
        }
    }

    fn assert_animation_pos(request: &Request, wid: WindowId, pos: CGPoint) {
        match request {
            Request::AnimationFrame {
                wid: req_wid,
                frame,
                set_size,
                txid,
            } => {
                assert_eq!(*req_wid, wid);
                assert_eq!(frame.origin, pos);
                assert!(!*set_size, "expected a position-only frame");
                assert_eq!(*txid, TransactionId::default());
            }
            _ => panic!("expected AnimationFrame, got {request:?}"),
        }
    }

    #[test]
    fn native_and_fallback_sampling_use_elapsed_time_and_finish_exactly() {
        let start = Instant::now();
        let mut results = Vec::new();
        for fps in [60.0, 120.0] {
            let (tx, mut rx) = crate::actor::channel();
            let handle = AppThreadHandle::new_for_test(tx);
            let mut a = Animation::new(fps, 0.3);
            a.add_window(
                &handle,
                WindowId::new(1, 1),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(100.0, 50.0, 20.0, 30.0),
                false,
                TransactionId::default(),
            );
            let mut manager = AnimationManager::new();
            manager.handle_message(Message::Replace(a));
            manager.active.as_mut().unwrap().started = start;
            collect_requests(&mut rx);
            let mut frames = Vec::new();
            for ms in [40, 150, 300] {
                manager.tick_at(start + Duration::from_millis(ms));
                frames.extend(collect_requests(&mut rx).into_iter().filter_map(|r| match r {
                    Request::AnimationFrame { frame, .. } => Some(frame),
                    _ => None,
                }));
                // A duplicate wake must not advance time or replay a frame.
                manager.tick_at(start + Duration::from_millis(ms));
                assert!(collect_requests(&mut rx).is_empty());
            }
            assert_eq!(frames.len(), 3);
            assert_eq!(frames[1].origin, CGPoint::new(50.0, 25.0));
            assert_eq!(frames[2], rect(100.0, 50.0, 20.0, 30.0));
            assert!(manager.active.is_none());
            results.push(frames);
        }
        assert_eq!(results[0], results[1]);
    }

    #[test]
    fn native_ticks_gate_generic_fps_skip_missed_intervals_and_keep_displays_independent() {
        let start = Instant::now();
        for (fps, expected) in [(60.0, 60), (30.0, 30)] {
            let (handle, mut rx) = AppThreadHandle::channel();
            let mut a = Animation::new(fps, 1.0);
            let wid = WindowId::new(1, 1);
            a.add_window(
                &handle,
                wid,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(1000.0, 0.0, 10.0, 10.0),
                false,
                TransactionId::default(),
            );
            let mut manager = AnimationManager::new();
            manager.handle_message(Message::Replace(a));
            let active = manager.active.as_mut().unwrap();
            active.started = start;
            active.next_sample = start + active.animation.interval;
            collect_requests(&mut rx);
            let mut count = 0;
            for tick in 1..=120 {
                manager.tick_at(start + Duration::from_secs_f64(tick as f64 / 120.0));
                count += collect_requests(&mut rx)
                    .iter()
                    .filter(|r| matches!(r, Request::AnimationFrame { .. }))
                    .count();
            }
            assert_eq!(count, expected);
            assert!(!manager.has_work());
        }
        let (handle, mut rx) = AppThreadHandle::channel();
        let wid = WindowId::new(1, 1);
        let mut a = animation(
            &handle,
            wid,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(1000.0, 0.0, 10.0, 10.0),
        );
        a.interval = Duration::from_millis(20);
        a.duration = Duration::from_secs(1);
        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(a));
        let active = manager.active.as_mut().unwrap();
        active.started = start;
        active.next_sample = start;
        collect_requests(&mut rx);
        manager.tick_at(start + Duration::from_millis(200));
        let frames = collect_requests(&mut rx);
        assert_eq!(frames.len(), 1, "missed frames must not replay");
        let Request::AnimationFrame { frame: last, .. } = frames[0] else {
            panic!("frame");
        };
        manager.handle_message(Message::Replace(animation(
            &handle,
            wid,
            last,
            rect(2000.0, 0.0, 10.0, 10.0),
        )));
        assert_eq!(manager.active.as_ref().unwrap().animation.windows[0].start, last);
        manager.tick_at(manager.active.as_ref().unwrap().next_sample - Duration::from_millis(1));
        assert!(
            collect_requests(&mut rx).is_empty(),
            "old timestamps cannot sample a new transition"
        );
    }

    #[test]
    fn generic_camera_drag_handoffs_fence_old_frames_and_retire_work() {
        use crate::layout_engine::systems::ScrollingLayoutSystem;
        let (handle, mut rx) = AppThreadHandle::channel();
        let wid = WindowId::new(1, 1);
        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(animation(
            &handle,
            wid,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(100.0, 0.0, 10.0, 10.0),
        )));
        collect_requests(&mut rx);
        let now = manager.active.as_ref().unwrap().started + Duration::from_millis(100);
        manager.tick_at(now);
        let Request::InteractiveFramesPending(old_wake) = rx.try_recv().unwrap().1 else {
            panic!("wake");
        };
        let mut system = ScrollingLayoutSystem::new(&Default::default());
        let layout = system.create_layout();
        system.add_window_after_selection(layout, wid);
        system.prepare_layout(
            layout,
            rect(0.0, 0.0, 1000.0, 800.0),
            &Default::default(),
            &Default::default(),
        );
        system.begin_viewport_gesture(layout, now);
        let camera = Arc::new(Mutex::new(CameraAnimation {
            presentation: system.presentation(layout).unwrap(),
            windows: vec![PresentedWindow {
                handle: handle.clone(),
                wid,
                wsid: None,
                frame: rect(0.0, 0.0, 10.0, 10.0),
            }],
            gesture: None,
            active: true,
            paused: false,
            display: 1,
            scale: 2.0,
            store: WindowTxStore::new(),
            interval: Duration::from_secs_f64(1.0 / 60.0),
            bound: None,
        }));
        manager.handle_message(Message::Camera(camera.clone()));
        assert!(manager.active.is_none());
        let Request::CancelWindowAnimation(_, through) = rx.try_recv().unwrap().1 else {
            panic!("cancel");
        };
        // A native camera frame reuses the old outstanding generic wake.
        camera.lock().presentation.frames[0].1.origin.x += 0.5;
        camera.lock().sample(now);
        assert!(rx.try_recv().is_err());
        old_wake.drain_with(|id, frame, _, _, source, sequence| {
            assert_eq!(id, wid);
            assert_eq!(source, FrameSource::Viewport);
            assert!(sequence > through);
            assert_eq!(
                frame.origin.x.fract().abs(),
                0.5,
                "camera must use its cached 2x scale"
            );
        });
        // Camera work remains eligible on consecutive 120Hz input refreshes.
        for tick in 1..=2 {
            let mut camera = camera.lock();
            camera.presentation.update(0.001, Duration::from_millis(tick * 8));
            camera.sample(now + Duration::from_secs_f64(tick as f64 / 120.0));
            drop(camera);
            assert_eq!(collect_requests(&mut rx).len(), 1);
        }
        camera.lock().presentation.update(0.001, Duration::from_millis(30));
        camera.lock().sample(now + Duration::from_millis(30));
        let Request::InteractiveFramesPending(old_wake) = rx.try_recv().unwrap().1 else {
            panic!("wake");
        };
        manager.handle_message(Message::Stop(vec![wid], None));
        let Request::CancelWindowAnimation(_, through) = rx.try_recv().unwrap().1 else {
            panic!("cancel");
        };
        let dragged = rect(12.0, 34.0, 10.0, 10.0);
        handle.send_interactive_frame(
            wid,
            dragged,
            false,
            TransactionId::default(),
            FrameSource::Drag,
        );
        camera.lock().sample(now + Duration::from_secs(1));
        assert!(!manager.has_work());
        assert!(rx.try_recv().is_err());
        old_wake.drain_with(|_, frame, _, _, source, sequence| {
            assert_eq!(frame, dragged);
            assert_eq!(source, FrameSource::Drag);
            assert!(sequence > through);
        });
        system.cancel_viewport_gesture(layout);
        {
            let mut c = camera.lock();
            c.presentation = system.presentation(layout).unwrap();
            c.presentation.frames[0].1.origin.x += 0.5;
            c.windows.push(PresentedWindow {
                handle,
                wid,
                wsid: None,
                frame: dragged,
            });
            c.active = true;
            c.sample(now);
            assert!(!c.active, "instant target must retire immediately");
        }
        let requests = collect_requests(&mut rx);
        let Request::AnimationFrame { frame, .. } = requests[0] else {
            panic!("frame");
        };
        assert_eq!(frame.origin.x.fract().abs(), 0.5);
        assert!(matches!(requests[1], Request::EndWindowAnimation(id) if id == wid));
    }

    #[test]
    fn cancellation_discards_pending_frames_and_does_not_replay_the_target() {
        let (tx, mut rx) = crate::actor::channel();
        let handle = AppThreadHandle::new_for_test(tx);
        let wid = WindowId::new(1, 1);
        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(animation(
            &handle,
            wid,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(100.0, 50.0, 10.0, 10.0),
        )));
        collect_requests(&mut rx);
        let now = manager.active.as_ref().unwrap().started + Duration::from_millis(100);
        manager.tick_at(now);
        let (done, ack) = crossbeam_channel::bounded(1);
        manager.handle_message(Message::Stop(vec![wid], Some(done)));
        let acknowledged = ack.try_recv().expect("cancel must acknowledge after retirement");
        assert_eq!(acknowledged.len(), 1);
        assert_eq!(acknowledged[0].0, wid);
        assert!(acknowledged[0].1.origin.x > 0.0 && acknowledged[0].1.origin.x < 100.0);
        let requests = collect_requests(&mut rx);
        assert!(
            matches!(requests.as_slice(), [Request::CancelWindowAnimation(id, _)] if *id == wid)
        );
        assert!(manager.tick_at(now + Duration::from_secs(1)).is_none());
        assert!(collect_requests(&mut rx).is_empty());
    }

    #[test]
    fn replacement_uses_last_animated_frame_for_continuing_windows() {
        let (tx, mut rx) = crate::actor::channel();
        let handle = AppThreadHandle::new_for_test(tx);
        let wid = WindowId::new(1, 1);
        let first = animation(
            &handle,
            wid,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(50.0, 60.0, 10.0, 10.0),
        );
        let second = animation(
            &handle,
            wid,
            rect(50.0, 60.0, 10.0, 10.0),
            rect(80.0, 90.0, 10.0, 10.0),
        );

        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(first));
        assert!(matches!(
            collect_requests(&mut rx).as_slice(),
            [Request::BeginWindowAnimation(req_wid)] if *req_wid == wid
        ));

        manager.tick_at(manager.active.as_ref().unwrap().started + Duration::from_millis(10));
        let active = manager.active.as_ref().unwrap();
        let continuing_frame = active.animation.windows[0].frame_after(active.progress);
        assert_animation_pos(&collect_requests(&mut rx)[0], wid, continuing_frame.origin);

        manager.handle_message(Message::Replace(second));
        assert!(collect_requests(&mut rx).is_empty());

        let resumed_start = manager.active.as_ref().unwrap().animation.windows[0].start;
        assert_eq!(resumed_start, continuing_frame);

        manager.tick_at(manager.active.as_ref().unwrap().started + Duration::from_millis(10));
        let expected_next = get_frame(resumed_start, rect(80.0, 90.0, 10.0, 10.0), 1.0 / 30.0);
        assert_animation_pos(&collect_requests(&mut rx)[0], wid, expected_next.origin);
    }

    fn animation_contains(manager: &AnimationManager, wid: WindowId) -> bool {
        manager
            .active
            .as_ref()
            .is_some_and(|active| active.animation.windows.iter().any(|w| w.wid == wid))
    }

    #[test]
    fn replacement_only_restarts_changed_windows() {
        let (tx, mut rx) = crate::actor::channel();
        let handle = AppThreadHandle::new_for_test(tx);
        let wid1 = WindowId::new(1, 1);
        let wid2 = WindowId::new(1, 2);
        let wid3 = WindowId::new(1, 3);
        let mut first = empty_animation();
        first.add_window(
            &handle,
            wid1,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(50.0, 60.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        first.add_window(
            &handle,
            wid2,
            rect(10.0, 0.0, 10.0, 10.0),
            rect(60.0, 60.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        let mut second = empty_animation();
        second.add_window(
            &handle,
            wid1,
            rect(50.0, 60.0, 10.0, 10.0),
            rect(80.0, 90.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        second.add_window(
            &handle,
            wid3,
            rect(20.0, 0.0, 10.0, 10.0),
            rect(90.0, 90.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );

        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(first));
        assert_eq!(collect_requests(&mut rx).len(), 2);
        manager.handle_message(Message::Replace(second));

        let requests = collect_requests(&mut rx);
        assert_eq!(requests.len(), 1);
        assert!(matches!(requests[0], Request::BeginWindowAnimation(req_wid) if req_wid == wid3));
        assert!(animation_contains(&manager, wid2));

        let carried = manager
            .active
            .as_ref()
            .unwrap()
            .animation
            .windows
            .iter()
            .find(|w| w.wid == wid2)
            .unwrap();
        assert_eq!(carried.finish, rect(60.0, 60.0, 10.0, 10.0));
    }

    #[test]
    fn replacement_does_not_carry_over_explicitly_handled_windows() {
        let (tx, mut rx) = crate::actor::channel();
        let handle = AppThreadHandle::new_for_test(tx);
        let wid1 = WindowId::new(1, 1);
        let wid2 = WindowId::new(1, 2);
        let mut first = empty_animation();
        first.add_window(
            &handle,
            wid1,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(50.0, 60.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        first.add_window(
            &handle,
            wid2,
            rect(10.0, 0.0, 10.0, 10.0),
            rect(60.0, 60.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        let mut second = empty_animation();
        second.add_window(
            &handle,
            wid1,
            rect(50.0, 60.0, 10.0, 10.0),
            rect(80.0, 90.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        second.mark_handled(wid2);

        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(first));
        let _ = collect_requests(&mut rx);
        manager.handle_message(Message::Replace(second));

        assert!(!animation_contains(&manager, wid2));
    }

    #[test]
    fn skip_to_end_finishes_active_animation_and_applies_new_layout() {
        let (tx, mut rx) = crate::actor::channel();
        let handle = AppThreadHandle::new_for_test(tx);
        let wid = WindowId::new(1, 1);
        let first = animation(
            &handle,
            wid,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(50.0, 60.0, 10.0, 10.0),
        );
        let second = animation(
            &handle,
            wid,
            rect(50.0, 60.0, 10.0, 10.0),
            rect(80.0, 90.0, 10.0, 10.0),
        );

        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(first));
        manager.handle_message(Message::SkipToEnd(second));

        let requests = collect_requests(&mut rx);
        assert_eq!(requests.len(), 4);
        assert!(matches!(requests[0], Request::BeginWindowAnimation(req_wid) if req_wid == wid));
        assert_animation_frame(&requests[1], wid, rect(50.0, 60.0, 10.0, 10.0));
        assert!(matches!(requests[2], Request::EndWindowAnimation(req_wid) if req_wid == wid));
        assert_set_window_frame(&requests[3], wid, rect(80.0, 90.0, 10.0, 10.0));
    }
}
