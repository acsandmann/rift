//! Native delivery arbitration only. Physical recognition belongs to multitouch.
//! Dock whole-sequence arbitration follows Loop's #1162 fix.
use std::time::{Duration, Instant};

use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGEvent, CGEventField, CGEventTapLocation, CGEventType};

pub const CGS_EVENT_GESTURE: u32 = 29;
pub const CGS_EVENT_DOCK_CONTROL: u32 = 30;
pub const EVENT_MASK: u64 = (1 << 29) | (1 << 30) | (1 << 22);
// Private CGS fields, shared with Loop's CGEventField extensions.
const HID_TYPE: CGEventField = CGEventField(110);
const MOTION: CGEventField = CGEventField(123);
const VELOCITY: CGEventField = CGEventField(129);
const PHASE: CGEventField = CGEventField(132);
const REPOST: i64 = 0x5249465447455354;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Owner {
    Undecided,
    Rift,
    #[default]
    System,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Ownership {
    pub session: u64,
    pub owner: Owner,
    pub consume: bool,
    pub touching: bool,
    /// Dock delivery is irrevocable once its begin has been forwarded.
    pub dock_owner: Option<Owner>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Sequence {
    #[default]
    Passing,
    Holding,
    Provisional,
    Dropping,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    Forward,
    Drop,
    Hold,
    Replay,
    Cancel,
}
#[derive(Default)]
struct Arbitration {
    sequence: Sequence,
    session: u64,
    began: Option<Instant>,
}
impl Arbitration {
    fn decide_dock(&mut self, phase: i64, owner: &mut Ownership, now: Instant) -> Decision {
        if phase == 1 {
            self.began = Some(now);
            self.session = owner.session;
            self.sequence = if owner.touching {
                if owner.consume && owner.owner != Owner::System {
                    Sequence::Dropping
                } else {
                    Sequence::Passing
                }
            } else {
                Sequence::Holding
            };
        }
        let end = phase & 12 != 0;
        let decision = match self.sequence {
            Sequence::Holding if end => Decision::Drop,
            Sequence::Holding
                if owner.touching && owner.consume && owner.owner != Owner::System =>
            {
                self.sequence = Sequence::Dropping;
                Decision::Drop
            }
            Sequence::Holding
                if owner.touching
                    || self
                        .began
                        .is_some_and(|t| now.duration_since(t) >= Duration::from_millis(60)) =>
            {
                self.sequence = Sequence::Passing;
                Decision::Replay
            }
            Sequence::Holding => Decision::Hold,
            Sequence::Dropping => Decision::Drop,
            _ => Decision::Forward,
        };
        owner.dock_owner = match self.sequence {
            Sequence::Passing => Some(Owner::System),
            Sequence::Dropping => Some(Owner::Rift),
            _ => None,
        };
        if owner.dock_owner == Some(Owner::System) && owner.consume && owner.touching {
            owner.owner = Owner::System;
        }
        if end {
            self.sequence = Sequence::Passing;
            self.began = None;
            owner.dock_owner = None;
        }
        decision
    }

    fn decide(&mut self, phase: i64, owner: Ownership, now: Instant) -> Decision {
        if phase == 1 {
            self.session = owner.session;
            self.began = Some(now);
            self.sequence = if !owner.consume || !owner.touching || owner.owner == Owner::System {
                Sequence::Passing
            } else if owner.owner == Owner::Rift {
                Sequence::Dropping
            } else {
                Sequence::Holding
            };
            return match self.sequence {
                Sequence::Holding => Decision::Hold,
                Sequence::Dropping => Decision::Drop,
                _ => Decision::Forward,
            };
        }
        let owns = owner.session == self.session && owner.consume && owner.owner == Owner::Rift;
        let decision = match self.sequence {
            // Native begin can precede physical acquisition. Late ownership
            // cancels delivery instead of leaving Dock with a partial stroke.
            Sequence::Passing
                if owner.touching && owner.consume && owner.owner != Owner::System =>
            {
                self.session = owner.session;
                self.sequence = Sequence::Provisional;
                if owner.owner == Owner::Rift {
                    self.sequence = Sequence::Dropping;
                    Decision::Cancel
                } else {
                    Decision::Forward
                }
            }
            Sequence::Dropping => Decision::Drop, // survives physical lift/momentum
            Sequence::Provisional if owns => {
                self.sequence = Sequence::Dropping;
                Decision::Cancel
            }
            Sequence::Passing | Sequence::Provisional => Decision::Forward,
            Sequence::Holding if owns => {
                self.sequence = Sequence::Dropping;
                Decision::Drop
            }
            Sequence::Holding => {
                if owner.session != self.session
                    || owner.owner == Owner::System
                    || !owner.consume
                    || !owner.touching
                    || phase & 12 != 0
                    || self
                        .began
                        .is_some_and(|start| now.duration_since(start) >= Duration::from_millis(60))
                {
                    self.sequence = Sequence::Provisional;
                    Decision::Replay
                } else {
                    Decision::Drop
                }
            }
        };
        if phase & 12 != 0 {
            self.sequence = Sequence::Passing;
            self.began = None;
        }
        decision
    }
}

#[derive(Default)]
struct NativeSequence {
    arbitration: Arbitration,
    held: Vec<CFRetained<CGEvent>>,
}
#[derive(Default)]
pub struct Filter {
    dock: NativeSequence,
    app: NativeSequence,
    scroll: NativeSequence,
}
impl Filter {
    /// No copies/allocations on the normal forward/drop path. Sequence state
    /// stays on the tap thread; only Ownership is read from the worker.
    pub fn forward<O: std::ops::DerefMut<Target = Ownership>>(
        &mut self,
        ty: CGEventType,
        event: &CGEvent,
        ownership: impl FnOnce() -> O,
        mut post: impl FnMut(&CGEvent),
    ) -> bool {
        let mut post = |held: &CGEvent| {
            CGEvent::set_integer_value_field(Some(held), CGEventField::EventSourceUserData, REPOST);
            post(held);
        };
        if CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData) == REPOST {
            return true;
        }
        let dock = ty.0 == CGS_EVENT_DOCK_CONTROL;
        if dock {
            let pid =
                CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUnixProcessID);
            if pid != 0 && pid != i64::from(std::process::id()) {
                return true;
            }
        }
        let mut owner;
        let (sequence, phase) = if ty == CGEventType::ScrollWheel {
            let phase = CGEvent::integer_value_field(
                Some(event),
                CGEventField::ScrollWheelEventScrollPhase,
            );
            let momentum = CGEvent::integer_value_field(
                Some(event),
                CGEventField::ScrollWheelEventMomentumPhase,
            );
            // Mouse wheels have neither phase. They must pass even while an
            // earlier owned trackpad sequence is waiting for momentum to end.
            if phase == 0 && momentum == 0 {
                return true;
            }
            if phase != 1
                && self.scroll.arbitration.began.is_none()
                && self.scroll.arbitration.sequence == Sequence::Passing
            {
                return true;
            }
            owner = ownership();
            // Scroll end precedes native momentum; retain ownership through it.
            let phase = if momentum & 4 != 0
                || (phase & 4 != 0
                    && self.scroll.arbitration.sequence != Sequence::Dropping
                    && owner.owner != Owner::Rift)
            {
                4
            } else if phase & 8 != 0 {
                8
            } else if phase == 1 {
                1
            } else {
                2
            };
            (&mut self.scroll, phase)
        } else {
            if ty.0 != CGS_EVENT_GESTURE && ty.0 != CGS_EVENT_DOCK_CONTROL {
                return true;
            }
            // DockSwipe and NavigationSwipe only; digitizer, zoom, rotate,
            // smart zoom and vertical Mission Control never enter the filter.
            let subtype = CGEvent::integer_value_field(Some(event), HID_TYPE);
            if !matches!(subtype, 16 | 23) || CGEvent::integer_value_field(Some(event), MOTION) != 1
            {
                return true;
            }
            owner = ownership();
            (
                if ty.0 == CGS_EVENT_DOCK_CONTROL {
                    &mut self.dock
                } else {
                    &mut self.app
                },
                CGEvent::integer_value_field(Some(event), PHASE),
            )
        };
        if phase == 1 {
            for held in sequence.held.drain(..) {
                // A new Dock begin abandons an unfinished held sequence.
                if !dock {
                    post(&held);
                }
            }
        }
        let decision = if dock {
            sequence.arbitration.decide_dock(phase, &mut owner, Instant::now())
        } else {
            sequence.arbitration.decide(phase, *owner, Instant::now())
        };
        match decision {
            Decision::Forward => true,
            Decision::Drop => {
                if sequence.arbitration.sequence != Sequence::Holding {
                    sequence.held.clear();
                } else if let Some(copy) = CGEvent::new_copy(Some(event)) {
                    sequence.held.push(copy);
                }
                false
            }
            Decision::Hold => {
                if let Some(copy) = CGEvent::new_copy(Some(event)) {
                    sequence.held.push(copy);
                } else {
                    // Never replay a suffix if its begin could not be copied.
                    sequence.held.clear();
                    sequence.arbitration.sequence = Sequence::Dropping;
                }
                false
            }
            Decision::Replay => {
                for held in sequence.held.drain(..) {
                    post(&held);
                }
                true
            }
            Decision::Cancel => {
                if let Some(cancel) = cancelled_event(event, ty) {
                    post(&cancel);
                }
                false
            }
        }
    }

    pub fn hold_deadline(&self) -> Option<Instant> {
        // Dock replay needs the current tap proxy; it resolves on the next
        // event rather than posting an unfinished begin outside a callback.
        [&self.app, &self.scroll]
            .into_iter()
            .filter(|s| s.arbitration.sequence == Sequence::Holding)
            .filter_map(|s| s.arbitration.began.map(|t| t + Duration::from_millis(60)))
            .min()
    }

    pub fn release_expired(&mut self, owner: Ownership) {
        let now = Instant::now();
        for s in [&mut self.app, &mut self.scroll] {
            if s.arbitration.sequence == Sequence::Holding
                && s.arbitration
                    .began
                    .is_some_and(|t| now.duration_since(t) >= Duration::from_millis(60))
            {
                if owner.session == s.arbitration.session
                    && owner.owner == Owner::Rift
                    && owner.consume
                {
                    s.held.clear();
                    s.arbitration.sequence = Sequence::Dropping;
                    continue;
                }
                for held in s.held.drain(..) {
                    repost(&held);
                }
                // Bound native latency without putting a deadline on physical
                // intent. A later horizontal claim cancels native delivery.
                s.arbitration.sequence = Sequence::Provisional;
            }
        }
    }

    pub fn reset(&mut self) {
        self.dock.held.clear();
        if self.dock.arbitration.sequence == Sequence::Holding {
            // Its begin never reached Dock. Suppress the remainder too, even
            // if a reset/rebuilt tap occurs before the physical stroke ends.
            self.dock.arbitration.sequence = Sequence::Dropping;
        }
        self.dock.arbitration.began = None;
        // A held begin has never reached the system. Releasing it on reset
        // preserves native delivery; subsequent events pass normally.
        for sequence in [&mut self.app, &mut self.scroll] {
            for held in sequence.held.drain(..) {
                repost(&held);
            }
            *sequence = NativeSequence::default();
        }
    }
}
fn cancelled_event(event: &CGEvent, ty: CGEventType) -> Option<CFRetained<CGEvent>> {
    let cancel = CGEvent::new_copy(Some(event))?;
    CGEvent::set_integer_value_field(Some(&cancel), PHASE, 8);
    CGEvent::set_double_value_field(Some(&cancel), VELOCITY, 0.0);
    if ty == CGEventType::ScrollWheel {
        CGEvent::set_integer_value_field(
            Some(&cancel),
            CGEventField::ScrollWheelEventScrollPhase,
            8,
        );
        CGEvent::set_integer_value_field(
            Some(&cancel),
            CGEventField::ScrollWheelEventMomentumPhase,
            0,
        );
    }
    Some(cancel)
}
fn repost(event: &CGEvent) {
    CGEvent::set_integer_value_field(Some(event), CGEventField::EventSourceUserData, REPOST);
    CGEvent::post(CGEventTapLocation::SessionEventTap, Some(event));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_transitions_preserve_native_sequences() {
        let now = Instant::now();
        let mut a = Arbitration::default();
        let mut o = Ownership {
            session: 7,
            owner: Owner::Undecided,
            consume: true,
            touching: true,
            dock_owner: None,
        };
        assert_eq!(a.decide(1, o, now), Decision::Hold);
        o.owner = Owner::Rift;
        assert_eq!(a.decide(2, o, now), Decision::Drop);
        o.touching = false;
        assert_eq!(a.decide(4, o, now), Decision::Drop);
        o = Ownership::default();
        assert_eq!(a.decide(1, o, now), Decision::Forward);
        o = Ownership {
            session: 8,
            owner: Owner::Undecided,
            consume: true,
            touching: true,
            dock_owner: None,
        };
        assert_eq!(a.decide(2, o, now), Decision::Forward);
        o.owner = Owner::Rift;
        assert_eq!(a.decide(2, o, now), Decision::Cancel);
        assert_eq!(a.decide(8, o, now), Decision::Drop);
        o.owner = Owner::Undecided;
        assert_eq!(a.decide(1, o, now), Decision::Hold);
        o.owner = Owner::System;
        assert_eq!(a.decide(2, o, now), Decision::Replay);
        assert_eq!(a.decide(4, o, now), Decision::Forward);
        o.consume = false;
        o.owner = Owner::Rift;
        assert_eq!(a.decide(1, o, now), Decision::Forward);
        assert_eq!(a.decide(2, o, now), Decision::Forward);
    }
    #[test]
    fn undecided_native_begin_has_bounded_hold() {
        let now = Instant::now();
        let mut a = Arbitration::default();
        let o = Ownership {
            session: 1,
            owner: Owner::Undecided,
            consume: true,
            touching: true,
            dock_owner: None,
            ..Default::default()
        };
        assert_eq!(a.decide(1, o, now), Decision::Hold);
        assert_eq!(a.decide(2, o, now + Duration::from_millis(61)), Decision::Replay);
    }
    #[test]
    fn unrelated_and_reposted_events_pass() {
        let event = CGEvent::new(None).unwrap();
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField(55),
            CGS_EVENT_DOCK_CONTROL as i64,
        );
        let mut f = Filter::default();
        let o = Ownership {
            session: 1,
            owner: Owner::Rift,
            consume: true,
            touching: true,
            dock_owner: None,
        };
        for subtype in [11, 17, 18, 19] {
            CGEvent::set_integer_value_field(Some(&event), HID_TYPE, subtype);
            assert!(f.forward(CGEventType(29), &event, || Box::new(o), |_| {}));
        }
        CGEvent::set_integer_value_field(Some(&event), HID_TYPE, 23);
        CGEvent::set_integer_value_field(Some(&event), MOTION, 2);
        assert!(f.forward(CGEventType(30), &event, || Box::new(o), |_| {}));
        CGEvent::set_integer_value_field(Some(&event), MOTION, 1);
        CGEvent::set_integer_value_field(Some(&event), PHASE, 1);
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::EventSourceUnixProcessID,
            i64::from(std::process::id()) + 1,
        );
        for phase in [1, 2, 4] {
            CGEvent::set_integer_value_field(Some(&event), PHASE, phase);
            assert!(f.forward(CGEventType(30), &event, || Box::new(o), |_| {}));
        }
        CGEvent::set_integer_value_field(Some(&event), CGEventField::EventSourceUnixProcessID, 0);
        CGEvent::set_integer_value_field(Some(&event), PHASE, 1);
        assert!(!f.forward(CGEventType(30), &event, || Box::new(o), |_| {}));
        CGEvent::set_integer_value_field(Some(&event), CGEventField::EventSourceUserData, REPOST);
        assert!(f.forward(CGEventType(30), &event, || Box::new(o), |_| {}));
    }
    #[test]
    fn cancellation_clears_native_velocity_and_scroll_momentum() {
        let event = CGEvent::new(None).unwrap();
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField(55),
            CGS_EVENT_DOCK_CONTROL as i64,
        );
        CGEvent::set_integer_value_field(Some(&event), HID_TYPE, 23);
        CGEvent::set_integer_value_field(Some(&event), PHASE, 2);
        CGEvent::set_double_value_field(Some(&event), VELOCITY, 42.0);
        let cancel = cancelled_event(&event, CGEventType(CGS_EVENT_DOCK_CONTROL)).unwrap();
        assert_eq!(CGEvent::integer_value_field(Some(&cancel), PHASE), 8);
        assert_eq!(CGEvent::double_value_field(Some(&cancel), VELOCITY), 0.0);
        assert_eq!(CGEvent::integer_value_field(Some(&event), PHASE), 2);
        assert_eq!(CGEvent::double_value_field(Some(&event), VELOCITY), 42.0);

        let scroll = CGEvent::new_scroll_wheel_event2(
            None,
            objc2_core_graphics::CGScrollEventUnit::Pixel,
            2,
            1,
            1,
            0,
        )
        .unwrap();
        CGEvent::set_integer_value_field(
            Some(&scroll),
            CGEventField::ScrollWheelEventScrollPhase,
            2,
        );
        CGEvent::set_integer_value_field(
            Some(&scroll),
            CGEventField::ScrollWheelEventMomentumPhase,
            1,
        );
        let cancel = cancelled_event(&scroll, CGEventType::ScrollWheel).unwrap();
        assert_eq!(
            CGEvent::integer_value_field(Some(&cancel), CGEventField::ScrollWheelEventScrollPhase),
            8
        );
        assert_eq!(
            CGEvent::integer_value_field(
                Some(&cancel),
                CGEventField::ScrollWheelEventMomentumPhase
            ),
            0
        );
    }
    #[test]
    fn claimed_trackpad_sequence_does_not_swallow_a_mouse_wheel_after_lift() {
        let scroll = |phase| {
            let event = CGEvent::new_scroll_wheel_event2(
                None,
                objc2_core_graphics::CGScrollEventUnit::Pixel,
                2,
                1,
                1,
                0,
            )
            .unwrap();
            if phase != 0 {
                CGEvent::set_integer_value_field(
                    Some(&event),
                    CGEventField::ScrollWheelEventScrollPhase,
                    phase,
                );
            }
            event
        };
        let mut filter = Filter::default();
        let owner = Ownership {
            session: 7,
            owner: Owner::Rift,
            consume: true,
            touching: true,
            dock_owner: None,
        };
        assert!(!filter.forward(CGEventType::ScrollWheel, &scroll(1), || Box::new(owner), |_| {}));
        assert!(!filter.forward(CGEventType::ScrollWheel, &scroll(4), || Box::new(owner), |_| {}));
        assert!(filter.forward(
            CGEventType::ScrollWheel,
            &scroll(0),
            || Box::new(Ownership::default()),
            |_| {}
        ));
        let momentum = scroll(0);
        CGEvent::set_integer_value_field(
            Some(&momentum),
            CGEventField::ScrollWheelEventMomentumPhase,
            2,
        );
        assert!(!filter.forward(
            CGEventType::ScrollWheel,
            &momentum,
            || Box::new(Ownership::default()),
            |_| {}
        ));
    }
}
