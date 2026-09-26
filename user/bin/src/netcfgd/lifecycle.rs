//! Incarnation gate shared by discovery and asynchronous DHCP completion.
use scarlet_abi::network::NetworkLinkInfoV1;

pub fn current_ready_link(expected: &NetworkLinkInfoV1, snapshot: &[NetworkLinkInfoV1]) -> bool {
    snapshot
        .iter()
        .any(|link| link.is_up() && link.same_incarnation(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_abi::network::*;
    fn link(id: u64, generation: u64, state: u8) -> NetworkLinkInfoV1 {
        NetworkLinkInfoV1 {
            id,
            generation,
            state,
            ..Default::default()
        }
    }
    #[test]
    fn disconnect_reconnect_and_name_reuse_reject_old_completions() {
        let first = link(1, 2, LINK_UP);
        assert!(current_ready_link(&first, &[first]));
        assert!(!current_ready_link(&first, &[link(1, 3, LINK_DOWN)]));
        assert!(!current_ready_link(&first, &[link(1, 4, LINK_UP)]));
        assert!(!current_ready_link(&first, &[link(2, 2, LINK_UP)]));
        assert!(!current_ready_link(&first, &[]));
    }
    #[test]
    fn another_interfaces_change_does_not_cancel_this_lease() {
        let wired = link(1, 2, LINK_UP);
        let wifi = link(2, 9, LINK_DOWN);
        assert!(current_ready_link(&wired, &[wired, wifi]));
        assert!(!current_ready_link(&wifi, &[wired, wifi]));
    }
}
