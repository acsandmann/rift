//! One recognizer per physical device; semantic lifecycle and latest motion.
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
    invert: bool,
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
}
impl Control {
    pub fn new(config: &Config) -> Self {
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
        }
    }

    pub fn ownership(&self) -> Ownership { *self.owner.lock() }

    pub fn ownership_guard(&self) -> parking_lot::MutexGuard<'_, Ownership> { self.owner.lock() }

    fn claim(&self, context: &Context) -> bool {
        let routing = self.routing.lock();
        if routing.epoch != context.epoch || !routing.enabled {
            return false;
        }
        let mut owner = self.owner.lock();
        if owner.session == context.session && owner.owner == Owner::System {
            return false;
        }
        *owner = Ownership {
            session: context.session,
            owner: Owner::Rift,
            consume: context.settings.consume,
            fingers: context.action.fingers,
            touching: true,
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
        _mode: LayoutMode,
        screens: Vec<(CGRect, SpaceId, LayoutMode)>,
        converter: CoordinateConverter,
    ) {
        let mut r = self.routing.lock();
        r.settings = settings;
        r.enabled = enabled;
        r.screens = screens;
        r.converter = converter;
    }

    pub fn retire_session(&self, epoch: u64, session: u64) {
        let mut r = self.routing.lock();
        let mut o = self.owner.lock();
        if r.epoch == epoch && o.session == session {
            r.epoch += 1;
            *o = Ownership::default();
        }
    }

    pub fn retire(&self) {
        self.routing.lock().epoch += 1;
        *self.owner.lock() = Ownership::default();
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

/// Normalized positions span a whole pad. One pad-width moves one working-area
/// width (not 1200 libinput units). Keep this calibration independent of columns.
pub fn pixels(translation: f64, width: f64, invert: bool) -> f64 {
    translation * width * if invert { -1.0 } else { 1.0 }
}
struct DeviceSession {
    context: Option<Context>,
    recognizer: GestureRecognizer,
    owner: Owner,
    blocked: bool,
    fired: bool,
    active: bool,
    epoch: u64,
    sample: Motion,
    last_frame: Instant,
    max_contacts: usize,
    lifting: Option<Duration>,
}
impl DeviceSession {
    fn new(context: Option<Context>, epoch: u64, session: u64, time: Duration) -> Self {
        let mut recognizer =
            GestureRecognizer::new(context.as_ref().map_or(3, |c| c.action.fingers))
                .with_exact_finger_count(true)
                .with_gesture_types(GestureTypes::SWIPE);
        recognizer.minimum_swipe_translation = 0.003;
        Self {
            context,
            recognizer,
            owner: Owner::Undecided,
            blocked: false,
            fired: false,
            active: false,
            epoch,
            sample: Motion {
                session,
                total_x: 0.0,
                timestamp: time,
            },
            last_frame: Instant::now(),
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

    /// Returns true only after all active fingers lift, including after a
    /// recognizer topology end/timeout. Semantic end is independently idempotent.
    fn frame(
        &mut self,
        contacts: &[multitouch::Contact],
        time: Duration,
        tx: &Sender,
        control: &Control,
        motion: &MotionPublisher,
    ) -> bool {
        let count = contacts.iter().filter(|c| !c.is_palm() && c.state().is_active()).count();
        if !control.valid(self.epoch) {
            self.end(tx, true, time);
            self.owner = Owner::System;
        }
        self.expire_lift(time, tx);
        if count == 0 {
            self.end(tx, false, time);
            return true;
        }
        if self.lifting.is_some() {
            if count >= self.recognizer.required_finger_count {
                self.end(tx, true, time);
            }
            return false;
        }
        if self.blocked {
            return false;
        }
        if self.active && count < self.recognizer.required_finger_count {
            // Real lifts are staggered. Freeze the last complete frame while
            // waiting briefly for full lift; persistent topology changes cancel.
            self.blocked = true;
            self.lifting = Some(time + Duration::from_millis(50));
            return false;
        }
        if self.owner == Owner::Undecided {
            if count < self.max_contacts {
                self.end(tx, true, time);
                self.owner = Owner::System;
                return false;
            }
            self.max_contacts = count;
            if let Some(c) = &mut self.context {
                if let Some(action) = c.settings.action_for(c.mode, count) {
                    if action != c.action {
                        c.action = action;
                        self.recognizer = GestureRecognizer::new(action.fingers)
                            .with_exact_finger_count(true)
                            .with_gesture_types(GestureTypes::SWIPE);
                        self.recognizer.minimum_swipe_translation = 0.003;
                    }
                } else if count
                    > c.settings
                        .scroll
                        .map_or(0, |a| a.fingers)
                        .max(c.settings.workspace.map_or(0, |a| a.fingers))
                {
                    self.end(tx, true, time);
                    self.owner = Owner::System;
                    return false;
                }
            }
        }
        if let Some(event) = self.recognizer.process(contacts) {
            self.event(event, time, tx, control, motion);
        }
        if self.owner == Owner::Rift && count > self.recognizer.required_finger_count {
            self.end(tx, true, time);
            // Keep owned native delivery suppressed through its actual end.
        }
        false
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
                        let o = control.ownership();
                        if o.session == c.session && o.owner == Owner::System {
                            self.owner = Owner::System;
                            return;
                        }
                        if y >= x.abs() || y > c.action.tolerance {
                            self.owner = Owner::System;
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
                    self.sample.total_x = pixels(x, c.width, c.action.invert);
                    self.sample.timestamp = time;
                    if c.action.scrolling {
                        motion.publish(self.sample);
                    } else if !self.fired && x.abs() >= c.action.threshold {
                        self.fired = true;
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
    let monitor = multitouch::Monitor::new();
    let stream = monitor.contacts();
    if !monitor.start() {
        tracing::warn!("Multitouch monitor unavailable");
        return;
    }
    let origin = Instant::now();
    let mut devices: HashMap<u64, (multitouch::Device, DeviceSession)> = HashMap::default();
    let motion = MotionPublisher::default();
    let mut next_session = 0;
    let mut ui_device = None;
    let mut last_liveness = Instant::now();
    while !control.stop.load(Ordering::Acquire) && !tx.is_closed() {
        let wait = if devices.is_empty() { 250 } else { 25 };
        if let Some((device, contacts)) = stream.recv_timeout(Duration::from_millis(wait)) {
            let Some(id) = device.device_id() else {
                continue;
            };
            // Count via the crate's contact semantics without allocating a filter Vec.
            let count = contacts.iter().filter(|c| !c.is_palm() && c.state().is_active()).count();
            let time = origin.elapsed();
            let epoch = control.routing.lock().epoch;
            if !devices.contains_key(&id) && count > 0 {
                next_session += 1;
                let context = if ui_device.is_none() {
                    control.context(next_session, time)
                } else {
                    None
                };
                if context.is_some() {
                    ui_device = Some(id);
                }
                devices.insert(
                    id,
                    (device, DeviceSession::new(context, epoch, next_session, time)),
                );
            }
            let Some((_, s)) = devices.get_mut(&id) else {
                continue;
            };
            s.last_frame = Instant::now();
            if s.frame(&contacts, time, &tx, &control, &motion) {
                devices.remove(&id);
                if ui_device == Some(id) {
                    ui_device = None;
                    control.owner.lock().touching = false;
                }
                continue;
            }
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
                        fingers: s.recognizer.required_finger_count,
                        touching: true,
                    };
                }
            }
        }
        if last_liveness.elapsed() < Duration::from_millis(25) {
            continue;
        }
        last_liveness = Instant::now();
        let time = origin.elapsed();
        for (&id, (source, s)) in &mut devices {
            s.expire_lift(time, &tx);
            if s.last_frame.elapsed() >= Duration::from_millis(250)
                || !control.valid(s.epoch)
                || !source.is_alive()
                || !source.is_running()
            {
                s.end(
                    &tx,
                    !control.valid(s.epoch) || !source.is_alive() || !source.is_running(),
                    time,
                );
                if ui_device == Some(id) {
                    control.owner.lock().touching = false;
                }
            }
        }
        // Silence ends semantics but never re-arms a still-down stroke. Only
        // full lift or actual source removal retires the device's lift gate.
        devices.retain(|id, (source, _)| {
            if source.is_alive() && source.is_running() {
                return true;
            }
            if ui_device == Some(*id) {
                ui_device = None;
                *control.owner.lock() = Ownership::default();
            }
            false
        });
    }
    for (_, s) in devices.values_mut() {
        s.end(&tx, true, origin.elapsed());
    }
    monitor.stop();
    *control.owner.lock() = Ownership::default();
}

#[cfg(test)]
mod tests {
    use multitouch::{Contact, ContactState, Finger, Hand, Point, Vector};

    use super::*;
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
    fn setup(
        scrolling: bool,
    ) -> (
        DeviceSession,
        Control,
        MotionPublisher,
        Sender,
        crate::actor::Receiver<Event>,
    ) {
        setup_settings(scrolling, 3, false)
    }
    fn setup_settings(
        scrolling: bool,
        workspace_fingers: usize,
        invert: bool,
    ) -> (
        DeviceSession,
        Control,
        MotionPublisher,
        Sender,
        crate::actor::Receiver<Event>,
    ) {
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
        control.configure(settings, true, mode, Vec::new(), CoordinateConverter::default());
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
        assert!((m.latest(1).unwrap().total_x + 30.0).abs() < 0.01);
        assert!(rx.try_recv().is_err()); // no per-frame actor traffic
        s.frame(&frame(4, 0.37, 0.6), Duration::from_millis(30), &tx, &c, &m);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::End { cancelled: true, .. })
        ));
        s.frame(&frame(3, 0.2, 0.4), Duration::from_millis(40), &tx, &c, &m);
        assert!(rx.try_recv().is_err());
        assert!(s.frame(&[], Duration::from_millis(50), &tx, &c, &m));
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
            assert!(s.frame(&[], Duration::from_millis(30), &tx, &c, &m));
        }
    }
    #[test]
    fn cumulative_publisher_rejects_old_session_and_scaling_is_column_independent() {
        let p = MotionPublisher::default();
        for total in [4.0, 9.0, 15.0] {
            p.publish(Motion {
                session: 1,
                total_x: total,
                timestamp: Duration::ZERO,
            });
        }
        assert_eq!(p.latest(1).unwrap().total_x, 15.0);
        p.publish(Motion {
            session: 2,
            total_x: 1.0,
            timestamp: Duration::ZERO,
        });
        assert!(p.latest(1).is_none());
        assert_eq!(pixels(0.1, 1000.0, false), 100.0);
        assert_eq!(pixels(0.1, 2000.0, true), -200.0);
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
}
