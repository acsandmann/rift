use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use tokio::sync::mpsc;
use tracing::{debug, trace};

use super::TransactionId;
use crate::actor::app::{AppThreadHandle, Request, WindowId, pid_t};
use crate::actor::reactor::Reactor;
use crate::common::collections::HashMap;
use crate::common::config::Config;
use crate::sys::geometry::{Round, SameAs};
use crate::sys::power;
use crate::sys::screen::SpaceId;
use crate::sys::timer::Timer;
use crate::sys::window_server::WindowServerId;

pub type Sender = mpsc::UnboundedSender<Message>;
pub type Receiver = mpsc::UnboundedReceiver<Message>;

/// Countable TRACE instrumentation for every animation-decline gate
/// (Q2 §§4.2-4.3 legitimate-skip list, northstar §5 G2).
///
/// ADDITIVE TRACING ONLY: nothing reads these counters on any decision path,
/// so they can never alter control flow or behavior. Each gate increments its
/// counter and emits a `decline_gate`-tagged TRACE line; `decline_counts()`
/// exposes the totals to tests (see `DeclineSnapshot`).
struct DeclineCounters {
    same_as: AtomicU64,
    tx_duplicate: AtomicU64,
    hidden_window: AtomicU64,
    is_resize: AtomicU64,
    low_power: AtomicU64,
    animate_off: AtomicU64,
    transition_none: AtomicU64,
    zero_duration: AtomicU64,
    no_windows: AtomicU64,
}

static DECLINE: DeclineCounters = DeclineCounters {
    same_as: AtomicU64::new(0),
    tx_duplicate: AtomicU64::new(0),
    hidden_window: AtomicU64::new(0),
    is_resize: AtomicU64::new(0),
    low_power: AtomicU64::new(0),
    animate_off: AtomicU64::new(0),
    transition_none: AtomicU64::new(0),
    zero_duration: AtomicU64::new(0),
    no_windows: AtomicU64::new(0),
};

/// Snapshot of decline-gate totals. Fields parallel the gate list above;
/// `animate_off` covers the remaining `SkipToEnd` arm (`animate = false`) plus
/// the workspace-switch gates that decline for the same reason, and
/// `transition_none` / `zero_duration` / `no_windows` are workspace-switch only.
/// Test read-out (production visibility is the TRACE lines themselves); the
/// issue-8 contract task can un-gate this if it needs runtime reads.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub struct DeclineSnapshot {
    pub same_as: u64,
    pub tx_duplicate: u64,
    pub hidden_window: u64,
    pub is_resize: u64,
    pub low_power: u64,
    pub animate_off: u64,
    pub transition_none: u64,
    pub zero_duration: u64,
    pub no_windows: u64,
}

/// Read current decline-gate totals. Pure load; never affects layout decisions.
#[cfg(test)]
pub fn decline_counts() -> DeclineSnapshot {
    DeclineSnapshot {
        same_as: DECLINE.same_as.load(Ordering::Relaxed),
        tx_duplicate: DECLINE.tx_duplicate.load(Ordering::Relaxed),
        hidden_window: DECLINE.hidden_window.load(Ordering::Relaxed),
        is_resize: DECLINE.is_resize.load(Ordering::Relaxed),
        low_power: DECLINE.low_power.load(Ordering::Relaxed),
        animate_off: DECLINE.animate_off.load(Ordering::Relaxed),
        transition_none: DECLINE.transition_none.load(Ordering::Relaxed),
        zero_duration: DECLINE.zero_duration.load(Ordering::Relaxed),
        no_windows: DECLINE.no_windows.load(Ordering::Relaxed),
    }
}

#[derive(Debug)]
pub enum Message {
    Replace(Animation),
    SkipToEnd(Animation),
}

#[derive(Debug, Default)]
pub struct AnimationManager {
    active: Option<ActiveAnimation>,
}

#[derive(Debug)]
struct ActiveAnimation {
    animation: Animation,
    next_frame: u32,
}

#[derive(Debug)]
pub struct Animation {
    interval: Duration,
    frames: u32,
    windows: Vec<AnimatedWindow>,
    handled_windows: Vec<WindowId>,
}

#[derive(Debug)]
struct AnimatedWindow {
    handle: AppThreadHandle,
    wid: WindowId,
    start: CGRect,
    finish: CGRect,
    is_focus: bool,
    txid: TransactionId,
}

impl AnimatedWindow {
    fn frame_after(&self, frame: u32, total_frames: u32) -> CGRect {
        if frame == 0 {
            return if self.is_focus {
                CGRect {
                    origin: self.start.origin,
                    size: self.finish.size,
                }
            } else {
                self.start
            };
        }

        let t = f64::from(frame) / f64::from(total_frames);
        let mut rect = get_frame(self.start, self.finish, t);
        if self.is_focus || frame * 2 >= total_frames {
            rect.size = self.finish.size;
        } else {
            rect.size = self.start.size;
        }
        rect
    }
}

impl AnimationManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn run(mut rx: Receiver) {
        let mut manager = Self::new();
        let mut tick_timer = Timer::manual();

        loop {
            tokio::select! {
                message = rx.recv() => {
                    let Some(message) = message else {
                        manager.finish_active();
                        break;
                    };
                    if let Some(delay) = manager.handle_message(message) {
                        tick_timer.set_next_fire(delay);
                    }
                }
                _ = tick_timer.next(), if manager.active.is_some() => {
                    if let Some(delay) = manager.tick() {
                        tick_timer.set_next_fire(delay);
                    }
                }
            }
        }
    }

    pub fn handle_message(&mut self, message: Message) -> Option<Duration> {
        match message {
            Message::Replace(animation) => {
                self.active = match self.active.take() {
                    Some(active) => Some(active.replace_with(animation)),
                    None => ActiveAnimation::start(animation),
                };
                self.active.as_ref().map(|active| active.animation.interval)
            }
            Message::SkipToEnd(animation) => {
                self.finish_active();
                animation.skip_to_end();
                None
            }
        }
    }

    pub fn tick(&mut self) -> Option<Duration> {
        let active = self.active.as_mut()?;
        active.send_next_frame();
        if active.is_complete() {
            let active = self.active.take().expect("animation disappeared while ticking");
            active.animation.end();
            None
        } else {
            Some(active.animation.interval)
        }
    }

    fn finish_active(&mut self) {
        if let Some(active) = self.active.take() {
            active.animation.skip_to_end_and_end();
        }
    }

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
        let mut anim = Animation::new(reactor.config.clone());
        let mut animated_count = 0;
        let mut any_frame_changed = false;

        for &(wid, target_frame) in layout {
            if skip_wid == Some(wid) {
                anim.mark_handled(wid);
                trace!(
                    ?wid,
                    "Skipping animated layout update for window currently being dragged"
                );
                continue;
            }

            let target_frame = target_frame.round();
            let (current_frame, window_server_id, txid) = {
                let window_store = &mut reactor.state.windows;
                match window_store.window_mut(wid) {
                    Some(window) => {
                        let current_frame = window.frame_monotonic;
                        // rift-ship-01: two guards for relaunch latch —
                        // (app) coalescing guard in app.rs (flush-before-coalesce
                        // bounded by txid) preserves per-batch backpressure but
                        // cannot unlatch reactor skips; (reactor) one-frame
                        // guard bypasses frame_monotonic/tx redundant checks when
                        // suppress_next_redundant_animation_check is armed at
                        // startup (relaunch). Deferred: partial snapshot
                        // (Reactor::remove_windows_missing_from_active_space_snapshot)
                        // and Ghostty rekey (same_pid fallback in
                        // reactor/events/window_discovery.rs:480,492,518) remain
                        // for follow-up ships — see AGENTS.md §14. ponytail
                        // ceiling: bool + HashMap guards only; generation/VecDeque
                        // if measured.
                        let wsid = window.info.sys_id;
                        let suppress = reactor.suppress_next_redundant_animation_check;
                        if !suppress {
                            if target_frame.same_as(current_frame) {
                                DECLINE.same_as.fetch_add(1, Ordering::Relaxed);
                                trace!(
                                    decline_gate = "same_as",
                                    ?wid,
                                    ?target_frame,
                                    "Declined animation: target matches current frame"
                                );
                                continue;
                            }
                            if let Some(wsid) = wsid {
                                if reactor
                                    .transaction_manager
                                    .get_target_frame(wsid)
                                    .is_some_and(|pending| pending.same_as(target_frame))
                                {
                                    DECLINE.tx_duplicate.fetch_add(1, Ordering::Relaxed);
                                    trace!(
                                        decline_gate = "tx_duplicate",
                                        ?wid,
                                        ?target_frame,
                                        "Skipping redundant layout request"
                                    );
                                    continue;
                                }
                            }
                        }
                        any_frame_changed = true;
                        let txid = wsid
                            .map(|wsid| reactor.transaction_manager.generate_next_txid(wsid))
                            .unwrap_or_default();
                        (current_frame, wsid, txid)
                    }
                    None => {
                        debug!(?wid, "Skipping - window no longer exists");
                        continue;
                    }
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
                .workspace_for_window(&reactor.state.windows, space, wid)
                .is_some_and(|ws| ws == active_ws);

            if is_active {
                trace!(?wid, ?current_frame, ?target_frame, "Animating visible window");
                anim.add_window(&app_state.handle, wid, current_frame, target_frame, false, txid);
                animated_count += 1;
                if let Some(wsid) = window_server_id {
                    reactor.transaction_manager.update_txid_entries([(wsid, txid, target_frame)]);
                }
            } else {
                anim.mark_handled(wid);
                DECLINE.hidden_window.fetch_add(1, Ordering::Relaxed);
                trace!(
                    decline_gate = "hidden_window",
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

            if let Some(window) = reactor.state.windows.window_mut(wid) {
                window.frame_monotonic = target_frame;
            }
        }

        if animated_count > 0 {
            let low_power = power::is_low_power_mode_enabled();
            let layout_animate = reactor
                .layout_manager
                .layout_engine
                .layout_specific_animate_settings(space)
                .unwrap_or(reactor.config.settings.animate);
            let skip_anim = is_resize || !layout_animate || low_power;
            // G2: count which gate forced the instant path. Attribution order is
            // is_resize -> low_power -> animate_off, so concurrent reasons count
            // once, deterministically. Counts only fire when an animation existed.
            if skip_anim {
                if is_resize {
                    DECLINE.is_resize.fetch_add(1, Ordering::Relaxed);
                    trace!(
                        decline_gate = "is_resize",
                        animated_windows = animated_count,
                        "Declined animation: live resize takes instant path"
                    );
                } else if low_power {
                    DECLINE.low_power.fetch_add(1, Ordering::Relaxed);
                    trace!(
                        decline_gate = "low_power",
                        animated_windows = animated_count,
                        "Declined animation: low power mode takes instant path"
                    );
                } else {
                    DECLINE.animate_off.fetch_add(1, Ordering::Relaxed);
                    trace!(
                        decline_gate = "animate_off",
                        animated_windows = animated_count,
                        "Declined animation: animation disabled takes instant path"
                    );
                }
            }
            if let Some(tx) = &reactor.animation_tx {
                let message = if skip_anim {
                    Message::SkipToEnd(anim)
                } else {
                    Message::Replace(anim)
                };
                if let Err(err) = tx.send(message) {
                    match err.0 {
                        Message::Replace(animation) => animation.skip_to_end(),
                        Message::SkipToEnd(animation) => animation.skip_to_end(),
                    }
                }
            } else {
                anim.skip_to_end();
            }
        }

        any_frame_changed
    }

    pub fn instant_layout(
        reactor: &mut Reactor,
        space: SpaceId,
        layout: &[(WindowId, CGRect)],
        skip_wid: Option<WindowId>,
    ) -> bool {
        Self::instant_layout_inner(reactor, space, layout, skip_wid, false)
    }

    /// Apply the position-only layout used while switching virtual workspaces.
    ///
    /// Keep this entry point separate from `instant_layout`: layouts merely suppressed
    /// while a switch is in progress may still change window sizes and must use the
    /// full-frame request.
    pub fn workspace_switch_layout(
        reactor: &mut Reactor,
        space: SpaceId,
        layout: &[(WindowId, CGRect)],
        skip_wid: Option<WindowId>,
    ) -> bool {
        Self::instant_layout_inner(reactor, space, layout, skip_wid, true)
    }

    /// Workspace-switch dispatch gate: whether the caller should attempt the
    /// animated transition at all. Counts the gate that declines it, so a switch
    /// that never enters `workspace_switch_animated` is still audited.
    /// Attribution order is animate_off -> transition_none -> low_power.
    pub(super) fn workspace_switch_wants_animation(reactor: &Reactor) -> bool {
        use crate::common::config::WorkspaceTransition;
        let cfg = &reactor.config.settings;
        let transition_none = cfg.workspace_transition == WorkspaceTransition::None;
        let low_power = power::is_low_power_mode_enabled();
        if cfg.animate && !transition_none && !low_power {
            return true;
        }
        if !cfg.animate {
            DECLINE.animate_off.fetch_add(1, Ordering::Relaxed);
            trace!(
                decline_gate = "animate_off",
                workspace_switch = true,
                "Declined workspace transition: animation disabled"
            );
        } else if transition_none {
            DECLINE.transition_none.fetch_add(1, Ordering::Relaxed);
            trace!(
                decline_gate = "transition_none",
                workspace_switch = true,
                "Declined workspace transition: no transition configured"
            );
        } else {
            DECLINE.low_power.fetch_add(1, Ordering::Relaxed);
            trace!(
                decline_gate = "low_power",
                workspace_switch = true,
                "Declined workspace transition: low power mode"
            );
        }
        false
    }

    /// Animated workspace transition (slide/fade). Returns true if an animation was
    /// started. Falls back to instant layout when no windows or low-power.
    pub fn workspace_switch_animated(
        reactor: &mut Reactor,
        space: SpaceId,
        layout: &[(WindowId, CGRect)],
        screen_frame: CGRect,
        skip_wid: Option<WindowId>,
    ) -> bool {
        use crate::common::config::WorkspaceTransition;
        let transition = reactor.config.settings.workspace_transition;
        if transition == WorkspaceTransition::None {
            DECLINE.transition_none.fetch_add(1, Ordering::Relaxed);
            trace!(
                decline_gate = "transition_none",
                workspace_switch = true,
                "Declined workspace transition: no transition configured"
            );
            return false;
        }
        if reactor.config.settings.animation_duration <= 0.0
            || reactor.config.settings.animation_fps <= 0.0
        {
            DECLINE.zero_duration.fetch_add(1, Ordering::Relaxed);
            trace!(
                decline_gate = "zero_duration",
                workspace_switch = true,
                "Declined workspace transition: non-positive duration or fps"
            );
            return false;
        }
        if power::is_low_power_mode_enabled() {
            DECLINE.low_power.fetch_add(1, Ordering::Relaxed);
            trace!(
                decline_gate = "low_power",
                workspace_switch = true,
                "Declined workspace transition: low power mode"
            );
            return false;
        }
        let layout_animate = reactor
            .layout_manager
            .layout_engine
            .layout_specific_animate_settings(space)
            .unwrap_or(reactor.config.settings.animate);
        if !layout_animate {
            DECLINE.animate_off.fetch_add(1, Ordering::Relaxed);
            trace!(
                decline_gate = "animate_off",
                workspace_switch = true,
                "Declined workspace transition: animation disabled for layout"
            );
            return false;
        }

        let mut anim = Animation::new(reactor.config.clone());
        let mut animated_count = 0usize;
        let mut any_frame_changed = false;

        for &(wid, target_frame) in layout {
            if skip_wid == Some(wid) {
                anim.mark_handled(wid);
                continue;
            }
            let target_frame = target_frame.round();
            let (start_frame, window_server_id, txid) = {
                let Some(window) = reactor.state.windows.window_mut(wid) else {
                    continue;
                };
                let current = window.frame_monotonic;
                if target_frame.same_as(current) && transition == WorkspaceTransition::Fade {
                    // For fade with identical frame, still animate (no positional change)
                    // so we need a start == target but still produce an animation.
                }
                let wsid = window.info.sys_id;
                let txid = wsid
                    .map(|wsid| reactor.transaction_manager.generate_next_txid(wsid))
                    .unwrap_or_default();
                let start = match transition {
                    WorkspaceTransition::Slide => {
                        // Slide incoming windows from offscreen right (+width) to target.
                        // Outgoing windows are not in this layout; their animation would
                        // require separate tracking. For prototype, only incoming slide.
                        let offscreen_x = screen_frame.max().x;
                        CGRect {
                            origin: CGPoint {
                                x: offscreen_x,
                                y: target_frame.origin.y,
                            },
                            size: target_frame.size,
                        }
                    }
                    WorkspaceTransition::Fade => {
                        // Fade keeps frame static; animation still runs to provide
                        // visual duration (future: alpha tween via SLSSetWindowAlpha).
                        target_frame
                    }
                    WorkspaceTransition::None => unreachable!(),
                };
                (start, wsid, txid)
            };

            let handle = {
                let Some(app_state) = reactor.app_manager.apps.get(&wid.pid) else {
                    continue;
                };
                app_state.handle.clone()
            };

            // Commit target to monotonic store immediately (visual tween is overlay)
            if let Some(window) = reactor.state.windows.window_mut(wid) {
                if !target_frame.same_as(window.frame_monotonic) {
                    any_frame_changed = true;
                }
                window.frame_monotonic = target_frame;
            } else {
                continue;
            }

            if let Some(wsid) = window_server_id {
                reactor.transaction_manager.update_txid_entries([(wsid, txid, target_frame)]);
            }

            // For slide we animate from offscreen; for fade we still create an entry
            // so the animation has duration even though start==finish.
            trace!(
                ?wid,
                ?start_frame,
                ?target_frame,
                ?transition,
                "Workspace transition anim"
            );
            anim.add_window(&handle, wid, start_frame, target_frame, false, txid);
            animated_count += 1;
        }

        if animated_count == 0 {
            DECLINE.no_windows.fetch_add(1, Ordering::Relaxed);
            trace!(
                decline_gate = "no_windows",
                workspace_switch = true,
                "Declined workspace transition: no windows to animate"
            );
            return false;
        }

        // If no geometry change but fade was requested, still report changed so
        // the caller does not fall back to instant path and lose the fade duration.
        if !any_frame_changed && transition == WorkspaceTransition::Fade {
            any_frame_changed = true;
        }

        if let Some(tx) = &reactor.animation_tx {
            if let Err(err) = tx.send(Message::Replace(anim)) {
                match err.0 {
                    Message::Replace(animation) => animation.skip_to_end(),
                    Message::SkipToEnd(animation) => animation.skip_to_end(),
                }
            }
        } else {
            anim.skip_to_end();
        }

        any_frame_changed
    }

    fn instant_layout_inner(
        reactor: &mut Reactor,
        space: SpaceId,
        layout: &[(WindowId, CGRect)],
        skip_wid: Option<WindowId>,
        position_only: bool,
    ) -> bool {
        let mut per_app: HashMap<pid_t, Vec<(WindowId, CGRect, bool)>> = HashMap::default();
        let mut any_frame_changed = false;

        for &(wid, target_frame) in layout {
            if skip_wid == Some(wid) {
                trace!(?wid, "Skipping layout update for window currently being dragged");
                continue;
            }

            let is_hidden = !reactor.layout_manager.layout_engine.is_window_in_active_workspace(
                &reactor.state.windows,
                space,
                wid,
            );
            let window_store = &mut reactor.state.windows;
            let Some(window) = window_store.window_mut(wid) else {
                debug!(?wid, "Skipping layout - window no longer exists");
                continue;
            };
            let target_frame = target_frame.round();
            let current_frame = window.frame_monotonic;
            if target_frame.same_as(current_frame) {
                DECLINE.same_as.fetch_add(1, Ordering::Relaxed);
                trace!(
                    decline_gate = "same_as",
                    decline_path = "instant",
                    ?wid,
                    ?target_frame,
                    "Declined instant layout: target matches current frame"
                );
                continue;
            }
            if let Some(wsid) = window.info.sys_id {
                if reactor
                    .transaction_manager
                    .get_target_frame(wsid)
                    .is_some_and(|pending| pending.same_as(target_frame))
                {
                    DECLINE.tx_duplicate.fetch_add(1, Ordering::Relaxed);
                    trace!(
                        decline_gate = "tx_duplicate",
                        decline_path = "instant",
                        ?wid,
                        ?target_frame,
                        "Skipping redundant instant layout request"
                    );
                    continue;
                }
            }
            any_frame_changed = true;
            trace!(
                ?wid,
                ?current_frame,
                ?target_frame,
                hidden = is_hidden,
                "Instant workspace positioning"
            );

            let size_unchanged = current_frame.size.same_as(target_frame.size);
            per_app.entry(wid.pid).or_default().push((wid, target_frame, size_unchanged));
            window.frame_monotonic = target_frame;
        }

        for (pid, frames) in per_app {
            if frames.is_empty() {
                continue;
            }

            let Some(app_state) = reactor.app_manager.apps.get(&pid) else {
                debug!(?pid, "Skipping layout update for app - app no longer exists");
                continue;
            };

            let handle = app_state.handle.clone();

            let (first_wid, first_target, _) = frames[0];
            let mut txid = TransactionId::default();
            let mut has_txid = false;
            let mut txid_entries: Vec<(WindowServerId, TransactionId, CGRect)> = Vec::new();
            if let Some(window) = reactor.state.windows.window_mut(first_wid) {
                if let Some(wsid) = window.info.sys_id {
                    txid = reactor.transaction_manager.generate_next_txid(wsid);
                    has_txid = true;
                    txid_entries.push((wsid, txid, first_target));
                }
            }

            if has_txid {
                for (wid, frame, _) in frames.iter().skip(1) {
                    if let Some(w) = reactor.state.windows.window_mut(*wid)
                        && let Some(wsid) = w.info.sys_id
                    {
                        reactor.transaction_manager.set_last_sent_txid(wsid, txid);
                        txid_entries.push((wsid, txid, *frame));
                    }
                }
                reactor.transaction_manager.update_txid_entries(txid_entries);
            }

            let requests = if position_only {
                let mut positions = Vec::new();
                let mut full_frames = Vec::new();
                for (wid, frame, size_unchanged) in frames {
                    if size_unchanged {
                        positions.push((wid, frame.origin));
                    } else {
                        full_frames.push((wid, frame));
                    }
                }

                let mut requests = Vec::with_capacity(2);
                if !positions.is_empty() {
                    requests.push(Request::SetWorkspaceSwitchPositions(positions, txid, true));
                }
                if !full_frames.is_empty() {
                    requests.push(Request::SetBatchWindowFrame(full_frames, txid, true));
                }
                requests
            } else {
                vec![Request::SetBatchWindowFrame(
                    frames.into_iter().map(|(wid, frame, _)| (wid, frame)).collect(),
                    txid,
                    true,
                )]
            };
            for request in requests {
                if let Err(e) = handle.send(request) {
                    debug!(
                        ?pid,
                        ?e,
                        "Failed to send instant layout request - app may have quit"
                    );
                    break;
                }
            }
        }

        any_frame_changed
    }
}

impl ActiveAnimation {
    fn start(animation: Animation) -> Option<Self> {
        if animation.is_empty() {
            return None;
        }
        animation.begin();
        Some(Self { animation, next_frame: 1 })
    }

    fn replace_with(self, mut next: Animation) -> Self {
        let current = self.current_frames();
        let continuing = next.patch_starts_from(&current);
        next.begin_windows_not_in(&continuing);
        next.carry_over(self.animation, &current);
        Self { animation: next, next_frame: 1 }
    }

    fn send_next_frame(&mut self) {
        self.animation.send_frame(self.next_frame);
        self.next_frame += 1;
    }

    fn is_complete(&self) -> bool {
        self.next_frame > self.animation.frames
    }

    fn current_frames(&self) -> Vec<(WindowId, CGRect)> {
        let frame = self.next_frame.saturating_sub(1);
        self.animation
            .windows
            .iter()
            .map(|window| (window.wid, window.frame_after(frame, self.animation.frames)))
            .collect()
    }
}

impl Animation {
    pub fn new(config: Config) -> Self {
        //const FPS: f64 = 100.0;
        //const DURATION: f64 = 0.30;
        let interval = Duration::from_secs_f64(1.0 / config.settings.animation_fps);
        Self {
            interval,
            frames: (config.settings.animation_duration * config.settings.animation_fps).round()
                as u32,
            windows: vec![],
            handled_windows: vec![],
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
        self.windows.push(AnimatedWindow {
            handle: handle.clone(),
            wid,
            start,
            finish,
            is_focus,
            txid,
        });
        self.mark_handled(wid);
    }

    fn mark_handled(&mut self, wid: WindowId) {
        if !self.handled_windows.contains(&wid) {
            self.handled_windows.push(wid);
        }
    }

    pub fn skip_to_end(&self) {
        for window in &self.windows {
            _ = window.handle.send(Request::SetWindowFrame(
                window.wid,
                window.finish,
                window.txid,
                true,
            ));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    fn begin(&self) {
        self.begin_windows_not_in(&[]);
    }

    fn begin_windows_not_in(&self, skip: &[WindowId]) {
        for window in &self.windows {
            if skip.contains(&window.wid) {
                continue;
            }
            _ = window.handle.send(Request::BeginWindowAnimation(window.wid));
            if window.is_focus {
                let frame = CGRect {
                    origin: window.start.origin,
                    size: window.finish.size,
                };
                _ = window.handle.send(Request::AnimationFrame {
                    wid: window.wid,
                    frame,
                    set_size: true,
                    txid: window.txid,
                });
            }
        }
    }

    fn finish_all(&self) {
        for window in &self.windows {
            _ = window.handle.send(Request::AnimationFrame {
                wid: window.wid,
                frame: window.finish,
                set_size: true,
                txid: window.txid,
            });
            _ = window.handle.send(Request::EndWindowAnimation(window.wid));
        }
    }

    fn send_frame(&self, frame: u32) {
        let t = f64::from(frame) / f64::from(self.frames);
        for window in &self.windows {
            let mut rect = get_frame(window.start, window.finish, t);
            let set_size = frame * 2 == self.frames || frame == self.frames;
            if set_size {
                rect.size = window.finish.size;
            }
            _ = window.handle.send(Request::AnimationFrame {
                wid: window.wid,
                frame: rect,
                set_size,
                txid: window.txid,
            });
        }
    }

    fn end(&self) {
        for window in &self.windows {
            _ = window.handle.send(Request::EndWindowAnimation(window.wid));
        }
    }

    fn patch_starts_from(&mut self, current_frames: &[(WindowId, CGRect)]) -> Vec<WindowId> {
        let mut continuing = Vec::new();
        for &(wid, current_frame) in current_frames {
            let Some(window) = self.windows.iter_mut().find(|window| window.wid == wid) else {
                continue;
            };
            window.start = current_frame;
            continuing.push(wid);
        }
        continuing
    }

    fn carry_over(&mut self, previous: Animation, current_frames: &[(WindowId, CGRect)]) {
        for mut window in previous.windows {
            if self.handled_windows.contains(&window.wid) {
                continue;
            }
            if self.windows.iter().any(|existing| existing.wid == window.wid) {
                continue;
            }
            if let Some(&(_, current_frame)) =
                current_frames.iter().find(|(wid, _)| *wid == window.wid)
            {
                window.start = current_frame;
            }
            self.windows.push(window);
        }
    }

    fn skip_to_end_and_end(self) {
        self.finish_all();
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

fn ease(t: f64) -> f64 {
    if t < 0.5 {
        (1.0 - f64::sqrt(1.0 - f64::powi(2.0 * t, 2))) / 2.0
    } else {
        (f64::sqrt(1.0 - f64::powi(-2.0 * t + 2.0, 2)) + 1.0) / 2.0
    }
}

fn blend(a: f64, b: f64, s: f64) -> f64 {
    (1.0 - s) * a + s * b
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGSize};

    use super::*;

    fn rect(origin_x: f64, origin_y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(CGPoint::new(origin_x, origin_y), CGSize::new(width, height))
    }

    fn config() -> Config {
        Config::default()
    }

    fn animation(handle: &AppThreadHandle, wid: WindowId, from: CGRect, to: CGRect) -> Animation {
        let mut animation = Animation::new(config());
        animation.add_window(handle, wid, from, to, false, TransactionId::default());
        animation
    }

    fn collect_requests(rx: &mut crate::actor::Receiver<Request>) -> Vec<Request> {
        let mut requests = Vec::new();
        while let Ok((_, request)) = rx.try_recv() {
            requests.push(request);
        }
        requests
    }

    fn assert_set_window_frame(request: &Request, wid: WindowId, frame: CGRect) {
        match request {
            Request::SetWindowFrame(req_wid, req_frame, txid, eui) => {
                assert_eq!(*req_wid, wid);
                assert_eq!(*req_frame, frame);
                assert_eq!(*txid, TransactionId::default());
                assert!(*eui);
            }
            _ => panic!("expected SetWindowFrame, got {request:?}"),
        }
    }

    fn assert_animation_frame(request: &Request, wid: WindowId, frame: CGRect) {
        match request {
            Request::AnimationFrame {
                wid: req_wid,
                frame: req_frame,
                set_size,
                txid,
            } => {
                assert_eq!(*req_wid, wid);
                assert_eq!(*req_frame, frame);
                assert!(*set_size, "expected a set_size frame");
                assert_eq!(*txid, TransactionId::default());
            }
            _ => panic!("expected AnimationFrame, got {request:?}"),
        }
    }

    fn assert_animation_pos(request: &Request, wid: WindowId, pos: CGPoint) {
        match request {
            Request::AnimationFrame {
                wid: req_wid,
                frame,
                set_size,
                txid,
            } => {
                assert_eq!(*req_wid, wid);
                assert_eq!(frame.origin, pos);
                assert!(!*set_size, "expected a position-only frame");
                assert_eq!(*txid, TransactionId::default());
            }
            _ => panic!("expected AnimationFrame, got {request:?}"),
        }
    }

    #[test]
    fn replacement_uses_last_animated_frame_for_continuing_windows() {
        let (tx, mut rx) = crate::actor::channel();
        let handle = AppThreadHandle::new_for_test(tx);
        let wid = WindowId::new(1, 1);
        let first = animation(
            &handle,
            wid,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(50.0, 60.0, 10.0, 10.0),
        );
        let second = animation(
            &handle,
            wid,
            rect(50.0, 60.0, 10.0, 10.0),
            rect(80.0, 90.0, 10.0, 10.0),
        );

        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(first));
        assert!(matches!(
            collect_requests(&mut rx).as_slice(),
            [Request::BeginWindowAnimation(req_wid)] if *req_wid == wid
        ));

        manager.tick();
        let continuing_frame = manager.active.as_ref().unwrap().current_frames()[0].1;
        assert_animation_pos(&collect_requests(&mut rx)[0], wid, continuing_frame.origin);

        manager.handle_message(Message::Replace(second));
        assert!(collect_requests(&mut rx).is_empty());

        let resumed_start = manager.active.as_ref().unwrap().animation.windows[0].start;
        assert_eq!(resumed_start, continuing_frame);

        manager.tick();
        let expected_next = get_frame(resumed_start, rect(80.0, 90.0, 10.0, 10.0), 1.0 / 30.0);
        assert_animation_pos(&collect_requests(&mut rx)[0], wid, expected_next.origin);
    }

    fn animation_contains(manager: &AnimationManager, wid: WindowId) -> bool {
        manager
            .active
            .as_ref()
            .is_some_and(|active| active.animation.windows.iter().any(|w| w.wid == wid))
    }

    #[test]
    fn replacement_only_restarts_changed_windows() {
        let (tx, mut rx) = crate::actor::channel();
        let handle = AppThreadHandle::new_for_test(tx);
        let wid1 = WindowId::new(1, 1);
        let wid2 = WindowId::new(1, 2);
        let wid3 = WindowId::new(1, 3);
        let mut first = Animation::new(config());
        first.add_window(
            &handle,
            wid1,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(50.0, 60.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        first.add_window(
            &handle,
            wid2,
            rect(10.0, 0.0, 10.0, 10.0),
            rect(60.0, 60.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        let mut second = Animation::new(config());
        second.add_window(
            &handle,
            wid1,
            rect(50.0, 60.0, 10.0, 10.0),
            rect(80.0, 90.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        second.add_window(
            &handle,
            wid3,
            rect(20.0, 0.0, 10.0, 10.0),
            rect(90.0, 90.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );

        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(first));
        assert_eq!(collect_requests(&mut rx).len(), 2);
        manager.handle_message(Message::Replace(second));

        let requests = collect_requests(&mut rx);
        assert_eq!(requests.len(), 1);
        assert!(matches!(requests[0], Request::BeginWindowAnimation(req_wid) if req_wid == wid3));
        assert!(animation_contains(&manager, wid2));

        let carried = manager
            .active
            .as_ref()
            .unwrap()
            .animation
            .windows
            .iter()
            .find(|w| w.wid == wid2)
            .unwrap();
        assert_eq!(carried.finish, rect(60.0, 60.0, 10.0, 10.0));
    }

    #[test]
    fn replacement_does_not_carry_over_explicitly_handled_windows() {
        let (tx, mut rx) = crate::actor::channel();
        let handle = AppThreadHandle::new_for_test(tx);
        let wid1 = WindowId::new(1, 1);
        let wid2 = WindowId::new(1, 2);
        let mut first = Animation::new(config());
        first.add_window(
            &handle,
            wid1,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(50.0, 60.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        first.add_window(
            &handle,
            wid2,
            rect(10.0, 0.0, 10.0, 10.0),
            rect(60.0, 60.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        let mut second = Animation::new(config());
        second.add_window(
            &handle,
            wid1,
            rect(50.0, 60.0, 10.0, 10.0),
            rect(80.0, 90.0, 10.0, 10.0),
            false,
            TransactionId::default(),
        );
        second.mark_handled(wid2);

        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(first));
        let _ = collect_requests(&mut rx);
        manager.handle_message(Message::Replace(second));

        assert!(!animation_contains(&manager, wid2));
    }

    #[test]
    fn skip_to_end_finishes_active_animation_and_applies_new_layout() {
        let (tx, mut rx) = crate::actor::channel();
        let handle = AppThreadHandle::new_for_test(tx);
        let wid = WindowId::new(1, 1);
        let first = animation(
            &handle,
            wid,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(50.0, 60.0, 10.0, 10.0),
        );
        let second = animation(
            &handle,
            wid,
            rect(50.0, 60.0, 10.0, 10.0),
            rect(80.0, 90.0, 10.0, 10.0),
        );

        let mut manager = AnimationManager::new();
        manager.handle_message(Message::Replace(first));
        manager.handle_message(Message::SkipToEnd(second));

        let requests = collect_requests(&mut rx);
        assert_eq!(requests.len(), 4);
        assert!(matches!(requests[0], Request::BeginWindowAnimation(req_wid) if req_wid == wid));
        assert_animation_frame(&requests[1], wid, rect(50.0, 60.0, 10.0, 10.0));
        assert!(matches!(requests[2], Request::EndWindowAnimation(req_wid) if req_wid == wid));
        assert_set_window_frame(&requests[3], wid, rect(80.0, 90.0, 10.0, 10.0));
    }
}
