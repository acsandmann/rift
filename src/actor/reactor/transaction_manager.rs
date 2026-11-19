use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use crate::model::tx_store::WindowTxStore;
use crate::sys::window_server::WindowServerId;

/// A per-window counter that tracks the last time the reactor sent a request to
/// change the window frame.
#[derive(Default, Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionId(u32);

impl TransactionId {
    pub(crate) fn new(id: u32) -> Self { Self(id) }

    #[cfg(test)]
    pub fn next(self) -> Self { Self(self.0.wrapping_add(1)) }
}

/// Manages window transaction IDs and their associated target frames.
#[derive(Debug)]
pub struct TransactionManager {
    pub store: WindowTxStore,
}

impl TransactionManager {
    pub fn new(store: WindowTxStore) -> Self { Self { store } }

    /// Stores a transaction ID for a window with its target frame.
    pub fn store_txid(&self, wsid: WindowServerId, txid: TransactionId, target: CGRect) {
        self.store.insert(wsid, txid, target);
    }

    /// Removes the transaction ID entry for a window.
    pub fn remove_for_window(&self, wsid: WindowServerId) { self.store.remove(&wsid); }

    /// Clears the pending target for a window while preserving its last txid.
    pub fn clear_target_for_window(&self, wsid: WindowServerId) { self.store.clear_target(&wsid); }

    pub fn begin(&self, wsid: WindowServerId, target: CGRect) -> TransactionId {
        self.store.begin(wsid, Some(target))
    }

    pub fn invalidate_for_window(&self, wsid: WindowServerId) { self.store.begin(wsid, None); }

    #[cfg(test)]
    pub fn generate_next_txid(&self, wsid: WindowServerId) -> TransactionId {
        self.store.begin(wsid, None)
    }

    #[cfg(test)]
    pub fn set_last_sent_txid(&self, wsid: WindowServerId, txid: TransactionId) {
        self.store.set_last_txid(wsid, txid);
    }

    /// Gets the last sent transaction ID for a window.
    #[cfg(test)]
    pub fn get_last_sent_txid(&self, wsid: WindowServerId) -> TransactionId {
        self.store.last_txid(&wsid)
    }

    /// Gets the target frame for a window's transaction, if it exists.
    pub fn get_target_frame(&self, wsid: WindowServerId) -> Option<CGRect> {
        self.store.get(&wsid)?.target()
    }
}
