//! One recognizer per physical device; semantic lifecycle and latest motion.
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use multitouch::{GestureEndReason, GestureEvent, GesturePhase, GestureRecognizer, GestureTypes};
use objc2_core_foundation::CGRect;
use objc2_core_graphics::CGEvent;
use parking_lot::Mutex;

use crate::actor::reactor::{Event, Sender};
use crate::common::collections::HashMap;
use crate::common::config::{Config, HapticPattern, LayoutMode};
use crate::sys::geometry::CGRectExt;
use crate::sys::gesture::{Owner, Ownership};
use crate::sys::screen::{CoordinateConverter, SpaceId};

#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub session: u64,
    pub total_x: f64,
    pub timestamp: Duration,
}
#[derive(Clone, Debug, Default)]
pub struct MotionPublisher(Arc<Mutex<Option<Motion>>>);
impl MotionPublisher {
    pub fn publish(&self, motion: Motion) { *self.0.lock() = Some(motion); }

    pub fn latest(&self, session: u64) -> Option<Motion> {
        self.0.lock().filter(|m| m.session == session)
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActionConfig {
    fingers: usize,
    pub invert: bool,
    tolerance: f64,
    threshold: f64,
    pub scrolling: bool,
    pub propagate: bool,
    pub boundary_threshold: f64,
    pub animate: Option<bool>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    scroll: Option<ActionConfig>,
    workspace: Option<ActionConfig>,
    consume: bool,
    pub skip_empty: Option<bool>,
    pub haptic: Option<HapticPattern>,
}
impl Context {
    pub fn new(
        session: u64,
        epoch: u64,
        space: SpaceId,
        width: f64,
        mode: LayoutMode,
        settings: Settings,
        started: Duration,
    ) -> Option<Self> {
        Some(Self {
            session,
            epoch,
            space,
            width,
            mode,
            action: settings.action(mode)?,
            settings,
            started,
        })
    }
}
impl Settings {
    pub fn new(config: &Config) -> Self {
        let g = &config.settings.gestures;
        let s = &config.settings.layout.scrolling.gestures;
        let tolerance = |x: f64| {
            if x > 1.0 {
                (x / 100.0).clamp(0.0, 1.0)
            } else {
                x.clamp(0.0, 1.0)
            }
        };
        Self {
            consume: g.consume_dock_swipe,
            skip_empty: Some(g.skip_empty),
            haptic: g.haptics_enabled.then_some(g.haptic_pattern),
            workspace: g.enabled.then_some(ActionConfig {
                fingers: g.fingers.max(1),
                invert: g.invert_horizontal_swipe,
                tolerance: tolerance(g.swipe_vertical_tolerance),
                threshold: g.distance_pct.clamp(0.01, 1.0),
                scrolling: false,
                propagate: false,
                boundary_threshold: 0.0,
                animate: None,
            }),
            scroll: s.enabled.then_some(ActionConfig {
                fingers: s.fingers.max(1),
                invert: s.invert_horizontal,
                tolerance: tolerance(s.vertical_tolerance),
                threshold: 0.0,
                scrolling: true,
                propagate: s.propagate_to_workspace_swipe,
                boundary_threshold: s.workspace_switch_threshold.max(0.0),
                animate: s.animate,
            }),
        }
    }

    pub fn enabled(self) -> bool { self.scroll.is_some() || self.workspace.is_some() }

    fn minimum_contacts(self) -> usize {
        self.scroll
            .into_iter()
            .chain(self.workspace)
            .map(|a| a.fingers)
            .min()
            .unwrap_or(usize::MAX)
    }

    fn action(self, mode: LayoutMode) -> Option<ActionConfig> {
        if mode == LayoutMode::Scrolling && self.scroll.is_some() {
            self.scroll
        } else {
            self.workspace
        }
    }

    fn action_for(self, mode: LayoutMode, fingers: usize) -> Option<ActionConfig> {
        self.scroll
            .filter(|a| mode == LayoutMode::Scrolling && a.fingers == fingers)
            .or_else(|| self.workspace.filter(|a| a.fingers == fingers))
    }
}
#[derive(Clone, Debug)]
pub struct Context {
    pub session: u64,
    pub epoch: u64,
    pub space: SpaceId,
    pub width: f64,
    mode: LayoutMode,
    pub action: ActionConfig,
    pub settings: Settings,
    pub started: Duration,
}
#[derive(Clone, Debug)]
pub enum Lifecycle {
    Begin {
        context: Context,
        motion: MotionPublisher,
        control: Control,
    },
    End {
        sample: Motion,
        cancelled: bool,
    },
    Workspace {
        context: Context,
        next: bool,
        control: Control,
    },
    Reset,
}
#[derive(Debug)]
struct Routing {
    epoch: u64,
    settings: Settings,
    enabled: bool,
    screens: Vec<(CGRect, SpaceId, LayoutMode)>,
    converter: CoordinateConverter,
}
#[derive(Clone, Debug)]
pub struct Control {
    routing: Arc<Mutex<Routing>>,
    owner: Arc<Mutex<Ownership>>,
    stop: Arc<AtomicBool>,
    pending: Arc<Mutex<PendingInput>>,
    wake_tx: crossbeam_channel::Sender<()>,
    wake_rx: crossbeam_channel::Receiver<()>,
}
#[derive(Debug, Default)]
struct PendingInput {
    frame: Option<(u64, Vec<multitouch::Contact>)>,
    minimum_contacts: usize,
    contact_counts: HashMap<u64, usize>,
    removed: Vec<u64>,
}
impl Control {
    pub fn new(config: &Config) -> Self {
        let (wake_tx, wake_rx) = crossbeam_channel::bounded(1);
        Self {
            routing: Arc::new(Mutex::new(Routing {
                epoch: 0,
                settings: Settings::new(config),
                enabled: false,
                screens: Vec::new(),
                converter: CoordinateConverter::default(),
            })),
            owner: Arc::default(),
            stop: Arc::default(),
            pending: Arc::new(Mutex::new(PendingInput {
                minimum_contacts: usize::MAX,
                ..PendingInput::default()
            })),
            wake_tx,
            wake_rx,
        }
    }

    fn wake(&self) { let _ = self.wake_tx.try_send(()); }

    fn publish(&self, event: multitouch::MonitorEvent<'_>) {
        match event {
            multitouch::MonitorEvent::Contacts { device, contacts } => {
                if let Some(id) = device.device_id() {
                    self.publish_contacts(id, contacts);
                }
            }
            multitouch::MonitorEvent::DeviceRemoved(id) => {
                let mut pending = self.pending.lock();
                pending.contact_counts.remove(&id);
                pending.removed.push(id);
                let evicted = if pending.frame.as_ref().is_some_and(|(device, _)| *device == id) {
                    pending.frame.take()
                } else {
                    None
                };
                drop(pending);
                self.wake();
                drop(evicted);
            }
        }
    }

    fn publish_contacts(&self, id: u64, contacts: &[multitouch::Contact]) {
        let count = contacts.iter().filter(|c| !c.is_palm() && c.state().is_active()).count();
        let mut pending = self.pending.lock();
        let previous = pending.contact_counts.insert(id, count);
        // Unsupported motion needs no recognition or ownership updates. Still
        // deliver every topology change, including staggered and full lifts.
        if count < pending.minimum_contacts && previous == Some(count) {
            return;
        }
        let evicted = pending.frame.replace((id, contacts.to_vec()));
        drop(pending);
        self.wake();
        drop(evicted);
    }

    fn wait(&self, deadline: Option<Instant>) -> PendingInput {
        if let Some(deadline) = deadline {
            let _ = self.wake_rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        } else {
            let _ = self.wake_rx.recv();
        }
        let mut pending = self.pending.lock();
        PendingInput {
            frame: pending.frame.take(),
            removed: std::mem::take(&mut pending.removed),
            ..PendingInput::default()
        }
    }

    pub fn ownership_guard(&self) -> parking_lot::MutexGuard<'_, Ownership> { self.owner.lock() }

    fn claim(&self, context: &Context) -> bool {
        let routing = self.routing.lock();
        if routing.epoch != context.epoch || !routing.enabled {
            return false;
        }
        let mut owner = self.owner.lock();
        if (owner.session == context.session && owner.owner == Owner::System)
            || (context.settings.consume && owner.dock_owner == Some(Owner::System))
        {
            return false;
        }
        *owner = Ownership {
            session: context.session,
            owner: Owner::Rift,
            consume: context.settings.consume,
            touching: true,
            dock_owner: owner.dock_owner,
        };
        true
    }

    pub fn valid(&self, epoch: u64) -> bool {
        let r = self.routing.lock();
        r.epoch == epoch && r.enabled
    }

    pub fn configure(
        &self,
        settings: Settings,
        enabled: bool,
        screens: Vec<(CGRect, SpaceId, LayoutMode)>,
        converter: CoordinateConverter,
    ) {
        let mut r = self.routing.lock();
        r.converter = converter;
        if r.settings == settings && r.enabled == enabled && r.screens == screens {
            return;
        }
        r.settings = settings;
        r.enabled = enabled;
        r.screens = screens;
        drop(r);
        let mut pending = self.pending.lock();
        pending.minimum_contacts = if enabled {
            settings.minimum_contacts()
        } else {
            usize::MAX
        };
        pending.contact_counts.clear();
        drop(pending);
        self.wake();
    }

    pub fn retire_session(&self, epoch: u64, session: u64) {
        let mut r = self.routing.lock();
        let mut o = self.owner.lock();
        if r.epoch == epoch && o.session == session {
            r.epoch += 1;
            *o = Ownership::default();
            drop(o);
            drop(r);
            self.wake();
        }
    }

    pub fn retire(&self) {
        self.routing.lock().epoch += 1;
        *self.owner.lock() = Ownership::default();
        self.wake();
    }

    pub fn reset(&self, tx: &Sender) {
        self.retire();
        tx.send(Event::Gesture(Lifecycle::Reset));
    }

    pub fn stop(&self, tx: &Sender) {
        self.stop.store(true, Ordering::Release);
        self.reset(tx);
    }

    fn context(&self, session: u64, started: Duration) -> Option<Context> {
        let r = self.routing.lock();
        if !r.enabled {
            return None;
        }
        // Exactly one cursor lookup per physical stroke, never per motion frame.
        let event = CGEvent::new(None)?;
        let point = CGEvent::location(Some(&event));
        let point = r.converter.convert_point(point).unwrap_or(point);
        let &(frame, space, mode) = r.screens.iter().find(|(frame, _, _)| frame.contains(point))?;
        Context::new(
            session,
            r.epoch,
            space,
            frame.size.width,
            mode,
            r.settings,
            started,
        )
    }

    pub fn start(&self, tx: Sender) {
        let control = self.clone();
        std::thread::Builder::new()
            .name("multitouch".into())
            .spawn(move || run(control, tx))
            .expect("start multitouch worker");
    }
}

/// A quarter-pad drag moves one working-area width, independent of column size.
const SCROLL_SENSITIVITY: f64 = 4.0;
const VERTICAL_INTENT_DISTANCE: f64 = 0.03;
pub fn pixels(translation: f64, width: f64, invert: bool) -> f64 {
    translation * SCROLL_SENSITIVITY * width * if invert { -1.0 } else { 1.0 }
}
fn swipe_recognizer(fingers: usize) -> GestureRecognizer {
    let mut recognizer = GestureRecognizer::new(fingers)
        .with_exact_finger_count(true)
        .with_gesture_types(GestureTypes::SWIPE);
    recognizer.minimum_swipe_translation = 0.003;
    recognizer
}
struct DeviceSession {
    context: Option<Context>,
    recognizer: GestureRecognizer,
    owner: Owner,
    blocked: bool,
    active: bool,
    epoch: u64,
    sample: Motion,
    max_contacts: usize,
    lifting: Option<Duration>,
}
impl DeviceSession {
    fn new(context: Option<Context>, epoch: u64, session: u64, time: Duration) -> Self {
        Self {
            recognizer: swipe_recognizer(context.as_ref().map_or(3, |c| c.action.fingers)),
            context,
            owner: Owner::Undecided,
            blocked: false,
            active: false,
            epoch,
            sample: Motion {
                session,
                total_x: 0.0,
                timestamp: time,
            },
            max_contacts: 0,
            lifting: None,
        }
    }

    fn end(&mut self, tx: &Sender, cancelled: bool, timestamp: Duration) {
        if self.active {
            self.sample.timestamp = timestamp;
            tx.send(Event::Gesture(Lifecycle::End { sample: self.sample, cancelled }));
            self.active = false;
        }
        self.blocked = true;
        self.lifting = None;
    }

    fn expire_lift(&mut self, time: Duration, tx: &Sender) {
        if self.lifting.is_some_and(|deadline| time >= deadline) {
            self.end(tx, true, time);
        }
    }

    /// Returns the active count, or None after full lift, including after a
    /// recognizer topology end/timeout. Semantic end is independently idempotent.
    fn frame(
        &mut self,
        contacts: &[multitouch::Contact],
        time: Duration,
        tx: &Sender,
        control: &Control,
        motion: &MotionPublisher,
    ) -> Option<usize> {
        let count = contacts.iter().filter(|c| !c.is_palm() && c.state().is_active()).count();
        if !control.valid(self.epoch) {
            self.end(tx, true, time);
            self.owner = Owner::System;
        }
        self.expire_lift(time, tx);
        if count == 0 {
            self.end(tx, false, time);
            return None;
        }
        if self.lifting.is_some() {
            if count >= self.recognizer.required_finger_count {
                self.end(tx, true, time);
            }
            return Some(count);
        }
        if self.blocked || self.owner == Owner::System || self.context.is_none() {
            return Some(count);
        }
        if self.active && count < self.recognizer.required_finger_count {
            // Real lifts are staggered. Freeze the last complete frame while
            // waiting briefly for full lift; persistent topology changes cancel.
            self.blocked = true;
            self.lifting = Some(time + Duration::from_millis(50));
            return Some(count);
        }
        if self.owner == Owner::Undecided {
            if count < self.max_contacts {
                self.end(tx, true, time);
                self.owner = Owner::System;
                return Some(count);
            }
            self.max_contacts = count;
            if let Some(c) = &mut self.context {
                if let Some(action) = c.settings.action_for(c.mode, count) {
                    if action != c.action {
                        c.action = action;
                        self.recognizer = swipe_recognizer(action.fingers);
                    }
                } else if count
                    > c.settings
                        .scroll
                        .map_or(0, |a| a.fingers)
                        .max(c.settings.workspace.map_or(0, |a| a.fingers))
                {
                    self.end(tx, true, time);
                    self.owner = Owner::System;
                    return Some(count);
                }
            }
        }
        // Below the configured count, only topology/lift bookkeeping matters.
        if count < self.recognizer.required_finger_count {
            return Some(count);
        }
        if let Some(event) = self.recognizer.process(contacts) {
            self.event(event, time, tx, control, motion);
        }
        if self.owner == Owner::Rift && count > self.recognizer.required_finger_count {
            self.end(tx, true, time);
            // Keep owned native delivery suppressed through its actual end.
        }
        Some(count)
    }

    fn event(
        &mut self,
        event: GestureEvent,
        time: Duration,
        tx: &Sender,
        control: &Control,
        motion: &MotionPublisher,
    ) {
        let Some(c) = &self.context else {
            self.owner = Owner::System;
            return;
        };
        if let GestureEvent::Swipe(swipe) = event {
            match swipe.phase {
                GesturePhase::Began | GesturePhase::Changed => {
                    let x = f64::from(swipe.translation.x);
                    let y = f64::from(swipe.translation.y).abs();
                    if self.owner == Owner::Undecided {
                        if y > c.action.tolerance || (y >= VERTICAL_INTENT_DISTANCE && y >= x.abs())
                        {
                            self.owner = Owner::System;
                        } else if y >= x.abs() {
                            // Like the old armed swipe handler, allow initial
                            // placement noise to resolve into horizontal intent.
                            return;
                        } else if control.claim(c) {
                            self.owner = Owner::Rift;
                            tracing::debug!(session = c.session, space = ?c.space, scrolling = c.action.scrolling, fingers = c.action.fingers, "gesture claimed");
                            if c.action.scrolling {
                                self.active = true;
                                tx.send(Event::Gesture(Lifecycle::Begin {
                                    context: c.clone(),
                                    motion: motion.clone(),
                                    control: control.clone(),
                                }));
                            }
                        } else {
                            self.owner = Owner::System;
                        }
                    }
                    if self.owner != Owner::Rift {
                        return;
                    }
                    let total_x = pixels(x, c.width, c.action.invert);
                    // Compare cumulative travel to the last accepted position:
                    // sensor wobble stays still, but slow sub-point steps accumulate.
                    if !c.action.scrolling || (total_x - self.sample.total_x).abs() >= 1.0 {
                        self.sample.total_x = total_x;
                    }
                    self.sample.timestamp = time;
                    if c.action.scrolling {
                        motion.publish(self.sample);
                    } else if x.abs() >= c.action.threshold {
                        self.blocked = true;
                        tx.send(Event::Gesture(Lifecycle::Workspace {
                            context: c.clone(),
                            next: (x < 0.0) != c.action.invert,
                            control: control.clone(),
                        }));
                    }
                }
                GesturePhase::Ended(reason) => self.end(
                    tx,
                    !matches!(reason, GestureEndReason::Lifted | GestureEndReason::TimedOut),
                    time,
                ),
                GesturePhase::Cancelled | GesturePhase::Failed => self.end(tx, true, time),
                _ => {}
            }
        } else if let GestureEvent::UnresolvedEnded(_) = event {
            self.end(tx, true, time);
            self.owner = Owner::System;
        }
    }
}
fn run(control: Control, tx: Sender) {
    let callback_control = control.clone();
    let monitor = multitouch::Monitor::with_handler(move |event| callback_control.publish(event));
    if !monitor.start() {
        tracing::warn!("Multitouch monitor unavailable");
        return;
    }
    let origin = Instant::now();
    let mut devices: HashMap<u64, DeviceSession> = HashMap::default();
    let motion = MotionPublisher::default();
    let mut next_session = 0;
    let mut ui_device = None;
    while !control.stop.load(Ordering::Acquire) && !tx.is_closed() {
        // Block until an event, or the exact one-shot staggered-lift deadline.
        let deadline = devices.values().filter_map(|s| s.lifting).min().map(|time| origin + time);
        let pending = control.wait(deadline);
        if control.stop.load(Ordering::Acquire) || tx.is_closed() {
            break;
        }
        let now = Instant::now();
        let time = now.duration_since(origin);
        for id in pending.removed {
            if let Some(mut s) = devices.remove(&id) {
                s.end(&tx, true, time);
            }
            if ui_device == Some(id) {
                ui_device = None;
                *control.owner.lock() = Ownership::default();
            }
        }
        for (&id, s) in &mut devices {
            s.expire_lift(time, &tx);
            if !control.valid(s.epoch) {
                s.end(&tx, true, time);
                if ui_device == Some(id) {
                    control.owner.lock().touching = false;
                }
            }
        }
        if let Some((id, contacts)) = pending.frame {
            let s = match devices.entry(id) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    if !contacts.iter().any(|c| !c.is_palm() && c.state().is_active()) {
                        continue;
                    }
                    next_session += 1;
                    let context = if ui_device.is_none() {
                        control.context(next_session, time)
                    } else {
                        None
                    };
                    let epoch =
                        context.as_ref().map_or_else(|| control.routing.lock().epoch, |c| c.epoch);
                    if context.is_some() {
                        ui_device = Some(id);
                    }
                    entry.insert(DeviceSession::new(context, epoch, next_session, time))
                }
            };
            let Some(count) = s.frame(&contacts, time, &tx, &control, &motion) else {
                devices.remove(&id);
                if ui_device == Some(id) {
                    ui_device = None;
                    let mut owner = control.owner.lock();
                    owner.touching = false;
                    owner.dock_owner = None;
                }
                continue;
            };
            if ui_device == Some(id) {
                let consume = s.context.as_ref().is_some_and(|c| c.settings.consume)
                    && (count == s.recognizer.required_finger_count || s.owner == Owner::Rift);
                let routing = control.routing.lock();
                // Consistent lock order: routing, then the tiny ownership snapshot.
                if routing.epoch == s.epoch && routing.enabled {
                    let mut owner = control.owner.lock();
                    if owner.session == s.sample.session && owner.owner == Owner::System {
                        s.owner = Owner::System;
                    }
                    *owner = Ownership {
                        session: s.sample.session,
                        owner: s.owner,
                        consume,
                        touching: true,
                        dock_owner: owner.dock_owner,
                    };
                }
            }
        }
    }
    for s in devices.values_mut() {
        s.end(&tx, true, origin.elapsed());
    }
    monitor.stop();
    *control.owner.lock() = Ownership::default();
}

#[cfg(test)]
mod tests {
    use multitouch::{Contact, ContactState, Finger, Hand, Point, Vector};

    use super::*;

    #[test]
    fn dormant_worker_is_interrupted_by_control_changes() {
        for change in ["configure", "retire", "stop"] {
            let config = Config::default();
            let control = Control::new(&config);
            let worker = control.clone();
            let (ready_tx, ready_rx) = crossbeam_channel::bounded(0);
            let (done_tx, done_rx) = crossbeam_channel::bounded(1);
            let thread = std::thread::spawn(move || {
                ready_tx.send(()).unwrap();
                worker.wait(None);
                done_tx.send(worker.stop.load(Ordering::Acquire)).unwrap();
            });
            ready_rx.recv().unwrap();
            match change {
                "configure" => control.configure(
                    Settings::new(&config),
                    true,
                    Vec::new(),
                    CoordinateConverter::default(),
                ),
                "retire" => control.retire(),
                "stop" => {
                    let (tx, _rx) = crate::actor::channel();
                    control.stop(&tx);
                }
                _ => unreachable!(),
            }
            assert_eq!(
                done_rx
                    .recv_timeout(Duration::from_secs(1))
                    .expect("control change must wake the worker"),
                change == "stop",
            );
            thread.join().unwrap();
        }
    }

    #[test]
    fn unsupported_contact_motion_does_not_wake_worker_but_topology_does() {
        let mut config = Config::default();
        config.settings.gestures.enabled = true;
        config.settings.gestures.fingers = 3;
        config.settings.layout.scrolling.gestures.enabled = false;
        let control = Control::new(&config);
        control.configure(
            Settings::new(&config),
            true,
            Vec::new(),
            CoordinateConverter::default(),
        );
        control.publish_contacts(1, &frame(2, 0.1, 0.5));
        assert_eq!(control.wait(None).frame.unwrap().0, 1);
        for step in 0..1000 {
            control.publish_contacts(1, &frame(2, step as f32 / 1000.0, 0.5));
        }
        assert!(control.wake_rx.try_recv().is_err());
        assert!(control.pending.lock().frame.is_none());
        control.configure(
            Settings::new(&config),
            true,
            Vec::new(),
            CoordinateConverter::default(),
        );
        control.publish_contacts(1, &frame(2, 0.7, 0.5));
        assert!(
            control.wake_rx.try_recv().is_err(),
            "unchanged routing must keep admission cached"
        );

        // Adding the configured third finger enables continuous motion. Both
        // staggered and full lifts must still reach the session's lifecycle.
        for count in [3, 3, 2, 0] {
            control.publish_contacts(1, &frame(count, 0.5, 0.5));
            assert_eq!(control.wait(None).frame.unwrap().1.len(), count);
        }
        // A second device and a changed configuration have separate admission.
        control.publish_contacts(2, &frame(2, 0.2, 0.5));
        assert_eq!(control.wait(None).frame.unwrap().0, 2);
        config.settings.gestures.fingers = 2;
        control.configure(
            Settings::new(&config),
            true,
            Vec::new(),
            CoordinateConverter::default(),
        );
        control.wait(None);
        for _ in 0..2 {
            control.publish_contacts(1, &frame(2, 0.5, 0.5));
            assert_eq!(control.wait(None).frame.unwrap().1.len(), 2);
        }
    }

    #[test]
    fn coalesced_wakeups_preserve_every_device_removal() {
        let control = Control::new(&Config::default());
        for id in [1, 2, 3] {
            control.publish(multitouch::MonitorEvent::DeviceRemoved(id));
        }
        control.retire();
        let pending = control.wait(None);
        assert_eq!(pending.removed, [1, 2, 3]);
        assert!(pending.frame.is_none());
    }
    fn frame(count: usize, x: f32, y: f32) -> Vec<Contact> {
        (0..count)
            .map(|i| {
                Contact::new(
                    0,
                    0.0,
                    i as i32,
                    ContactState::Touching,
                    Some(Finger::Index),
                    Some(Hand::Right),
                    Vector::new(Point::new(x + i as f32 * 0.05, y), Point::ZERO),
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    Vector::new(Point::ZERO, Point::ZERO),
                    0.0,
                )
            })
            .collect()
    }
    type Fixture = (
        DeviceSession,
        Control,
        MotionPublisher,
        Sender,
        crate::actor::Receiver<Event>,
    );

    fn setup(scrolling: bool) -> Fixture { setup_settings(scrolling, 3, false) }
    fn setup_settings(scrolling: bool, workspace_fingers: usize, invert: bool) -> Fixture {
        let mut config = Config::default();
        config.settings.gestures.enabled = true;
        config.settings.gestures.fingers = workspace_fingers;
        config.settings.gestures.invert_horizontal_swipe = invert;
        config.settings.gestures.distance_pct = 0.08;
        config.settings.gestures.haptics_enabled = false;
        config.settings.layout.scrolling.gestures.enabled = scrolling;
        config.settings.layout.scrolling.gestures.fingers = 3;
        let settings = Settings::new(&config);
        let control = Control::new(&config);
        let mode = if scrolling {
            LayoutMode::Scrolling
        } else {
            LayoutMode::Traditional
        };
        control.configure(settings, true, Vec::new(), CoordinateConverter::default());
        let c =
            Context::new(1, 0, SpaceId::new(10), 1000.0, mode, settings, Duration::ZERO).unwrap();
        let (tx, rx) = crate::actor::channel();
        (
            DeviceSession::new(Some(c), 0, 1, Duration::ZERO),
            control,
            MotionPublisher::default(),
            tx,
            rx,
        )
    }
    #[test]
    fn continuous_motion_commits_once_and_topology_requires_full_lift() {
        let (mut s, c, m, tx, mut rx) = setup(true);
        s.frame(&frame(2, 0.4, 0.4), Duration::ZERO, &tx, &c, &m);
        assert!(rx.try_recv().is_err());
        s.frame(&frame(3, 0.4, 0.4), Duration::ZERO, &tx, &c, &m);
        s.frame(&frame(3, 0.39, 0.401), Duration::from_millis(10), &tx, &c, &m);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::Begin { .. })
        ));
        s.frame(&frame(3, 0.37, 0.6), Duration::from_millis(20), &tx, &c, &m);
        assert_eq!(s.owner, Owner::Rift); // committed direction ignores later wobble
        assert!((m.latest(1).unwrap().total_x + 120.0).abs() < 0.01);
        assert!(rx.try_recv().is_err()); // no per-frame actor traffic
        s.frame(&frame(4, 0.37, 0.6), Duration::from_millis(30), &tx, &c, &m);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::End { cancelled: true, .. })
        ));
        s.frame(&frame(3, 0.2, 0.4), Duration::from_millis(40), &tx, &c, &m);
        assert!(rx.try_recv().is_err());
        assert!(s.frame(&[], Duration::from_millis(50), &tx, &c, &m).is_none());
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn workspace_threshold_and_inversion_fire_only_once() {
        for invert in [false, true] {
            let (mut s, c, m, tx, mut rx) = setup_settings(false, 3, invert);
            for (i, x) in [0.5, 0.49, 0.45, 0.4, 0.3].into_iter().enumerate() {
                s.frame(
                    &frame(3, x, 0.5),
                    Duration::from_millis(i as u64 * 10),
                    &tx,
                    &c,
                    &m,
                );
                if i < 3 {
                    assert!(rx.try_recv().is_err());
                }
            }
            let Event::Gesture(Lifecycle::Workspace { next, context, .. }) =
                rx.try_recv().unwrap().1
            else {
                panic!("workspace action");
            };
            assert_eq!(next, !invert);
            assert_eq!(context.space, SpaceId::new(10));
            assert!(rx.try_recv().is_err());
            assert!(m.latest(1).is_none());
        }
    }
    #[test]
    fn vertical_release_and_config_reset_do_not_rearm() {
        for reset in [false, true] {
            let (mut s, c, m, tx, mut rx) = setup(true);
            s.frame(&frame(3, 0.5, 0.5), Duration::ZERO, &tx, &c, &m);
            if reset {
                c.reset(&tx);
                rx.try_recv().unwrap();
            }
            s.frame(&frame(3, 0.501, 0.6), Duration::from_millis(10), &tx, &c, &m);
            s.frame(&frame(3, 0.3, 0.6), Duration::from_millis(20), &tx, &c, &m);
            assert_eq!(s.owner, Owner::System);
            assert!(rx.try_recv().is_err());
            assert!(s.frame(&[], Duration::from_millis(30), &tx, &c, &m).is_none());
        }
    }

    #[test]
    fn slow_horizontal_swipe_tolerates_initial_finger_placement_noise() {
        for scrolling in [false, true] {
            let (mut s, c, m, tx, mut rx) = setup(scrolling);
            s.frame(&frame(3, 0.4, 0.4), Duration::ZERO, &tx, &c, &m);
            s.frame(&frame(3, 0.401, 0.405), Duration::from_millis(100), &tx, &c, &m);
            assert_eq!(
                s.owner,
                Owner::Undecided,
                "placement noise must not reject the stroke"
            );
            s.frame(&frame(3, 0.41, 0.405), Duration::from_millis(500), &tx, &c, &m);
            s.frame(&frame(3, 0.5, 0.405), Duration::from_secs(5), &tx, &c, &m);
            assert_eq!(s.owner, Owner::Rift);
            let event = rx.try_recv().unwrap().1;
            assert!(
                matches!(event, Event::Gesture(Lifecycle::Begin { .. })) && scrolling
                    || matches!(event, Event::Gesture(Lifecycle::Workspace { .. })) && !scrolling
            );
            assert!(rx.try_recv().is_err());
        }
    }
    #[test]
    fn stationary_wobble_is_filtered_without_losing_slow_accumulated_motion() {
        let (mut s, c, m, tx, mut rx) = setup(true);
        s.frame(&frame(3, 0.5, 0.5), Duration::ZERO, &tx, &c, &m);
        s.frame(&frame(3, 0.55, 0.5), Duration::from_millis(10), &tx, &c, &m);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::Begin { .. })
        ));
        let held = m.latest(1).unwrap().total_x;
        for (time, x) in [(20, 0.55005), (30, 0.54995), (40, 0.5501), (50, 0.5502)] {
            s.frame(&frame(3, x, 0.5), Duration::from_millis(time), &tx, &c, &m);
            let sample = m.latest(1).unwrap();
            assert_eq!(
                sample.total_x, held,
                "stationary fingers must not move the camera"
            );
            assert_eq!(sample.timestamp, Duration::from_millis(time));
        }
        s.frame(&frame(3, 0.5504, 0.5), Duration::from_secs(1), &tx, &c, &m);
        assert!((m.latest(1).unwrap().total_x - held - 1.6).abs() < 0.01);
        assert!(s.active);
        assert!(rx.try_recv().is_err());
        s.frame(&[], Duration::from_millis(1010), &tx, &c, &m);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::End { cancelled: false, .. })
        ));
    }
    #[test]
    fn dock_delivery_prevents_a_late_physical_claim() {
        for native_owner in [Owner::System, Owner::Rift] {
            let (mut s, c, m, tx, mut rx) = setup(true);
            c.ownership_guard().dock_owner = Some(native_owner);
            s.frame(&frame(3, 0.5, 0.5), Duration::ZERO, &tx, &c, &m);
            s.frame(&frame(3, 0.6, 0.5), Duration::from_secs(1), &tx, &c, &m);
            if native_owner == Owner::System {
                assert!(
                    rx.try_recv().is_err(),
                    "Dock and Rift must not both handle a stroke"
                );
            } else {
                assert!(
                    matches!(rx.try_recv().unwrap().1, Event::Gesture(Lifecycle::Begin { .. })),
                    "reserving native delivery must still allow slow physical recognition"
                );
            }
        }
    }
    #[test]
    fn scrolling_scale_tracks_screen_width_and_prior_inversion() {
        assert_eq!(pixels(0.25, 1000.0, false), 1000.0);
        assert_eq!(pixels(0.25, 2000.0, true), -2000.0);
    }
    #[test]
    fn separate_recognizers_do_not_combine_devices_and_reset_ends_once() {
        let (mut a, c, m, tx, mut rx) = setup(true);
        let mut b = DeviceSession::new(None, 0, 2, Duration::ZERO);
        for s in [&mut a, &mut b] {
            s.frame(&frame(3, 0.5, 0.5), Duration::ZERO, &tx, &c, &m);
            s.frame(&frame(3, 0.45, 0.5), Duration::from_millis(10), &tx, &c, &m);
        }
        assert!(a.active);
        assert!(!b.active);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::Begin { .. })
        ));
        a.end(&tx, true, Duration::from_millis(20));
        a.end(&tx, true, Duration::from_millis(30));
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::End { cancelled: true, .. })
        ));
        assert!(rx.try_recv().is_err());
        assert!(!a.active);
    }
    #[test]
    fn sequential_finger_lift_keeps_normal_release() {
        let (mut s, c, m, tx, mut rx) = setup(true);
        for (time, count, x) in [
            (0, 3, 0.5),
            (10, 3, 0.51),
            (20, 3, 0.55),
            (25, 2, 0.55),
            (30, 0, 0.55),
        ] {
            s.frame(&frame(count, x, 0.5), Duration::from_millis(time), &tx, &c, &m);
        }
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::Begin { .. })
        ));
        let Event::Gesture(Lifecycle::End { sample, cancelled }) = rx.try_recv().unwrap().1 else {
            panic!("release");
        };
        assert!(
            !cancelled,
            "lifting fingers one at a time is an ordinary release"
        );
        assert_eq!(sample.timestamp, Duration::from_millis(30));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn scrolling_and_workspace_swipes_can_use_distinct_finger_counts() {
        let (mut s, c, m, tx, mut rx) = setup_settings(true, 4, false);
        for (time, count, x) in [(0, 1, 0.5), (5, 3, 0.5), (10, 4, 0.5), (20, 4, 0.4)] {
            s.frame(&frame(count, x, 0.5), Duration::from_millis(time), &tx, &c, &m);
        }
        let Event::Gesture(Lifecycle::Workspace { next, .. }) =
            rx.try_recv().expect("four-finger workspace swipe").1
        else {
            panic!("workspace action");
        };
        assert!(next);
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn partial_lift_that_stays_down_cancels_and_never_rearms() {
        let (mut s, c, m, tx, mut rx) = setup(true);
        s.frame(&frame(3, 0.5, 0.5), Duration::ZERO, &tx, &c, &m);
        s.frame(&frame(3, 0.55, 0.5), Duration::from_millis(10), &tx, &c, &m);
        rx.try_recv().unwrap();
        s.frame(&frame(2, 0.55, 0.5), Duration::from_millis(20), &tx, &c, &m);
        assert!(rx.try_recv().is_err());
        s.expire_lift(Duration::from_millis(75), &tx);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::End { cancelled: true, .. })
        ));
        s.frame(&frame(3, 0.7, 0.5), Duration::from_millis(80), &tx, &c, &m);
        assert!(rx.try_recv().is_err());
        assert!(s.frame(&[], Duration::from_millis(90), &tx, &c, &m).is_none());
        assert!(rx.try_recv().is_err());
    }
}
