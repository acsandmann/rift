use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use dashmap::DashMap;
use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use crate::model::reactor::WindowState;
use crate::model::window_frame::{ActualFrame, PlannedFrame};
use crate::sys::geometry::SameAs;
use crate::sys::window_server::WindowServerId;

/// The id of a frame write. Every write gets a new one; an app's report of a
/// frame names the latest write it had applied.
#[derive(Default, Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionId(u32);

impl TransactionId {
    #[cfg(test)]
    pub fn next(self) -> Self { Self(self.0.wrapping_add(1)) }
}

#[derive(Clone, Copy, Debug)]
enum FrameWrite {
    Idle,
    Pending(CGRect),
    Settled { target: CGRect, observed: CGRect },
}

#[derive(Clone, Copy, Debug)]
pub struct TxRecord {
    txid: TransactionId,
    write: FrameWrite,
}

impl TxRecord {
    /// The id of the latest write to the window.
    pub fn txid(self) -> TransactionId { self.txid }

    pub fn target(self) -> Option<CGRect> {
        match self.write {
            FrameWrite::Pending(frame) => Some(frame),
            _ => None,
        }
    }

    /// Whether this write already takes a window that is at `actual` to `desired`.
    fn covers(self, desired: CGRect, actual: ActualFrame) -> bool {
        let actual = actual.rect();
        match self.write {
            FrameWrite::Idle => actual.same_as(desired),
            FrameWrite::Pending(target) => target.same_as(desired),
            FrameWrite::Settled { target, observed } => {
                target.same_as(desired) && observed.same_as(actual)
            }
        }
    }

    /// Whether the app answered a write to `desired` with another frame, and
    /// the window is still at what it answered.
    fn refused(self, desired: CGRect, actual: ActualFrame) -> bool {
        matches!(self.write, FrameWrite::Settled { target, observed }
            if !target.same_as(observed) && self.covers(desired, actual))
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
        TransactionId(if id == 0 {
            self.0.sequence.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
        } else {
            id
        })
    }

    /// Starts a write to `target` under a fresh id.
    pub fn begin(&self, id: WindowServerId, target: CGRect) -> TransactionId {
        let txid = self.next_id();
        self.insert(id, txid, target);
        txid
    }

    /// Issues a fresh id with no write, so writes still in flight stop being current.
    pub fn fence(&self, id: WindowServerId) -> TransactionId {
        let txid = self.next_id();
        self.0.records.insert(id, TxRecord { txid, write: FrameWrite::Idle });
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

    /// The frame a write in flight is taking the window to.
    pub fn target(&self, id: &WindowServerId) -> Option<CGRect> { self.get(id)?.target() }

    /// Where a window will be: the target of a write in flight, otherwise
    /// where the window is.
    pub(crate) fn planned(&self, window: &WindowState) -> PlannedFrame {
        PlannedFrame::new(
            window
                .info
                .sys_id
                .and_then(|id| self.target(&id))
                .unwrap_or(window.actual().rect()),
        )
    }

    /// Whether the window's write, if any, already takes it to `desired`.
    pub(crate) fn covers(&self, window: &WindowState, desired: CGRect) -> bool {
        match window.info.sys_id.and_then(|id| self.get(&id)) {
            Some(record) => record.covers(desired, window.actual()),
            None => window.actual().rect().same_as(desired),
        }
    }

    /// Whether the app answered the window's write to `desired` with another
    /// frame, and the window is still at what it answered.
    pub(crate) fn refused(&self, window: &WindowState, desired: CGRect) -> bool {
        window
            .info
            .sys_id
            .and_then(|id| self.get(&id))
            .is_some_and(|record| record.refused(desired, window.actual()))
    }

    pub fn remove(&self, id: &WindowServerId) { self.0.records.remove(id); }

    pub fn clear_target(&self, id: &WindowServerId) {
        if let Some(mut record) = self.0.records.get_mut(id) {
            record.write = FrameWrite::Idle;
        }
    }

    pub fn clear_target_if_current(&self, id: &WindowServerId, txid: TransactionId) {
        if let Some(mut record) = self.0.records.get_mut(id)
            && record.txid == txid
        {
            record.write = FrameWrite::Idle;
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
        let txid = store.fence(wsid);
        store.insert(wsid, txid, target);
        assert_eq!(txid, TransactionId::default().next());
        assert_eq!(store.get(&wsid).unwrap().target(), Some(target));

        store.clear_target(&wsid);

        let record = store.get(&wsid).expect("tx record should exist");
        assert_eq!(record.txid, txid);
        assert_eq!(record.target(), None);

        let newer = store.fence(wsid);
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

        let txid_1 = store.fence(wsid);
        let target = CGRect::new(CGPoint::new(1.0, 2.0), CGSize::new(3.0, 4.0));
        store.insert(wsid, txid_1, target);
        store.clear_target(&wsid);

        let txid_2 = store.fence(wsid);
        assert_eq!(txid_2, txid_1.next());
    }
}
