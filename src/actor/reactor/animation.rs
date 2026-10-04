use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use parking_lot::Mutex;
use tracing::{debug, trace};

use super::TransactionId;
use crate::actor::app::{AppThreadHandle, FrameMode, FrameSource, Request, WindowId, pid_t};
use crate::actor::gesture::{Context, Control};
use crate::actor::reactor::Reactor;
use crate::common::collections::{HashMap, HashSet};
use crate::layout_engine::systems::scrolling::{PresentedViewport, ViewportPresentation};
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
    control: Arc<Mutex<PresenterControl>>,
}
impl AnimationSender {
    pub fn channel() -> (Self, AnimationReceiver) {
        let (tx, commands) = crossbeam_channel::unbounded();
        let control = Arc::default();
        (
            Self {
                tx,
                control: Arc::clone(&control),
            },
            AnimationReceiver { commands, control },
        )
    }

    pub fn send(&self, message: Message) -> Result<(), crossbeam_channel::SendError<Message>> {
        self.tx.send(message)
    }

    pub fn cancel(&self, windows: Vec<WindowId>) -> Vec<(WindowId, CGRect)> {
        let mut control = self.control.lock();
        let frames = windows
            .iter()
            .filter_map(|wid| {
                let (handle, frame) = control.frames.remove(wid)?;
                handle.cancel_window_animation(*wid);
                Some((*wid, frame))
            })
            .collect();
        control.cancelled.extend(windows.iter().copied());
        let _ = self.send(Message::Stop(windows));
        frames
    }
}

#[derive(Debug, Default)]
struct PresenterControl {
    frames: HashMap<WindowId, (AppThreadHandle, CGRect)>,
    cancelled: HashSet<WindowId>,
}
pub struct AnimationReceiver {
    pub(super) commands: Receiver<Message>,
    control: Arc<Mutex<PresenterControl>>,
}

#[derive(Debug)]
pub enum Message {
    Replace(Animation),
    SkipToEnd(Animation),
    Stop(Vec<WindowId>),
    Camera(Box<CameraAnimation>),
    StopCamera(SpaceId),
    Displays(Vec<u32>),
}

#[derive(Debug, Default)]
pub struct AnimationManager {
    motions: Vec<Motion>,
    frames: FrameBatch,
}

/// Display scheduling, cancellation and retirement are shared by both motion
/// policies. A display may present a viewport alongside unrelated transitions.
#[derive(Debug)]
enum Motion {
    Transition(ActiveAnimation),
    Viewport(CameraAnimation),
}

type Frame = (WindowId, CGRect, bool, TransactionId, FrameSource);

/// Reusable presenter-local batches: all motions on a display publish through
/// one lock/wake per application, and terminal leases follow that publication.
#[derive(Debug, Default)]
struct FrameBatch(HashMap<pid_t, (AppThreadHandle, Vec<Frame>)>);
impl FrameBatch {
    fn extend(
        &mut self,
        handle: &AppThreadHandle,
        pid: pid_t,
        frames: impl Iterator<Item = Frame>,
    ) {
        let (actor, pending) = self.0.entry(pid).or_insert_with(|| (handle.clone(), Vec::new()));
        if !actor.same_actor(handle) {
            *actor = handle.clone();
        }
        pending.extend(frames);
    }

    fn flush(&mut self) {
        for (handle, frames) in self.0.values_mut() {
            if !frames.is_empty() {
                handle.send_interactive_frames(frames.drain(..));
            }
        }
    }
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
    ended: bool,
}

#[derive(Debug)]
pub struct Animation {
    interval: Duration,
    duration: Duration,
    display: u32,
    windows: Vec<PresentedWindow>,
    handled_windows: Vec<WindowId>,
}

#[derive(Clone, Debug)]
pub(super) struct CameraIdentity {
    pub space: SpaceId,
    pub workspace: VirtualWorkspaceId,
    pub layout: LayoutId,
}

#[derive(Debug)]
pub(super) struct ViewportHandle {
    pub identity: CameraIdentity,
    // Headless execution uses the same renderer without a presenter thread.
    pub state: Arc<Mutex<PresentedCamera>>,
    windows: Vec<WindowId>,
    pub headless: Option<CameraAnimation>,
}

#[derive(Debug)]
pub(super) struct PresentedCamera {
    viewport: PresentedViewport,
    gesture: Option<(f64, Duration)>,
    active: bool,
    stopped: bool,
}

#[derive(Debug)]
pub struct CameraAnimation {
    pub(super) presentation: ViewportPresentation,
    windows: Vec<PresentedWindow>,
    pub(super) gesture: Option<(Context, Control, f64, Duration)>,
    pub(super) active: bool,
    identity: CameraIdentity,
    events: Option<super::Sender>,
    state: Arc<Mutex<PresentedCamera>>,
    display: u32,
    animate: bool,
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
    // Transitions interpolate from/to screen frames. Viewport motion translates
    // the prepared world frame in `to`; `frame` is always the last publication.
    from: CGRect,
    to: CGRect,
    frame: CGRect,
    txid: TransactionId,
    fixed: bool,
    leased: bool,
    announced: bool,
}

#[derive(Clone, Copy)]
enum Sample<'a> {
    Transition {
        progress: f64,
        eased: f64,
        set_size: bool,
    },
    Viewport {
        presentation: &'a ViewportPresentation,
        offset: f64,
        scale: f64,
        bound: Option<CGRect>,
    },
}

impl PresentedWindow {
    fn begin(&mut self) {
        if !std::mem::replace(&mut self.leased, true) {
            let _ = self.handle.send(Request::BeginWindowAnimation(self.wid));
        }
    }

    fn end(&mut self) {
        if std::mem::replace(&mut self.leased, false) {
            let _ = self.handle.send(Request::EndWindowAnimation(self.wid));
        }
    }

    fn cancel(&mut self, store: Option<&WindowTxStore>) {
        self.leased = false;
        self.handle.cancel_window_animation(self.wid);
        if let Some(store) = store
            && let Some(wsid) = self.wsid
        {
            store.clear_target_if_current(&wsid, self.txid);
        }
    }

    fn sample(&mut self, sample: Sample<'_>) -> Option<Frame> {
        let (frame, set_size, source, force) = match sample {
            Sample::Transition { progress, eased, set_size } => {
                let mut frame = interpolate_frame(self.from, self.to, eased);
                frame.size = if progress >= 0.5 {
                    self.to.size
                } else {
                    self.from.size
                };
                (frame, set_size, FrameSource::Ordinary, set_size)
            }
            Sample::Viewport {
                presentation,
                offset,
                scale,
                bound,
            } => {
                if self.fixed && self.announced {
                    return None;
                }
                let frame = presentation.frame_at_offset(self.to, self.fixed, scale, offset);
                let frame = bound.map_or(frame, |screen| {
                    super::managers::bound_frame_to_screen(frame, screen)
                });
                (
                    frame,
                    !frame.size.same_as(self.frame.size),
                    FrameSource::Viewport,
                    !self.announced,
                )
            }
        };
        // Ordinary frames preserve subpixel easing; camera frames are already
        // aligned to the cached display scale.
        let changed = frame != self.frame || force;
        self.frame = frame;
        self.announced = true;
        changed.then_some((self.wid, frame, set_size, self.txid, source))
    }
}

fn stage_windows(windows: &mut [PresentedWindow], sample: Sample<'_>, frames: &mut FrameBatch) {
    for group in windows.chunk_by_mut(|a, b| a.wid.pid == b.wid.pid) {
        let handle = group[0].handle.clone();
        frames.extend(
            &handle,
            group[0].wid.pid,
            group.iter_mut().filter_map(|window| window.sample(sample)),
        );
    }
}

impl CameraAnimation {
    pub(super) fn sample(&mut self, now: Instant) {
        let mut frames = FrameBatch::default();
        self.sample_into(now, &mut frames);
        frames.flush();
        if !self.active {
            self.end();
        }
    }

    fn sample_into(&mut self, now: Instant, frames: &mut FrameBatch) {
        let scale = self.scale;
        if self.state.lock().stopped {
            self.stop();
        }
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
        stage_windows(
            &mut self.windows,
            Sample::Viewport {
                presentation: &self.presentation,
                offset: self.presentation.offset(),
                scale,
                bound: self.bound,
            },
            frames,
        );
        self.active = ongoing;
        self.publish_state(now);
    }

    fn publish_state(&self, now: Instant) {
        let mut state = self.state.lock();
        state.viewport = self.presentation.snapshot(now);
        state.gesture = self.gesture.as_ref().map(|(_, _, total, time)| (*total, *time));
        state.active = self.active;
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
                    from += new.to.origin.x - old.to.origin.x;
                }
                (from, velocity)
            } else {
                // Native frames can be clamped after parking. The viewport
                // retains its own starting position when a target changes.
                (self.presentation.offset(), 0.0)
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
        self.windows.sort_unstable_by_key(|w| w.wid.pid);
        for window in &mut self.windows {
            window.announced = false;
            window.txid = window.wsid.map_or_else(TransactionId::default, |wsid| {
                let txid = self.store.next_txid(wsid);
                if self.gesture.is_none() && !self.presentation.gesturing() {
                    let frame = self.presentation.target_frame(window.to, window.fixed, self.scale);
                    let frame = self.bound.map_or(frame, |screen| {
                        super::managers::bound_frame_to_screen(frame, screen)
                    });
                    self.store.insert(wsid, txid, frame);
                }
                txid
            });
            window.begin();
        }
    }

    fn cancel_windows(&mut self, windows: &[WindowId]) {
        self.windows.retain_mut(|window| {
            if windows.binary_search(&window.wid).is_ok() {
                window.cancel(Some(&self.store));
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
            window.end();
        }
    }

    pub(super) fn stop(&mut self) {
        if std::mem::replace(&mut self.active, false) {
            for window in &mut self.windows {
                window.cancel(Some(&self.store));
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
        for camera in self.presentations.values_mut() {
            if let Some(state) = &mut camera.headless {
                frames.extend(
                    state
                        .windows
                        .iter()
                        .filter(|w| windows.binary_search(&w.wid).is_ok())
                        .map(|w| (w.wid, w.frame)),
                );
                state.cancel_windows(&windows);
                state.publish_state(Instant::now());
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
        self.commit_presentations();
    }

    pub(super) fn commit_presentations(&mut self) {
        let control = self.animation_tx.as_ref().map(|tx| tx.control.clone());
        let control = control.as_ref().map(|c| c.lock());
        let mut completed = Vec::new();
        for (space, camera) in &self.presentations {
            let state = camera.state.lock();
            let identity = &camera.identity;
            if let Some(ws) = self
                .layout_manager
                .layout_engine
                .workspaces_mut()
                .workspaces
                .get_mut(identity.workspace)
                && let LayoutSystemKind::Scrolling(system) = &mut ws.layout_system
            {
                system.commit_presented_viewport(identity.layout, &state.viewport);
            }
            for &wid in &camera.windows {
                let frame = if let Some(control) = &control {
                    control.frames.get(&wid).map(|(_, frame)| *frame)
                } else {
                    camera
                        .headless
                        .as_ref()
                        .and_then(|c| c.windows.iter().find(|w| w.wid == wid))
                        .map(|w| w.frame)
                };
                if let Some(frame) = frame
                    && let Some(model) = self.state.windows.window_mut(wid)
                {
                    model.frame_monotonic = frame;
                }
            }
            if let Some(session) = &mut self.viewport_gesture
                && session.workspace == identity.workspace
                && session.layout == identity.layout
                && let Some((total, time)) = state.gesture
            {
                session.applied = total;
                session.timestamp = time;
            }
            if !state.active {
                completed.push(*space);
            }
        }
        for space in completed {
            self.presentations.remove(&space);
            if self
                .viewport_gesture
                .as_ref()
                .is_some_and(|s| s.released && s.context.space == space)
            {
                self.retire_viewport_session();
            }
        }
    }

    pub(super) fn stop_camera(&mut self, space: SpaceId) {
        if let Some(mut camera) = self.presentations.remove(&space) {
            camera.state.lock().stopped = true;
            let frames = if let Some(headless) = &mut camera.headless {
                let frames = headless.windows.iter().map(|w| (w.wid, w.frame)).collect();
                headless.stop();
                frames
            } else if let Some(tx) = &self.animation_tx {
                tx.cancel(camera.windows)
            } else {
                Vec::new()
            };
            for (wid, frame) in frames {
                if let Some(window) = self.state.windows.window_mut(wid) {
                    window.frame_monotonic = frame;
                }
            }
            if let Some(tx) = &self.animation_tx {
                let _ = tx.send(Message::StopCamera(space));
            }
        }
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
        let previous = self.presentations.remove(&space).and_then(|p| p.headless);
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
                    from: window.frame_monotonic,
                    to: base_frame,
                    fixed,
                    announced: false,
                    leased: false,
                    frame: window.frame_monotonic,
                    txid: TransactionId::default(),
                })
            })
            .collect();
        if !animate {
            presentation.finish();
        }
        let identity = CameraIdentity { space, workspace, layout };
        let state = Arc::new(Mutex::new(PresentedCamera {
            viewport: presentation.snapshot(now),
            gesture: None,
            active: true,
            stopped: false,
        }));
        let mut next = CameraAnimation {
            presentation,
            windows,
            gesture,
            active: true,
            identity: identity.clone(),
            events: self.communication_manager.events_tx.clone(),
            state: state.clone(),
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
        let ids = next.windows.iter().map(|w| w.wid).collect();
        self.presentations.insert(space, ViewportHandle {
            identity,
            state,
            windows: ids,
            headless: None,
        });
        if let Some(tx) = &self.animation_tx {
            let _ = tx.send(Message::Camera(Box::new(next)));
        } else {
            if previous_was_absent {
                next.replace(None);
            }
            next.begin();
            next.sample(now);
            self.presentations.get_mut(&space).unwrap().headless = Some(next);
            self.commit_presentations();
        }
        Some(windows)
    }
}

impl AnimationManager {
    pub fn new() -> Self { Self::default() }

    /// One runner for all displays. Commands are semantic work; display wakes
    /// are bounded to one permit and each link retains only its newest timing.
    pub fn run(receiver: AnimationReceiver) {
        let AnimationReceiver { commands: rx, control } = receiver;
        let (wake, ticks) = crossbeam_channel::bounded(1);
        let mut displays: HashMap<u32, DisplayPresenter> = HashMap::default();
        loop {
            let deadline = displays
                .values()
                .filter(|d| d.manager.has_work())
                .map(|d| d.fallback_deadline)
                .min();
            let timeout =
                deadline.map_or(Duration::MAX, |t| t.saturating_duration_since(Instant::now()));
            crossbeam_channel::select! {
                recv(rx) -> message => {
                    let Ok(message) = message else { break };
                    match message {
                        Message::Displays(known) => {
                            displays.retain(|display, presenter| {
                                if known.contains(display) { return true; }
                                presenter.manager.stop_all();
                                false
                            });
                        }
                        Message::StopCamera(space) => {
                            for d in displays.values_mut() {
                                for motion in &mut d.manager.motions {
                                    if let Motion::Viewport(camera) = motion && camera.identity.space == space { camera.stop(); }
                                }
                                d.manager.retire();
                            }
                        }
                        Message::Stop(mut windows) => {
                            windows.sort_unstable();
                            for d in displays.values_mut() { d.manager.stop_windows(&windows); d.manager.retire(); }
                            let mut shared = control.lock();
                            for wid in &windows { shared.cancelled.remove(wid); shared.frames.remove(wid); }
                        }
                        mut message => {
                            let display = match &message {
                                Message::Replace(a) | Message::SkipToEnd(a) => a.display,
                                Message::Camera(c) => c.display,
                                Message::Stop(..) | Message::StopCamera(..) | Message::Displays(..) => unreachable!(),
                            };
                            let entry = displays.entry(display).or_insert_with(|| DisplayPresenter {
                                manager: Self::new(), link: DisplayLink::for_display(display, wake.clone()),
                                fallback_deadline: Instant::now(),
                            });
                            let mut shared = control.lock();
                            let mut cancelled: Vec<_> = shared.cancelled.iter().copied().collect();
                            cancelled.sort_unstable();
                            entry.manager.stop_windows(&cancelled);
                            entry.manager.retire();
                            // A command queued before cancellation cannot resurrect its
                            // windows or replay an obsolete instant-layout target.
                            match &mut message {
                                Message::Replace(a) | Message::SkipToEnd(a) => a.windows.retain(|w| !shared.cancelled.contains(&w.wid)),
                                Message::Camera(c) => {
                                    c.windows.retain(|w| !shared.cancelled.contains(&w.wid));
                                    c.active = !c.windows.is_empty();
                                }
                                _ => unreachable!(),
                            }
                            entry.manager.handle_message(message);
                            entry.manager.publish_frames(&mut shared, true);
                            entry.manager.retire();
                            entry.link.set_paused(!entry.manager.has_work());
                            entry.fallback_deadline = Instant::now();
                        }
                    }
                }
                recv(ticks) -> _ => {}
                default(timeout) => {}
            }
            let now = Instant::now();
            let mut shared = control.lock();
            let cancelled: Vec<_> = {
                let mut w: Vec<_> = shared.cancelled.iter().copied().collect();
                w.sort_unstable();
                w
            };
            for DisplayPresenter {
                manager,
                link,
                fallback_deadline: deadline,
            } in displays.values_mut()
            {
                manager.stop_windows(&cancelled);
                if !manager.has_work() {
                    manager.retire();
                    link.set_paused(true);
                    continue;
                }
                let native = link.latest();
                let sample =
                    native.map(|(_, target)| target).or_else(|| (now >= *deadline).then_some(now));
                if let Some(sample) = sample {
                    let delay =
                        manager.tick_at(sample).unwrap_or(Duration::from_secs_f64(1.0 / 60.0));
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
                manager.publish_frames(&mut shared, false);
                manager.retire();
            }
            for d in displays.values() {
                d.link.set_paused(!d.manager.has_work());
            }
        }
        for d in displays.values_mut() {
            for motion in &mut d.manager.motions {
                match motion {
                    Motion::Transition(a) => a.animation.finish_all(),
                    Motion::Viewport(c) => c.stop(),
                }
                motion.complete();
            }
        }
    }

    fn retire(&mut self) {
        self.motions.retain_mut(|motion| {
            if motion.has_work() {
                return true;
            }
            motion.complete();
            false
        });
    }

    fn stop_all(&mut self) {
        for motion in &mut self.motions {
            motion.stop();
        }
        self.retire();
    }

    fn publish_frames(&self, shared: &mut PresenterControl, initialize: bool) {
        for w in self.motions.iter().flat_map(Motion::windows) {
            let (handle, frame) =
                shared.frames.entry(w.wid).or_insert_with(|| (w.handle.clone(), w.frame));
            *frame = w.frame;
            if initialize {
                *handle = w.handle.clone();
            }
        }
    }

    fn has_work(&self) -> bool { self.motions.iter().any(Motion::has_work) }

    fn stop_windows(&mut self, windows: &[WindowId]) {
        if !windows.is_empty() {
            for motion in &mut self.motions {
                motion.stop_windows(windows);
            }
        }
    }

    pub fn handle_message(&mut self, message: Message) {
        self.retire();
        match message {
            Message::Camera(mut camera) => {
                let old = self.motions.iter().position(|m| matches!(m, Motion::Viewport(c) if c.identity.space == camera.identity.space))
                    .map(|idx| self.motions.swap_remove(idx));
                camera.replace(old.map(|m| {
                    let Motion::Viewport(c) = m else { unreachable!() };
                    c
                }));
                let mut windows: Vec<_> = camera.windows.iter().map(|w| w.wid).collect();
                windows.sort_unstable();
                self.stop_windows(&windows);
                camera.begin();
                self.motions.push(Motion::Viewport(*camera));
            }
            Message::Stop(mut windows) => {
                windows.sort_unstable();
                self.stop_windows(&windows);
            }
            Message::Replace(animation) => {
                let old = self
                    .motions
                    .iter()
                    .position(|m| matches!(m, Motion::Transition(_)))
                    .map(|idx| self.motions.swap_remove(idx));
                let next = match old {
                    Some(Motion::Transition(active)) => Some(active.replace_with(animation)),
                    None => ActiveAnimation::start(animation),
                    _ => unreachable!(),
                };
                if let Some(next) = next {
                    self.motions.push(Motion::Transition(next));
                }
            }
            Message::SkipToEnd(animation) => {
                self.motions.retain_mut(|m| {
                    if let Motion::Transition(a) = m {
                        a.animation.finish_all();
                        false
                    } else {
                        true
                    }
                });
                animation.skip_to_end();
            }
            _ => unreachable!("runner handles display control"),
        }
    }

    pub fn tick_at(&mut self, now: Instant) -> Option<Duration> {
        let delay = self
            .motions
            .iter_mut()
            .filter_map(|motion| motion.sample(now, &mut self.frames))
            .min();
        self.frames.flush();
        for motion in &mut self.motions {
            motion.finish_frame();
        }
        if !self.has_work() {
            self.frames.0.clear();
        }
        delay
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
                    app_state.handle.send(Request::set_window_frame(wid, target_frame, txid, true))
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
                        Message::Stop(..)
                        | Message::Camera(_)
                        | Message::StopCamera(_)
                        | Message::Displays(_) => {}
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
                positions.push((wid, target_frame));
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
                    .then(|| Request::SetWindowFrames(positions, txid, FrameMode::Position, true)),
                (!frames.is_empty())
                    .then(|| Request::SetWindowFrames(frames, txid, FrameMode::Full, true)),
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

impl Motion {
    fn windows(&self) -> &[PresentedWindow] {
        match self {
            Self::Transition(a) => &a.animation.windows,
            Self::Viewport(c) => &c.windows,
        }
    }

    fn windows_mut(&mut self) -> &mut Vec<PresentedWindow> {
        match self {
            Self::Transition(a) => &mut a.animation.windows,
            Self::Viewport(c) => &mut c.windows,
        }
    }

    fn store(&self) -> Option<WindowTxStore> {
        match self {
            Self::Transition(_) => None,
            Self::Viewport(c) => Some(c.store.clone()),
        }
    }

    fn stop_windows(&mut self, windows: &[WindowId]) {
        let store = self.store();
        self.windows_mut().retain_mut(|window| {
            if windows.binary_search(&window.wid).is_ok() {
                window.cancel(store.as_ref());
                false
            } else {
                true
            }
        });
        if let Self::Viewport(c) = self
            && c.windows.is_empty()
        {
            c.active = false;
        }
    }

    fn stop(&mut self) {
        let store = self.store();
        for window in self.windows_mut().iter_mut() {
            window.cancel(store.as_ref());
        }
        self.windows_mut().clear();
        if let Self::Viewport(c) = self {
            c.active = false;
        }
    }

    fn has_work(&self) -> bool {
        match self {
            Self::Transition(a) => !a.animation.is_empty() && !a.is_complete(),
            Self::Viewport(c) => c.active,
        }
    }

    fn sample(&mut self, now: Instant, frames: &mut FrameBatch) -> Option<Duration> {
        match self {
            Self::Viewport(c) => {
                c.sample_into(now, frames);
                c.active.then_some(c.interval)
            }
            Self::Transition(a) => {
                if a.animation.is_empty() || a.is_complete() {
                    return None;
                }
                if now + Duration::from_micros(1) < a.next_sample
                    && now.saturating_duration_since(a.started) < a.animation.duration
                {
                    return Some(a.next_sample - now);
                }
                let missed = now.saturating_duration_since(a.next_sample).as_nanos()
                    / a.animation.interval.as_nanos();
                a.next_sample += a.animation.interval.mul_f64((missed + 1) as f64);
                a.send_frame(now, frames);
                (!a.is_complete()).then_some(a.animation.interval)
            }
        }
    }

    fn finish_frame(&mut self) {
        match self {
            Self::Transition(a) if a.is_complete() && !a.ended => {
                a.animation.end();
                a.ended = true;
            }
            Self::Viewport(c) if !c.active => c.end(),
            _ => {}
        }
    }

    fn complete(&mut self) {
        match self {
            Self::Transition(_) => {}
            Self::Viewport(c) => {
                c.publish_state(Instant::now());
                if let Some(events) = c.events.take() {
                    events.send(super::Event::CameraFinished);
                }
            }
        }
    }
}

impl ActiveAnimation {
    fn start(mut animation: Animation) -> Option<Self> {
        if animation.is_empty() {
            return None;
        }
        animation.windows.sort_unstable_by_key(|w| w.wid.pid);
        animation.begin();
        let started = Instant::now();
        let next_sample = started + animation.interval;
        Some(Self {
            animation,
            started,
            progress: 0.0,
            next_sample,
            ended: false,
        })
    }

    fn replace_with(self, mut next: Animation) -> Self {
        for window in &mut next.windows {
            if let Some(old) = self.animation.windows.iter().find(|old| old.wid == window.wid) {
                window.from = old.frame;
                window.leased = old.leased;
            } else {
                window.begin();
            }
        }
        for mut old in self.animation.windows {
            if !next.handled_windows.contains(&old.wid) {
                old.from = old.frame;
                next.windows.push(old);
            } else if !next.windows.iter().any(|w| w.wid == old.wid) {
                old.cancel(None);
            }
        }
        next.windows.sort_unstable_by_key(|w| w.wid.pid);
        let started = Instant::now();
        let next_sample = started + next.interval;
        Self {
            animation: next,
            started,
            progress: 0.0,
            next_sample,
            ended: false,
        }
    }

    fn send_frame(&mut self, now: Instant, frames: &mut FrameBatch) {
        let t = if self.animation.duration.is_zero() {
            1.0
        } else {
            (now.saturating_duration_since(self.started).as_secs_f64()
                / self.animation.duration.as_secs_f64())
            .clamp(0.0, 1.0)
        };
        let t = t.max(self.progress);
        self.animation.stage_frame(t, self.progress, frames);
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
        self.windows.push(PresentedWindow {
            handle: handle.clone(),
            wid,
            wsid: None,
            from: start,
            to: finish,
            fixed: false,
            announced: false,
            frame: start,
            txid,
            leased: false,
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
            _ = window.handle.send(Request::set_window_frame(
                window.wid,
                window.to,
                window.txid,
                true,
            ));
        }
    }

    fn is_empty(&self) -> bool { self.windows.is_empty() }

    fn begin(&mut self) {
        for window in &mut self.windows {
            window.begin();
        }
    }

    fn finish_all(&mut self) {
        let mut frames = FrameBatch::default();
        self.stage_frame(1.0, 0.0, &mut frames);
        frames.flush();
        self.end();
    }

    fn stage_frame(&mut self, t: f64, previous: f64, frames: &mut FrameBatch) {
        stage_windows(
            &mut self.windows,
            Sample::Transition {
                progress: t,
                eased: ease(t),
                set_size: (previous < 0.5 && t >= 0.5) || t == 1.0,
            },
            frames,
        );
    }

    fn end(&mut self) {
        for window in &mut self.windows {
            window.end();
        }
    }
}

#[cfg(test)]
fn get_frame(a: CGRect, b: CGRect, t: f64) -> CGRect { interpolate_frame(a, b, ease(t)) }

fn interpolate_frame(a: CGRect, b: CGRect, s: f64) -> CGRect {
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

    fn transition_state(manager: &AnimationManager) -> Option<&ActiveAnimation> {
        manager.motions.iter().find_map(|m| match m {
            Motion::Transition(a) if !a.is_complete() && !a.animation.is_empty() => Some(a),
            _ => None,
        })
    }
    fn active_mut(manager: &mut AnimationManager) -> Option<&mut ActiveAnimation> {
        manager.motions.iter_mut().find_map(|m| match m {
            Motion::Transition(a) => Some(a),
            _ => None,
        })
    }
    fn camera_mut(manager: &mut AnimationManager) -> Option<&mut CameraAnimation> {
        manager.motions.iter_mut().find_map(|m| match m {
            Motion::Viewport(c) => Some(c),
            _ => None,
        })
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
            Observed::Request(Request::SetWindowFrames(frames, txid, FrameMode::Full, eui)) => {
                let (req_wid, req_frame) = frames[0];
                assert_eq!(req_wid, wid);
                assert_eq!(req_frame, frame);
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
            active_mut(&mut manager).unwrap().started = start;
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
            assert!(transition_state(&manager).is_none());
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
            let active = active_mut(&mut manager).unwrap();
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
        let active = active_mut(&mut manager).unwrap();
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
        assert_eq!(
            transition_state(&manager).unwrap().animation.windows[0].from,
            last
        );
        manager.tick_at(transition_state(&manager).unwrap().next_sample - Duration::from_millis(1));
        assert!(
            collect_requests(&mut rx).is_empty(),
            "old timestamps cannot sample a new transition"
        );
    }

    #[test]
    fn completed_camera_keeps_its_offset_after_native_window_clamping() {
        use crate::common::config::{ScrollingAlignment, ScrollingLayoutSettings};
        use crate::layout_engine::systems::ScrollingLayoutSystem;

        let (handle, _rx) = AppThreadHandle::channel();
        let hidden = WindowId::new(1, 1);
        let visible = WindowId::new(1, 2);
        let mut system = ScrollingLayoutSystem::new(&ScrollingLayoutSettings {
            column_width_ratio: 1.0,
            max_column_width_ratio: 1.0,
            alignment: ScrollingAlignment::Left,
            preserve_window_sizes: false,
            ..Default::default()
        });
        let layout = system.create_layout();
        system.add_window_after_selection(layout, hidden);
        system.add_window_after_selection(layout, visible);
        system.select_window(layout, hidden);
        system.prepare_layout(
            layout,
            rect(0.0, 0.0, 1000.0, 800.0),
            &Default::default(),
            &Default::default(),
        );
        let make_camera = |system: &ScrollingLayoutSystem, native: &[(WindowId, CGRect)]| {
            let (presentation, frames) = system.presentation(layout).unwrap();
            let state = Arc::new(Mutex::new(PresentedCamera {
                viewport: presentation.snapshot(Instant::now()),
                gesture: None,
                active: true,
                stopped: false,
            }));
            CameraAnimation {
                presentation,
                windows: frames
                    .into_iter()
                    .map(|(wid, world, fixed)| {
                        let frame = native.iter().find(|(id, _)| *id == wid).unwrap().1;
                        PresentedWindow {
                            handle: handle.clone(),
                            wid,
                            wsid: None,
                            from: frame,
                            to: world,
                            fixed,
                            announced: false,
                            leased: false,
                            frame,
                            txid: TransactionId::default(),
                        }
                    })
                    .collect(),
                gesture: None,
                active: true,
                identity: CameraIdentity {
                    space: SpaceId::new(1),
                    workspace: VirtualWorkspaceId::default(),
                    layout,
                },
                events: None,
                state,
                display: 1,
                animate: true,
                scale: 1.0,
                store: WindowTxStore::new(),
                interval: Duration::from_secs_f64(1.0 / 60.0),
                bound: None,
            }
        };
        let native = vec![
            (hidden, rect(0.0, 0.0, 1000.0, 800.0)),
            (visible, rect(1000.0, 0.0, 1000.0, 800.0)),
        ];
        system.select_window(layout, visible);
        let mut manager = AnimationManager::new();
        let camera = make_camera(&system, &native);
        let state = camera.state.clone();
        manager.handle_message(Message::Camera(Box::new(camera)));
        assert_eq!(camera_mut(&mut manager).unwrap().presentation.offset(), 0.0);
        let started = Instant::now();
        manager.tick_at(started + Duration::from_millis(50));
        let offset = camera_mut(&mut manager).unwrap().presentation.offset();
        assert!(
            offset > 0.0 && offset < 1000.0,
            "focus changes must still animate"
        );
        manager.tick_at(started + Duration::from_secs(2));
        system.commit_presented_viewport(layout, &state.lock().viewport);
        manager.retire();
        assert!(manager.motions.is_empty());

        // The completed animation parked the first window at -1000. macOS
        // reports it 40 points farther right. A click in the selected window
        // must not turn that native correction into a new viewport position.
        let native = vec![
            (hidden, rect(-960.0, 0.0, 1000.0, 800.0)),
            (visible, rect(0.0, 0.0, 1000.0, 800.0)),
        ];
        system.select_window(layout, visible);
        manager.handle_message(Message::Camera(Box::new(make_camera(&system, &native))));
        assert_eq!(
            camera_mut(&mut manager).unwrap().presentation.offset(),
            1000.0,
            "an unchanged viewport must keep its completed offset"
        );
        manager.tick_at(Instant::now() + Duration::from_millis(16));
        let camera = camera_mut(&mut manager).unwrap();
        assert_eq!(
            camera.windows.iter().find(|w| w.wid == visible).unwrap().frame,
            rect(0.0, 0.0, 1000.0, 800.0)
        );
        assert!(!manager.has_work(), "a click must not start another scroll");
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
        let now = transition_state(&manager).unwrap().started + Duration::from_millis(100);
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
        let store = WindowTxStore::new();
        let wsid = WindowServerId::new(1);
        let camera = CameraAnimation {
            presentation: system.presentation(layout).unwrap().0,
            windows: vec![PresentedWindow {
                handle: handle.clone(),
                wid,
                wsid: Some(wsid),
                from: rect(0.0, 0.0, 10.0, 10.0),
                to: system.presentation(layout).unwrap().1[0].1,
                fixed: false,
                announced: false,
                leased: false,
                frame: rect(0.0, 0.0, 10.0, 10.0),
                txid: TransactionId::default(),
            }],
            gesture: None,
            active: true,
            identity: CameraIdentity {
                space: SpaceId::new(1),
                workspace: VirtualWorkspaceId::default(),
                layout,
            },
            events: None,
            state: Arc::new(Mutex::new(PresentedCamera {
                viewport: system.presentation(layout).unwrap().0.snapshot(now),
                gesture: None,
                active: true,
                stopped: false,
            })),
            display: 1,
            animate: false,
            scale: 2.0,
            store: store.clone(),
            interval: Duration::from_secs_f64(1.0 / 60.0),
            bound: None,
        };
        manager.handle_message(Message::Camera(Box::new(camera)));
        assert!(transition_state(&manager).is_none());
        let Request::CancelWindowAnimation(_, through) = rx.try_recv().unwrap().1 else {
            panic!("cancel");
        };
        assert!(matches!(rx.try_recv().unwrap().1, Request::BeginWindowAnimation(id) if id == wid));
        // A native camera frame reuses the old outstanding generic wake.
        camera_mut(&mut manager).unwrap().windows[0].to.origin.x += 0.5;
        camera_mut(&mut manager).unwrap().sample(now);
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
        let presentation_txid = store.last_txid(&wsid);
        // Camera work remains eligible on consecutive 120Hz input refreshes.
        for tick in 1..=2 {
            let camera = camera_mut(&mut manager).unwrap();
            camera.presentation.update(0.001, Duration::from_millis(tick * 8));
            camera.sample(now + Duration::from_secs_f64(tick as f64 / 120.0));
            assert_eq!(collect_requests(&mut rx).len(), 1);
            assert_eq!(
                store.last_txid(&wsid),
                presentation_txid,
                "display sampling cannot advance transactions"
            );
            assert_eq!(
                store.get(&wsid).unwrap().target,
                None,
                "free gestures have no semantic target"
            );
        }
        camera_mut(&mut manager)
            .unwrap()
            .presentation
            .update(0.001, Duration::from_millis(30));
        camera_mut(&mut manager).unwrap().sample(now + Duration::from_millis(30));
        let Request::InteractiveFramesPending(old_wake) = rx.try_recv().unwrap().1 else {
            panic!("wake");
        };
        manager.handle_message(Message::Stop(vec![wid]));
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
        camera_mut(&mut manager).unwrap().sample(now + Duration::from_secs(1));
        assert!(!manager.has_work());
        assert!(rx.try_recv().is_err());
        old_wake.drain_with(|_, frame, _, _, source, sequence| {
            assert_eq!(frame, dragged);
            assert_eq!(source, FrameSource::Drag);
            assert!(sequence > through);
        });
        system.cancel_viewport_gesture(layout);
        {
            let c = camera_mut(&mut manager).unwrap();
            let (presentation, frames) = system.presentation(layout).unwrap();
            c.presentation = presentation;
            let mut base_frame = frames[0].1;
            base_frame.origin.x += 0.5;
            c.windows.push(PresentedWindow {
                handle,
                wid,
                wsid: None,
                from: dragged,
                to: base_frame,
                fixed: false,
                announced: false,
                leased: false,
                frame: dragged,
                txid: TransactionId::default(),
            });
            c.active = true;
            c.begin();
            collect_requests(&mut rx);
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
        let idx = manager.motions.iter().position(|m| matches!(m, Motion::Viewport(_))).unwrap();
        let Motion::Viewport(mut camera) = manager.motions.swap_remove(idx) else {
            panic!("viewport");
        };
        // Removing completed cameras must not make the next focus jump instant.
        system.scroll_by_delta(layout, 1.0);
        camera.presentation = system.presentation(layout).unwrap().0;
        camera.animate = true;
        camera.active = true;
        let before = camera.windows[0].frame;
        let restarted = Instant::now();
        camera.replace(None);
        assert_eq!(
            camera.presentation.frame_at_offset(
                camera.windows[0].to,
                false,
                2.0,
                camera.presentation.offset()
            ),
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
        let (sender, commands_rx) = AnimationSender::channel();
        let (events, mut events_rx) = crate::actor::channel();
        camera.events = Some(events);
        let state = camera.state.clone();
        state.lock().active = true;
        let runner = std::thread::spawn(move || AnimationManager::run(commands_rx));
        sender.send(Message::Camera(Box::new(camera))).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while state.lock().active && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            !state.lock().active,
            "completion cannot depend on another reactor event"
        );
        assert_eq!(sender.control.lock().frames[&wid].1, expected_final);
        drop(sender);
        runner.join().unwrap();
        assert!(matches!(
            events_rx.try_recv().unwrap().1,
            super::super::Event::CameraFinished
        ));
        let terminal = collect_requests(&mut rx);
        assert!(matches!(terminal.as_slice(), [
            Observed::Request(Request::BeginWindowAnimation(_)),
            Observed::Frame { frame, .. },
            Observed::Request(Request::EndWindowAnimation(_))
        ] if *frame == expected_final));
    }

    #[test]
    fn cancellation_discards_pending_frames_and_does_not_replay_the_target() {
        let (handle, mut rx) = AppThreadHandle::channel();
        let wid = WindowId::new(1, 1);
        let (sender, commands_rx) = AnimationSender::channel();
        let runner = std::thread::spawn(move || AnimationManager::run(commands_rx));
        sender
            .send(Message::Replace(animation(
                &handle,
                wid,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(100.0, 50.0, 10.0, 10.0),
            )))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        // Leave the display wake outstanding so cancellation must discard it.
        while !sender
            .control
            .lock()
            .frames
            .get(&wid)
            .is_some_and(|(_, frame)| frame.origin.x > 0.0)
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        let frames = sender.cancel(vec![wid]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, wid);
        assert!(frames[0].1.origin.x > 0.0 && frames[0].1.origin.x < 100.0);
        drop(sender);
        runner.join().unwrap();
        let requests = collect_requests(&mut rx);
        assert!(requests.iter().any(
            |r| matches!(r, Observed::Request(Request::CancelWindowAnimation(id, _)) if *id == wid)
        ));
        assert!(
            requests.iter().all(|r| matches!(
                r,
                Observed::Request(
                    Request::BeginWindowAnimation(_) | Request::CancelWindowAnimation(..)
                )
            )),
            "cancelled frames and the target cannot be replayed: {requests:?}"
        );
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

        manager.tick_at(transition_state(&manager).unwrap().started + Duration::from_millis(10));
        let active = transition_state(&manager).unwrap();
        let continuing_frame = active.animation.windows[0].frame;
        assert_animation_pos(&collect_requests(&mut rx)[0], wid, continuing_frame.origin);

        manager.handle_message(Message::Replace(second));
        assert!(collect_requests(&mut rx).is_empty());

        let resumed_start = transition_state(&manager).unwrap().animation.windows[0].from;
        assert_eq!(resumed_start, continuing_frame);

        manager.tick_at(transition_state(&manager).unwrap().started + Duration::from_millis(10));
        let expected_next = get_frame(resumed_start, rect(80.0, 90.0, 10.0, 10.0), 1.0 / 30.0);
        assert_animation_pos(&collect_requests(&mut rx)[0], wid, expected_next.origin);
    }

    fn animation_contains(manager: &AnimationManager, wid: WindowId) -> bool {
        transition_state(manager).is_some_and(|a| a.animation.windows.iter().any(|w| w.wid == wid))
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

        let carried = transition_state(&manager)
            .unwrap()
            .animation
            .windows
            .iter()
            .find(|w| w.wid == wid2)
            .unwrap();
        assert_eq!(carried.to, rect(60.0, 60.0, 10.0, 10.0));
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
