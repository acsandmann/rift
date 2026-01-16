use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use tracing::{debug, trace};

use super::TransactionId;
use crate::actor::app::{AppThreadHandle, Request, WindowId, pid_t};
use crate::actor::channel;
use crate::actor::reactor::Reactor;
use crate::common::collections::HashMap;
use crate::common::config::AnimationEasing;
use crate::sys::display_link::DisplayLink;
use crate::sys::geometry::{Round, SameAs};
use crate::sys::power;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

#[derive(Debug)]
pub struct Animation {
    start: Instant,
    interval: Duration,
    frames: u32,

    windows: Vec<AnimationGroup>,
}

#[derive(Clone, Debug)]
struct AnimationWindow {
    wid: WindowId,
    from: CGRect,
    to: CGRect,
    is_focus: bool,
}

#[derive(Clone, Debug)]
struct AnimationGroup {
    handle: AppThreadHandle,
    pid: pid_t,
    txid: TransactionId,
    windows: Vec<AnimationWindow>,
}

#[derive(Clone, Debug)]
pub struct AnimationState {
    start: Instant,
    from: CGRect,
    to: CGRect,
    duration: Duration,
}

impl AnimationState {
    pub fn new(start: Instant, from: CGRect, to: CGRect, duration: Duration) -> Self {
        Self { start, from, to, duration }
    }

    pub fn current_frame(&self, now: Instant) -> Option<CGRect> {
        if self.duration.is_zero() {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.start);
        if elapsed >= self.duration {
            return None;
        }
        let t = elapsed.as_secs_f64() / self.duration.as_secs_f64();
        Some(get_frame(self.from, self.to, t))
    }
}

#[derive(Clone, Debug)]
pub struct AnimationCancel {
    generation: u64,
    token: Arc<AtomicU64>,
}

impl AnimationCancel {
    pub fn new(token: Arc<AtomicU64>, generation: u64) -> Self { Self { generation, token } }

    pub fn is_cancelled(&self) -> bool { self.token.load(Ordering::Relaxed) != self.generation }
}

impl Animation {
    pub fn new(fps: f64, duration: f64, _: AnimationEasing) -> Self {
        let resolved_fps = if fps > 0.0 {
            fps
        } else {
            DisplayLink::new(|| false)
                .ok()
                .and_then(|link| link.get_refresh_rate())
                .filter(|rate| *rate > 0.0)
                .unwrap_or(60.0)
        };
        let interval = Duration::from_secs_f64(1.0 / resolved_fps);
        let now = Instant::now();

        Animation {
            start: now,
            interval,
            frames: (duration * resolved_fps).round() as u32,
            windows: vec![],
        }
    }

    pub fn add_window(
        &mut self,
        handle: &AppThreadHandle,
        wid: WindowId,
        start: CGRect,
        finish: CGRect,
        is_focus: bool,
        txid: TransactionId,
    ) {
        if let Some(group) =
            self.windows.iter_mut().find(|group| group.pid == wid.pid && group.txid == txid)
        {
            group.windows.push(AnimationWindow {
                wid,
                from: start,
                to: finish,
                is_focus,
            });
            return;
        }

        self.windows.push(AnimationGroup {
            handle: handle.clone(),
            pid: wid.pid,
            txid,
            windows: vec![AnimationWindow {
                wid,
                from: start,
                to: finish,
                is_focus,
            }],
        });
    }

    pub fn run(self, cancel: Option<AnimationCancel>) {
        if self.windows.is_empty() {
            return;
        }

        for group in &self.windows {
            for window in &group.windows {
                _ = group.handle.send(Request::BeginWindowAnimation(window.wid));
                // Resize new windows immediately.
                if window.is_focus {
                    let frame = CGRect {
                        origin: window.from.origin,
                        size: window.to.size,
                    };
                    _ = group
                        .handle
                        .send(Request::SetWindowFrame(window.wid, frame, group.txid, true));
                }
            }
        }

        if self.frames == 0 {
            for group in &self.windows {
                for window in &group.windows {
                    _ = group.handle.send(Request::EndWindowAnimation(window.wid));
                }
            }
            return;
        }

        let start = self.start;
        let interval = self.interval;
        let frames = self.frames;
        let total_duration = interval.mul_f64(frames as f64);
        let windows = self.windows;
        let windows_for_link = windows.clone();
        let cancel_for_link = cancel.clone();

        let (done_tx, mut done_rx) = channel();
        let mut last_frame_sent = 0u32;
        let mut mid_resize_sent = false;
        let mut completed = false;

        let display_link = DisplayLink::new(move || {
            if completed {
                return false;
            }

            if cancel_for_link.as_ref().map_or(false, |c| c.is_cancelled()) {
                for group in &windows_for_link {
                    for window in &group.windows {
                        _ = group.handle.send(Request::EndWindowAnimation(window.wid));
                    }
                }
                completed = true;
                let _ = done_tx.send(());
                return false;
            }

            let elapsed = Instant::now().saturating_duration_since(start);
            let t = if total_duration.is_zero() {
                1.0
            } else {
                (elapsed.as_secs_f64() / total_duration.as_secs_f64()).min(1.0)
            };
            let frame_index = if frames == 0 {
                0
            } else {
                (t * f64::from(frames)).floor() as u32
            };

            if frame_index == last_frame_sent && last_frame_sent < frames {
                return true;
            }

            let should_resize = if t >= 1.0 {
                true
            } else if !mid_resize_sent && t >= 0.5 {
                mid_resize_sent = true;
                true
            } else {
                false
            };

            for group in &windows_for_link {
                let mut frame_updates: Vec<(WindowId, CGRect)> = Vec::new();
                let mut pos_updates: Vec<(WindowId, CGPoint)> = Vec::new();

                for window in &group.windows {
                    let mut rect = get_frame(window.from, window.to, t);
                    // Actually don't animate size, too slow. Resize halfway through
                    // and then set the size again at the end, in case it got
                    // clipped during the animation.
                    if should_resize {
                        rect.size = window.to.size;
                        frame_updates.push((window.wid, rect));
                    } else {
                        pos_updates.push((window.wid, rect.origin));
                    }
                }

                if !frame_updates.is_empty() {
                    _ = group.handle.send(Request::SetBatchWindowFrame(frame_updates, group.txid));
                }
                if !pos_updates.is_empty() {
                    _ = group.handle.send(Request::SetBatchWindowPos(
                        pos_updates,
                        group.txid,
                        true,
                    ));
                }
            }

            last_frame_sent = frame_index;

            if last_frame_sent >= frames {
                for group in &windows_for_link {
                    for window in &group.windows {
                        _ = group.handle.send(Request::EndWindowAnimation(window.wid));
                    }
                }
                completed = true;
                let _ = done_tx.send(());
                return false;
            }

            true
        });

        match display_link {
            Ok(link) => {
                link.start();
                let _ = done_rx.blocking_recv();
            }
            Err(_) => {
                for group in &windows {
                    for window in &group.windows {
                        _ = group
                            .handle
                            .send(Request::SetWindowFrame(window.wid, window.to, group.txid, true));
                        _ = group.handle.send(Request::EndWindowAnimation(window.wid));
                    }
                }
            }
        }
    }

    pub fn run_async(self, cancel: Option<AnimationCancel>) {
        std::thread::spawn(move || self.run(cancel));
    }

    #[allow(dead_code)]
    pub fn skip_to_end(self) {
        for group in &self.windows {
            for window in &group.windows {
                _ = group
                    .handle
                    .send(Request::SetWindowFrame(window.wid, window.to, group.txid, true));
            }
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
        let mut animated_states: Vec<(WindowId, AnimationState)> = Vec::new();
        let mut carry_over: Vec<(WindowId, CGRect, CGRect, AppThreadHandle, WindowServerId)> =
            Vec::new();
        let mut per_app_txid: HashMap<pid_t, TransactionId> = HashMap::default();
        let mut animated_wids_wsids: Vec<u32> = Vec::new();
        let mut any_frame_changed = false;
        let now = Instant::now();
        let animation_duration =
            Duration::from_secs_f64(reactor.config.settings.animation_duration);

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
            let (current_frame, window_server_id, txid, carry_same_target) =
                match reactor.window_manager.windows.get_mut(&wid) {
                    Some(window) => {
                        let mut current_frame = window.frame_monotonic;
                        let mut carry_same_target = false;
                        if let Some(state) = window.anim_state.as_ref() {
                            if let Some(frame) = state.current_frame(now) {
                                current_frame = frame;
                                carry_same_target = target_frame.same_as(state.to);
                            } else {
                                window.anim_state = None;
                            }
                        }
                        if !carry_same_target && target_frame.same_as(current_frame) {
                            continue;
                        }
                        any_frame_changed = true;
                        let wsid = window.window_server_id.unwrap();
                        let txid = per_app_txid.entry(wid.pid).or_insert_with(|| {
                            reactor.transaction_manager.generate_next_txid(wsid)
                        });
                        (current_frame, Some(wsid), *txid, carry_same_target)
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
                if carry_same_target {
                    if let Some(wsid) = window_server_id {
                        carry_over.push((
                            wid,
                            current_frame,
                            target_frame,
                            app_state.handle.clone(),
                            wsid,
                        ));
                    }
                } else {
                    animated_wids_wsids.push(wid.idx.into());
                    if let Some(wsid) = window_server_id {
                        anim.add_window(
                            &app_state.handle,
                            wid,
                            current_frame,
                            target_frame,
                            false,
                            txid,
                        );
                        animated_count += 1;
                        animated_states.push((
                            wid,
                            AnimationState::new(
                                now,
                                current_frame,
                                target_frame,
                                animation_duration,
                            ),
                        ));
                        reactor.transaction_manager.update_txid_entries([(
                            wsid,
                            txid,
                            target_frame,
                        )]);
                    }
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
                    if let Err(e) = app_state.handle.send(Request::SetWindowFrame(
                        wid,
                        target_frame,
                        txid,
                        true,
                    )) {
                        debug!(?wid, ?e, "Failed to send frame request for hidden window");
                        continue;
                    }
                }
            }

            if let Some(window) = reactor.window_manager.windows.get_mut(&wid) {
                window.frame_monotonic = target_frame;
                if !is_active {
                    window.anim_state = None;
                }
            }
        }

        if animated_count > 0 && !carry_over.is_empty() {
            for (wid, from, to, handle, wsid) in carry_over {
                let txid = per_app_txid
                    .entry(wid.pid)
                    .or_insert_with(|| reactor.transaction_manager.generate_next_txid(wsid));
                anim.add_window(&handle, wid, from, to, false, *txid);
                animated_count += 1;
                animated_states.push((wid, AnimationState::new(now, from, to, animation_duration)));
                reactor.transaction_manager.update_txid_entries([(wsid, *txid, to)]);
            }
        }

        if animated_count > 0 {
            let low_power = power::is_low_power_mode_enabled();
            if is_resize || !reactor.config.settings.animate || low_power {
                anim.skip_to_end();
                for (wid, _) in animated_states {
                    if let Some(window) = reactor.window_manager.windows.get_mut(&wid) {
                        window.anim_state = None;
                    }
                }
            } else {
                let generation = reactor.animation_generation.fetch_add(1, Ordering::Relaxed) + 1;
                let cancel = AnimationCancel::new(reactor.animation_generation.clone(), generation);
                for (wid, state) in animated_states {
                    if let Some(window) = reactor.window_manager.windows.get_mut(&wid) {
                        window.anim_state = Some(state);
                    }
                }
                anim.run_async(Some(cancel));
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
                    window.anim_state = None;
                }
            }
        }

        any_frame_changed
    }
}
