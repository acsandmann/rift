use std::sync::Arc;

use dashmap::DashMap;
use objc2_core_foundation::CGRect;

use crate::actor::reactor::transaction_manager::TransactionId;
use crate::sys::window_server::WindowServerId;

#[derive(Clone, Copy, Debug, Default)]
pub struct TxRecord {
    pub txid: TransactionId,
    pub target: Option<CGRect>,
    /// A live gesture can await terminal readback without a fixed target.
    pub presentation: bool,
}

/// Thread-safe cache mapping window server IDs to their last known transaction.
#[derive(Clone, Default, Debug)]
pub struct WindowTxStore(Arc<DashMap<WindowServerId, TxRecord>>);

impl WindowTxStore {
    pub fn new() -> Self { Self::default() }

    pub fn get(&self, id: &WindowServerId) -> Option<TxRecord> {
        self.0.get(id).map(|entry| *entry)
    }

    pub fn remove(&self, id: &WindowServerId) { self.0.remove(id); }

    pub fn clear_target(&self, id: &WindowServerId) {
        if let Some(mut record) = self.0.get_mut(id) {
            record.target = None;
            record.presentation = false;
        }
    }

    pub fn clear_presentation_target(&self, id: WindowServerId) {
        if let Some(mut record) = self.0.get_mut(&id)
            && record.presentation
        {
            record.target = None;
            record.presentation = false;
        }
    }

    /// Completion retires an attempt, not proof that its target was applied.
    /// Future layouts compare against observation and can request the frame again.
    pub fn finish(&self, id: WindowServerId, txid: TransactionId) {
        if let Some(mut record) = self.0.get_mut(&id)
            && record.txid == txid
        {
            record.target = None;
            record.presentation = false;
        }
    }

    pub fn begin(
        &self,
        id: WindowServerId,
        target: Option<CGRect>,
        presentation: bool,
    ) -> TransactionId {
        let mut record = self.0.entry(id).or_default();
        *record = TxRecord {
            txid: record.txid.next(),
            target,
            presentation,
        };
        record.txid
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
        let txid = store.begin(wsid, Some(target), false);
        assert_eq!(txid, TransactionId::default().next());
        assert_eq!(store.get(&wsid).unwrap().target, Some(target));

        store.clear_target(&wsid);

        let record = store.get(&wsid).expect("tx record should exist");
        assert_eq!(record.txid, txid);
        assert_eq!(record.target, None);

        let newer = store.begin(wsid, Some(target), true);
        store.finish(wsid, txid);
        assert_eq!(
            store.get(&wsid).unwrap().target,
            Some(target),
            "late retirement must preserve the new target"
        );
        store.finish(wsid, newer);
        assert_eq!(store.get(&wsid).unwrap().target, None);
        assert_eq!(store.last_txid(&wsid), newer);
    }

    #[test]
    fn next_txid_advances_after_clearing_target() {
        let store = WindowTxStore::new();
        let wsid = WindowServerId::new(2);

        let target = CGRect::new(CGPoint::new(1.0, 2.0), CGSize::new(3.0, 4.0));
        let txid_1 = store.begin(wsid, Some(target), false);
        store.clear_target(&wsid);

        let txid_2 = store.begin(wsid, None, false);
        assert_eq!(txid_2, txid_1.next());
    }
}
