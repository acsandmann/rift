use std::time::{Duration, Instant};

use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use super::TransactionId;
use crate::actor::app::{AppThreadHandle, Request, WindowId};
use crate::common::config::AnimationEasing;
use crate::sys::timer::Timer;

#[derive(Debug)]
pub struct Animation<'a> {
    //start: CFAbsoluteTime,
    //interval: CFTimeInterval,
    start: Instant,
    interval: Duration,
    frames: u32,

    windows: Vec<(
        &'a AppThreadHandle,
        WindowId,
        CGRect,
        CGRect,
        bool,
        TransactionId,
    )>,
}

impl<'a> Animation<'a> {
    pub fn new(fps: f64, duration: f64, _: AnimationEasing) -> Self {
        let interval = Duration::from_secs_f64(1.0 / fps);
        // let now = unsafe { CFAbsoluteTimeGetCurrent() };
        let now = Instant::now();
        Animation {
            start: now, // + interval, // not necessary, provide one extra frame to get things going
            interval,
            frames: (duration * fps).round() as u32,
            windows: vec![],
        }
    }

    pub fn add_window(
        &mut self,
        handle: &'a AppThreadHandle,
        wid: WindowId,
        start: CGRect,
        finish: CGRect,
        is_focus: bool,
        txid: TransactionId,
    ) {
        self.windows.push((handle, wid, start, finish, is_focus, txid))
    }

    pub fn run(self) {
        if self.windows.is_empty() {
            return;
        }

        for &(handle, wid, from, to, is_focus, txid) in &self.windows {
            _ = handle.send(Request::BeginWindowAnimation(wid));
            // Resize new windows immediately.
            if is_focus {
                let frame = CGRect {
                    origin: from.origin,
                    size: to.size,
                };
                _ = handle.send(Request::SetWindowFrame(wid, frame, txid, true));
            }
        }

        let mut next_frames = Vec::with_capacity(self.windows.len());
        for frame in 1..=self.frames {
            let t: f64 = f64::from(frame) / f64::from(self.frames);

            next_frames.clear();
            for (_, _, from, to, _, _) in &self.windows {
                next_frames.push(get_frame(*from, *to, t));
            }

            let deadline = self.start + frame * self.interval;
            let duration = deadline - Instant::now();
            if duration < Duration::ZERO {
                continue;
            }
            Timer::sleep(duration);

            for (&(handle, wid, _, to, _, txid), rect) in self.windows.iter().zip(&next_frames) {
                let mut rect = *rect;
                // Actually don't animate size, too slow. Resize halfway through
                // and then set the size again at the end, in case it got
                // clipped during the animation.
                if frame * 2 == self.frames || frame == self.frames {
                    rect.size = to.size;
                    _ = handle.send(Request::SetWindowFrame(wid, rect, txid, true));
                } else {
                    _ = handle.send(Request::SetWindowPos(wid, rect.origin, txid, true));
                }
            }
        }

        for &(handle, wid, ..) in &self.windows {
            _ = handle.send(Request::EndWindowAnimation(wid));
        }
    }

    #[allow(dead_code)]
    pub fn skip_to_end(self) {
        for &(handle, wid, _from, to, _, txid) in &self.windows {
            _ = handle.send(Request::SetWindowFrame(wid, to, txid, true));
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

// https://notes.yvt.jp/Graphics/Easing-Functions/
fn ease(t: f64) -> f64 {
    if t < 0.5 {
        (1.0 - f64::sqrt(1.0 - f64::powi(2.0 * t, 2))) / 2.0
    } else {
        (f64::sqrt(1.0 - f64::powi(-2.0 * t + 2.0, 2)) + 1.0) / 2.0
    }
}

fn blend(a: f64, b: f64, s: f64) -> f64 { (1.0 - s) * a + s * b }
