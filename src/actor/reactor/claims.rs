//! Window claims: an external manager's Mach session, or the held left button,
//! temporarily excludes windows from Rift management. Ownership lives in
//! [`WindowStore`](crate::model::WindowStore); this module only applies the layout,
//! presentation and focus effects.

use rift_protocol::WindowClaimFlags;

use super::{Changes, LayoutEvent, Reactor};
use crate::actor::app::WindowId;
use crate::layout_engine::systems::{LayoutSystem, WindowChange};
use crate::model::window_store::{ClaimHolder, ClaimOwner};

const OWNED_ELSEWHERE: &str = "Window owned by another external manager";

impl Reactor {
    /// A session (by its Mach client port) claims a window.
    pub(crate) fn claim_window(
        &mut self,
        wid: WindowId,
        port: u32,
        flags: WindowClaimFlags,
    ) -> Result<(), &'static str> {
        if !flags.is_supported() {
            return Err("Unsupported window claim flags");
        }
        let wid = self.state.windows.resolve_claim_target(port, wid).ok_or("Window not found")?;
        match self.state.windows.claim(wid) {
            Some(claim) if claim.owner() != ClaimOwner::Ipc(port) => return Err(OWNED_ELSEWHERE),
            // Flags only affect focus policy, never layout.
            Some(_) => {
                self.with_claim_focus_policy(|r| r.state.windows.set_claim_flags(wid, flags));
                return Ok(());
            }
            None => {}
        }
        self.with_claim_focus_policy(|reactor| {
            // The owner takes over like a user drag: stop Rift's motion and pending writes.
            reactor.cancel_window_presentations(vec![wid]);
            if let Some(wsid) = reactor.state.windows.window(wid).and_then(|w| w.info.sys_id) {
                reactor.transaction_manager.clear_target_for_window(wsid);
            }
            reactor.state.windows.cancel_transfer(wid);
            // Leave the projection (keeping floating state and the layout slot) before the
            // claim takes the assignment, but arrange only after: no write may target it.
            let mut changes = Changes::default();
            reactor.reconcile_layout_event(&mut changes, LayoutEvent::WindowSuspended(wid));
            let holder = ClaimHolder::Ipc { port, flags, original: wid };
            reactor.state.windows.insert_claim(wid, holder);
            changes.refresh_menu = true;
            reactor.finish(changes, None, None);
        });
        Ok(())
    }

    pub(crate) fn release_window(&mut self, wid: WindowId, port: u32) -> Result<(), &'static str> {
        let wid = self.state.windows.resolve_claim_target(port, wid).ok_or("Window not found")?;
        match self.state.windows.claim(wid) {
            None => Ok(()),
            Some(claim) if claim.owner() != ClaimOwner::Ipc(port) => Err(OWNED_ELSEWHERE),
            Some(_) => {
                self.finish_claim_release(|reactor, changes| {
                    reactor.release_claims_of(changes, [wid]);
                });
                Ok(())
            }
        }
    }

    /// The session ended: return all of its windows, arranging each space once.
    pub(crate) fn release_claims(&mut self, port: u32) {
        self.finish_claim_release(|reactor, changes| {
            reactor.release_owner_claims(changes, ClaimOwner::Ipc(port));
        });
    }

    /// Return every window `owner` holds: a session's at its end, the press's at the release.
    pub(super) fn release_owner_claims(&mut self, changes: &mut Changes, owner: ClaimOwner) {
        let windows = self.state.windows.owner_claims(owner).to_vec();
        self.release_claims_of(changes, windows);
    }

    /// The left button was released. Windows AX has reported return now. Nothing can admit the
    /// others until AX reports them: the dragged one waits with its drop in a landing claim
    /// (which `commit` then no longer lands now), and the rest just lose their claim.
    pub(super) fn release_press(
        &mut self,
        changes: &mut Changes,
        mut commit: Option<crate::actor::drag::DragCommit>,
    ) -> Option<crate::actor::drag::DragCommit> {
        let mut reported = Vec::new();
        for window in self.state.windows.owner_claims(ClaimOwner::Press).to_vec() {
            if self.state.windows.contains_window(window) {
                reported.push(window);
            } else if let Some(drop) = commit.take_if(|commit| commit.source.window == window) {
                self.state.windows.hand_over_claim(window, ClaimHolder::Landing(drop));
            } else {
                self.state.windows.remove_claim(window);
            }
        }
        self.release_claims_of(changes, reported);
        commit
    }

    /// AX reported a window waiting to land: it joins the layout and lands on its drop. If
    /// the claim kept a destination (an identity merge, a restored layout), the window returns
    /// there first and the drop then moves it: the drop wins.
    pub(super) fn land_arrival(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
        drop: crate::actor::drag::DragCommit,
    ) {
        self.release_claims_of(changes, [wid]);
        self.land_drag(changes, drop);
    }

    fn release_claims_of(
        &mut self,
        changes: &mut Changes,
        windows: impl IntoIterator<Item = WindowId>,
    ) {
        self.with_claim_focus_policy(|reactor| {
            for window in windows {
                changes.refresh_menu = true;
                reactor.readmit(changes, window);
            }
        });
    }

    /// A release outside event dispatch reconciles on its own.
    fn finish_claim_release(&mut self, release: impl FnOnce(&mut Self, &mut Changes)) {
        let mut changes = Changes::default();
        release(self, &mut changes);
        self.finish(changes, None, None);
    }

    /// Replays normal reconciliation for a released window.
    fn readmit(&mut self, changes: &mut Changes, wid: WindowId) {
        let Some(claim) = self.state.windows.remove_claim(wid) else {
            return;
        };
        let engine = &mut self.layout_manager.layout_engine;
        if let Some(workspace) =
            claim.workspace().filter(|id| engine.workspaces().workspaces.contains_key(*id))
        {
            self.state.windows.assign_window_to_workspace(wid, workspace);
            // The owner may have changed the claim's destination. Reconcile only
            // at release; an ordinary return keeps its canonical slot untouched.
            for (id, current) in engine.workspaces_mut().workspaces.iter_mut() {
                if id != workspace {
                    current.layout_system.update(WindowChange::Remove(wid));
                }
            }
            // Like any external move of a floating window, the owner's geometry is authoritative.
            if engine.is_window_floating(wid)
                && let Some(window) = self.state.windows.window(wid)
            {
                engine.store_floating_position(workspace, wid, window.frame_monotonic);
            }
        }
        if self.state.windows.native_fullscreen_record_for_window(wid).is_some() {
            // Suspended windows return through native fullscreen recovery.
            self.restore_native_fullscreen(changes, None, wid, None);
            return;
        }
        if let Some(space) = self.best_space_for_window_id(wid) {
            self.reassign_window_to_authoritative_space(changes, wid, space, false);
        }
    }

    /// Recompute focus-follows-mouse only when the focused window's claim policy changed.
    fn with_claim_focus_policy(&mut self, change: impl FnOnce(&mut Self)) {
        let suppressed = self.state.windows.suppresses_focus_follows_mouse(self.main_window());
        change(self);
        if suppressed != self.state.windows.suppresses_focus_follows_mouse(self.main_window()) {
            self.update_focus_follows_mouse_state();
        }
    }
}
