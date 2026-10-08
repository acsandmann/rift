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
            Entry::Occupied(mut entry) => *entry.get_mut() = TxRecord { txid, target: Some(target) },
            Entry::Vacant(entry) => {
                entry.insert(TxRecord { txid, target: Some(target) });
            }
        }
    }

    pub fn get(&self, id: &WindowServerId) -> Option<TxRecord> { self.0.get(id).map(|entry| *entry) }

    pub fn remove(&self, id: &WindowServerId) { self.0.remove(id); }

    pub fn clear_target(&self, id: &WindowServerId) {
        if let Some(mut record) = self.0.get_mut(id) {
            record.target = None;
        }
    }

    /// A retiring presentation cannot clear a newer semantic transaction.
    pub fn clear_target_if_current(&self, id: &WindowServerId, txid: TransactionId) {
        if let Some(mut record) = self.0.get_mut(id)
            && record.txid == txid
        {
            record.target = None;
        }
    }

    pub fn next_txid(&self, id: WindowServerId) -> TransactionId { self.advance(id, None) }

    fn advance(&self, id: WindowServerId, target: Option<CGRect>) -> TransactionId {
        let mut record = self.0.entry(id).or_default();
        record.txid = record.txid.next();
        record.target = target;
        record.txid
    }

    pub fn set_last_txid(&self, id: WindowServerId, txid: TransactionId) {
        match self.0.entry(id) {
            Entry::Occupied(mut entry) => {
                let record = entry.get_mut();
                record.txid = txid;
                record.target = None;
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
    fn clear_target_keeps_last_txid_and_fences_retired_presentations() {
        let store = WindowTxStore::new();
        let wsid = WindowServerId::new(1);
        let target = CGRect::new(CGPoint::new(10.0, 20.0), CGSize::new(30.0, 40.0));
        let txid = store.next_txid(wsid);
        store.insert(wsid, txid, target);
        assert_eq!(txid, TransactionId::default().next());
        assert_eq!(store.get(&wsid).unwrap().target, Some(target));

        store.clear_target(&wsid);

        let record = store.get(&wsid).expect("tx record should exist");
        assert_eq!(record.txid, txid);
        assert_eq!(record.target, None);

        let newer = store.next_txid(wsid);
        store.insert(wsid, newer, target);
        store.clear_target_if_current(&wsid, txid);
        assert_eq!(
            store.get(&wsid).unwrap().target,
            Some(target),
            "late retirement must preserve the new target"
        );
        store.clear_target_if_current(&wsid, newer);
        assert_eq!(store.get(&wsid).unwrap().target, None);
        assert_eq!(store.last_txid(&wsid), newer);
    }

    #[test]
    fn next_txid_advances_after_clearing_target() {
        let store = WindowTxStore::new();
        let wsid = WindowServerId::new(2);

        let txid_1 = store.next_txid(wsid);
        let target = CGRect::new(CGPoint::new(1.0, 2.0), CGSize::new(3.0, 4.0));
        store.insert(wsid, txid_1, target);
        store.clear_target(&wsid);

        let txid_2 = store.next_txid(wsid);
        assert_eq!(txid_2, txid_1.next());
    }

    #[test]
    fn set_last_txid_clears_any_stale_target() {
        let store = WindowTxStore::new();
        let wsid = WindowServerId::new(3);
        let txid_1 = store.next_txid(wsid);
        let target = CGRect::new(CGPoint::new(5.0, 6.0), CGSize::new(7.0, 8.0));
        store.insert(wsid, txid_1, target);

        let txid_2 = txid_1.next();
        store.set_last_txid(wsid, txid_2);

        let record = store.get(&wsid).expect("tx record should exist");
        assert_eq!(record.txid, txid_2);
        assert_eq!(record.target, None);
    }
}
