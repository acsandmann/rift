use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use dashmap::DashMap;
use objc2_core_foundation::CGRect;

use crate::actor::reactor::transaction_manager::TransactionId;
use crate::sys::geometry::SameAs;
use crate::sys::window_server::WindowServerId;

#[derive(Clone, Copy, Debug, Default)]
enum FrameWrite {
    #[default]
    Idle,
    Pending(CGRect),
    Failed,
    Settled {
        target: CGRect,
        observed: CGRect,
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TxRecord {
    pub txid: TransactionId,
    write: FrameWrite,
}

impl TxRecord {
    pub fn target(self) -> Option<CGRect> {
        match self.write {
            FrameWrite::Pending(frame) => Some(frame),
            _ => None,
        }
    }

    pub fn covers(self, desired: CGRect, current: CGRect) -> bool {
        match self.write {
            FrameWrite::Idle => current.same_as(desired),
            FrameWrite::Pending(target) => target.same_as(desired),
            FrameWrite::Failed => false,
            FrameWrite::Settled { target, observed } => {
                target.same_as(desired) && observed.same_as(current)
            }
        }
    }

    pub fn needs_full_retry(self) -> bool { matches!(self.write, FrameWrite::Failed) }

    pub fn refused(self, desired: CGRect, current: CGRect) -> bool {
        matches!(self.write, FrameWrite::Settled { target, observed }
            if !target.same_as(observed) && self.covers(desired, current))
    }
}

#[derive(Debug, Default)]
struct Transactions {
    records: DashMap<WindowServerId, TxRecord>,
    sequence: AtomicU32,
}

/// Shared frame-write state. A batch may share an ID across windows.
#[derive(Clone, Default, Debug)]
pub struct WindowTxStore(Arc<Transactions>);

impl WindowTxStore {
    pub fn new() -> Self { Self::default() }

    pub fn next_id(&self) -> TransactionId {
        // Zero is reserved for writes to windows with no server identity.
        let id = self.0.sequence.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        TransactionId::new(if id == 0 {
            self.0.sequence.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
        } else {
            id
        })
    }

    pub fn begin(&self, id: WindowServerId, target: Option<CGRect>) -> TransactionId {
        let mut record = self.0.records.entry(id).or_default();
        let txid = self.next_id();
        *record = TxRecord {
            txid,
            write: target.map_or(FrameWrite::Idle, FrameWrite::Pending),
        };
        txid
    }

    pub fn insert(&self, id: WindowServerId, txid: TransactionId, target: CGRect) {
        self.0.records.insert(id, TxRecord {
            txid,
            write: FrameWrite::Pending(target),
        });
    }

    pub fn get(&self, id: &WindowServerId) -> Option<TxRecord> {
        self.0.records.get(id).map(|entry| *entry)
    }

    pub fn remove(&self, id: &WindowServerId) { self.0.records.remove(id); }

    pub fn clear_target(&self, id: &WindowServerId) {
        if let Some(mut record) = self.0.records.get_mut(id)
            && !record.needs_full_retry()
        {
            record.write = FrameWrite::Idle;
        }
    }

    pub fn clear_target_if_current(&self, id: &WindowServerId, txid: TransactionId) {
        if let Some(mut record) = self.0.records.get_mut(id)
            && record.txid == txid
            && !record.needs_full_retry()
        {
            record.write = FrameWrite::Idle;
        }
    }

    pub fn fail(&self, id: WindowServerId, txid: TransactionId) {
        if let Some(mut record) = self.0.records.get_mut(&id)
            && record.txid == txid
        {
            record.write = FrameWrite::Failed;
        }
    }

    pub fn acknowledge(&self, id: WindowServerId, txid: TransactionId, observed: CGRect) {
        if let Some(mut record) = self.0.records.get_mut(&id)
            && record.txid == txid
            && let FrameWrite::Pending(target) | FrameWrite::Settled { target, .. } = record.write
        {
            record.write = FrameWrite::Settled { target, observed };
        }
    }

    #[cfg(test)]
    fn next_txid(&self, id: WindowServerId) -> TransactionId { self.begin(id, None) }

    #[cfg(test)]
    pub fn set_last_txid(&self, id: WindowServerId, txid: TransactionId) {
        self.0.records.insert(id, TxRecord { txid, write: FrameWrite::Idle });
    }

    #[cfg(test)]
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
        assert_eq!(store.get(&wsid).unwrap().target(), Some(target));

        store.clear_target(&wsid);

        let record = store.get(&wsid).expect("tx record should exist");
        assert_eq!(record.txid, txid);
        assert_eq!(record.target(), None);

        let newer = store.next_txid(wsid);
        store.insert(wsid, newer, target);
        store.clear_target_if_current(&wsid, txid);
        assert_eq!(
            store.get(&wsid).unwrap().target(),
            Some(target),
            "late retirement must preserve the new target"
        );
        store.clear_target_if_current(&wsid, newer);
        assert_eq!(store.get(&wsid).unwrap().target(), None);
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
        assert_eq!(record.target(), None);
    }
}
