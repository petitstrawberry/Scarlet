//! SMMUv2 fault handling at an exclusively owned, unrouted bank boundary.
//! ARM IHI0062D.c 3.7.3, 5.1, 16.5.9: FORMAT/SS are not W1C fault bits.
//! This is never an active-submission recovery or diagnostic acknowledgement.

pub(super) const STALLED: u32 = 1 << 30;
pub(super) const W1C_FAULTS: u32 = 0x8000_01fe;

pub(super) const fn pending(status: u32) -> bool {
    status & (W1C_FAULTS | STALLED) != 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReuseError {
    Routed,
    Disable,
    Synchronize,
    Stalled,
    Acknowledge,
}

pub(super) trait BankIo {
    fn routed(&self) -> bool;
    fn disable(&self) -> bool;
    fn synchronize_disabled(&self) -> bool;
    fn status(&self) -> u32;
    fn record(&self, status: u32);
    fn acknowledge(&self, bits: u32);
    fn barrier(&self);
}

/// Caller holds exclusive software ownership and the routing lock. Failure
/// leaves that ownership reserved; no configuration restore/reuse is allowed.
pub(super) fn prepare_reuse(io: &impl BankIo) -> Result<(), ReuseError> {
    if io.routed() {
        return Err(ReuseError::Routed);
    }
    if !io.disable() {
        return Err(ReuseError::Disable);
    }
    let mut status = io.status();
    if pending(status) {
        io.record(status);
    }
    if status & STALLED != 0 {
        // W1C cannot clear SS. Never resume an unknown former owner's DMA.
        return Err(ReuseError::Stalled);
    }
    if !io.synchronize_disabled() {
        return Err(ReuseError::Synchronize);
    }
    // Disabling translation plus synchronized invalidation drains speculative
    // table walks too. Preserve a fault that arrived while draining them.
    let settled = io.status();
    if settled != status && pending(settled) {
        io.record(settled);
    }
    status = settled;
    if status & STALLED != 0 {
        return Err(ReuseError::Stalled);
    }
    if status & W1C_FAULTS != 0 {
        io.acknowledge(status & W1C_FAULTS);
        io.barrier();
        let remaining = io.status();
        if pending(remaining) {
            io.record(remaining);
            return Err(ReuseError::Acknowledge);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{vec, vec::Vec};
    use core::cell::RefCell;

    struct Fake {
        routed: bool,
        disabled: bool,
        synced: bool,
        reads: RefCell<Vec<u32>>,
        events: RefCell<Vec<(&'static str, u32)>>,
    }
    impl Fake {
        fn new(reads: &[u32]) -> Self {
            Self { routed: false, disabled: true, synced: true,
                reads: RefCell::new(reads.to_vec()), events: RefCell::new(Vec::new()) }
        }
        fn event(&self, name: &'static str, value: u32) {
            self.events.borrow_mut().push((name, value));
        }
    }
    impl BankIo for Fake {
        fn routed(&self) -> bool { self.routed }
        fn disable(&self) -> bool { self.event("disable", 0); self.disabled }
        fn synchronize_disabled(&self) -> bool { self.event("sync", 0); self.synced }
        fn status(&self) -> u32 {
            let mut reads = self.reads.borrow_mut();
            let value = reads[0];
            if reads.len() > 1 { reads.remove(0); }
            self.event("read", value); value
        }
        fn record(&self, value: u32) { self.event("record", value); }
        fn acknowledge(&self, value: u32) { self.event("ack", value); }
        fn barrier(&self) { self.event("barrier", 0); }
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn format_only_is_not_a_fault() {
        for status in [0, 0x200, 0x400, 0x600] {
            let io = Fake::new(&[status]);
            assert!(!pending(status));
            assert_eq!(prepare_reuse(&io), Ok(()));
            assert_eq!(*io.events.borrow(), vec![("disable", 0), ("read", status), ("sync", 0), ("read", status)]);
        }
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn records_original_fault_before_w1c_and_checks_readback() {
        let io = Fake::new(&[0x80000402, 0x80000402, 0x400]);
        assert_eq!(prepare_reuse(&io), Ok(()));
        assert_eq!(*io.events.borrow(), vec![
            ("disable", 0), ("read", 0x80000402), ("record", 0x80000402),
            ("sync", 0), ("read", 0x80000402), ("ack", 0x80000002), ("barrier", 0), ("read", 0x400),
        ]);
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn stalled_bank_is_not_acknowledged_or_resumed() {
        let io = Fake::new(&[STALLED | 0x402]);
        assert_eq!(prepare_reuse(&io), Err(ReuseError::Stalled));
        assert_eq!(io.events.borrow().last(), Some(&("record", STALLED | 0x402)));
        assert!(!io.events.borrow().iter().any(|e| e.0 == "ack"));
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn failed_or_new_fault_after_acknowledgement_prevents_reuse() {
        for remaining in [0x402, STALLED | 0x400, 0x408] {
            let io = Fake::new(&[0x402, 0x402, remaining]);
            assert_eq!(prepare_reuse(&io), Err(ReuseError::Acknowledge));
            assert_eq!(io.events.borrow().last(), Some(&("record", remaining)));
        }
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn ownership_disable_and_sync_failures_never_acknowledge() {
        for (routed, disabled, synced, expected) in [
            (true, true, true, ReuseError::Routed),
            (false, false, true, ReuseError::Disable),
            (false, true, false, ReuseError::Synchronize),
        ] {
            let io = Fake { routed, disabled, synced, ..Fake::new(&[0x402]) };
            assert_eq!(prepare_reuse(&io), Err(expected));
            assert!(!io.events.borrow().iter().any(|e| e.0 == "ack"));
            if routed || !disabled {
                assert!(!io.events.borrow().iter().any(|e| e.0 == "read"));
            }
        }
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn fault_arriving_during_disabled_sync_is_recorded_before_clear() {
        let io = Fake::new(&[0x400, 0x408, 0x400]);
        assert_eq!(prepare_reuse(&io), Ok(()));
        assert_eq!(*io.events.borrow(), vec![
            ("disable", 0), ("read", 0x400), ("sync", 0), ("read", 0x408),
            ("record", 0x408), ("ack", 8), ("barrier", 0), ("read", 0x400),
        ]);
    }
}
