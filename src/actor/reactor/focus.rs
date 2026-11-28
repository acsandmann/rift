//! Focus policy lives on the reactor. App actors hold revocable permission to
//! make native calls, and never wait for WindowServer to report their effects.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use objc2_core_foundation::CGPoint;
use tracing::trace;

use super::{Changes, Presentation, Reactor};
use crate::actor::app::{Quiet, Request, WindowId};
use crate::layout_engine::LayoutEvent;

const QUEUED: u8 = 0;
const ISSUED: u8 = 1;
const REVOKED: u8 = 2;
const CONFIRMED: u8 = 3;
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

    fn issued(&self) -> bool { self.state.load(Ordering::Acquire) == ISSUED }

    fn revoke(&self) -> bool { self.state.swap(REVOKED, Ordering::AcqRel) == ISSUED }
}

#[derive(Clone, Debug, Default)]
pub(super) struct FocusPlan {
    pub raise_windows: Vec<Vec<WindowId>>,
    pub focus_window: Option<(WindowId, Option<CGPoint>)>,
    pub focus_quiet: Quiet,
    pub pointer: bool,
    pub overview: Option<Instant>,
}

pub(super) struct FocusIntent {
    pub window: WindowId,
    pub pointer: bool,
    quiet: Quiet,
    authority: FocusAuthority,
    created: Instant,
    outgoing: Option<WindowId>,
    corrected: bool,
    force_raise: bool,
    warp: Option<CGPoint>,
    overview: Option<Instant>,
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

    pub fn cancel(&mut self) {
        if let Some(intent) = self.latest.take()
            && intent.authority.revoke()
        {
            self.remember(intent.window, intent.authority.deadline);
        }
        if let Some(stack) = self.stacking.take()
            && stack.authority.revoke()
        {
            for window in stack.current {
                self.remember(window, stack.authority.deadline);
            }
        }
    }

    pub fn external_input(&mut self) {
        self.cancel();
        self.superseded.clear();
    }

    pub fn deadline(&self) -> Option<(u64, Instant)> {
        let stack = self.stacking.as_ref()?;
        Some((stack.authority.generation, stack.authority.deadline))
    }

    pub fn expire(&mut self) {
        if self.latest.as_ref().is_some_and(|intent| !intent.authority.allows_warp()) {
            self.cancel();
        }
    }

    pub fn shadows_activation(
        &self,
        pid: i32,
        native: Option<WindowId>,
        ax: Option<WindowId>,
    ) -> bool {
        self.pending().is_some()
            || self.latest.as_ref().is_some_and(|intent| {
                intent.window.pid == pid
                    && (native == Some(intent.window) || ax == Some(intent.window))
                    && intent.authority.allows_warp()
            })
    }

    pub fn pending(&self) -> Option<&FocusIntent> {
        self.latest.as_ref().filter(|intent| intent.authority.is_current())
    }

    pub fn warp_authority(&self) -> Option<FocusAuthority> {
        self.latest.as_ref().map(|intent| intent.authority.clone())
    }
}

impl Reactor {
    pub(super) fn submit_focus(&mut self, mut plan: FocusPlan) {
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
        self.focus.cancel();
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
                quiet: plan.focus_quiet,
                authority: FocusAuthority::new(generation, now + FOCUS_LIFETIME),
                created: now,
                outgoing: self
                    .main_window_tracker
                    .window_server_focus()
                    .or_else(|| self.main_window()),
                corrected: false,
                force_raise,
                warp,
                overview: plan.overview,
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
                        intent.force_raise
                            || self
                                .main_window_tracker
                                .window_server_focus()
                                .is_some_and(|focused| focused != window),
                    ))
                    .is_err()
            })
        {
            self.focus.cancel();
        } else if let Some(point) = intent.warp.take()
            && let Some(input) = &self.communication_manager.input_tx
        {
            _ = input.send(crate::actor::input::Request::Warp(
                point,
                Some(intent.authority.clone()),
            ));
        }
    }

    // Quiet confirmations update the command display without replaying layout
    // selection. An ignored observation still updates the native metadata.
    pub(super) fn observe_focus(
        &mut self,
        changes: &mut Changes,
        window: WindowId,
        sampled_at: Option<Instant>,
    ) -> Option<Quiet> {
        self.focus.expire();
        let now = Instant::now();
        let obsolete = self
            .focus
            .superseded
            .iter()
            .any(|(old, deadline)| *old == window && now < *deadline);
        let Some(intent) = self.focus.pending() else {
            if let Some(stack) = &self.focus.stacking
                && stack.current.contains(&window)
            {
                return None;
            }
            if let Some(confirmed) = &self.focus.latest
                && confirmed.window != window
            {
                if sampled_at.is_some_and(|sampled| sampled < confirmed.created) {
                    return None;
                }
                if obsolete && confirmed.authority.allows_warp() && !confirmed.corrected {
                    self.correct_focus();
                    return None;
                }
                self.focus.external_input();
            }
            return Some(Quiet::No);
        };
        if sampled_at.is_some_and(|sampled| sampled < intent.created) {
            return None;
        }
        if let Some(stack) = &self.focus.stacking
            && (window == intent.window || stack.current.contains(&window))
        {
            return None;
        }
        if window == intent.window {
            if !intent.authority.issued() && intent.overview.is_none() {
                // An observation can satisfy a queued intent, but it cannot be
                // attributed to an app actor that has issued no mutation.
                self.focus.cancel();
                return Some(Quiet::No);
            }
            let quiet = intent.quiet;
            trace!(?window, age = ?intent.authority.focus_age(), "Native focus confirmed");
            self.confirm_focus(changes, window);
            return Some(quiet);
        }

        let restored = intent.authority.issued() && window.pid == intent.window.pid;
        if obsolete || restored || Some(window) == intent.outgoing {
            if (restored || obsolete) && intent.authority.issued() && !intent.corrected {
                self.correct_focus();
            }
            return None;
        }

        // A different target without evidence of an outgoing mutation is an
        // external focus change. Input edges also revoke before observation.
        self.focus.external_input();
        Some(Quiet::No)
    }

    pub(super) fn accept_resolved_activation(
        &mut self,
        changes: &mut Changes,
        pid: i32,
        window: Option<WindowId>,
    ) -> bool {
        let native = self.main_window_tracker.window_server_focus();
        let Some(intent) = self.focus.pending() else {
            return !self.focus.shadows_activation(pid, native, window);
        };
        if intent.window.pid == pid {
            if native.is_none() && window == Some(intent.window) && self.focus.stacking.is_none() {
                if intent.authority.issued() {
                    self.confirm_focus(changes, intent.window);
                    return false;
                }
                self.focus.cancel();
                return true;
            }
            return false;
        }

        // Carbon + resolved AX can still supply an external activation when 808/815
        // cannot resolve a usable window. An outgoing app or an issued obsolete
        // mutation can also produce this edge, so wait for native evidence there.
        if intent.outgoing.is_some_and(|outgoing| outgoing.pid == pid)
            || self
                .focus
                .stacking
                .as_ref()
                .is_some_and(|stack| stack.current.iter().any(|window| window.pid == pid))
            || self
                .focus
                .superseded
                .iter()
                .any(|(old, expires)| old.pid == pid && Instant::now() < *expires)
        {
            return false;
        }
        self.focus.external_input();
        true
    }

    fn correct_focus(&mut self) {
        let intent = self.focus.latest.as_mut().unwrap();
        self.focus.generation += 1;
        intent.authority.revoke();
        intent.authority = FocusAuthority::new(self.focus.generation, intent.authority.deadline);
        intent.corrected = true;
        intent.force_raise = true;
        self.dispatch_focus();
    }

    pub(super) fn focus_result(
        &mut self,
        changes: &mut Changes,
        window: WindowId,
        generation: u64,
        failed: Option<bool>,
    ) {
        if !self.focus.pending().is_some_and(|intent| {
            intent.window == window && intent.authority.generation == generation
        }) {
            return;
        }
        if let Some(missing) = failed {
            self.focus.cancel();
            if missing {
                self.request_window_inventory(window.pid);
            }
        } else {
            self.confirm_focus(changes, window);
        }
    }

    fn confirm_focus(&mut self, changes: &mut Changes, window: WindowId) {
        let intent = self.focus.latest.as_ref().unwrap();
        intent.authority.state.store(CONFIRMED, Ordering::Release);
        let overview = intent.overview;
        if let Some((space, selected, stamp)) = self.overview_focus
            && window == selected
            && overview == Some(stamp)
        {
            self.overview_focus = None;
            self.stop_camera(space);
            self.reconcile_layout_event(changes, LayoutEvent::WindowFocused(space, selected));
            changes.arrange(Some(space), Presentation::Instant);
        }
    }
}
