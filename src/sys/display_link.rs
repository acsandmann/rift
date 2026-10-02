//! Display cadence for a locked screen. AppKit objects stay on the main thread.
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use crossbeam_channel::Sender;
use dispatchr::queue;
use dispatchr::time::Time;
use objc2::rc::Retained;
use objc2::{AnyThread, DeclaredClass, MainThreadMarker, define_class, msg_send, sel};
use objc2_app_kit::NSScreen;
use objc2_foundation::{NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;
use parking_lot::Mutex;

use super::dispatch::DispatchExt;
use super::screen::{NSScreenExt, ScreenId};

thread_local! {
    static LINKS: RefCell<HashMap<u64, Retained<CADisplayLink>>> = RefCell::new(HashMap::new());
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// One replaceable tick, never a queue. CA and Rust monotonic clocks are bridged
/// at the callback, so sleep/wake cannot stale the bridge.
#[derive(Clone, Copy, Debug)]
pub struct DisplayTick {
    pub timestamp: f64,
    pub target_timestamp: f64,
    pub sequence: u64,
}

#[derive(Default)]
struct LatestTick(Option<(DisplayTick, Instant)>);
impl LatestTick {
    fn publish(&mut self, timestamp: f64, target_timestamp: f64, target: Instant) {
        let sequence = self.0.map_or(1, |(tick, _)| tick.sequence.wrapping_add(1));
        self.0 = Some((
            DisplayTick {
                timestamp,
                target_timestamp,
                sequence,
            },
            target,
        ));
    }

    fn after(&self, sequence: &mut u64) -> Option<(DisplayTick, Instant)> {
        let (tick, target) = self.0?;
        if tick.sequence == *sequence {
            return None;
        }
        *sequence = tick.sequence;
        Some((tick, target))
    }
}

struct State {
    wake: Sender<()>,
    latest: Mutex<LatestTick>,
    cancelled: AtomicBool,
}

define_class! {
    // NSObject has no subclassing requirements; the selector matches CADisplayLink's callback.
    #[unsafe(super(NSObject))]
    #[ivars = Arc<State>]
    struct DisplayLinkTarget;

    impl DisplayLinkTarget {
        #[unsafe(method(tick:))]
        fn tick(&self, link: &CADisplayLink) {
            let state = self.ivars();
            if state.cancelled.load(Ordering::Acquire) {
                link.invalidate();
                return;
            }
            let timestamp = link.timestamp();
            let target_timestamp = link.targetTimestamp();
            let now = Instant::now();
            let delta = target_timestamp - objc2_quartz_core::CACurrentMediaTime();
            // Bound malformed/runtime timing without losing valid past targets.
            let target = if delta.is_finite() && delta.abs() < 1.0 {
                let duration = std::time::Duration::from_secs_f64(delta.abs());
                if delta >= 0.0 { now + duration } else { now - duration }
            } else { now };
            state.latest.lock().publish(timestamp, target_timestamp, target);
            let _ = state.wake.try_send(());
        }
    }
}

/// Sendable lifetime handle; the retained link and its run-loop operations never leave main.
pub struct DisplayLink {
    id: u64,
    state: Arc<State>,
}

impl DisplayLink {
    pub fn for_display(display: u32, wake: Sender<()>) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(State {
            wake,
            latest: Mutex::default(),
            cancelled: AtomicBool::new(false),
        });
        queue::main().after_f_s(
            Time::NOW,
            (id, display, state.clone()),
            |(id, display, state)| {
                if state.cancelled.load(Ordering::Acquire) {
                    return;
                }
                let mtm = MainThreadMarker::new().expect("main dispatch queue");
                let Some(screen) = NSScreen::screens(mtm)
                    .iter()
                    .find(|screen| screen.get_number() == Ok(ScreenId::new(display)))
                else {
                    return;
                };
                // Older macOS releases keep the same sampler with a timer clock.
                if !screen.respondsToSelector(sel!(displayLinkWithTarget:selector:)) {
                    return;
                }
                let target = DisplayLinkTarget::alloc().set_ivars(state);
                let target: Retained<DisplayLinkTarget> = unsafe { msg_send![super(target), init] };
                // The target is retained by the link. It does not retain the link in return.
                let link = unsafe { screen.displayLinkWithTarget_selector(&target, sel!(tick:)) };
                unsafe {
                    link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes)
                };
                LINKS.with(|links| {
                    links.borrow_mut().insert(id, link);
                });
            },
        );
        Self { id, state }
    }

    pub fn latest(&self, after: &mut u64) -> Option<(DisplayTick, Instant)> {
        self.state.latest.lock().after(after)
    }
}

impl Drop for DisplayLink {
    fn drop(&mut self) {
        self.state.cancelled.store(true, Ordering::Release);
        queue::main().after_f_s(Time::NOW, self.id, |id| {
            LINKS.with(|links| {
                if let Some(link) = links.borrow_mut().remove(&id) {
                    link.invalidate();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_replaces_stale_ticks_and_never_replays_a_sample() {
        let mut ticks = LatestTick::default();
        let mut sequence = 0;
        assert!(ticks.after(&mut sequence).is_none());
        for timestamp in [1.0, 2.0, 3.0] {
            ticks.publish(timestamp, timestamp + 0.008, Instant::now());
        }
        let (newest, _) = ticks.after(&mut sequence).unwrap();
        assert_eq!(newest.timestamp, 3.0);
        assert_eq!(newest.target_timestamp, 3.008);
        assert_eq!(newest.sequence, 3);
        assert!(ticks.after(&mut sequence).is_none());
        ticks.publish(4.0, 4.008, Instant::now());
        assert_eq!(ticks.after(&mut sequence).unwrap().0.sequence, 4);
        assert!(ticks.after(&mut sequence).is_none());
    }
}
