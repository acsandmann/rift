use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use once_cell::sync::Lazy;
use parking_lot::{Condvar, Mutex, RwLock};

use super::TransactionId;
use crate::actor::app::{AppThreadHandle, Request, WindowId};
use crate::common::config::AnimationEasing;
use crate::sys::display_link::DisplayLink;
use crate::sys::enhanced_ui::with_system_enhanced_ui_disabled;
use crate::sys::power::get_max_fps_for_power_state;
use crate::sys::skylight::{SLSDisableUpdate, SLSMainConnectionID, SLSReenableUpdate, cid_t};

pub static G_CONNECTION: Lazy<cid_t> = Lazy::new(|| unsafe { SLSMainConnectionID() });

#[derive(Debug)]
struct WindowAnim {
    handle: AppThreadHandle,
    wid: WindowId,
    from: CGRect,
    to: CGRect,
    last: CGRect,
    bounds: CGRect,
    txid: TransactionId,
    #[allow(dead_code)]
    heavy: bool,
    frame_times: VecDeque<Duration>,
    effective_fps: f64,
    update_interval: Duration,
    last_update_time: Option<std::time::Instant>,
    max_fps: f64,
}

impl WindowAnim {
    fn should_update(&mut self, current_time: std::time::Instant) -> bool {
        match self.last_update_time {
            Some(last) => current_time.duration_since(last) >= self.update_interval,
            None => true,
        }
    }

    fn update_performance(&mut self, frame_time: Duration) {
        self.frame_times.push_back(frame_time);
        if self.frame_times.len() > 5 {
            self.frame_times.pop_front();
        }

        if self.frame_times.len() >= 3 {
            let avg_time =
                self.frame_times.iter().sum::<Duration>() / self.frame_times.len() as u32;
            let max_recent = self.frame_times.iter().max().unwrap_or(&Duration::ZERO);

            if avg_time > Duration::from_millis(20) || *max_recent > Duration::from_millis(33) {
                self.effective_fps = (self.effective_fps * 0.75).max(15.0);
            } else if avg_time < Duration::from_millis(12)
                && *max_recent < Duration::from_millis(16)
            {
                self.effective_fps = (self.effective_fps * 1.1).min(self.max_fps);
            }
            self.update_interval = Duration::from_secs_f64(1.0 / self.effective_fps);
        }
    }
}

#[derive(Debug)]
pub struct Animation {
    _duration: f64,
    frames: u32,
    easing: AnimationEasing,
    windows: Vec<WindowAnim>,
    size_threshold: f64,
    max_fps: f64,
}

impl Animation {
    pub fn new(fps: f64, duration: f64, easing: AnimationEasing) -> Self {
        let effective_fps = get_max_fps_for_power_state(fps);

        Animation {
            _duration: duration,
            frames: (duration * effective_fps).round() as u32,
            windows: Vec::new(),
            easing,
            size_threshold: 1.5,
            max_fps: effective_fps,
        }
    }

    pub fn add_window(
        &mut self,
        handle: AppThreadHandle,
        wid: WindowId,
        start: CGRect,
        finish: CGRect,
        bounds: CGRect,
        txid: TransactionId,
        heavy: bool,
    ) {
        let area = finish.size.width * finish.size.height;
        let base_fps = if heavy {
            35.0
        } else if area > 1_000_000.0 {
            45.0
        } else if area > 500_000.0 {
            50.0
        } else {
            self.max_fps
        };

        let effective_fps = get_max_fps_for_power_state(base_fps);

        self.windows.push(WindowAnim {
            handle,
            wid,
            from: start,
            to: finish,
            last: start,
            bounds,
            txid,
            heavy,
            frame_times: VecDeque::with_capacity(10),
            effective_fps,
            update_interval: Duration::from_secs_f64(1.0 / effective_fps),
            last_update_time: None,
            max_fps: self.max_fps,
        });
    }

    pub fn run(mut self) {
        if self.windows.is_empty() {
            return;
        }

        for w in &mut self.windows {
            let _ = w.handle.send(Request::BeginWindowAnimation(w.wid));
        }

        with_system_enhanced_ui_disabled(|| {
            // Use RwLock for thread-safe interior mutability
            let windows = Arc::new(RwLock::new(self.windows));
            let counter = Arc::new(AtomicU32::new(0));
            let total = self.frames;
            let easing = self.easing;
            let size_eps = self.size_threshold;

            let done_pair: Arc<(Mutex<bool>, Condvar)> =
                Arc::new((Mutex::new(false), Condvar::new()));
            let done_pair_cb = Arc::clone(&done_pair);

            let windows_cloned = windows.clone();
            let counter_cloned = counter.clone();

            let _link = DisplayLink::new(move || {
                let current_time = std::time::Instant::now();

                unsafe {
                    SLSDisableUpdate(*G_CONNECTION);
                }

                let idx = counter_cloned.fetch_add(1, Ordering::SeqCst);
                let t = (idx as f64 + 1.0) / total as f64;
                let s = ease_value(t, &easing);

                let mut windows_mut = windows_cloned.write();
                for w in windows_mut.iter_mut() {
                    if !w.should_update(current_time) {
                        continue;
                    }

                    let mut rect = interpolate_rect_adaptive(w.from, w.to, s, t);

                    if w.bounds != CGRect::zero() {
                        rect = clamp_to_bounds(rect, w.bounds);
                    }

                    let pos_changed = (rect.origin.x - w.last.origin.x).abs() > 0.5
                        || (rect.origin.y - w.last.origin.y).abs() > 0.5;
                    let size_changed = (rect.size.width - w.last.size.width).abs() > size_eps
                        || (rect.size.height - w.last.size.height).abs() > size_eps;

                    if pos_changed || size_changed {
                        let _ = w.handle.send(Request::SetWindowFrame(w.wid, rect, w.txid, false));

                        let frame_time = w
                            .last_update_time
                            .map(|last| current_time.duration_since(last))
                            .unwrap_or(Duration::ZERO);
                        w.update_performance(frame_time);
                        w.last = rect;
                    }
                    w.last_update_time = Some(current_time);
                }

                drop(windows_mut);

                if idx + 1 >= total {
                    unsafe {
                        SLSReenableUpdate(*G_CONNECTION);
                    }

                    let windows_ref = windows_cloned.read();
                    for w in windows_ref.iter() {
                        let mut final_rect = w.to;
                        if w.bounds != CGRect::zero() {
                            final_rect = clamp_to_bounds(final_rect, w.bounds);
                        }

                        let _ = w
                            .handle
                            .send(Request::SetWindowFrame(w.wid, final_rect, w.txid, false));
                        let _ = w.handle.send(Request::EndWindowAnimation(w.wid));
                    }

                    let (lock, cvar) = &*done_pair_cb;
                    let mut done = lock.lock();
                    *done = true;
                    cvar.notify_one();
                    return false;
                }

                unsafe {
                    SLSReenableUpdate(*G_CONNECTION);
                }
                true
            })
            .expect("Failed to create display link");

            _link.start();

            let (lock, cvar) = &*done_pair;
            let mut done = lock.lock();
            while !*done {
                cvar.wait(&mut done);
            }
        });
    }

    pub fn skip_to_end(self) {
        for w in self.windows {
            let mut final_rect = w.to;
            if w.bounds != CGRect::zero() {
                final_rect = clamp_to_bounds(final_rect, w.bounds);
            }
            let _ = w.handle.send(Request::SetWindowFrame(w.wid, final_rect, w.txid, true));
        }
    }
}

fn clamp_to_bounds(rect: CGRect, bounds: CGRect) -> CGRect {
    let mut result = rect;

    if result.origin.x < bounds.origin.x {
        result.origin.x = bounds.origin.x;
    }
    if result.origin.y < bounds.origin.y {
        result.origin.y = bounds.origin.y;
    }
    if result.max_x() > bounds.max_x() {
        result.origin.x = bounds.max_x() - result.size.width;
    }
    if result.max_y() > bounds.max_y() {
        result.origin.y = bounds.max_y() - result.size.height;
    }

    result
}

fn interpolate_rect_adaptive(from: CGRect, to: CGRect, progress: f64, t: f64) -> CGRect {
    // delay size changes for the first 30%
    if t < 0.3 {
        let target_x = from.origin.x + (to.origin.x - from.origin.x) * progress;
        let target_y = from.origin.y + (to.origin.y - from.origin.y) * progress;

        let size_progress = (progress * 3.0).min(1.0);
        let target_width = from.size.width + (to.size.width - from.size.width) * size_progress;
        let target_height = from.size.height + (to.size.height - from.size.height) * size_progress;

        CGRect {
            origin: CGPoint {
                x: target_x.round(),
                y: target_y.round(),
            },
            size: CGSize {
                width: target_width.round(),
                height: target_height.round(),
            },
        }
    } else {
        interpolate_rect_precise(from, to, progress)
    }
}

fn interpolate_rect_precise(from: CGRect, to: CGRect, progress: f64) -> CGRect {
    let target_x = from.origin.x + (to.origin.x - from.origin.x) * progress;
    let target_y = from.origin.y + (to.origin.y - from.origin.y) * progress;
    let target_width = from.size.width + (to.size.width - from.size.width) * progress;
    let target_height = from.size.height + (to.size.height - from.size.height) * progress;

    CGRect {
        origin: CGPoint {
            x: target_x.round(),
            y: target_y.round(),
        },
        size: CGSize {
            width: target_width.round(),
            height: target_height.round(),
        },
    }
}

trait RectExt {
    fn max_x(&self) -> f64;
    fn max_y(&self) -> f64;
    fn zero() -> Self;
}

impl RectExt for CGRect {
    #[inline]
    fn max_x(&self) -> f64 { self.origin.x + self.size.width }

    #[inline]
    fn max_y(&self) -> f64 { self.origin.y + self.size.height }

    #[inline]
    fn zero() -> Self {
        CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: 0.0, height: 0.0 },
        }
    }
}

fn ease_value(t: f64, easing: &AnimationEasing) -> f64 {
    match easing {
        AnimationEasing::Linear => t,
        AnimationEasing::EaseInOut => {
            if t < 0.5 {
                (1.0 - f64::sqrt(1.0 - f64::powi(2.0 * t, 2))) / 2.0
            } else {
                (f64::sqrt(1.0 - f64::powi(-2.0 * t + 2.0, 2)) + 1.0) / 2.0
            }
        }
        AnimationEasing::EaseInSine => 1.0 - f64::cos((t * std::f64::consts::PI) / 2.0),
        AnimationEasing::EaseOutSine => f64::sin((t * std::f64::consts::PI) / 2.0),
        AnimationEasing::EaseInOutSine => -(f64::cos(std::f64::consts::PI * t) - 1.0) / 2.0,
        AnimationEasing::EaseInQuad => t * t,
        AnimationEasing::EaseOutQuad => 1.0 - (1.0 - t) * (1.0 - t),
        AnimationEasing::EaseInOutQuad => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                1.0 - f64::powi(-2.0 * t + 2.0, 2) as f64 / 2.0
            }
        }
        AnimationEasing::EaseInCubic => t * t * t,
        AnimationEasing::EaseOutCubic => 1.0 - f64::powi(1.0 - t, 3) as f64,
        AnimationEasing::EaseInOutCubic => {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                1.0 - f64::powi(-2.0 * t + 2.0, 3) as f64 / 2.0
            }
        }
        AnimationEasing::EaseInQuart => t * t * t * t,
        AnimationEasing::EaseOutQuart => 1.0 - f64::powi(1.0 - t, 4) as f64,
        AnimationEasing::EaseInOutQuart => {
            if t < 0.5 {
                8.0 * t * t * t * t
            } else {
                1.0 - f64::powi(-2.0 * t + 2.0, 4) as f64 / 2.0
            }
        }
        AnimationEasing::EaseInQuint => t * t * t * t * t,
        AnimationEasing::EaseOutQuint => 1.0 - f64::powi(1.0 - t, 5) as f64,
        AnimationEasing::EaseInOutQuint => {
            if t < 0.5 {
                16.0 * t * t * t * t * t
            } else {
                1.0 - f64::powi(-2.0 * t + 2.0, 5) as f64 / 2.0
            }
        }
        AnimationEasing::EaseInExpo => {
            if t == 0.0 {
                0.0
            } else {
                f64::powf(2.0, 10.0 * t - 10.0)
            }
        }
        AnimationEasing::EaseOutExpo => {
            if t == 1.0 {
                1.0
            } else {
                1.0 - f64::powf(2.0, -10.0 * t)
            }
        }
        AnimationEasing::EaseInOutExpo => {
            if t == 0.0 {
                0.0
            } else if t == 1.0 {
                1.0
            } else if t < 0.5 {
                f64::powf(2.0, 20.0 * t - 10.0) / 2.0
            } else {
                (2.0 - f64::powf(2.0, -20.0 * t + 10.0)) / 2.0
            }
        }
        AnimationEasing::EaseInCirc => 1.0 - f64::sqrt(1.0 - t * t),
        AnimationEasing::EaseOutCirc => f64::sqrt(1.0 - f64::powi(t - 1.0, 2) as f64),
        AnimationEasing::EaseInOutCirc => {
            if t < 0.5 {
                (1.0 - f64::sqrt(1.0 - f64::powi(2.0 * t, 2))) / 2.0
            } else {
                (f64::sqrt(1.0 - f64::powi(-2.0 * t + 2.0, 2)) + 1.0) / 2.0
            }
        }
    }
}
