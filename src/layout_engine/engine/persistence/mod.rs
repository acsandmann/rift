use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use objc2_core_foundation::{CGRect, CGSize};
pub use rift_protocol::{RestoreScope, RestoreSource};
use serde::{Deserialize, Serialize};

use super::{FloatingManager, LayoutEngine};
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{BTreeMap, BTreeSet, HashMap};
use crate::common::config::{LayoutSettings, VirtualWorkspaceSettings, WorkspaceMode};
use crate::layout_engine::LayoutSystem;
use crate::model::broadcast::BroadcastSender;
use crate::model::reactor::WindowState;
use crate::model::{
    AppRuleEngine, FloatingPositionStore, VirtualWorkspaceId, WindowStore, WorkspaceStore,
};
use crate::sys::screen::SpaceId;

static SAVE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreRequest {
    pub scope: RestoreScope,
    pub active_space: SpaceId,
    pub source: RestoreSource,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub matched: usize,
    pub unmatched: usize,
    pub workspaces_replaced: usize,
    /// Windows the restore assigned to another native Space: `(window, from, to)`.
    pub moved: Vec<(WindowId, SpaceId, SpaceId)>,
}

impl RestoreReport {
    pub fn summary(&self) -> String {
        let mut summary = format!(
            "Restored {} workspace(s); matched {} window(s)",
            self.workspaces_replaced, self.matched
        );
        if self.unmatched > 0 {
            summary.push_str(&format!(
                "; ignored {} saved window(s) that are not currently available",
                self.unmatched
            ));
        }
        summary
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WindowFingerprint {
    #[serde(default)]
    window_server_id: Option<u32>,
    title: Option<String>,
    width: f64,
    height: f64,
    app_id: Option<String>,
}

impl WindowFingerprint {
    /// `WindowInfo` has no bundle id when its server lookup missed, but the process still names it.
    fn of(id: WindowId, window: &WindowState) -> Self {
        Self {
            window_server_id: window.info.sys_id.map(|id| id.as_u32()),
            title: Some(window.info.title.clone()).filter(|title| !title.trim().is_empty()),
            width: window.presented_frame.size.width,
            height: window.presented_frame.size.height,
            app_id: (window.info.bundle_id.clone())
                .or_else(|| crate::sys::app::bundle_info_for_pid(id.pid).0),
        }
    }

    /// A fallback must never cross two known bundle identities: titles such as "Untitled" and
    /// common default sizes are not globally unique across applications.
    fn same_server_window(&self, live: &Self) -> bool {
        self.window_server_id.is_some()
            && self.window_server_id == live.window_server_id
            && self.app_id.as_ref().zip(live.app_id.as_ref()).is_none_or(|(a, b)| a == b)
            && (self.same_known_app(live) || self.same_title_and_size(live))
    }

    /// WindowId is process-local, so it is direct evidence only while stronger saved identity does
    /// not contradict it.
    fn same_process_window(&self, live: &Self) -> bool {
        if self.window_server_id.is_some() && live.window_server_id.is_some() {
            self.same_server_window(live)
        } else {
            self.same_known_app(live) && self.same_title_and_size(live)
        }
    }

    fn same_known_app(&self, live: &Self) -> bool {
        self.app_id.is_some() && self.app_id == live.app_id
    }

    fn same_title(&self, live: &Self) -> bool { self.title.is_some() && self.title == live.title }

    fn size_delta(&self, live: &Self) -> f64 {
        (self.width - live.width).abs() + (self.height - live.height).abs()
    }

    fn same_title_and_size(&self, live: &Self) -> bool {
        self.same_title(live) && self.size_delta(live) <= 8.0
    }
}

/// A saved window waiting for its live identity. `workspace` is the one location resolved when the
/// snapshot was read; a tiled window also holds a tree slot there, while a floating window (`frame`
/// is set) exists only here until it matches.
#[derive(Debug)]
struct SavedWindow {
    fingerprint: WindowFingerprint,
    workspace: VirtualWorkspaceId,
    frame: Option<CGRect>,
}

/// Restoration in progress. Live windows are described by `WindowStore` and have no entry here, so
/// once matching finishes this state is empty and every window path skips it in O(1).
#[derive(Debug, Default)]
pub(super) struct PersistenceState {
    pending: HashMap<WindowId, SavedWindow>,
    /// True until a startup restore has seen a complete display topology.
    awaiting_topology: bool,
    /// Application discoveries finished before that, in `discard_unmatched_candidates_for_app`'s
    /// terms, to replay once saved destinations mean something.
    awaited_discoveries: Vec<(pid_t, Option<String>, Vec<SpaceId>)>,
    /// Saved native Spaces not proven to exist. They are destinations for nothing, which keeps
    /// their workspaces intact for a display that may return.
    unresolved_spaces: BTreeSet<SpaceId>,
}

impl LayoutEngine {
    pub(super) fn is_restoring(&self) -> bool {
        !self.persistence.pending.is_empty() && !self.persistence.awaiting_topology
    }

    pub(super) fn saved_space_is_unresolved(&self, space: Option<SpaceId>) -> bool {
        space.is_some_and(|space| self.persistence.unresolved_spaces.contains(&space))
    }

    pub(super) fn observe_window_for_persistence(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        window: WindowId,
        title: Option<&str>,
        size: CGSize,
        app_id: Option<&str>,
    ) -> bool {
        if !self.is_restoring() {
            return false;
        }
        let live = WindowFingerprint {
            window_server_id: window_store
                .window(window)
                .and_then(|window| window.info.sys_id)
                .map(|id| id.as_u32()),
            title: title.filter(|title| !title.trim().is_empty()).map(str::to_owned),
            width: size.width,
            height: size.height,
            app_id: app_id.filter(|app_id| !app_id.trim().is_empty()).map(str::to_owned),
        };
        self.reconcile_saved_window(window_store, window, &live, matcher::MatchScope::Space(space))
    }

    pub(super) fn forget_persisted_window(&mut self, window: WindowId) {
        if let Some(saved) = self.persistence.pending.remove(&window) {
            self.discard_saved_window(window, saved);
        }
    }

    pub(super) fn forget_persisted_app(&mut self, pid: pid_t) {
        self.persistence.pending.retain(|window, _| window.pid != pid);
    }

    pub(super) fn forget_persisted_workspace(&mut self, workspace: VirtualWorkspaceId) {
        self.persistence.pending.retain(|_, saved| saved.workspace != workspace);
    }
}

mod matcher;
mod restore;
mod snapshot;
mod storage;
mod topology;

#[cfg(test)]
mod tests;
