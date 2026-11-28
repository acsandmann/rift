//! Focus policy lives on the reactor. App actors hold revocable permission to
//! make native calls, and never wait for WindowServer to report their effects.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use objc2_core_foundation::CGPoint;
use tracing::trace;

use super::{Event, Reactor};
use crate::actor::app::{Quiet, Request, WindowId};
use crate::actor::window_notify;
use crate::actor::wm_controller::WmEvent;
use crate::sys::screen::SpaceId;

const QUEUED: u8 = 0;
const ISSUED: u8 = 1;
const REVOKED: u8 = 2;
const FINISHED: u8 = 3;
const FOCUS_LIFETIME: Duration = Duration::from_secs(1);
const STACK_BUDGET: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
pub struct FocusAuthority {
    pub(crate) generation: u64,
    deadline: Instant,
    state: Arc<AtomicU8>,
}

impl FocusAuthority {
    fn new(generation: u64, deadline: Instant) -> Self {
        Self {
            generation,
            deadline,
            state: Arc::new(AtomicU8::new(QUEUED)),
        }
    }

    pub(crate) fn is_current(&self) -> bool {
        self.state.load(Ordering::Acquire) < REVOKED && Instant::now() < self.deadline
    }

    // Claim the first native write against revocation. A claimed synchronous
    // call can still finish late. Every subsequent write rechecks authority.
    pub(crate) fn try_issue(&self) -> bool {
        Instant::now() < self.deadline
            && self
                .state
                .try_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                    (state < REVOKED).then_some(ISSUED)
                })
                .is_ok()
    }

    pub(crate) fn focus_age(&self) -> Duration { (self.deadline - FOCUS_LIFETIME).elapsed() }

    pub(crate) fn allows_warp(&self) -> bool {
        self.state.load(Ordering::Acquire) != REVOKED && Instant::now() < self.deadline
    }

    fn finish(&self) { self.state.store(FINISHED, Ordering::Release); }

    fn issued(&self) -> bool { self.state.load(Ordering::Acquire) == ISSUED }

    fn revoke(&self) -> bool { self.state.swap(REVOKED, Ordering::AcqRel) == ISSUED }
}

#[derive(Clone, Copy, Debug)]
pub enum FocusExecution {
    Issued,
    AlreadyFocused,
    AxFallback(Option<WindowId>),
    Cancelled,
    Failed,
    Missing,
}

#[derive(Clone, Debug, Default)]
pub(super) struct FocusPlan {
    pub raise_windows: Vec<Vec<WindowId>>,
    pub focus_window: Option<(WindowId, Option<CGPoint>)>,
    pub focus_quiet: Quiet,
    pub pointer: bool,
}

pub(super) struct FocusIntent {
    pub window: WindowId,
    pub pointer: bool,
    quiet: Quiet,
    authority: FocusAuthority,
    sample_after: Instant,
    observed: Option<(Instant, Option<WindowId>)>,
    execution: Option<FocusExecution>,
    awaiting_activation: bool,
    pub(super) space: Option<SpaceId>,
    corrected: bool,
    force_raise: bool,
    warp: Option<CGPoint>,
    pub overview: Option<Instant>,
}

struct Stacking {
    authority: FocusAuthority,
    batches: std::vec::IntoIter<Vec<WindowId>>,
    current: Vec<WindowId>,
}

#[derive(Default)]
pub(super) struct FocusState {
    latest: Option<FocusIntent>,
    generation: u64,
    stacking: Option<Stacking>,
    warp: Option<FocusAuthority>,
    // Only issued operations can have late native effects. Keep a bounded
    // history while another intent is pending. Native events have no request ID.
    superseded: VecDeque<(WindowId, Instant)>,
}

impl FocusState {
    fn remember(&mut self, window: WindowId, deadline: Instant) {
        if self.superseded.len() == 8 {
            self.superseded.pop_front();
        }
        self.superseded.push_back((window, deadline));
    }

    fn cancel(&mut self) {
        if let Some(intent) = self.latest.take()
            && intent.authority.revoke()
        {
            self.remember(intent.window, intent.authority.deadline);
        }
        if let Some(warp) = self.warp.take() {
            warp.revoke();
        }
        if let Some(stack) = self.stacking.take()
            && stack.authority.revoke()
        {
            for window in stack.current {
                self.remember(window, stack.authority.deadline);
            }
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.latest
            .iter()
            .map(|intent| intent.authority.deadline)
            .chain(self.stacking.iter().map(|stack| stack.authority.deadline))
            .min()
    }

    pub fn covers(&self, space: SpaceId) -> bool {
        self.latest
            .as_ref()
            .is_some_and(|intent| intent.overview.is_some() && intent.space == Some(space))
    }

    pub fn pending(&self) -> Option<&FocusIntent> { self.latest.as_ref() }

    pub fn warp_authority(&mut self) -> Option<FocusAuthority> {
        let authority = self.latest.as_ref()?.authority.clone();
        self.warp = Some(authority.clone());
        Some(authority)
    }
}

impl Reactor {
    pub(super) fn submit_focus(
        &mut self,
        mut plan: FocusPlan,
        overview: Option<(SpaceId, Instant)>,
    ) {
        #[cfg(test)]
        if let Some(tx) = &self.focus_plans {
            let _ = tx.send(plan.clone());
        }

        if plan
            .focus_window
            .is_some_and(|(window, _)| !self.app_manager.apps.contains_key(&window.pid))
        {
            return;
        }
        if plan.focus_window.is_none() && self.focus.pending().is_some() {
            return;
        }
        self.cancel_focus();
        self.focus.generation += 1;
        let generation = self.focus.generation;
        let now = Instant::now();
        if let Some((window, warp)) = plan.focus_window {
            let force_raise = plan
                .raise_windows
                .iter()
                .any(|batch| batch.contains(&window) && batch.len() > 1);
            for batch in &mut plan.raise_windows {
                batch.retain(|wid| *wid != window);
            }
            self.focus.latest = Some(FocusIntent {
                window,
                pointer: plan.pointer,
                quiet: overview.map_or(plan.focus_quiet, |_| Quiet::Yes),
                authority: FocusAuthority::new(generation, now + FOCUS_LIFETIME),
                sample_after: now,
                observed: None,
                execution: None,
                awaiting_activation: !self.main_window_tracker.is_globally_frontmost(window.pid),
                space: overview
                    .map(|(space, _)| space)
                    .or_else(|| self.assigned_space_for_window_id(window))
                    .or_else(|| self.state.windows.native_space(window))
                    .or_else(|| {
                        self.state
                            .windows
                            .window(window)
                            .and_then(|state| self.best_space_for_frame(&state.frame_monotonic))
                    }),
                corrected: false,
                force_raise,
                warp,
                overview: overview.map(|(_, stamp)| stamp),
            });
        }

        if plan.raise_windows.iter().any(|batch| !batch.is_empty()) {
            self.focus.stacking = Some(Stacking {
                authority: FocusAuthority::new(generation, now + STACK_BUDGET),
                batches: plan.raise_windows.into_iter(),
                current: Vec::new(),
            });
            self.advance_stacking(generation, false);
        } else {
            self.dispatch_focus();
        }
    }

    pub(super) fn advance_stacking(&mut self, generation: u64, expired: bool) {
        let Some(stack) = &mut self.focus.stacking else { return };
        if stack.authority.generation != generation {
            return;
        }
        // Retain possible late AX z-order effects from the previous batch.
        if stack.authority.issued() {
            let deadline = stack.authority.deadline + FOCUS_LIFETIME;
            for window in std::mem::take(&mut stack.current) {
                self.focus.remember(window, deadline);
            }
        }
        let stack = self.focus.stacking.as_mut().unwrap();
        if !expired && stack.authority.is_current() {
            for batch in stack.batches.by_ref() {
                let Some(window) = batch.first() else { continue };
                if let Some(app) = self.app_manager.apps.get(&window.pid)
                    && app
                        .handle
                        .send(Request::Stack(batch.clone(), stack.authority.clone()))
                        .is_ok()
                {
                    stack.current = batch;
                    return;
                }
            }
        }
        self.focus.stacking.take().unwrap().authority.revoke();
        self.dispatch_focus();
    }

    fn dispatch_focus(&mut self) {
        let Some(intent) = &mut self.focus.latest else { return };
        let window = intent.window;
        if !intent.authority.is_current()
            || self.app_manager.apps.get(&window.pid).is_none_or(|app| {
                app.handle
                    .send(Request::Focus(
                        window,
                        intent.authority.clone(),
                        !intent.force_raise
                            && !intent.awaiting_activation
                            && self
                                .focus
                                .superseded
                                .iter()
                                .all(|(_, until)| *until <= Instant::now())
                            && self.main_window_tracker.native_focus_at(window, intent.space),
                    ))
                    .is_err()
            })
        {
            self.cancel_focus();
        } else if let Some(point) = intent.warp.take()
            && let Some(input) = &self.communication_manager.input_tx
        {
            let authority = intent.authority.clone();
            self.focus.warp = Some(authority.clone());
            _ = input.send(crate::actor::input::Request::Warp(point, Some(authority)));
        }
    }

    pub(super) fn cancel_focus(&mut self) {
        if let Some(stamp) = self.focus.latest.as_ref().and_then(|intent| intent.overview) {
            self.overview_focus_outcome(stamp, false);
        }
        self.focus.cancel();
    }

    pub(super) fn focus_input(&mut self) {
        self.cancel_focus();
        self.focus.superseded.clear();
    }

    pub(super) fn expire_focus(&mut self, now: Instant) {
        if self
            .focus
            .latest
            .as_ref()
            .is_some_and(|intent| now >= intent.authority.deadline)
        {
            self.cancel_focus();
        }
        if let Some(stack) = &self.focus.stacking
            && now >= stack.authority.deadline
        {
            self.advance_stacking(stack.authority.generation, true);
        }
    }

    pub(super) fn overview_focus_outcome(&self, stamp: Instant, focused: bool) {
        if let Some(wm) = &self.communication_manager.wm_sender {
            wm.send(WmEvent::OverviewFocusOutcome(stamp, focused));
        }
    }

    pub(super) fn observe_focus(
        &mut self,
        window: WindowId,
        space: SpaceId,
        sampled_at: Option<Instant>,
    ) -> Option<Quiet> {
        let Some(intent) = &self.focus.latest else {
            let intermediate = self
                .focus
                .stacking
                .as_ref()
                .is_some_and(|stack| stack.current.contains(&window));
            return (!intermediate).then_some(Quiet::No);
        };
        let matching = window == intent.window
            && intent.space.is_none_or(|dest| dest == space)
            && sampled_at.is_none_or(|at| at >= intent.sample_after);
        let selection = matching.then_some(intent.quiet);
        if self.focus.stacking.is_none() {
            let at = sampled_at.unwrap_or_else(Instant::now);
            if intent.space.is_none_or(|dest| dest == space) {
                self.record_focus_sample(Some(window), at);
            } else if window != intent.window
                && intent
                    .observed
                    .is_some_and(|(last, wid)| at >= last && wid == Some(intent.window))
            {
                // A different Space cannot identify the destination key
                // window, but can invalidate an older matching sample.
                self.focus.latest.as_mut().unwrap().observed = Some((at, None));
                self.reconcile_focus(true);
            }
        }
        selection.map(|quiet| {
            if self.focus.pending().is_some() {
                Quiet::Yes
            } else {
                quiet
            }
        })
    }

    fn record_focus_sample(&mut self, window: Option<WindowId>, at: Instant) {
        let Some(intent) = &mut self.focus.latest else { return };
        if at >= intent.sample_after && intent.observed.is_none_or(|(last, _)| at >= last) {
            intent.observed = Some((at, window));
            self.reconcile_focus(false);
        }
    }

    pub(super) fn accept_resolved_activation(
        &mut self,
        pid: i32,
        window: Option<WindowId>,
    ) -> bool {
        let Some(intent) = &mut self.focus.latest else {
            return true;
        };
        if intent.window.pid == pid && intent.awaiting_activation {
            intent.awaiting_activation = false;
            // The app can restore its old main window after an early native
            // sample. Recheck the destination if resolved activation disagrees.
            if window != Some(intent.window)
                && intent.observed.is_some_and(|(_, observed)| observed == Some(intent.window))
            {
                intent.sample_after = Instant::now();
                intent.observed = None;
            }
            if matches!(intent.execution, Some(FocusExecution::AxFallback(_))) {
                intent.execution = Some(FocusExecution::AxFallback(window));
            }
            self.reconcile_focus(true);
        }
        false
    }

    pub(super) fn focus_app_deactivated(&mut self, pid: i32) {
        if let Some(intent) = &mut self.focus.latest
            && intent.window.pid == pid
        {
            intent.awaiting_activation = true;
            intent.sample_after = Instant::now();
            intent.observed = None;
        }
    }

    pub(super) fn observe_ax_focus(&mut self, pid: i32, window: Option<WindowId>) {
        if let Some(intent) = &mut self.focus.latest
            && intent.window.pid == pid
            && self.main_window_tracker.is_globally_frontmost(pid)
            && matches!(intent.execution, Some(FocusExecution::AxFallback(_)))
        {
            intent.execution = Some(FocusExecution::AxFallback(window));
            self.reconcile_focus(false);
        }
    }

    pub(super) fn focus_verified(
        &mut self,
        generation: u64,
        space: SpaceId,
        window: Option<WindowId>,
        at: Instant,
    ) {
        let Some(intent) = &self.focus.latest else { return };
        if intent.authority.generation != generation
            || intent.space != Some(space)
            || at < intent.sample_after
            || intent.observed.is_some_and(|(last, _)| at < last)
        {
            return;
        }
        if let Some(window) = window {
            let _ = self.main_window_tracker.handle_event(&Event::WindowServerFocusChanged(
                window,
                space,
                Some(at),
            ));
        }
        self.record_focus_sample(window, at);
    }

    pub(super) fn focus_result(
        &mut self,
        window: WindowId,
        generation: u64,
        result: FocusExecution,
    ) {
        let Some(intent) = &mut self.focus.latest else { return };
        if intent.window != window || intent.authority.generation != generation {
            return;
        }
        match result {
            FocusExecution::Failed | FocusExecution::Cancelled | FocusExecution::Missing => {
                self.cancel_focus();
                if matches!(result, FocusExecution::Missing) {
                    self.request_window_inventory(window.pid);
                }
            }
            _ => {
                intent.execution = Some(result);
                self.reconcile_focus(true);
            }
        }
    }

    fn reconcile_focus(&mut self, verify: bool) {
        let Some(intent) = &mut self.focus.latest else { return };
        let Some(execution) = intent.execution else { return };
        if intent.awaiting_activation {
            return;
        }
        let observed = match execution {
            FocusExecution::AxFallback(window) => window,
            _ => intent.observed.and_then(|(_, window)| window),
        };
        if observed == Some(intent.window) {
            let intent = self.focus.latest.take().unwrap();
            trace!(window = ?intent.window, age = ?intent.authority.focus_age(), "Native focus converged");
            intent.authority.finish();
            if let Some(stamp) = intent.overview {
                self.overview_focus_outcome(stamp, true);
            }
            return;
        }
        if verify
            && !matches!(execution, FocusExecution::AxFallback(_))
            && let Some(space) = intent.space
            && let Some(tx) = &self.notification_manager.window_notify_tx
            && tx
                .try_send(window_notify::Request::VerifyFocus(
                    space,
                    intent.authority.clone(),
                ))
                .is_ok()
        {
            return;
        }
        if !intent.corrected
            && (matches!(execution, FocusExecution::AlreadyFocused) && intent.observed.is_some()
                || observed.is_some_and(|window| {
                    window.pid == intent.window.pid
                        || self
                            .focus
                            .superseded
                            .iter()
                            .any(|(old, until)| *old == window && Instant::now() < *until)
                }))
        {
            trace!(target = ?intent.window, ?observed, "Correcting native focus underneath the current presentation");
            self.focus.generation += 1;
            intent.authority.revoke();
            intent.authority =
                FocusAuthority::new(self.focus.generation, intent.authority.deadline);
            intent.corrected = true;
            intent.force_raise = true;
            intent.sample_after = Instant::now();
            intent.observed = None;
            intent.execution = None;
            self.dispatch_focus();
        }
    }
}
