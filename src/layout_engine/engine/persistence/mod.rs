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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreSource {
    /// Portable layout files restore the native space that was active when the file was saved.
    #[default]
    SavedActiveSpace,
    /// The master file is a complete Rift snapshot and restores the current native-space entry.
    CurrentSpace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreRequest {
    pub scope: RestoreScope,
    pub active_space: SpaceId,
    pub source: RestoreSource,
}

impl RestoreRequest {
    /// Restore a portable layout file from the space that was active when it was saved.
    pub fn new(scope: RestoreScope, active_space: SpaceId) -> Self {
        Self {
            scope,
            active_space,
            source: RestoreSource::SavedActiveSpace,
        }
    }

    /// Restore the master file's entry for the current native space when available.
    pub fn from_master_file(scope: RestoreScope, active_space: SpaceId) -> Self {
        Self {
            scope,
            active_space,
            source: RestoreSource::CurrentSpace,
        }
    }
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
        if self.duplicates_removed > 0 {
            summary.push_str(&format!(
                "; repaired {} duplicate saved identity record(s)",
                self.duplicates_removed
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
    /// Native space active when an explicit save was requested.
    ///
    /// A layout file contains every initialized native space. This hint makes a portable file's
    /// intended source unambiguous when its SpaceIds do not exist in the current session.
    #[serde(default)]
    saved_active_space: Option<u64>,
    #[serde(skip)]
    pending_windows: HashSet<WindowId>,
}

impl PersistenceState {
    fn validate(&self) -> anyhow::Result<()> {
        for (window, fingerprint) in &self.windows {
            if !fingerprint.width.is_finite()
                || !fingerprint.height.is_finite()
                || fingerprint.width < 0.0
                || fingerprint.height < 0.0
            {
                return Err(anyhow::anyhow!(
                    "window {window:?} has an invalid persisted size"
                ));
            }
        }
        Ok(())
    }

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

    fn set_saved_active_space(&mut self, space: Option<SpaceId>) {
        self.saved_active_space = space.map(|space| space.get());
    }
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
