//! One recognizer per physical device; semantic lifecycle and latest motion.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use multitouch::{GestureEndReason, GestureEvent, GesturePhase, GestureRecognizer, GestureTypes};
use objc2_core_foundation::CGRect;
use objc2_core_graphics::CGEvent;
use parking_lot::Mutex;

use crate::actor::reactor::{Event, Sender};
use crate::common::config::{Config, HapticPattern, LayoutMode};
use crate::sys::dispatch::{DispatchExt, gesture_deadline_queue};
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
    next_session: u64,
    ui_session: u64,
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
                next_session: 0,
                ui_session: 0,
                settings: Settings::new(config),
                enabled: false,
                screens: Vec::new(),
                converter: CoordinateConverter::default(),
            })),
            owner: Arc::default(),
            stop: Arc::default(),
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
    }

    pub fn retire_session(&self, epoch: u64, session: u64) {
        let mut r = self.routing.lock();
        let mut o = self.owner.lock();
        if r.epoch == epoch && o.session == session {
            r.epoch += 1;
            *o = Ownership::default();
            r.ui_session = 0;
        }
    }

    pub fn retire(&self) {
        let mut routing = self.routing.lock();
        routing.epoch += 1;
        *self.owner.lock() = Ownership::default();
        routing.ui_session = 0;
    }

    pub fn reset(&self, tx: &Sender) {
        self.retire();
        tx.send(Event::Gesture(Lifecycle::Reset));
    }

    pub fn stop(&self, tx: &Sender) {
        self.stop.store(true, Ordering::Release);
        self.reset(tx);
    }

    fn context(&self, session: u64, epoch: u64, started: Duration) -> Option<Context> {
        if !self.routing.lock().enabled {
            return None;
        }
        // Exactly one cursor lookup per candidate stroke, outside state locks.
        let event = CGEvent::new(None)?;
        let point = CGEvent::location(Some(&event));
        let r = self.routing.lock();
        if !r.enabled || r.epoch != epoch || r.ui_session != session {
            return None;
        }
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

    fn begin(&self, time: Duration) -> DeviceSession {
        let (session, epoch, available) = {
            let mut r = self.routing.lock();
            r.next_session += 1;
            let available = r.enabled && r.ui_session == 0;
            if available {
                r.ui_session = r.next_session;
            }
            (r.next_session, r.epoch, available)
        };
        let context = available.then(|| self.context(session, epoch, time)).flatten();
        if context.is_none() {
            let mut r = self.routing.lock();
            if r.ui_session == session {
                r.ui_session = 0;
            }
        }
        DeviceSession::new(context, epoch, session, time)
    }

    pub fn start(&self, tx: Sender) -> multitouch::Monitor {
        let control = self.clone();
        let origin = Instant::now();
        let motion = MotionPublisher::default();
        let monitor = multitouch::Monitor::with_device_handler(move |_| {
            let state = Arc::new(Mutex::new(DeviceInput {
                session: None,
                control: control.clone(),
                tx: tx.clone(),
                motion: motion.clone(),
                origin,
                last_count: 0,
            }));
            move |event| DeviceInput::deliver(&state, event)
        });
        if !monitor.start() {
            tracing::warn!("Multitouch monitor unavailable");
        }
        monitor
    }
}

/// A quarter-pad drag moves one working-area width, independent of column size.
const SCROLL_SENSITIVITY: f64 = 4.0;
const VERTICAL_INTENT_DISTANCE: f64 = 0.03;
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
        count: usize,
        time: Duration,
        tx: &Sender,
        control: &Control,
        motion: &MotionPublisher,
    ) -> Option<usize> {
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
                    let total_x =
                        x * SCROLL_SENSITIVITY * c.width * if c.action.invert { -1.0 } else { 1.0 };
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
/// One state per physical device. The native subscription serializes contact
/// deliveries; this lock also synchronizes the single delayed lift callback.
/// CGEventTap only reads `Control::owner`, never this state or the recognizer.
struct DeviceInput {
    session: Option<DeviceSession>,
    control: Control,
    tx: Sender,
    motion: MotionPublisher,
    origin: Instant,
    last_count: usize,
}
impl DeviceInput {
    fn deliver(state: &Arc<Mutex<Self>>, event: multitouch::ContactEvent<'_>) {
        let mut input = state.lock();
        let time = input.origin.elapsed();
        match event {
            multitouch::ContactEvent::Stopped => input.finish(true, time),
            multitouch::ContactEvent::Reset => {
                let Self { session, control, tx, .. } = &mut *input;
                if let Some(s) = session {
                    s.end(tx, true, time);
                    s.owner = Owner::System;
                    control.retire_session(s.epoch, s.sample.session);
                }
            }
            multitouch::ContactEvent::Frame(contacts) => {
                if input.control.stop.load(Ordering::Acquire) || input.tx.is_closed() {
                    input.finish(true, time);
                    return;
                }
                let count =
                    contacts.iter().filter(|c| !c.is_palm() && c.state().is_active()).count();
                // Contacts below the configured minimum need only counting.
                // Topology changes always reach the session; no motion is copied.
                if count == input.last_count
                    && input.session.as_ref().is_some_and(|s| {
                        s.context.is_none()
                            || (!s.active && count < s.recognizer.required_finger_count)
                    })
                {
                    return;
                }
                input.last_count = count;
                if count == 0 {
                    input.finish(false, time);
                    return;
                }
                if input.session.is_none() {
                    if count < input.control.routing.lock().settings.minimum_contacts() {
                        return;
                    }
                    let control = input.control.clone();
                    drop(input);
                    let session = control.begin(time);
                    input = state.lock();
                    input.session = Some(session);
                }
                let Self {
                    session, tx, control, motion, ..
                } = &mut *input;
                let s = session.as_mut().unwrap();
                let prior_lift = s.lifting;
                s.frame(contacts, count, time, tx, control, motion);
                {
                    let routing = control.routing.lock();
                    if routing.ui_session == s.sample.session
                        && routing.epoch == s.epoch
                        && routing.enabled
                    {
                        let consume = s.context.as_ref().is_some_and(|c| c.settings.consume)
                            && (count == s.recognizer.required_finger_count
                                || s.owner == Owner::Rift);
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
                if prior_lift != s.lifting
                    && let Some(deadline) = s.lifting
                {
                    let weak = Arc::downgrade(state);
                    let session = s.sample.session;
                    let delay = deadline.saturating_sub(time).as_nanos() as i64;
                    gesture_deadline_queue().after_f_s(
                        dispatchr::time::Time::new_after(dispatchr::time::Time::NOW, delay),
                        (weak, session, deadline),
                        |(state, session, deadline)| {
                            if let Some(state) = state.upgrade() {
                                state.lock().expire_lift(session, deadline);
                            }
                        },
                    );
                }
            }
        }
    }

    fn expire_lift(&mut self, session: u64, deadline: Duration) {
        let Some(s) = &mut self.session else { return };
        if s.sample.session != session || s.lifting != Some(deadline) {
            return;
        }
        let time = self.origin.elapsed();
        if self.control.valid(s.epoch) {
            s.expire_lift(time, &self.tx);
        } else {
            s.end(&self.tx, true, time);
        }
    }

    fn finish(&mut self, cancelled: bool, time: Duration) {
        if let Some(mut s) = self.session.take() {
            s.expire_lift(time, &self.tx);
            s.end(&self.tx, cancelled, time);
            let mut routing = self.control.routing.lock();
            if routing.ui_session == s.sample.session {
                routing.ui_session = 0;
                let mut owner = self.control.owner.lock();
                if cancelled {
                    *owner = Ownership::default();
                } else {
                    owner.touching = false;
                    owner.dock_owner = None;
                }
            }
        }
        self.last_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use multitouch::{Contact, ContactState, Finger, Hand, Point, Vector};

    use super::*;

    #[test]
    fn inline_delivery_keeps_devices_isolated_and_removal_cancels_only_the_owner() {
        let (session, control, motion, tx, mut rx) = setup(true);
        control.routing.lock().ui_session = 1;
        let input = Arc::new(Mutex::new(DeviceInput {
            session: Some(session),
            control: control.clone(),
            motion: motion.clone(),
            tx: tx.clone(),
            origin: Instant::now(),
            last_count: 0,
        }));
        let other = Arc::new(Mutex::new(DeviceInput {
            session: Some(DeviceSession::new(None, 0, 2, Duration::ZERO)),
            control: control.clone(),
            motion: motion.clone(),
            tx: tx.clone(),
            origin: Instant::now(),
            last_count: 0,
        }));
        DeviceInput::deliver(&input, multitouch::ContactEvent::Frame(&frame(3, 0.5, 0.5)));
        DeviceInput::deliver(&input, multitouch::ContactEvent::Frame(&frame(3, 0.6, 0.5)));
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::Begin { .. })
        ));
        for _ in 0..100 {
            DeviceInput::deliver(&other, multitouch::ContactEvent::Frame(&frame(2, 0.1, 0.5)));
        }
        DeviceInput::deliver(&other, multitouch::ContactEvent::Stopped);
        assert_eq!(control.owner.lock().owner, Owner::Rift);
        assert!(control.owner.lock().touching);
        assert!(rx.try_recv().is_err());
        DeviceInput::deliver(&input, multitouch::ContactEvent::Stopped);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::End { cancelled: true, .. })
        ));
        assert!(!control.owner.lock().touching);
        assert_eq!(control.routing.lock().ui_session, 0);
        DeviceInput::deliver(&input, multitouch::ContactEvent::Stopped);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn ordinary_contacts_do_not_reserve_routing_before_a_gesture_candidate() {
        let (_, control, motion, tx, mut rx) = setup_settings(true, 4, false);
        let settings = control.routing.lock().settings;
        control.configure(
            settings,
            true,
            vec![(
                CGRect::new(
                    objc2_core_foundation::CGPoint::new(-1e9, -1e9),
                    objc2_core_foundation::CGSize::new(2e9, 2e9),
                ),
                SpaceId::new(10),
                LayoutMode::Scrolling,
            )],
            CoordinateConverter::default(),
        );
        let device = || {
            Arc::new(Mutex::new(DeviceInput {
                session: None,
                control: control.clone(),
                motion: motion.clone(),
                tx: tx.clone(),
                origin: Instant::now(),
                last_count: 0,
            }))
        };
        let a = device();
        let b = device();
        DeviceInput::deliver(&a, multitouch::ContactEvent::Frame(&frame(1, 0.5, 0.5)));
        assert!(a.lock().session.is_none());
        {
            let routing = control.routing.lock();
            assert_eq!(routing.ui_session, 0);
            assert_eq!(routing.next_session, 0);
        }
        for x in [0.5, 0.4] {
            DeviceInput::deliver(&b, multitouch::ContactEvent::Frame(&frame(3, x, 0.5)));
        }
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::Begin { .. })
        ));
        assert_eq!(control.owner.lock().owner, Owner::Rift);
        assert!(motion.latest(control.routing.lock().ui_session).unwrap().total_x < 0.0);
        DeviceInput::deliver(&b, multitouch::ContactEvent::Frame(&[]));
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::End { cancelled: false, .. })
        ));
        DeviceInput::deliver(&a, multitouch::ContactEvent::Frame(&frame(2, 0.5, 0.5)));
        assert!(a.lock().session.is_none());
        assert_eq!(control.routing.lock().ui_session, 0);
        DeviceInput::deliver(&a, multitouch::ContactEvent::Frame(&frame(3, 0.5, 0.5)));
        {
            let input = a.lock();
            let session = input.session.as_ref().unwrap();
            assert!(session.context.is_some());
            assert_eq!(control.routing.lock().ui_session, session.sample.session);
        }
        for x in [0.5, 0.4] {
            DeviceInput::deliver(&a, multitouch::ContactEvent::Frame(&frame(4, x, 0.5)));
        }
        let Event::Gesture(Lifecycle::Workspace { context, .. }) = rx.try_recv().unwrap().1 else {
            panic!("four-finger workspace action");
        };
        assert_eq!(context.action.fingers, 4);
        assert!(!context.action.scrolling);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn partial_lift_deadline_cancels_without_another_contact_frame() {
        let (session, control, motion, tx, mut rx) = setup(true);
        control.routing.lock().ui_session = 1;
        let input = Arc::new(Mutex::new(DeviceInput {
            session: Some(session),
            control,
            motion,
            tx,
            origin: Instant::now(),
            last_count: 0,
        }));
        for (count, x) in [(3, 0.5), (3, 0.6), (2, 0.6)] {
            DeviceInput::deliver(&input, multitouch::ContactEvent::Frame(&frame(count, x, 0.5)));
        }
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::Begin { .. })
        ));
        let (ended_tx, ended_rx) = std::sync::mpsc::channel();
        let receiver = std::thread::spawn(move || {
            ended_tx.send(rx.blocking_recv().unwrap().1).unwrap();
            rx
        });
        let event = ended_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            event,
            Event::Gesture(Lifecycle::End { cancelled: true, .. })
        ));
        let mut rx = receiver.join().unwrap();
        DeviceInput::deliver(&input, multitouch::ContactEvent::Frame(&[]));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn delayed_lift_is_stale_safe_and_reset_requires_a_full_lift() {
        for full_lift in [false, true] {
            let (mut session, control, motion, tx, mut rx) = setup(true);
            // A deadline from the prior stroke cannot cancel this active session.
            session.active = true;
            session.lifting = Some(Duration::from_millis(50));
            control.routing.lock().ui_session = 1;
            control.owner.lock().session = 1;
            let input = Arc::new(Mutex::new(DeviceInput {
                session: Some(session),
                control: control.clone(),
                motion,
                tx,
                origin: Instant::now() - Duration::from_millis(60),
                last_count: 2,
            }));
            input.lock().expire_lift(99, Duration::from_millis(50));
            input.lock().expire_lift(1, Duration::from_millis(49));
            assert!(rx.try_recv().is_err());
            if full_lift {
                DeviceInput::deliver(&input, multitouch::ContactEvent::Frame(&[]));
            } else {
                input.lock().expire_lift(1, Duration::from_millis(50));
            }
            assert!(matches!(
                rx.try_recv().unwrap().1,
                Event::Gesture(Lifecycle::End { cancelled: true, .. })
            ));
            input.lock().expire_lift(1, Duration::from_millis(50));
            if full_lift {
                assert!(input.lock().session.is_none());
                assert!(rx.try_recv().is_err());
                continue;
            }
            DeviceInput::deliver(&input, multitouch::ContactEvent::Reset);
            for x in [0.5, 0.6, 0.7] {
                DeviceInput::deliver(&input, multitouch::ContactEvent::Frame(&frame(3, x, 0.5)));
            }
            assert!(rx.try_recv().is_err());
            assert_eq!(control.owner.lock().owner, Owner::System);
            DeviceInput::deliver(&input, multitouch::ContactEvent::Frame(&[]));
            assert!(input.lock().session.is_none());
        }
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
        s.frame(&frame(2, 0.4, 0.4), 2, Duration::ZERO, &tx, &c, &m);
        assert!(rx.try_recv().is_err());
        s.frame(&frame(3, 0.4, 0.4), 3, Duration::ZERO, &tx, &c, &m);
        s.frame(&frame(3, 0.39, 0.401), 3, Duration::from_millis(10), &tx, &c, &m);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::Begin { .. })
        ));
        s.frame(&frame(3, 0.37, 0.6), 3, Duration::from_millis(20), &tx, &c, &m);
        assert_eq!(s.owner, Owner::Rift); // committed direction ignores later wobble
        assert!((m.latest(1).unwrap().total_x + 120.0).abs() < 0.01);
        assert!(rx.try_recv().is_err()); // no per-frame actor traffic
        s.frame(&frame(4, 0.37, 0.6), 4, Duration::from_millis(30), &tx, &c, &m);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::End { cancelled: true, .. })
        ));
        s.frame(&frame(3, 0.2, 0.4), 3, Duration::from_millis(40), &tx, &c, &m);
        assert!(rx.try_recv().is_err());
        assert!(s.frame(&[], 0, Duration::from_millis(50), &tx, &c, &m).is_none());
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn workspace_threshold_and_inversion_fire_only_once() {
        for invert in [false, true] {
            let (mut s, c, m, tx, mut rx) = setup_settings(false, 3, invert);
            for (i, x) in [0.5, 0.49, 0.45, 0.4, 0.3].into_iter().enumerate() {
                s.frame(
                    &frame(3, x, 0.5),
                    3,
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
            s.frame(&frame(3, 0.5, 0.5), 3, Duration::ZERO, &tx, &c, &m);
            if reset {
                c.reset(&tx);
                rx.try_recv().unwrap();
            }
            s.frame(
                &frame(3, if reset { 0.6 } else { 0.501 }, if reset { 0.5 } else { 0.6 }),
                3,
                Duration::from_millis(10),
                &tx,
                &c,
                &m,
            );
            s.frame(&frame(3, 0.3, 0.6), 3, Duration::from_millis(20), &tx, &c, &m);
            assert_eq!(s.owner, Owner::System);
            assert!(rx.try_recv().is_err());
            assert!(s.frame(&[], 0, Duration::from_millis(30), &tx, &c, &m).is_none());
        }
    }

    #[test]
    fn slow_horizontal_swipe_tolerates_initial_finger_placement_noise() {
        for scrolling in [false, true] {
            let (mut s, c, m, tx, mut rx) = setup(scrolling);
            s.frame(&frame(3, 0.4, 0.4), 3, Duration::ZERO, &tx, &c, &m);
            s.frame(
                &frame(3, 0.401, 0.405),
                3,
                Duration::from_millis(100),
                &tx,
                &c,
                &m,
            );
            assert_eq!(
                s.owner,
                Owner::Undecided,
                "placement noise must not reject the stroke"
            );
            s.frame(
                &frame(3, 0.41, 0.405),
                3,
                Duration::from_millis(500),
                &tx,
                &c,
                &m,
            );
            s.frame(&frame(3, 0.5, 0.405), 3, Duration::from_secs(5), &tx, &c, &m);
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
        s.frame(&frame(3, 0.5, 0.5), 3, Duration::ZERO, &tx, &c, &m);
        s.frame(&frame(3, 0.55, 0.5), 3, Duration::from_millis(10), &tx, &c, &m);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::Begin { .. })
        ));
        let held = m.latest(1).unwrap().total_x;
        for (time, x) in [(20, 0.55005), (30, 0.54995), (40, 0.5501), (50, 0.5502)] {
            s.frame(&frame(3, x, 0.5), 3, Duration::from_millis(time), &tx, &c, &m);
            let sample = m.latest(1).unwrap();
            assert_eq!(
                sample.total_x, held,
                "stationary fingers must not move the camera"
            );
            assert_eq!(sample.timestamp, Duration::from_millis(time));
        }
        s.frame(&frame(3, 0.5504, 0.5), 3, Duration::from_secs(1), &tx, &c, &m);
        assert!((m.latest(1).unwrap().total_x - held - 1.6).abs() < 0.01);
        assert!(s.active);
        assert!(rx.try_recv().is_err());
        s.frame(&[], 0, Duration::from_millis(1010), &tx, &c, &m);
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
            s.frame(&frame(3, 0.5, 0.5), 3, Duration::ZERO, &tx, &c, &m);
            s.frame(&frame(3, 0.6, 0.5), 3, Duration::from_secs(1), &tx, &c, &m);
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
    fn sequential_finger_lift_keeps_normal_release() {
        let (mut s, c, m, tx, mut rx) = setup(true);
        for (time, count, x) in [
            (0, 3, 0.5),
            (10, 3, 0.51),
            (20, 3, 0.55),
            (25, 2, 0.55),
            (30, 0, 0.55),
        ] {
            s.frame(
                &frame(count, x, 0.5),
                count,
                Duration::from_millis(time),
                &tx,
                &c,
                &m,
            );
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
            s.frame(
                &frame(count, x, 0.5),
                count,
                Duration::from_millis(time),
                &tx,
                &c,
                &m,
            );
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
        s.frame(&frame(3, 0.5, 0.5), 3, Duration::ZERO, &tx, &c, &m);
        s.frame(&frame(3, 0.55, 0.5), 3, Duration::from_millis(10), &tx, &c, &m);
        rx.try_recv().unwrap();
        s.frame(&frame(2, 0.55, 0.5), 2, Duration::from_millis(20), &tx, &c, &m);
        assert!(rx.try_recv().is_err());
        s.expire_lift(Duration::from_millis(75), &tx);
        assert!(matches!(
            rx.try_recv().unwrap().1,
            Event::Gesture(Lifecycle::End { cancelled: true, .. })
        ));
        s.frame(&frame(3, 0.7, 0.5), 3, Duration::from_millis(80), &tx, &c, &m);
        assert!(rx.try_recv().is_err());
        assert!(s.frame(&[], 0, Duration::from_millis(90), &tx, &c, &m).is_none());
        assert!(rx.try_recv().is_err());
    }
}
