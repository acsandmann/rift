//! External window claims: a Mach session temporarily excludes windows from
//! Rift management. Ownership lives in [`WindowStore`](crate::model::WindowStore);
//! this module only applies the layout, presentation and focus effects.

use rift_protocol::WindowClaimFlags;

use super::{LayoutEvent, Reactor};
use crate::actor::app::WindowId;
use crate::common::collections::HashSet;
use crate::model::window_store::ClaimOwner;
use crate::sys::screen::SpaceId;

const OWNED_ELSEWHERE: &str = "Window owned by another external manager";

impl Reactor {
    pub(crate) fn claim_window(
        &mut self,
        wid: WindowId,
        owner: ClaimOwner,
        flags: WindowClaimFlags,
    ) -> Result<(), &'static str> {
        if !flags.is_supported() {
            return Err("Unsupported window claim flags");
        }
        let wid = self.state.windows.resolve_claim_target(owner, wid).ok_or("Window not found")?;
        match self.state.windows.claim(wid) {
            Some(claim) if claim.owner != owner => return Err(OWNED_ELSEWHERE),
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
            reactor.drag_manager.forget_window(wid);
            // Leave the projection (keeping floating state and the layout slot) before the
            // claim takes the assignment, but arrange only after: no write may target it.
            let space = reactor.assigned_space_for_window_id(wid);
            let removal = LayoutEvent::WindowSuspended(wid);
            let removed = reactor
                .layout_manager
                .layout_engine
                .handle_event(&mut reactor.state.windows, removal)
                .changed;
            reactor.state.windows.insert_claim(wid, owner, flags);
            if removed {
                reactor.update_layout_or_warn(false, false, space);
            }
            reactor.maybe_send_menu_update();
        });
        Ok(())
    }

    pub(crate) fn release_window(
        &mut self,
        wid: WindowId,
        owner: ClaimOwner,
    ) -> Result<(), &'static str> {
        let wid = self.state.windows.resolve_claim_target(owner, wid).ok_or("Window not found")?;
        match self.state.windows.claim(wid) {
            None => Ok(()),
            Some(claim) if claim.owner != owner => Err(OWNED_ELSEWHERE),
            Some(_) => {
                self.release_claims_of([wid]);
                Ok(())
            }
        }
    }

    /// The owner's session ended: return all of its windows, arranging each space once.
    pub(crate) fn release_claims(&mut self, owner: ClaimOwner) {
        let windows = self.state.windows.owner_claims(owner).to_vec();
        if !windows.is_empty() {
            self.release_claims_of(windows);
        }
    }

    fn release_claims_of(&mut self, windows: impl IntoIterator<Item = WindowId>) {
        self.with_claim_focus_policy(|reactor| {
            let spaces: HashSet<_> =
                windows.into_iter().filter_map(|wid| reactor.readmit(wid)).collect();
            for space in spaces {
                reactor.update_layout_or_warn(false, false, Some(space));
            }
            reactor.maybe_send_menu_update();
        });
    }

    /// Replays normal reconciliation for a released window; returns an active space to arrange.
    fn readmit(&mut self, wid: WindowId) -> Option<SpaceId> {
        let claim = self.state.windows.remove_claim(wid)?;
        let engine = &mut self.layout_manager.layout_engine;
        if let Some(workspace) =
            claim.workspace().filter(|id| engine.workspaces().workspaces.contains_key(*id))
        {
            self.state.windows.assign_window_to_workspace(wid, workspace);
            // Like any external move of a floating window, the owner's geometry is authoritative.
            if engine.is_window_floating(wid)
                && let Some(window) = self.state.windows.window(wid)
            {
                engine.store_floating_position(workspace, wid, window.frame_monotonic);
            }
        }
        if self.state.windows.native_fullscreen_record_for_window(wid).is_some() {
            // Suspended windows return through native fullscreen recovery.
            let outcome = self.restore_native_fullscreen(None, wid, None);
            self.apply_event_outcome(outcome);
            return None;
        }
        let space = self.best_space_for_window_id(wid)?;
        self.reassign_window_to_authoritative_space(wid, space, false);
        self.is_space_active(space).then_some(space)
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
