use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
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
    finished_tx: Sender<CameraAnimation>,
    finished: Receiver<CameraAnimation>,
}
impl From<Sender<Message>> for AnimationSender {
    fn from(tx: Sender<Message>) -> Self {
        let (finished_tx, finished) = crossbeam_channel::unbounded();
        Self { tx, finished_tx, finished }
    }
}
impl AnimationSender {
    pub fn send(&self, message: Message) -> Result<(), crossbeam_channel::SendError<Message>> {
        self.tx.send(message)
    }

    pub fn cancel(&self, windows: Vec<WindowId>) -> Vec<(WindowId, CGRect)> {
        let (done, wait) = crossbeam_channel::bounded(1);
        if self.send(Message::Stop(windows, Some(done))).is_ok() {
            wait.recv().unwrap_or_default()
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
    Camera(Box<CameraAnimation>),
    Snapshot(Option<SpaceId>, bool, Sender<Vec<CameraAnimation>>),
}

#[derive(Debug, Default)]
pub struct AnimationManager {
    active: Option<ActiveAnimation>,
    camera: Option<CameraAnimation>,
}

struct DisplayPresenter {
    manager: AnimationManager,
    link: DisplayLink,
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
    txid: TransactionId,
}

#[derive(Clone, Debug)]
pub(super) struct CameraIdentity {
    pub space: SpaceId,
    pub workspace: VirtualWorkspaceId,
    pub layout: LayoutId,
    pub generation: Instant,
}

#[derive(Debug)]
pub(super) struct CameraRegistration {
    pub identity: CameraIdentity,
    // Frozen semantic handoffs (and headless operation) own state by value.
    pub frozen: Option<CameraAnimation>,
}

#[derive(Clone, Debug)]
pub struct CameraAnimation {
    pub(super) presentation: ViewportPresentation,
    windows: Vec<PresentedWindow>,
    pub(super) gesture: Option<(Context, Control, f64, Duration)>,
    pub(super) active: bool,
    identity: CameraIdentity,
    events: Option<super::Sender>,
    finished: Option<Sender<CameraAnimation>>,
    display: u32,
    animate: bool,
    scale: f64,
    store: WindowTxStore,
    interval: Duration,
    bound: Option<CGRect>,
}

#[derive(Clone, Debug)]
struct PresentedWindow {
    handle: AppThreadHandle,
    wid: WindowId,
    wsid: Option<WindowServerId>,
    base_frame: CGRect,
    fixed: bool,
    leased: bool,
    frame: CGRect,
}

impl CameraAnimation {
    pub(super) fn sample(&mut self, now: Instant) {
        let scale = self.scale;
        if !self.active {
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
        for window in &mut self.windows {
            let frame = self.presentation.frame(window.base_frame, window.fixed, scale);
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
        }
        if !ongoing {
            self.end();
        }
    }

    fn replace(&mut self, mut previous: Option<Self>) {
        if let Some(old) = &mut previous
            && (old.identity.workspace, old.identity.layout)
                != (self.identity.workspace, self.identity.layout)
        {
            old.stop();
            previous = None;
        }
        let now = Instant::now();
        if self.animate && self.gesture.is_none() && !self.presentation.animated() {
            let (from, velocity) = if let Some(old) = &previous {
                let (mut from, velocity) = old.presentation.position_velocity(now);
                if let Some((new, old)) = self.windows.iter().find_map(|new| {
                    old.windows.iter().find(|old| old.wid == new.wid).map(|old| (new, old))
                }) {
                    from += new.base_frame.origin.x - old.base_frame.origin.x;
                }
                (from, velocity)
            } else {
                // Finished cameras are gone. Start from semantic window geometry,
                // using a visible window so parking cannot distort the inverse.
                let from = self
                    .windows
                    .iter()
                    .filter(|w| !w.fixed)
                    .find_map(|w| self.presentation.offset_for_frame(w.base_frame, w.frame))
                    .unwrap_or(self.presentation.offset());
                (from, 0.0)
            };
            self.presentation.retarget(from, velocity, now);
        }
        if let Some(mut old) = previous {
            for window in &mut self.windows {
                if let Some(old) = old.windows.iter().find(|old| old.wid == window.wid) {
                    window.frame = old.frame;
                    window.leased = old.leased;
                }
            }
            old.windows.retain(|old| !self.windows.iter().any(|new| new.wid == old.wid));
            old.stop();
        }
    }

    fn begin(&mut self) {
        for window in &mut self.windows {
            if !std::mem::replace(&mut window.leased, true) {
                let _ = window.handle.send(Request::BeginWindowAnimation(window.wid));
            }
        }
    }

    fn cancel_windows(&mut self, windows: &[WindowId]) {
        self.windows.retain(|window| {
            if windows.contains(&window.wid) {
                window.handle.cancel_window_animation(window.wid);
                if let Some(wsid) = window.wsid {
                    self.store.clear_target(&wsid);
                }
                false
            } else {
                true
            }
        });
        if self.windows.is_empty() {
            self.active = false;
        }
    }

    fn end(&mut self) {
        self.active = false;
        for window in &mut self.windows {
            window.leased = false;
            let _ = window.handle.send(Request::EndWindowAnimation(window.wid));
        }
    }

    pub(super) fn stop(&mut self) {
        if self.active {
            self.active = false;
            for window in &self.windows {
                window.handle.cancel_window_animation(window.wid);
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
        let mut completed = Vec::new();
        for camera in self.presentations.values_mut() {
            if let Some(state) = &mut camera.frozen {
                frames.extend(
                    state
                        .windows
                        .iter()
                        .filter(|w| windows.binary_search(&w.wid).is_ok())
                        .map(|w| (w.wid, w.frame)),
                );
                state.cancel_windows(&windows);
                if !state.active {
                    completed.push(state.clone());
                }
            }
        }
        if let Some(tx) = &self.animation_tx {
            frames.extend(tx.cancel(windows));
            completed.extend(tx.finished.try_iter());
        }
        for (wid, frame) in frames {
            if let Some(window) = self.state.windows.window_mut(wid) {
                window.frame_monotonic = frame;
                if let Some(wsid) = window.info.sys_id {
                    self.transaction_manager.clear_target_for_window(wsid);
                }
            }
        }
        for camera in completed {
            self.reconcile_camera(&camera);
        }
    }

    // Snapshot/freeze requests run only at semantic boundaries.
    fn camera_snapshots(&mut self, space: Option<SpaceId>, freeze: bool) -> Vec<CameraAnimation> {
        let running = self
            .presentations
            .iter()
            .any(|(key, p)| p.frozen.is_none() && space.is_none_or(|space| space == *key));
        let mut cameras = Vec::new();
        for (key, prepared) in &mut self.presentations {
            if space.is_none_or(|space| space == *key) {
                if freeze {
                    cameras.extend(prepared.frozen.take());
                } else {
                    cameras.extend(prepared.frozen.clone());
                }
            }
        }
        if let Some(tx) = &self.animation_tx {
            if running {
                let (done, wait) = crossbeam_channel::bounded(1);
                if tx.send(Message::Snapshot(space, freeze, done)).is_ok() {
                    cameras.extend(wait.recv().unwrap_or_default());
                }
            }
            cameras.extend(tx.finished.try_iter());
        }
        cameras
    }

    pub(super) fn reconcile_camera(&mut self, state: &CameraAnimation) {
        let identity = &state.identity;
        let Some(camera) = self.presentations.get(&identity.space) else {
            return;
        };
        if camera.identity.generation != identity.generation {
            return;
        }
        if let Some(ws) = self
            .layout_manager
            .layout_engine
            .workspaces_mut()
            .workspaces
            .get_mut(identity.workspace)
            && let LayoutSystemKind::Scrolling(system) = &mut ws.layout_system
        {
            system.reconcile_presentation(identity.layout, &state.presentation);
        }
        for window in &state.windows {
            if let Some(model) = self.state.windows.window_mut(window.wid) {
                model.frame_monotonic = window.frame;
            }
        }
        if let Some(session) = &mut self.viewport_gesture
            && session.workspace == identity.workspace
            && session.layout == identity.layout
            && let Some((_, _, total, time)) = &state.gesture
        {
            session.applied = *total;
            session.timestamp = *time;
        }
        if !state.active {
            self.presentations.remove(&identity.space);
            if self
                .viewport_gesture
                .as_ref()
                .is_some_and(|s| s.released && s.context.space == identity.space)
            {
                self.retire_viewport_session();
            }
        }
    }

    pub(super) fn reconcile_presentations(&mut self) {
        for state in self.camera_snapshots(None, false) {
            self.reconcile_camera(&state);
        }
    }

    pub(super) fn freeze_camera(&mut self, space: SpaceId) -> Option<CameraAnimation> {
        let mut selected = None;
        for camera in self.camera_snapshots(Some(space), true) {
            let current = self
                .presentations
                .get(&camera.identity.space)
                .is_some_and(|p| p.identity.generation == camera.identity.generation);
            if !camera.active {
                self.reconcile_camera(&camera);
            }
            if current && camera.identity.space == space {
                selected = Some(camera);
            }
        }
        selected
    }

    pub(super) fn stop_camera(&mut self, space: SpaceId) {
        if let Some(mut camera) = self.freeze_camera(space) {
            camera.stop();
        }
        self.presentations.remove(&space);
    }

    pub(super) fn retire_presentations(&mut self) {
        let invalid: Vec<_> = self.presentations.iter().filter_map(|(space, camera)| {
            let valid = self.layout_manager.layout_engine.workspaces().active_layout_for_space(*space)
                == Some((camera.identity.workspace, camera.identity.layout))
                && self.active_spaces.contains(space)
                && self.space_state.screen_by_space(*space).is_some()
                && self.layout_manager.layout_engine.workspaces().workspaces.get(camera.identity.workspace)
                    .is_some_and(|ws| matches!(&ws.layout_system, LayoutSystemKind::Scrolling(system) if system.contains_layout(camera.identity.layout)))
                && matches!(self.mission_control_manager.mission_control_state, super::MissionControlState::Inactive);
            (!valid).then_some(*space)
        }).collect();
        for space in invalid {
            self.stop_camera(space);
        }
    }

    pub(super) fn present_camera(
        &mut self,
        space: SpaceId,
        animate: bool,
        gesture: Option<(Context, Control, f64, Duration)>,
        skip: Option<WindowId>,
    ) -> Option<HashSet<WindowId>> {
        if self.animation_tx.is_none() && gesture.is_none() && self.viewport_gesture.is_none() {
            return None;
        }
        let screen = self.space_state.screen_by_space(space)?;
        let display = screen.id.as_u32();
        let scale = screen.backing_scale;
        let bound = (self.active_spaces.len() > 1).then_some(screen.frame);
        let (workspace, layout) =
            self.layout_manager.layout_engine.workspaces().active_layout_for_space(space)?;
        let LayoutSystemKind::Scrolling(system) =
            &self.layout_manager.layout_engine.workspaces()[workspace].layout_system
        else {
            return None;
        };
        let (mut presentation, frames) = system.presentation(layout)?;
        let previous = self.presentations.remove(&space).and_then(|p| p.frozen);
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
        let windows = frames
            .into_iter()
            .filter_map(|(wid, base_frame, fixed)| {
                if Some(wid) == skip {
                    return None;
                }
                let window = self.state.windows.window(wid)?;
                let app = self.app_manager.apps.get(&wid.pid)?;
                Some(PresentedWindow {
                    handle: app.handle.clone(),
                    wid,
                    wsid: window.info.sys_id,
                    base_frame,
                    fixed,
                    leased: false,
                    frame: window.frame_monotonic,
                })
            })
            .collect();
        if !animate {
            presentation.finish();
        }
        let identity = CameraIdentity {
            space,
            workspace,
            layout,
            generation: now,
        };
        let mut next = CameraAnimation {
            presentation,
            windows,
            gesture,
            active: true,
            identity: identity.clone(),
            events: self.communication_manager.events_tx.clone(),
            finished: self.animation_tx.as_ref().map(|tx| tx.finished_tx.clone()),
            display,
            animate,
            scale,
            store: self.transaction_manager.store.clone(),
            interval: Duration::from_secs_f64(1.0 / self.config.settings.animation_fps),
            bound,
        };
        let previous_was_absent = previous.is_none();
        if !previous_was_absent {
            next.replace(previous);
        }
        let windows = next.windows.iter().map(|w| w.wid).collect();
        self.presentations.insert(space, CameraRegistration { identity, frozen: None });
        if let Some(tx) = &self.animation_tx {
            let _ = tx.send(Message::Camera(Box::new(next)));
        } else {
            if previous_was_absent {
                next.replace(None);
            }
            next.begin();
            next.sample(now);
            self.reconcile_camera(&next);
            if next.active {
                self.presentations.get_mut(&space).unwrap().frozen = Some(next);
            }
        }
        Some(windows)
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

    fn begin(&self) { let _ = self.handle.send(Request::BeginWindowAnimation(self.wid)); }

    fn frame_after(&self, t: f64) -> CGRect {
        let mut rect = get_frame(self.start, self.finish, t);
        if t >= 0.5 {
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
                        Message::Snapshot(space, freeze, done) => {
                            let mut cameras = Vec::new();
                            for d in displays.values_mut() {
                                if d.manager.camera.as_ref().is_some_and(|c| space.is_none_or(|space| c.identity.space == space)) {
                                    cameras.extend(if freeze { d.manager.camera.take() } else { d.manager.camera.clone() });
                                }
                            }
                            let _ = done.send(cameras);
                        }
                        Message::Stop(mut windows, done) => {
                            windows.sort_unstable();
                            let mut frames = Vec::new();
                            for d in displays.values_mut() {
                                if done.is_some() { frames.extend(d.manager.presented_frames(&windows)); }
                                d.manager.stop_windows(&windows);
                                d.manager.retire_camera();
                            }
                            if let Some(done) = done { let _ = done.send(frames); }
                        }
                        message => {
                            let display = match &message {
                                Message::Replace(a) | Message::SkipToEnd(a) => a.display,
                                Message::Camera(c) => c.display,
                                Message::Stop(..) | Message::Snapshot(..) => unreachable!(),
                            };
                            let entry = displays.entry(display).or_insert_with(|| DisplayPresenter {
                                manager: Self::new(), link: DisplayLink::for_display(display, wake.clone()),
                                fallback_deadline: Instant::now(),
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
                fallback_deadline: deadline,
            } in displays.values_mut()
            {
                let native = link.latest();
                let sample =
                    native.map(|(_, target)| target).or_else(|| (now >= *deadline).then_some(now));
                if let Some(sample) = sample {
                    if let Some(camera) = &mut manager.camera {
                        camera.sample(sample);
                    }
                    let delay = manager.tick_at(sample).unwrap_or_else(|| {
                        manager
                            .camera
                            .as_ref()
                            .map_or(Duration::from_secs_f64(1.0 / 60.0), |c| c.interval)
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
                }
                manager.retire_camera();
            }
            displays.retain(|_, d| d.manager.has_work());
        }
        for d in displays.values_mut() {
            d.manager.finish_active();
            if let Some(camera) = &mut d.manager.camera {
                camera.stop();
            }
        }
    }

    fn retire_camera(&mut self) {
        if self.camera.as_ref().is_some_and(|c| !c.active) {
            let mut camera = self.camera.take().unwrap();
            let events = camera.events.take();
            if let Some(finished) = camera.finished.take() {
                let _ = finished.send(camera);
            }
            if let Some(events) = events {
                events.send(super::Event::CameraFinished);
            }
        }
    }

    fn has_work(&self) -> bool {
        self.active.is_some() || self.camera.as_ref().is_some_and(|c| c.active)
    }

    fn stop_windows(&mut self, windows: &[WindowId]) {
        if let Some(camera) = &mut self.camera {
            camera.cancel_windows(windows);
        }
        if let Some(active) = &mut self.active {
            active.animation.windows.retain(|window| {
                if windows.binary_search(&window.wid).is_ok() {
                    window.handle.cancel_window_animation(window.wid);
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
        if let Some(camera) = &self.camera
            && camera.active
        {
            frames.extend(
                camera
                    .windows
                    .iter()
                    .filter(|w| windows.binary_search(&w.wid).is_ok())
                    .map(|w| (w.wid, w.frame)),
            );
        }
        frames
    }

    pub fn handle_message(&mut self, message: Message) {
        match message {
            Message::Camera(mut camera) => {
                camera.replace(self.camera.take());
                let mut windows: Vec<_> = camera.windows.iter().map(|w| w.wid).collect();
                windows.sort_unstable();
                self.stop_windows(&windows);
                camera.begin();
                self.camera = Some(*camera);
            }
            Message::Snapshot(_, _, _) => unreachable!("runner handles snapshots"),
            Message::Stop(mut windows, done) => {
                windows.sort_unstable();
                let frames = done.as_ref().map(|_| self.presented_frames(&windows));
                self.stop_windows(&windows);
                if let Some(done) = done {
                    let _ = done.send(frames.unwrap());
                }
            }
            Message::Replace(animation) => {
                self.active = match self.active.take() {
                    Some(active) => Some(active.replace_with(animation)),
                    None => ActiveAnimation::start(animation),
                };
            }
            Message::SkipToEnd(animation) => {
                self.finish_active();
                animation.skip_to_end();
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
        let presentation = reactor.present_camera(space, animate_camera, None, skip_wid);
        let camera = presentation.is_some();
        let presented = presentation.unwrap_or_default();
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
            let Some(window) = reactor.state.windows.window(wid) else {
                continue;
            };
            let current_frame = window.frame_monotonic;
            let window_server_id = window.info.sys_id;
            let pending = window_server_id
                .and_then(|wsid| reactor.transaction_manager.get_target_frame(wsid));
            // Matching the observed frame cannot cancel an older, different target.
            if pending.is_some_and(|frame| frame.same_as(target_frame))
                || (pending.is_none() && target_frame.same_as(current_frame))
            {
                continue;
            }
            any_frame_changed = true;
            let txid = window_server_id
                .map(|wsid| reactor.transaction_manager.generate_next_txid(wsid))
                .unwrap_or_default();

            let Some(app_state) = &reactor.app_manager.apps.get(&wid.pid) else {
                debug!(?wid, "Skipping for window - app no longer exists");
                continue;
            };

            let is_active = reactor
                .state
                .windows
                .workspace_for_window(space, wid)
                .is_some_and(|ws| ws == active_ws);

            if let Some(wsid) = window_server_id {
                reactor.transaction_manager.update_txid_entries([(wsid, txid, target_frame)]);
            }
            if is_active {
                trace!(?wid, ?current_frame, ?target_frame, "Animating visible window");
                anim.add_window(&app_state.handle, wid, current_frame, target_frame, txid);
            } else {
                anim.mark_handled(wid);
                trace!(
                    ?wid,
                    ?current_frame,
                    ?target_frame,
                    "Direct positioning hidden window"
                );
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

        if !anim.is_empty() {
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
                        Message::Stop(..) | Message::Camera(_) | Message::Snapshot(..) => {}
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
        let mut per_app: HashMap<pid_t, _> = HashMap::default();
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
            let (frames, positions, first_wsid) =
                per_app.entry(wid.pid).or_insert_with(|| (Vec::new(), Vec::new(), None));
            *first_wsid = first_wsid.or(window.info.sys_id);
            if position_only && size_unchanged {
                positions.push((wid, target_frame.origin));
            } else {
                frames.push((wid, target_frame));
            }
        }

        for (pid, (frames, positions, first_wsid)) in per_app {
            let Some(app_state) = reactor.app_manager.apps.get(&pid) else {
                debug!(?pid, "Skipping layout update for app - app no longer exists");
                continue;
            };

            let handle = &app_state.handle;

            let txid = first_wsid
                .map(|wsid| reactor.transaction_manager.generate_next_txid(wsid))
                .unwrap_or_default();
            for wid in frames.iter().map(|(wid, _)| wid).chain(positions.iter().map(|(wid, _)| wid))
            {
                if let Some(window) = reactor.state.windows.window(*wid)
                    && let Some(wsid) = window.info.sys_id
                {
                    reactor.transaction_manager.store_txid(wsid, txid, window.frame_monotonic);
                }
            }
            let requests = [
                (!positions.is_empty())
                    .then(|| Request::SetWorkspaceSwitchPositions(positions, txid, true)),
                (!frames.is_empty()).then(|| Request::SetBatchWindowFrame(frames, txid, true)),
            ];
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
                old.handle.cancel_window_animation(old.wid);
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

    fn add_window(
        &mut self,
        handle: &AppThreadHandle,
        wid: WindowId,
        start: CGRect,
        finish: CGRect,
        txid: TransactionId,
    ) {
        self.windows.push(AnimatedWindow {
            handle: handle.clone(),
            wid,
            start,
            finish,
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

    fn is_empty(&self) -> bool { self.windows.is_empty() }

    fn begin(&self) {
        for window in &self.windows {
            window.begin();
        }
    }

    fn finish_all(&self) {
        self.send_frame(1.0, 0.0);
        self.end();
    }

    fn send_frame(&self, t: f64, previous: f64) {
        for window in &self.windows {
            let rect = window.frame_after(t);
            let set_size = (previous < 0.5 && t >= 0.5) || t == 1.0;
            if rect != window.frame_after(previous) || set_size {
                window.send_frame(rect, set_size);
            }
        }
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
        animation.add_window(handle, wid, from, to, TransactionId::default());
        animation
    }

    #[derive(Debug)]
    enum Observed {
        Frame {
            wid: WindowId,
            frame: CGRect,
            set_size: bool,
            txid: TransactionId,
        },
        Request(Request),
    }
    fn collect_requests(rx: &mut crate::actor::Receiver<Request>) -> Vec<Observed> {
        let mut requests = Vec::new();
        while let Ok((_, request)) = rx.try_recv() {
            if let Request::InteractiveFramesPending(queue) = request {
                queue.drain_with(|wid, frame, set_size, txid, _, _| {
                    requests.push(Observed::Frame { wid, frame, set_size, txid })
                });
            } else {
                requests.push(Observed::Request(request));
            }
        }
        requests
    }

    fn assert_set_window_frame(request: &Observed, wid: WindowId, frame: CGRect) {
        match request {
            Observed::Request(Request::SetWindowFrame(req_wid, req_frame, txid, eui)) => {
                assert_eq!(*req_wid, wid);
                assert_eq!(*req_frame, frame);
                assert_eq!(*txid, TransactionId::default());
                assert!(*eui);
            }
            _ => panic!("expected SetWindowFrame, got {request:?}"),
        }
    }

    fn assert_animation_frame(request: &Observed, wid: WindowId, frame: CGRect) {
        match request {
            Observed::Frame {
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
            _ => panic!("expected coalesced frame, got {request:?}"),
        }
    }

    fn assert_animation_pos(request: &Observed, wid: WindowId, pos: CGPoint) {
        match request {
            Observed::Frame {
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
            _ => panic!("expected coalesced frame, got {request:?}"),
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
                    Observed::Frame { frame, .. } => Some(frame),
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
                    .filter(|r| matches!(r, Observed::Frame { .. }))
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
        let Observed::Frame { frame: last, .. } = frames[0] else {
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
        let camera = CameraAnimation {
            presentation: system.presentation(layout).unwrap().0,
            windows: vec![PresentedWindow {
                handle: handle.clone(),
                wid,
                wsid: None,
                base_frame: system.presentation(layout).unwrap().1[0].1,
                fixed: false,
                leased: false,
                frame: rect(0.0, 0.0, 10.0, 10.0),
            }],
            gesture: None,
            active: true,
            identity: CameraIdentity {
                space: SpaceId::new(1),
                workspace: VirtualWorkspaceId::default(),
                layout,
                generation: now,
            },
            events: None,
            finished: None,
            display: 1,
            animate: false,
            scale: 2.0,
            store: WindowTxStore::new(),
            interval: Duration::from_secs_f64(1.0 / 60.0),
            bound: None,
        };
        manager.handle_message(Message::Camera(Box::new(camera)));
        assert!(manager.active.is_none());
        let Request::CancelWindowAnimation(_, through) = rx.try_recv().unwrap().1 else {
            panic!("cancel");
        };
        assert!(matches!(rx.try_recv().unwrap().1, Request::BeginWindowAnimation(id) if id == wid));
        // A native camera frame reuses the old outstanding generic wake.
        manager.camera.as_mut().unwrap().windows[0].base_frame.origin.x += 0.5;
        manager.camera.as_mut().unwrap().sample(now);
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
            let camera = manager.camera.as_mut().unwrap();
            camera.presentation.update(0.001, Duration::from_millis(tick * 8));
            camera.sample(now + Duration::from_secs_f64(tick as f64 / 120.0));
            assert_eq!(collect_requests(&mut rx).len(), 1);
        }
        manager
            .camera
            .as_mut()
            .unwrap()
            .presentation
            .update(0.001, Duration::from_millis(30));
        manager.camera.as_mut().unwrap().sample(now + Duration::from_millis(30));
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
        manager.camera.as_mut().unwrap().sample(now + Duration::from_secs(1));
        assert!(!manager.has_work());
        assert!(rx.try_recv().is_err());
        old_wake.drain_with(|_, frame, _, _, source, sequence| {
            assert_eq!(frame, dragged);
            assert_eq!(source, FrameSource::Drag);
            assert!(sequence > through);
        });
        system.cancel_viewport_gesture(layout);
        {
            let c = manager.camera.as_mut().unwrap();
            let (presentation, frames) = system.presentation(layout).unwrap();
            c.presentation = presentation;
            let mut base_frame = frames[0].1;
            base_frame.origin.x += 0.5;
            c.windows.push(PresentedWindow {
                handle,
                wid,
                wsid: None,
                base_frame,
                fixed: false,
                leased: false,
                frame: dragged,
            });
            c.active = true;
            c.sample(now);
            assert!(!c.active, "instant target must retire immediately");
        }
        let requests = collect_requests(&mut rx);
        let Observed::Frame { frame, .. } = requests[0] else {
            panic!("frame");
        };
        assert_eq!(frame.origin.x.fract().abs(), 0.5);
        assert!(
            matches!(requests[1], Observed::Request(Request::EndWindowAnimation(id)) if id == wid)
        );

        // A completed camera must wake semantic reconciliation and leave the
        // presenter even when no new reactor command arrives.
        let mut camera = manager.camera.take().unwrap();
        // Removing completed cameras must not make the next focus jump instant.
        system.scroll_by_delta(layout, 1.0);
        camera.presentation = system.presentation(layout).unwrap().0;
        camera.animate = true;
        camera.active = true;
        let before = camera.windows[0].frame;
        let restarted = Instant::now();
        camera.replace(None);
        assert_eq!(
            camera.presentation.frame(camera.windows[0].base_frame, false, 2.0),
            before
        );
        let target = camera.presentation.target();
        let from = camera.presentation.offset();
        assert_ne!(from, target, "next camera must start at the displayed position");
        camera.sample(restarted + Duration::from_millis(50));
        let current = camera.presentation.offset();
        assert!(
            (current - from) * (target - from) > 0.0 && (target - current) * (target - from) > 0.0
        );
        camera.sample(restarted + Duration::from_secs(1));
        assert!(!camera.active);
        assert_eq!(camera.presentation.offset(), target);
        collect_requests(&mut rx);
        camera.animate = false;
        camera.active = true;
        let expected_final = camera.windows[0].frame;
        let (commands, commands_rx) = crossbeam_channel::unbounded();
        let sender = AnimationSender::from(commands);
        let (events, mut events_rx) = crate::actor::channel();
        camera.events = Some(events);
        camera.finished = Some(sender.finished_tx.clone());
        let runner = std::thread::spawn(move || AnimationManager::run(commands_rx));
        sender.send(Message::Camera(Box::new(camera))).unwrap();
        let completed = sender
            .finished
            .recv_timeout(Duration::from_secs(1))
            .expect("completion cannot depend on another reactor event");
        assert!(!completed.active);
        assert_eq!(completed.windows[0].frame, expected_final);
        let (done, wait) = crossbeam_channel::bounded(1);
        sender.send(Message::Snapshot(None, false, done)).unwrap();
        assert!(wait.recv_timeout(Duration::from_secs(1)).unwrap().is_empty());
        assert!(matches!(
            events_rx.try_recv().unwrap().1,
            super::super::Event::CameraFinished
        ));
        let terminal = collect_requests(&mut rx);
        assert!(matches!(terminal.as_slice(), [
            Observed::Request(Request::BeginWindowAnimation(_)),
            Observed::Request(Request::EndWindowAnimation(_))
        ]));
        let mut cancelled = completed.clone();
        cancelled.active = true;
        system.begin_viewport_gesture(layout, Instant::now());
        cancelled.presentation = system.presentation(layout).unwrap().0;
        cancelled.finished = Some(sender.finished_tx.clone());
        sender.send(Message::Camera(Box::new(cancelled))).unwrap();
        let stopped = sender.cancel(vec![wid]);
        assert_eq!(stopped.len(), 1);
        let retired = sender.finished.try_recv().expect("Stop reply must include retirement");
        assert!(!retired.active);
        assert!(retired.windows.is_empty());
        drop(sender);
        runner.join().unwrap();

        // An old completion waiting in the mailbox cannot supersede the camera
        // transferred by a newer freeze request.
        let mut current = completed.clone();
        current.active = true;
        current.identity.generation += Duration::from_secs(1);
        let generation = current.identity.generation;
        let space = current.identity.space;
        let (commands, _commands_rx) = crossbeam_channel::unbounded();
        let sender = AnimationSender::from(commands);
        sender.finished_tx.send(completed).unwrap();
        let mut reactor = super::super::testing::test_reactor();
        reactor.animation_tx = Some(sender);
        reactor.presentations.insert(space, CameraRegistration {
            identity: current.identity.clone(),
            frozen: Some(current),
        });
        let frozen = reactor.freeze_camera(space).unwrap();
        assert_eq!(frozen.identity.generation, generation);
        assert!(frozen.active);
        assert_eq!(reactor.presentations[&space].identity.generation, generation);
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
            matches!(requests.as_slice(), [Observed::Request(Request::CancelWindowAnimation(id, _))] if *id == wid)
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
            [Observed::Request(Request::BeginWindowAnimation(req_wid))] if *req_wid == wid
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
            TransactionId::default(),
        );
        first.add_window(
            &handle,
            wid2,
            rect(10.0, 0.0, 10.0, 10.0),
            rect(60.0, 60.0, 10.0, 10.0),
            TransactionId::default(),
        );
        let mut second = empty_animation();
        second.add_window(
            &handle,
            wid1,
            rect(50.0, 60.0, 10.0, 10.0),
            rect(80.0, 90.0, 10.0, 10.0),
            TransactionId::default(),
        );
        second.add_window(
            &handle,
            wid3,
            rect(20.0, 0.0, 10.0, 10.0),
            rect(90.0, 90.0, 10.0, 10.0),
            TransactionId::default(),
        );

        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(first));
        assert_eq!(collect_requests(&mut rx).len(), 2);
        manager.handle_message(Message::Replace(second));

        let requests = collect_requests(&mut rx);
        assert_eq!(requests.len(), 1);
        assert!(
            matches!(requests[0], Observed::Request(Request::BeginWindowAnimation(req_wid)) if req_wid == wid3)
        );
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
            TransactionId::default(),
        );
        first.add_window(
            &handle,
            wid2,
            rect(10.0, 0.0, 10.0, 10.0),
            rect(60.0, 60.0, 10.0, 10.0),
            TransactionId::default(),
        );
        let mut second = empty_animation();
        second.add_window(
            &handle,
            wid1,
            rect(50.0, 60.0, 10.0, 10.0),
            rect(80.0, 90.0, 10.0, 10.0),
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
        assert!(
            matches!(requests[0], Observed::Request(Request::BeginWindowAnimation(req_wid)) if req_wid == wid)
        );
        assert_animation_frame(&requests[1], wid, rect(50.0, 60.0, 10.0, 10.0));
        assert!(
            matches!(requests[2], Observed::Request(Request::EndWindowAnimation(req_wid)) if req_wid == wid)
        );
        assert_set_window_frame(&requests[3], wid, rect(80.0, 90.0, 10.0, 10.0));
    }
}
