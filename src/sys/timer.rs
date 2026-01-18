//! Timer functionality using CFRunLoopTimer that integrates with the async executor.
//!
//! This module provides async timer functionality that works seamlessly with the
//! CFRunLoop-based async executor. Timers can be used for delays, timeouts, periodic
//! tasks, and other time-based operations in async code.
//!
//! # Examples
//!
//! ## Basic Sleep
//!
//! ```rust
//! use std::time::Duration;
//! use rift_wm::sys::timer::Timer;
//! use rift_wm::sys::executor::Executor;
//!
//! Executor::run(async {
//!     println!("Starting...");
//!     Timer::sleep(Duration::from_millis(100)).await;
//!     println!("Done after 100ms!");
//! });
//! ```
//!
//! ## Timeout Pattern
//!
//! ```rust
//! use std::time::Duration;
//! use rift_wm::sys::timer::Timer;
//!
//! async fn with_timeout() {
//!     let timeout = Timer::sleep(Duration::from_secs(5));
//!     let operation = some_long_operation();
//!
//!     // In real code, you might use tokio::select! or similar
//!     timeout.await;
//!     println!("Operation timed out!");
//! }
//!
//! async fn some_long_operation() {
//!     Timer::sleep(Duration::from_secs(10)).await;
//! }
//! ```
//!
//! ## Repeating Timer
//!
//! ```rust
//! use std::time::Duration;
//! use rift_wm::sys::timer::Timer;
//!
//! async fn monitoring_loop() {
//!     let mut monitor = Timer::repeating(
//!         Duration::from_millis(0),   // start immediately
//!         Duration::from_millis(1000) // check every second
//!     );
//!
//!     let mut check = 1;
//!     while let Some(_) = monitor.next().await {
//!         println!("System check #{}", check);
//!         check += 1;
//!
//!         // Early termination based on condition
//!         if should_stop() {
//!             monitor.cancel();
//!         }
//!     }
//! }
//!
//! fn should_stop() -> bool { false }
//! ```
//!
//! ## Timer Cancellation
//!
//! ```rust
//! use std::time::Duration;
//! use rift_wm::sys::timer::Timer;
//!
//! async fn cancellable_operation() {
//!     let timer = Timer::sleep(Duration::from_secs(10));
//!
//!     // Cancel the timer early
//!     timer.cancel();
//!
//!     // This will complete immediately since timer was cancelled
//!     timer.await;
//!     println!("Timer was cancelled!");
//! }
//! ```

use std::ffi::c_void;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use objc2_core_foundation::{
    CFAbsoluteTime, CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopTimer,
    CFRunLoopTimerContext, CFTimeInterval, kCFAllocatorDefault, kCFRunLoopCommonModes,
};
use parking_lot::Mutex;
use tokio_stream::Stream;

/// A timer that can be awaited in async code.
///
/// This timer integrates with the CFRunLoop-based async executor and provides
/// a way to schedule delayed execution. Timers are created using either
/// [`Timer::sleep`] for simple delays or [`Timer::new`] for more control.
///
/// # Thread Safety
///
/// Timers are installed on the RunLoop of the thread where they are created.
/// The RunLoop must be running for the timer to fire.
///
/// Timers are thread-safe and can be safely shared between threads.
///
/// # Examples
///
/// ```rust
/// use std::time::Duration;
/// use rift_wm::sys::timer::Timer;
///
/// async fn example() {
///     // Simple delay
///     Timer::sleep(Duration::from_millis(100)).await;
///
///     // Timer that can be cancelled
///     let timer = Timer::sleep(Duration::from_secs(5));
///     timer.cancel(); // Cancel early
///     timer.await;    // Completes immediately
/// }
/// ```
pub struct Timer {
    inner: Arc<Mutex<TimerState>>,
}

struct TimerState {
    cf_timer: Option<CFRetained<CFRunLoopTimer>>,
    waker: Option<Waker>,
    status: TimerStatus,
}

enum TimerStatus {
    Pending,
    Cancelled,
    Fired,
}

impl Timer {
    /// Creates a new timer that will fire after the specified duration.
    ///
    /// This is the most common way to create a timer for simple delays.
    /// The returned `Timer` can be awaited to suspend execution until the timer fires.
    ///
    /// # Thread Safety
    ///
    /// This method is safe to call from any thread, but the timer will be added
    /// to the current thread's run loop.
    pub fn sleep(duration: Duration) -> Self {
        let fire_time = CFAbsoluteTimeGetCurrent() + duration.as_secs_f64();
        Self::new(fire_time, 0.0) // 0.0 interval means fire once
    }

    /// Creates a repeating timer that fires at regular intervals.
    ///
    /// This creates a CFRunLoopTimer with a repeat interval. You can await the timer
    /// multiple times with the convenience method `timer.next().await`.
    ///
    /// # Arguments
    ///
    /// * `initial_delay` - How long to wait before the first fire
    /// * `interval` - The repeat interval
    pub fn repeating(initial_delay: Duration, interval: Duration) -> Self {
        let fire_time = CFAbsoluteTimeGetCurrent() + initial_delay.as_secs_f64();
        Self::new(fire_time, interval.as_secs_f64())
    }

    /// Creates a repeating timer that can be controlled with `set_next_fire`.
    ///
    /// This is much cheaper than creating a new timer every time you want to
    /// set a new fire time.
    pub fn manual() -> Self {
        // Make a repeating timer with a very long duration. This is recommended in the Apple docs:
        // https://developer.apple.com/documentation/corefoundation/cfrunlooptimersetnextfiredate(_:_:)
        Self::repeating(Duration::MAX, Duration::MAX)
    }

    /// Creates a new timer that fires at the specified absolute time.
    ///
    /// This is a lower-level method for creating timers with precise timing control.
    /// For simple delays, prefer [`Timer::sleep`] instead.
    ///
    /// # Arguments
    ///
    /// * `fire_date` - The absolute time when the timer should fire (CFAbsoluteTime)
    /// * `interval` - The repeat interval in seconds (0.0 for one-shot timer)
    pub fn new(fire_date: CFAbsoluteTime, interval: CFTimeInterval) -> Self {
        let inner = Arc::new(Mutex::new(TimerState {
            cf_timer: None,
            waker: None,
            status: TimerStatus::Pending,
        }));

        // We use a Weak reference to avoid circular references
        let timer_ref = Arc::downgrade(&inner);

        // This extra level of indirection isn't necessary, but it's more
        // convenient to use Arc::{increment,decrement}_strong_count below and
        // the same functions don't exist for weak counts.
        let callback_info = Arc::new(timer_ref);

        unsafe extern "C-unwind" fn retain(info: *const c_void) -> *const c_void {
            // SAFETY: The pointer was passed to CFRunLoopTimerContext.info below.
            unsafe { Arc::increment_strong_count(info.cast::<Weak<Mutex<TimerState>>>()) };
            info
        }

        unsafe extern "C-unwind" fn release(info: *const c_void) {
            // SAFETY: The pointer was passed to CFRunLoopTimerContext.info below.
            unsafe { Arc::decrement_strong_count(info.cast::<Weak<Mutex<TimerState>>>()) };
        }

        unsafe extern "C-unwind" fn timer_fire_callback(
            _timer: *mut CFRunLoopTimer,
            info: *mut c_void,
        ) {
            if info.is_null() {
                return;
            }

            // SAFETY: The pointer was passed to CFRunLoopTimerContext.info below.
            let timer_ref = unsafe { &*info.cast::<Weak<Mutex<TimerState>>>() };

            let Some(timer) = timer_ref.upgrade() else { return };
            let mut state = timer.lock();
            state.status = TimerStatus::Fired;
            if let Some(waker) = state.waker.take() {
                waker.wake();
            }
        }

        // This is marked `mut` to match the signature of `CFRunLoopTimer::new`,
        // but the information is copied, and not actually mutated.
        let mut context = CFRunLoopTimerContext {
            version: 0,
            // This pointer is retained by CF on creation.
            info: Arc::as_ptr(&callback_info) as *mut c_void,
            retain: Some(retain),
            release: Some(release),
            copyDescription: None,
        };

        // SAFETY: The retain/release callbacks and info are thread-safe.
        let cf_timer = unsafe {
            CFRunLoopTimer::new(
                kCFAllocatorDefault,
                fire_date,
                interval,
                0, // flags - documentation says to pass 0 for future compatibility
                0, // order
                Some(timer_fire_callback),
                &mut context,
            )
        }
        .expect("Failed to create CFRunLoopTimer");

        // Add timer to current run loop
        let current_loop = CFRunLoop::current().expect("Failed to get current run loop");
        current_loop.add_timer(Some(&cf_timer), unsafe { kCFRunLoopCommonModes });

        {
            let mut state = inner.lock();
            state.cf_timer = Some(cf_timer);
        }

        Timer { inner }
    }

    /// Sets the next time the timer will fire.
    pub fn set_next_fire(&self, delay: Duration) {
        let mut state = self.inner.lock();
        let Some(cf_timer) = state.cf_timer.as_mut() else {
            return;
        };
        let fire_time = CFAbsoluteTimeGetCurrent() + delay.as_secs_f64();
        cf_timer.set_next_fire_date(fire_time);
    }

    /// Cancels the timer, preventing it from firing.
    ///
    /// After calling this method, awaiting the timer will complete immediately
    /// without the timer having fired. `next()` will return `None`.
    pub fn cancel(&self) {
        let mut state = self.inner.lock();
        if let Some(cf_timer) = state.cf_timer.take() {
            cf_timer.invalidate();
        }
        state.status = TimerStatus::Cancelled;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    /// Convenience method to get the next timer event.
    pub fn next(&mut self) -> impl Future<Output = Option<()>> {
        tokio_stream::StreamExt::next(self)
    }
}

impl Stream for Timer {
    type Item = ();

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = self.inner.lock();
        match state.status {
            TimerStatus::Cancelled => Poll::Ready(None),
            TimerStatus::Fired => {
                // Reset the fired state for repeating timers
                state.status = TimerStatus::Pending;
                Poll::Ready(Some(()))
            }
            TimerStatus::Pending => {
                state.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl Future for Timer {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.poll_next(cx).map(|_| ())
    }
}

impl Drop for Timer {
    fn drop(&mut self) { self.cancel(); }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::sys::executor::Executor;

    #[test]
    fn timer_fires_after_delay() {
        let start = Instant::now();
        let delay = Duration::from_millis(50);

        Executor::run(async move {
            Timer::sleep(delay).await;
        });

        let elapsed = start.elapsed();
        // Allow some tolerance; documentation allows sub-millisecond tweaks.
        assert!(elapsed >= delay - Duration::from_millis(1));
        // Useful for verifying units, but shouldn't run in CI:
        // assert!(elapsed < delay + Duration::from_millis(20));
    }

    #[test]
    fn timer_can_be_cancelled() {
        let timer = Timer::sleep(Duration::from_secs(100));
        timer.cancel();

        let start = Instant::now();
        Executor::run(async move {
            timer.await;
        });

        // Should complete immediately since timer was cancelled
        assert!(start.elapsed() < Duration::from_secs(45));
    }

    #[test]
    fn multiple_timers_work() {
        let start = Instant::now();

        Executor::run(async move {
            let timer1 = Timer::sleep(Duration::from_millis(30));
            let timer2 = Timer::sleep(Duration::from_millis(60));

            timer1.await;
            let mid = start.elapsed();

            timer2.await;
            let end = start.elapsed();

            assert!(mid >= Duration::from_millis(25));
            assert!(end >= Duration::from_millis(55));
            // Useful for verifying units, but shouldn't run in CI:
            // assert!(mid < Duration::from_millis(50));
            // assert!(end < Duration::from_millis(80));
        });
    }

    #[test]
    fn timer_thread_safety() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;

        // This test validates that our Arc<Mutex<TimerState>> approach is thread-safe
        // by ensuring multiple threads can safely interact with timers

        let timer_fired = Arc::new(AtomicBool::new(false));
        let timer_fired_clone = Arc::clone(&timer_fired);

        let handle = thread::spawn(move || {
            Executor::run(async move {
                let timer = Timer::sleep(Duration::from_millis(50));
                timer.await;
                timer_fired_clone.store(true, Ordering::Relaxed);
            });
        });

        // Wait for completion
        handle.join().unwrap();

        // Timer should have fired
        assert!(timer_fired.load(Ordering::Relaxed));
    }

    #[test]
    fn repeating_timer_works() {
        Executor::run(async {
            let start = Instant::now();
            let mut timer = Timer::repeating(
                Duration::from_millis(20), // initial delay
                Duration::from_millis(10), // repeat interval
            );

            timer.next().await; // First fire (after initial delay)
            let elapsed1 = start.elapsed();

            timer.next().await; // Second fire (after repeat interval)
            let elapsed2 = start.elapsed();

            timer.next().await; // Third fire
            let elapsed3 = start.elapsed();

            // Verify timing
            assert!(elapsed1 >= Duration::from_millis(20 - 1));
            assert!(elapsed2 >= Duration::from_millis(30 - 1));
            assert!(elapsed3 >= Duration::from_millis(40 - 1));
        });
    }
}
