use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use objc2_core_foundation::CGSize;
use serde::{Deserialize, Serialize};

use super::{FloatingManager, LayoutEngine, WorkspaceLayouts};
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{LayoutSettings, VirtualWorkspaceSettings};
use crate::layout_engine::LayoutSystem;
use crate::model::broadcast::BroadcastSender;
use crate::model::{
    AppRuleEngine, FloatingPositionStore, VirtualWorkspaceId, WindowStore, WorkspaceStore,
};
use crate::sys::screen::SpaceId;

static SAVE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreScope {
    Workspace,
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreRequest {
    pub scope: RestoreScope,
    pub active_space: SpaceId,
}

impl RestoreRequest {
    pub fn new(scope: RestoreScope, active_space: SpaceId) -> Self { Self { scope, active_space } }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreWarning {
    UnmatchedWindows(usize),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub matched: usize,
    pub unmatched: usize,
    pub duplicates_removed: usize,
    pub workspaces_replaced: usize,
    pub warnings: Vec<RestoreWarning>,
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
    fn app_compatible_with(&self, live: &Self) -> bool {
        // A non-exact fallback must never cross two known bundle identities. Titles such as
        // "Untitled" and common default sizes are not globally unique across applications.
        self.app_id
            .as_ref()
            .zip(live.app_id.as_ref())
            .is_none_or(|(saved, current)| saved == current)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct PersistenceState {
    #[serde(default, rename = "persisted_windows")]
    windows: HashMap<WindowId, WindowFingerprint>,
    #[serde(skip)]
    pending_windows: HashSet<WindowId>,
}

impl PersistenceState {
    fn record(&mut self, window: WindowId, fingerprint: WindowFingerprint) {
        self.windows.insert(window, fingerprint);
    }

    fn fingerprint(&self, window: WindowId) -> Option<&WindowFingerprint> {
        self.windows.get(&window)
    }

    fn forget_window(&mut self, window: WindowId) {
        self.windows.remove(&window);
        self.pending_windows.remove(&window);
    }

    fn forget_app(&mut self, pid: pid_t) {
        self.windows.retain(|window, _| window.pid != pid);
        self.pending_windows.retain(|window| window.pid != pid);
    }

    fn rekey(&mut self, from: WindowId, to: WindowId) {
        if let Some(fingerprint) = self.windows.remove(&from) {
            self.windows.insert(to, fingerprint);
        }
        if self.pending_windows.remove(&from) {
            self.pending_windows.insert(to);
        }
    }

    fn replace_pending(&mut self, windows: impl IntoIterator<Item = WindowId>) {
        self.pending_windows = windows.into_iter().collect();
    }

    fn remove_candidate(&mut self, window: WindowId) {
        self.pending_windows.remove(&window);
        self.windows.remove(&window);
    }

    fn pending_len(&self) -> usize { self.pending_windows.len() }

    fn live_fingerprints(&self) -> HashMap<WindowId, WindowFingerprint> { self.windows.clone() }
}

impl LayoutEngine {
    pub(super) fn observe_window_for_persistence(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        window: WindowId,
        title: Option<&str>,
        size: CGSize,
        app_id: Option<&str>,
    ) {
        let fingerprint = WindowFingerprint {
            window_server_id: window_store
                .window(window)
                .and_then(|window| window.info.sys_id)
                .map(|id| id.as_u32()),
            title: title.filter(|title| !title.trim().is_empty()).map(str::to_owned),
            width: size.width,
            height: size.height,
            app_id: app_id.filter(|app_id| !app_id.trim().is_empty()).map(str::to_owned),
        };
        self.reconcile_restored_window(window_store, space, window, &fingerprint);
        self.persistence.record(window, fingerprint);
    }

    pub(super) fn forget_persisted_window(&mut self, window: WindowId) {
        self.persistence.forget_window(window);
    }

    pub(super) fn forget_persisted_app(&mut self, pid: pid_t) { self.persistence.forget_app(pid); }

    pub(super) fn transfer_persisted_window_identity(&mut self, from: WindowId, to: WindowId) {
        self.persistence.rekey(from, to);
    }
}

mod matcher;
mod reconcile;
mod restore;
mod snapshot;
mod storage;

#[cfg(test)]
mod tests;
