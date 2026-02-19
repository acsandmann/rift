use std::time::Instant;

use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use crate::common::collections::HashMap;
use crate::model::tx_store::WindowTxStore;
use crate::sys::window_server::WindowServerId;

/// How long after a RIFT-initiated move completes before we accept
/// unsolicited frame-change events as user-initiated.  This prevents late
/// SLS/WindowNotify events from being misclassified and triggering re-tile
/// feedback loops.
const SETTLE_DURATION_MS: u64 = 100;

/// A per-window counter that tracks the last time the reactor sent a request to
/// change the window frame.
#[derive(Default, Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionId(u32);

impl TransactionId {
    pub fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// Manages window transaction IDs and their associated target frames.
#[derive(Debug)]
pub struct TransactionManager {
    pub store: WindowTxStore,
    /// Tracks when each window's RIFT-initiated target was last cleared.
    /// Used to implement a short settling cooldown that prevents late
    /// notification events from being misclassified as user-initiated moves.
    settling_until: HashMap<WindowServerId, Instant>,
}

impl TransactionManager {
    pub fn new(store: WindowTxStore) -> Self {
        Self {
            store,
            settling_until: HashMap::default(),
        }
    }

    /// Stores a transaction ID for a window with its target frame.
    pub fn store_txid(&self, wsid: WindowServerId, txid: TransactionId, target: CGRect) {
        self.store.insert(wsid, txid, target);
    }

    /// Updates multiple transaction ID entries.
    pub fn update_txid_entries<I>(&self, entries: I)
    where
        I: IntoIterator<Item = (WindowServerId, TransactionId, CGRect)>,
    {
        for (wsid, txid, target) in entries {
            self.store.insert(wsid, txid, target);
        }
    }

    /// Removes the transaction ID entry for a window.
    pub fn remove_for_window(&self, wsid: WindowServerId) {
        self.store.remove(&wsid);
    }

    /// Clears the pending target for a window while preserving its last txid.
    /// Also starts a settling cooldown to suppress late notification events.
    pub fn clear_target_for_window(&mut self, wsid: WindowServerId) {
        self.store.clear_target(&wsid);
        self.settling_until.insert(
            wsid,
            Instant::now() + std::time::Duration::from_millis(SETTLE_DURATION_MS),
        );
    }

    /// Returns `true` if the window is still within the settling cooldown
    /// after a RIFT-initiated move completed.
    pub fn is_settling(&self, wsid: WindowServerId) -> bool {
        self.settling_until
            .get(&wsid)
            .is_some_and(|deadline| Instant::now() < *deadline)
    }

    /// Generates the next transaction ID for a window.
    pub fn generate_next_txid(&self, wsid: WindowServerId) -> TransactionId {
        self.store.next_txid(wsid)
    }

    /// Sets the last sent transaction ID for a window.
    pub fn set_last_sent_txid(&self, wsid: WindowServerId, txid: TransactionId) {
        self.store.set_last_txid(wsid, txid);
    }

    /// Gets the last sent transaction ID for a window.
    pub fn get_last_sent_txid(&self, wsid: WindowServerId) -> TransactionId {
        self.store.last_txid(&wsid)
    }

    /// Gets the target frame for a window's transaction, if it exists.
    pub fn get_target_frame(&self, wsid: WindowServerId) -> Option<CGRect> {
        self.store.get(&wsid)?.target
    }
}
