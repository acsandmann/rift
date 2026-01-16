use std::time::{Duration, Instant};

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use tracing::{debug, trace};

use super::TransactionId;
use crate::actor::app::{AppThreadHandle, Request, WindowId, pid_t};
use crate::actor::reactor::Reactor;
use crate::common::collections::HashMap;
use crate::common::config::AnimationEasing;
use crate::sys::geometry::{Round, SameAs};
use crate::sys::power;
use crate::sys::screen::SpaceId;
use crate::sys::timer::Timer;
use crate::sys::window_server::WindowServerId;

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

pub struct AnimationManager;

impl AnimationManager {
    pub fn animate_layout(
        reactor: &mut Reactor,
        space: SpaceId,
        layout: &[(WindowId, CGRect)],
        is_resize: bool,
        skip_wid: Option<WindowId>,
    ) -> bool {
        let Some(active_ws) = reactor.layout_manager.layout_engine.active_workspace(space) else {
            return false;
        };
        let mut anim = Animation::new(
            reactor.config.settings.animation_fps,
            reactor.config.settings.animation_duration,
            reactor.config.settings.animation_easing.clone(),
        );
        let mut animated_count = 0;
        let mut animated_wids_wsids: Vec<u32> = Vec::new();
        let mut any_frame_changed = false;

        for &(wid, target_frame) in layout {
            // Skip applying layout frames and animations for the window currently being dragged.
            if skip_wid == Some(wid) {
                trace!(
                    ?wid,
                    "Skipping animated layout update for window currently being dragged"
                );
                continue;
            }

            let target_frame = target_frame.round();
            let (current_frame, window_server_id, txid) =
                match reactor.window_manager.windows.get_mut(&wid) {
                    Some(window) => {
                        let current_frame = window.frame_monotonic;
                        if target_frame.same_as(current_frame) {
                            continue;
                        }
                        any_frame_changed = true;
                        let wsid = window.window_server_id.unwrap();
                        let txid = reactor.transaction_manager.generate_next_txid(wsid);
                        (current_frame, Some(wsid), txid)
                    }
                    None => {
                        debug!(?wid, "Skipping - window no longer exists");
                        continue;
                    }
                };

            let Some(app_state) = &reactor.app_manager.apps.get(&wid.pid) else {
                debug!(?wid, "Skipping for window - app no longer exists");
                continue;
            };

            let is_active = reactor
                .layout_manager
                .layout_engine
                .virtual_workspace_manager()
                .workspace_for_window(space, wid)
                .map_or(false, |ws| ws == active_ws);

            if is_active {
                trace!(?wid, ?current_frame, ?target_frame, "Animating visible window");
                animated_wids_wsids.push(wid.idx.into());
                anim.add_window(&app_state.handle, wid, current_frame, target_frame, false, txid);
                animated_count += 1;
                if let Some(wsid) = window_server_id {
                    reactor.transaction_manager.update_txid_entries([(wsid, txid, target_frame)]);
                }
            } else {
                trace!(
                    ?wid,
                    ?current_frame,
                    ?target_frame,
                    "Direct positioning hidden window"
                );
                if let Some(wsid) = window_server_id {
                    reactor.transaction_manager.update_txid_entries([(wsid, txid, target_frame)]);
                }
                if let Err(e) =
                    app_state.handle.send(Request::SetWindowFrame(wid, target_frame, txid, true))
                {
                    debug!(?wid, ?e, "Failed to send frame request for hidden window");
                    continue;
                }
            }

            if let Some(window) = reactor.window_manager.windows.get_mut(&wid) {
                window.frame_monotonic = target_frame;
            }
        }

        if animated_count > 0 {
            let low_power = power::is_low_power_mode_enabled();
            if is_resize || !reactor.config.settings.animate || low_power {
                anim.skip_to_end();
            } else {
                anim.run();
            }
        }

        any_frame_changed
    }

    pub fn instant_layout(
        reactor: &mut Reactor,
        layout: &[(WindowId, CGRect)],
        skip_wid: Option<WindowId>,
    ) -> bool {
        let mut per_app: HashMap<pid_t, Vec<(WindowId, CGRect)>> = HashMap::default();
        let mut any_frame_changed = false;

        for &(wid, target_frame) in layout {
            // Skip applying a layout frame for the window currently being dragged.
            if skip_wid == Some(wid) {
                trace!(?wid, "Skipping layout update for window currently being dragged");
                continue;
            }

            let Some(window) = reactor.window_manager.windows.get_mut(&wid) else {
                debug!(?wid, "Skipping layout - window no longer exists");
                continue;
            };
            let target_frame = target_frame.round();
            let current_frame = window.frame_monotonic;
            if target_frame.same_as(current_frame) {
                continue;
            }
            any_frame_changed = true;
            trace!(
                ?wid,
                ?current_frame,
                ?target_frame,
                "Instant workspace positioning"
            );

            per_app.entry(wid.pid).or_default().push((wid, target_frame));
        }

        for (pid, frames) in per_app.into_iter() {
            if frames.is_empty() {
                continue;
            }

            let Some(app_state) = reactor.app_manager.apps.get(&pid) else {
                debug!(?pid, "Skipping layout update for app - app no longer exists");
                continue;
            };

            let handle = app_state.handle.clone();

            let (first_wid, first_target) = frames[0];
            let mut txid = TransactionId::default();
            let mut has_txid = false;
            let mut txid_entries: Vec<(WindowServerId, TransactionId, CGRect)> = Vec::new();
            if let Some(window) = reactor.window_manager.windows.get_mut(&first_wid) {
                if let Some(wsid) = window.window_server_id {
                    txid = reactor.transaction_manager.generate_next_txid(wsid);
                    has_txid = true;
                    txid_entries.push((wsid, txid, first_target));
                }
            }

            if has_txid {
                for (wid, frame) in frames.iter().skip(1) {
                    if let Some(w) = reactor.window_manager.windows.get_mut(wid) {
                        if let Some(wsid) = w.window_server_id {
                            reactor.transaction_manager.set_last_sent_txid(wsid, txid);
                            txid_entries.push((wsid, txid, *frame));
                        }
                    }
                }
                reactor.transaction_manager.update_txid_entries(txid_entries);
            }

            let frames_to_send = frames.clone();
            if let Err(e) = handle.send(Request::SetBatchWindowFrame(frames_to_send, txid)) {
                debug!(
                    ?pid,
                    ?e,
                    "Failed to send batch frame request - app may have quit"
                );
                continue;
            }

            for (wid, target_frame) in &frames {
                if let Some(window) = reactor.window_manager.windows.get_mut(wid) {
                    window.frame_monotonic = *target_frame;
                }
            }
        }

        any_frame_changed
    }
}
