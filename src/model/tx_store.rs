use std::sync::Arc;

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use objc2_core_foundation::CGRect;

use crate::actor::reactor::transaction_manager::TransactionId;
use crate::sys::window_server::WindowServerId;

#[derive(Clone, Copy, Debug, Default)]
pub struct TxRecord {
    pub txid: TransactionId,
    pub target: Option<CGRect>,
}

/// Thread-safe cache mapping window server IDs to their last known transaction.
#[derive(Clone, Default, Debug)]
pub struct WindowTxStore(Arc<DashMap<WindowServerId, TxRecord>>);

impl WindowTxStore {
    pub fn new() -> Self { Self::default() }

    pub fn insert(&self, id: WindowServerId, txid: TransactionId, target: CGRect) {
        match self.0.entry(id) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() = TxRecord { txid, target: Some(target) }
            }
            Entry::Vacant(entry) => {
                entry.insert(TxRecord { txid, target: Some(target) });
            }
        }
    }

    pub fn get(&self, id: &WindowServerId) -> Option<TxRecord> {
        self.0.get(id).map(|entry| *entry)
    }

    pub fn clear_target(&self, id: &WindowServerId) {
        if let Some(mut entry) = self.0.get_mut(id) {
            entry.target = None;
        }
    }

    pub fn remove(&self, id: &WindowServerId) { self.0.remove(id); }

    pub fn next_txid(&self, id: WindowServerId) -> TransactionId {
        let new_txid = match self.0.entry(id) {
            Entry::Occupied(mut entry) => {
                let record = entry.get_mut();
                let new_txid = record.txid.next();
                *record = TxRecord { txid: new_txid, target: None };
                new_txid
            }
            Entry::Vacant(entry) => {
                let txid = TransactionId::default().next();
                entry.insert(TxRecord { txid, target: None });
                txid
            }
        };
        new_txid
    }

    pub fn set_last_txid(&self, id: WindowServerId, txid: TransactionId) {
        match self.0.entry(id) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().txid = txid;
            }
            Entry::Vacant(entry) => {
                entry.insert(TxRecord { txid, target: None });
            }
        }
    }

    pub fn last_txid(&self, id: &WindowServerId) -> TransactionId {
        self.get(id).map(|record| record.txid).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::*;

    #[test]
    fn clear_target_preserves_txid_sequence() {
        let store = WindowTxStore::new();
        let wsid = WindowServerId(42);
        let frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 100.0,
                height: 100.0,
            },
        };

        let tx1 = store.next_txid(wsid);
        assert_eq!(tx1, TransactionId::default().next());
        store.insert(wsid, tx1, frame);
        store.clear_target(&wsid);

        let rec = store.get(&wsid).expect("tx record should remain after clear_target");
        assert_eq!(rec.txid, tx1);
        assert!(rec.target.is_none());

        let tx2 = store.next_txid(wsid);
        assert_eq!(tx2, tx1.next());
    }

    #[test]
    fn remove_purges_txid_state() {
        let store = WindowTxStore::new();
        let wsid = WindowServerId(7);
        let tx1 = store.next_txid(wsid);
        store.remove(&wsid);
        assert!(store.get(&wsid).is_none());

        let tx2 = store.next_txid(wsid);
        assert_eq!(tx2, TransactionId::default().next());
        assert_eq!(tx1, tx2);
    }
}
