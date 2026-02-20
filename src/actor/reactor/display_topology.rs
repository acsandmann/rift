use std::time::Instant;

use tracing::{debug, info};

use crate::common::collections::{HashMap, HashSet};
use crate::sys::screen::{ScreenInfo, SpaceId};
use crate::sys::skylight::DisplayReconfigFlags;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

#[derive(Debug, Clone)]
pub struct WindowSnapshot {
    pub info: WindowServerInfo,
}

#[derive(Debug, Clone)]
pub struct DisplaySnapshot {
    pub ordered_screens: Vec<ScreenInfo>,
    pub active_spaces: HashSet<SpaceId>,
    pub inactive_spaces: HashSet<SpaceId>,
    pub windows: HashMap<WindowServerId, WindowSnapshot>,
}

#[derive(Debug, Clone)]
pub enum TopologyState {
    Stable,
    Churning {
        epoch: u64,
        started_at: Instant,
        flags: DisplayReconfigFlags,
    },
    AwaitingCommitSnapshot {
        epoch: u64,
        started_at: Instant,
        flags: DisplayReconfigFlags,
        pre_known_wsids: HashSet<WindowServerId>,
    },
}

impl Default for TopologyState {
    fn default() -> Self { TopologyState::Stable }
}

#[derive(Debug, Default, Clone)]
pub struct QuarantineStats {
    pub appeared_dropped: u64,
    pub destroyed_dropped: u64,
    pub resync_dropped: u64,
}

#[derive(Debug, Default, Clone)]
pub struct DisplayTopologyManager {
    state: TopologyState,
    pub quarantine_stats: QuarantineStats,
    churn_pre_known_wsids: HashSet<WindowServerId>,
}

impl DisplayTopologyManager {
    #[cfg(test)]
    pub(crate) fn state(&self) -> &TopologyState { &self.state }

    pub fn is_churning_or_awaiting_commit(&self) -> bool {
        matches!(
            self.state,
            TopologyState::Churning { .. } | TopologyState::AwaitingCommitSnapshot { .. }
        )
    }

    pub fn begin_churn(
        &mut self,
        epoch: u64,
        flags: DisplayReconfigFlags,
        pre_known_wsids: HashSet<WindowServerId>,
    ) {
        let now = Instant::now();
        self.state = TopologyState::Churning { epoch, started_at: now, flags };
        self.churn_pre_known_wsids = pre_known_wsids;
        debug!(
            epoch,
            flags = ?flags,
            pre_known = self.churn_pre_known_wsids.len(),
            "display churn begin"
        );
        self.quarantine_stats = QuarantineStats::default();
    }

    pub fn end_churn_to_awaiting(&mut self, epoch: u64, flags: DisplayReconfigFlags) {
        let now = Instant::now();
        let pre_known_wsids = std::mem::take(&mut self.churn_pre_known_wsids);
        self.state = TopologyState::AwaitingCommitSnapshot {
            epoch,
            started_at: now,
            flags,
            pre_known_wsids,
        };
        info!(
            epoch,
            flags = ?flags,
            dropped_appeared = self.quarantine_stats.appeared_dropped,
            dropped_destroyed = self.quarantine_stats.destroyed_dropped,
            dropped_resync = self.quarantine_stats.resync_dropped,
            "display churn ended; awaiting commit snapshot"
        );
    }

    pub fn take_awaiting_commit(
        &mut self,
    ) -> Option<(u64, Instant, DisplayReconfigFlags, HashSet<WindowServerId>)> {
        match std::mem::replace(&mut self.state, TopologyState::Stable) {
            TopologyState::AwaitingCommitSnapshot {
                epoch,
                started_at,
                flags,
                pre_known_wsids,
            } => Some((epoch, started_at, flags, pre_known_wsids)),
            other => {
                self.state = other;
                None
            }
        }
    }

    pub fn restore_awaiting_commit(
        &mut self,
        epoch: u64,
        started_at: Instant,
        flags: DisplayReconfigFlags,
        pre_known_wsids: HashSet<WindowServerId>,
    ) {
        self.state = TopologyState::AwaitingCommitSnapshot {
            epoch,
            started_at,
            flags,
            pre_known_wsids,
        };
    }

    pub fn current_churn(&self) -> Option<(u64, Instant, DisplayReconfigFlags)> {
        match self.state {
            TopologyState::Churning { epoch, started_at, flags } => {
                Some((epoch, started_at, flags))
            }
            _ => None,
        }
    }

    pub fn quarantine_appeared(&mut self) { self.quarantine_stats.appeared_dropped += 1; }

    pub fn quarantine_destroyed(&mut self) { self.quarantine_stats.destroyed_dropped += 1; }

    pub fn quarantine_resync(&mut self) { self.quarantine_stats.resync_dropped += 1; }

    pub fn mark_stable(&mut self) { self.state = TopologyState::Stable; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn churn_state_machine_round_trips() {
        let mut manager = DisplayTopologyManager::default();
        let mut pre_known = HashSet::default();
        pre_known.insert(WindowServerId::new(10));

        manager.begin_churn(1, DisplayReconfigFlags::ADD, pre_known.clone());
        assert!(matches!(manager.state(), TopologyState::Churning { .. }));

        manager.end_churn_to_awaiting(1, DisplayReconfigFlags::ADD);
        assert!(matches!(
            manager.state(),
            TopologyState::AwaitingCommitSnapshot { .. }
        ));

        let taken = manager.take_awaiting_commit();
        assert!(taken.is_some());
        assert!(matches!(manager.state(), TopologyState::Stable));
    }

    #[test]
    fn restore_awaiting_keeps_pending_commit() {
        let mut manager = DisplayTopologyManager::default();
        let mut pre_known = HashSet::default();
        pre_known.insert(WindowServerId::new(22));

        manager.restore_awaiting_commit(9, Instant::now(), DisplayReconfigFlags::REMOVE, pre_known);
        assert!(matches!(
            manager.state(),
            TopologyState::AwaitingCommitSnapshot { .. }
        ));
    }
}
