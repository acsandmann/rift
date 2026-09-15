use std::ffi::c_short;
use std::ptr::NonNull;
use std::sync::Arc;
#[cfg(debug_assertions)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Wake;

use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint};

/// Arbitrary tag used to distinguish Rift's wake event from events owned by
/// other AppKit clients.
const RIFT_WAKE_EVENT_SUBTYPE: c_short = 0x7266;
const RIFT_WAKE_EVENT_DATA: isize = 0x72696674;

#[cfg(debug_assertions)]
#[derive(Default)]
struct EventLoopStats {
    real_events: AtomicU64,
    synthetic_wakes: AtomicU64,
    coalesced_wakes: AtomicU64,
    wake_calls: AtomicU64,
    empty_returns: AtomicU64,
    update_windows: AtomicU64,
    iterations: AtomicU64,
}

/// A thread-safe waker for the main Cocoa event pump.
pub(super) struct EventLoopWaker {
    pending: AtomicBool,
    app: NonNull<NSApplication>,
    event: Retained<NSEvent>,
    #[cfg(debug_assertions)]
    stats: EventLoopStats,
}

unsafe impl Send for EventLoopWaker {}
unsafe impl Sync for EventLoopWaker {}

impl EventLoopWaker {
    fn new(app: &NSApplication) -> Self {
        Self {
            pending: AtomicBool::new(false),
            app: NonNull::from(app),
            event: Self::make_wake_event(),
            #[cfg(debug_assertions)]
            stats: EventLoopStats::default(),
        }
    }

    fn request_wake(&self) -> bool { !self.pending.swap(true, Ordering::AcqRel) }

    fn post_wake(&self) {
        #[cfg(debug_assertions)]
        self.stats.wake_calls.fetch_add(1, Ordering::Relaxed);
        if !self.request_wake() {
            #[cfg(debug_assertions)]
            self.stats.coalesced_wakes.fetch_add(1, Ordering::Relaxed);
            return;
        }

        unsafe { self.app.as_ref() }.postEvent_atStart(&self.event, false);
    }

    fn make_wake_event() -> Retained<NSEvent> {
        NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::ZERO,
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            RIFT_WAKE_EVENT_SUBTYPE,
            RIFT_WAKE_EVENT_DATA,
            0,
        ).expect("unable to create Rift's Cocoa wake event")
    }

    fn rearm_before_poll(&self) { self.pending.store(false, Ordering::Release); }

    fn is_rift_wake(event: &NSEvent) -> bool {
        event.r#type() == NSEventType::ApplicationDefined
            && event.subtype().0 == RIFT_WAKE_EVENT_SUBTYPE
            && event.data1() == RIFT_WAKE_EVENT_DATA
    }
}

impl Wake for EventLoopWaker {
    fn wake(self: Arc<Self>) { self.post_wake(); }

    fn wake_by_ref(self: &Arc<Self>) { self.post_wake(); }
}

pub(super) struct EventLoop {
    app: Retained<NSApplication>,
    waker: Arc<EventLoopWaker>,
    blocking_deadline: Retained<NSDate>,
    drain_deadline: Retained<NSDate>,
}

impl EventLoop {
    pub fn new(app: Retained<NSApplication>) -> Self {
        let waker = Arc::new(EventLoopWaker::new(&app));
        Self {
            app,
            waker,
            blocking_deadline: NSDate::distantFuture(),
            drain_deadline: NSDate::distantPast(),
        }
    }

    pub fn waker(&self) -> Arc<EventLoopWaker> { self.waker.clone() }

    pub fn wait(&self) {
        #[cfg(debug_assertions)]
        self.waker.stats.iterations.fetch_add(1, Ordering::Relaxed);

        autoreleasepool(|_| {
            let mut deadline = &*self.blocking_deadline;
            let mut dispatched_real_event = false;

            loop {
                let event = unsafe {
                    self.app.nextEventMatchingMask_untilDate_inMode_dequeue(
                        NSEventMask::Any,
                        Some(deadline),
                        NSDefaultRunLoopMode,
                        true,
                    )
                };
                let Some(event) = event else {
                    #[cfg(debug_assertions)]
                    if std::ptr::eq(deadline, &*self.blocking_deadline) {
                        self.waker.stats.empty_returns.fetch_add(1, Ordering::Relaxed);
                    }
                    break;
                };

                deadline = &self.drain_deadline;
                if EventLoopWaker::is_rift_wake(&event) {
                    #[cfg(debug_assertions)]
                    self.waker.stats.synthetic_wakes.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                self.app.sendEvent(&event);
                dispatched_real_event = true;
                #[cfg(debug_assertions)]
                self.waker.stats.real_events.fetch_add(1, Ordering::Relaxed);
            }

            if dispatched_real_event {
                self.app.updateWindows();
                #[cfg(debug_assertions)]
                self.waker.stats.update_windows.fetch_add(1, Ordering::Relaxed);
            }
        });
        self.waker.rearm_before_poll();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_coalesce_until_the_next_poll() {
        // These tests exercise only the atomic state, never posting through app.
        let waker = EventLoopWaker {
            pending: AtomicBool::new(false),
            app: NonNull::dangling(),
            event: EventLoopWaker::make_wake_event(),
            #[cfg(debug_assertions)]
            stats: EventLoopStats::default(),
        };
        assert!(waker.request_wake());
        for _ in 1..100 {
            assert!(!waker.request_wake());
        }
        waker.rearm_before_poll();
        assert!(waker.request_wake());
    }

    #[test]
    fn published_work_survives_a_coalesced_wake_before_rearming() {
        let waker = EventLoopWaker {
            pending: AtomicBool::new(false),
            app: NonNull::dangling(),
            event: EventLoopWaker::make_wake_event(),
            #[cfg(debug_assertions)]
            stats: EventLoopStats::default(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(waker.request_wake());
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let tx = &tx;
                let waker = &waker;
                scope.spawn(move || {
                    for _ in 0..25 {
                        tx.send(()).unwrap();
                        assert!(!waker.request_wake());
                    }
                });
            }
        });
        // The batch ends before polling the queue, even if all wakes coalesced.
        waker.rearm_before_poll();
        assert_eq!(rx.try_iter().count(), 100);
        // A producer racing the subsequent poll must post a new event.
        tx.send(()).unwrap();
        assert!(waker.request_wake());
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn only_rift_tagged_application_events_are_consumed() {
        assert!(EventLoopWaker::is_rift_wake(&EventLoopWaker::make_wake_event()));
        let other = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::ZERO,
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            RIFT_WAKE_EVENT_SUBTYPE,
            0,
            0,
        ).unwrap();
        assert!(!EventLoopWaker::is_rift_wake(&other));
    }
}

#[cfg(debug_assertions)]
impl Drop for EventLoop {
    fn drop(&mut self) {
        tracing::debug!(
            real_cocoa_events = self.waker.stats.real_events.load(Ordering::Relaxed),
            synthetic_rift_wakes = self.waker.stats.synthetic_wakes.load(Ordering::Relaxed),
            coalesced_wake_requests = self.waker.stats.coalesced_wakes.load(Ordering::Relaxed),
            wake_requests = self.waker.stats.wake_calls.load(Ordering::Relaxed),
            empty_returns = self.waker.stats.empty_returns.load(Ordering::Relaxed),
            update_windows_calls = self.waker.stats.update_windows.load(Ordering::Relaxed),
            main_loop_iterations = self.waker.stats.iterations.load(Ordering::Relaxed),
            "Cocoa event loop stopped"
        );
    }
}
