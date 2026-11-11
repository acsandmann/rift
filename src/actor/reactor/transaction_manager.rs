use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use crate::model::tx_store::WindowTxStore;
use crate::sys::window_server::WindowServerId;

/// A per-window counter that tracks the last time the reactor sent a request to
/// change the window frame.
#[derive(Default, Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionId(u32);

impl TransactionId {
    pub fn next(self) -> Self { Self(self.0.wrapping_add(1)) }
}

/// Manages window transaction IDs and their associated target frames.
#[derive(Debug)]
pub struct TransactionManager {
    pub store: WindowTxStore,
}

impl TransactionManager {
    pub fn new(store: WindowTxStore) -> Self { Self { store } }

    pub fn request_frame(
        &self,
        wsid: WindowServerId,
        target: CGRect,
        presentation: bool,
    ) -> TransactionId {
        self.store.begin(wsid, Some(target), presentation)
    }

    /// Removes the transaction ID entry for a window.
    pub fn remove_for_window(&self, wsid: WindowServerId) { self.store.remove(&wsid); }

    /// Clears the pending target for a window while preserving its last txid.
    pub fn clear_target_for_window(&self, wsid: WindowServerId) { self.store.clear_target(&wsid); }

    /// Generates the next transaction ID for a window.
    pub fn generate_next_txid(&self, wsid: WindowServerId) -> TransactionId {
        self.store.begin(wsid, None, false)
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
