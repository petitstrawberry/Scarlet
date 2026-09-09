//! Coherent RTT estimation and publication of the resulting timeout.
//!
//! Sampling and estimation update related fields under one short lock. The
//! native 64-bit timeout snapshot avoids that lock. Clock reads, packet I/O,
//! timer operations and callbacks all happen outside this module's state lock.

use super::{TcpSocket, backed_off_retransmission_timeout_ns, is_seq_acknowledged};
use crate::sync::IrqSpinLock;
#[cfg(target_has_atomic = "64")]
use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy)]
struct Measurement {
    sequence: u32,
    sent_at_ns: u64,
}

struct RttState {
    srtt_x8_ns: u64,
    variation_x4_ns: u64,
    timeout_ns: u64,
    measurement: Option<Measurement>,
}

impl RttState {
    const fn new() -> Self {
        Self {
            srtt_x8_ns: 0,
            variation_x4_ns: 0,
            timeout_ns: TcpSocket::INITIAL_RTO_NS,
            measurement: None,
        }
    }

    fn observe(&mut self, sample_ns: u64) {
        if self.srtt_x8_ns == 0 {
            self.srtt_x8_ns = sample_ns.saturating_mul(8);
            self.variation_x4_ns = sample_ns.saturating_mul(2);
        } else {
            let difference = (self.srtt_x8_ns >> 3).abs_diff(sample_ns);
            self.variation_x4_ns =
                (self.variation_x4_ns.saturating_mul(3) >> 2).saturating_add(difference);
            self.srtt_x8_ns = (self.srtt_x8_ns.saturating_mul(7) >> 3).saturating_add(sample_ns);
        }
        self.timeout_ns = (self.srtt_x8_ns >> 3)
            .saturating_add(
                (self.variation_x4_ns >> 2)
                    .saturating_mul(4)
                    .max(TcpSocket::MIN_RTO_NS),
            )
            .clamp(TcpSocket::MIN_RTO_NS, TcpSocket::MAX_RTO_NS);
    }
}

pub(super) struct RetransmissionTiming {
    state: IrqSpinLock<RttState>,
    #[cfg(target_has_atomic = "64")]
    published_timeout_ns: AtomicU64,
}

impl RetransmissionTiming {
    pub const fn new() -> Self {
        Self {
            state: IrqSpinLock::new(RttState::new()),
            #[cfg(target_has_atomic = "64")]
            published_timeout_ns: AtomicU64::new(TcpSocket::INITIAL_RTO_NS),
        }
    }

    // Called while holding state, so competing updates publish in the same order.
    fn publish(&self, _state: &RttState) {
        #[cfg(target_has_atomic = "64")]
        self.published_timeout_ns
            .store(_state.timeout_ns, Ordering::Relaxed);
    }

    pub fn timeout_ns(&self) -> u64 {
        #[cfg(target_has_atomic = "64")]
        {
            self.published_timeout_ns.load(Ordering::Relaxed)
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.state.lock().timeout_ns
        }
    }

    pub fn reset(&self) {
        let mut state = self.state.lock();
        *state = RttState::new();
        self.publish(&state);
    }

    pub fn start_measurement(&self, sequence: u32, sent_at_ns: u64) {
        let mut state = self.state.lock();
        if state.measurement.is_none() {
            state.measurement = Some(Measurement {
                sequence,
                sent_at_ns,
            });
        }
    }

    /// Consume a matching sample once, including sequence-number wraparound.
    pub fn acknowledge(&self, ack_sequence: u32, now_ns: u64) -> bool {
        let mut state = self.state.lock();
        let Some(measurement) = state.measurement else {
            return false;
        };
        if !is_seq_acknowledged(measurement.sequence, ack_sequence) {
            return false;
        }
        state.measurement = None;
        if now_ns > measurement.sent_at_ns {
            state.observe(now_ns - measurement.sent_at_ns);
            self.publish(&state);
        }
        true
    }

    pub fn cancel_measurement(&self) {
        self.state.lock().measurement = None;
    }

    pub fn backoff(&self) {
        let mut state = self.state.lock();
        // Karn's rule applies to the same transition as retransmission backoff.
        state.measurement = None;
        state.timeout_ns = backed_off_retransmission_timeout_ns(state.timeout_ns);
        self.publish(&state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn rtt_measurements_preserve_wide_time_and_acknowledge_once() {
        let timing = RetransmissionTiming::new();
        let start = 0x1_0000_0000;
        timing.start_measurement(u32::MAX - 1, start);
        timing.start_measurement(100, start + 1_000_000_000);
        assert!(!timing.acknowledge(u32::MAX - 2, start + 1));
        assert_eq!(timing.timeout_ns(), TcpSocket::INITIAL_RTO_NS);
        assert!(timing.acknowledge(1, start + 2_000_000_000));
        assert_eq!(timing.timeout_ns(), 6_000_000_000);
        assert!(!timing.acknowledge(1, start + 10_000_000_000));
        assert_eq!(timing.timeout_ns(), 6_000_000_000);
    }

    #[test_case]
    fn retransmission_and_reset_discard_obsolete_rtt_samples() {
        let timing = RetransmissionTiming::new();
        timing.start_measurement(10, 0x1_0000_0000);
        timing.backoff();
        assert_eq!(timing.timeout_ns(), 2 * TcpSocket::INITIAL_RTO_NS);
        assert!(!timing.acknowledge(11, 0x2_0000_0000));
        for _ in 0..16 {
            timing.backoff();
        }
        assert_eq!(timing.timeout_ns(), TcpSocket::MAX_RTO_NS);
        timing.start_measurement(20, 0);
        timing.reset();
        assert!(!timing.acknowledge(21, 100));
        assert_eq!(timing.timeout_ns(), TcpSocket::INITIAL_RTO_NS);
    }
}
