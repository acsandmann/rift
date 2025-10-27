use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use crate::common::collections::HashMap;
use crate::model::tx_store::WindowTxStore;
use crate::sys::window_server::WindowServerId;

/// A per-window counter that tracks the last time the reactor sent a request to
/// change the window frame.
#[derive(Default, Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionId(u32);

/// Manages window transaction IDs and their associated target frames.
#[derive(Debug)]
pub struct TransactionManager {
    pub store: Option<WindowTxStore>,
    pub last_sent_txids: HashMap<WindowServerId, TransactionId>,
}

impl TransactionManager {
    /// Sets the transaction store.
    pub fn set_store(&mut self, store: WindowTxStore) { self.store = Some(store); }

    /// Stores a transaction ID for a window with its target frame.
    pub fn store_txid(&self, wsid: WindowServerId, txid: TransactionId, target: CGRect) {
        if let (Some(store), id) = (self.store.as_ref(), wsid) {
            store.insert(id, txid, target);
        }
    }

    /// Updates multiple transaction ID entries.
    pub fn update_txid_entries<I>(&self, entries: I)
    where I: IntoIterator<Item = (WindowServerId, TransactionId, CGRect)> {
        if let Some(store) = self.store.as_ref() {
            for (wsid, txid, target) in entries {
                store.insert(wsid, txid, target);
            }
        }
    }

    /// Removes the transaction ID entry for a window.
    pub fn remove_for_window(&self, wsid: WindowServerId) {
        if let (Some(store), id) = (self.store.as_ref(), wsid) {
            store.remove(&id);
        }
    }

    /// Generates the next transaction ID for a window.
    pub fn generate_next_txid(&mut self, wsid: WindowServerId) -> TransactionId {
        let txid = self.last_sent_txids.entry(wsid).or_default();
        txid.0 += 1;
        *txid
    }

    /// Sets the last sent transaction ID for a window.
    pub fn set_last_sent_txid(&mut self, wsid: WindowServerId, txid: TransactionId) {
        self.last_sent_txids.insert(wsid, txid);
    }

    /// Gets the last sent transaction ID for a window.
    pub fn get_last_sent_txid(&self, wsid: WindowServerId) -> TransactionId {
        self.last_sent_txids.get(&wsid).copied().unwrap_or_default()
    }

    /// Gets the target frame for a window's transaction, if it exists.
    pub fn get_target_frame(&self, wsid: WindowServerId) -> Option<CGRect> {
        self.store.as_ref()?.get(&wsid).map(|record| record.target)
    }
}
