use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;

/// Pending method call information.
#[derive(Clone, Debug)]
pub(crate) struct PendingCall {
    pub(crate) caller_client_id: usize,
    pub(crate) destination: String,
    pub(crate) destination_client_id: usize,
}

pub(crate) fn oldest_pending_id_for_destination(
    pending_calls: &BTreeMap<u64, PendingCall>,
    destination_client_id: usize,
) -> Option<u64> {
    pending_calls
        .iter()
        .find(|(_, pending)| pending.destination_client_id == destination_client_id)
        .map(|(pending_id, _)| *pending_id)
}

/// Remove calls whose destination disappeared and return the live callers
/// that must be failed.
///
/// A call whose *caller* disconnected must deliberately remain in the FIFO as
/// a tombstone. Services currently return serial 0, so a late reply can only
/// be correlated by destination order. Removing the abandoned call would make
/// that reply consume the next caller's entry and shift every subsequent
/// response to the wrong application.
pub(crate) fn remove_pending_calls_for_disconnected_client(
    pending_calls: &mut BTreeMap<u64, PendingCall>,
    disconnected_client_id: usize,
) -> Vec<PendingCall> {
    let to_remove: Vec<u64> = pending_calls
        .iter()
        .filter(|(_, pending)| pending.destination_client_id == disconnected_client_id)
        .map(|(pending_id, _)| *pending_id)
        .collect();
    let mut failed = Vec::new();
    for pending_id in to_remove {
        if let Some(pending) = pending_calls.remove(&pending_id)
            && pending.caller_client_id != disconnected_client_id
        {
            failed.push(pending);
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::{
        PendingCall, oldest_pending_id_for_destination,
        remove_pending_calls_for_disconnected_client,
    };
    use std::collections::BTreeMap;

    fn pending(caller: usize, destination_client: usize) -> PendingCall {
        PendingCall {
            caller_client_id: caller,
            destination: String::from("org.scarlet.Test"),
            destination_client_id: destination_client,
        }
    }

    #[test]
    fn caller_disconnect_keeps_fifo_tombstone_for_late_reply() {
        let mut calls = BTreeMap::new();
        calls.insert(1, pending(10, 30));
        calls.insert(2, pending(11, 30));

        let failed = remove_pending_calls_for_disconnected_client(&mut calls, 10);

        assert!(failed.is_empty());
        assert_eq!(calls.len(), 2);
        let first = oldest_pending_id_for_destination(&calls, 30).unwrap();
        let first = calls.remove(&first).unwrap();
        assert_eq!(first.caller_client_id, 10);
        assert_eq!(first.destination, "org.scarlet.Test");
        let second = oldest_pending_id_for_destination(&calls, 30).unwrap();
        assert_eq!(calls.remove(&second).unwrap().caller_client_id, 11);
    }

    #[test]
    fn destination_disconnect_removes_only_its_pending_calls() {
        let mut calls = BTreeMap::new();
        calls.insert(1, pending(10, 30));
        calls.insert(2, pending(11, 30));
        calls.insert(3, pending(12, 31));

        let failed = remove_pending_calls_for_disconnected_client(&mut calls, 30);

        assert_eq!(failed.len(), 2);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls.get(&3).unwrap().destination_client_id, 31);
    }

    #[test]
    fn self_call_disconnect_is_removed_as_destination_failure() {
        let mut calls = BTreeMap::new();
        calls.insert(1, pending(30, 30));

        let failed = remove_pending_calls_for_disconnected_client(&mut calls, 30);

        assert!(failed.is_empty());
        assert!(calls.is_empty());
    }
}
