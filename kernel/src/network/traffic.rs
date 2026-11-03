//! Native traffic accounting and admission budgets. A budget is an explicitly
//! shared object, independent of an ABI, interface name or process namespace.
//! Queued packets retain reservations; failed/cancelled sends return credit.
use crate::sync::IrqSpinLock;
use alloc::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Receive,
    Transmit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counter {
    pub packets: u64,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub received: Counter,
    pub transmitted: Counter,
    pub denied: Counter,
    pub reserved_bytes: u64,
    /// Number of armed thresholds crossed by committed traffic.
    pub alerts: u64,
}

#[derive(Default)]
struct State {
    snapshot: Snapshot,
    committed: u64,
    limit: Option<u64>,
    alert_at: Option<u64>,
}

pub struct Budget {
    state: IrqSpinLock<State>,
}

impl core::fmt::Debug for Budget {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TrafficBudget").finish_non_exhaustive()
    }
}

impl Budget {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: IrqSpinLock::new(State::default()),
        })
    }

    /// Replace future admission and alert thresholds relative to bytes already
    /// committed. In-flight reservations remain charged, including across a
    /// reconfiguration; reducing a limit does not revoke accepted packets.
    /// Validate the entire update before changing either threshold.
    pub fn configure(
        &self,
        remaining: Option<u64>,
        alert_after: Option<u64>,
    ) -> Result<(), &'static str> {
        let mut state = self.state.lock();
        let absolute = |value: Option<u64>| {
            value
                .map(|n| {
                    state
                        .committed
                        .checked_add(n)
                        .ok_or("traffic threshold overflow")
                })
                .transpose()
        };
        let limit = absolute(remaining)?;
        let alert_at = absolute(alert_after)?;
        state.limit = limit;
        state.alert_at = alert_at;
        Self::check_alert(&mut state);
        Ok(())
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().snapshot
    }

    pub fn reserve(
        self: &Arc<Self>,
        direction: Direction,
        bytes: usize,
    ) -> Result<Reservation, &'static str> {
        let bytes = u64::try_from(bytes).map_err(|_| "traffic size overflow")?;
        if bytes == 0 {
            return Err("empty traffic reservation");
        }
        let mut state = self.state.lock();
        let used = state
            .committed
            .checked_add(state.snapshot.reserved_bytes)
            .and_then(|n| n.checked_add(bytes));
        if used.is_none() || state.limit.is_some_and(|limit| used.unwrap() > limit) {
            state.snapshot.denied.packets = state.snapshot.denied.packets.saturating_add(1);
            state.snapshot.denied.bytes = state.snapshot.denied.bytes.saturating_add(bytes);
            return Err("traffic budget exhausted");
        }
        state.snapshot.reserved_bytes += bytes;
        Ok(Reservation {
            budget: Some(self.clone()),
            direction,
            bytes,
        })
    }

    fn check_alert(state: &mut State) {
        if state
            .alert_at
            .is_some_and(|threshold| state.committed >= threshold)
        {
            state.alert_at = None;
            state.snapshot.alerts = state.snapshot.alerts.saturating_add(1);
        }
    }
}

/// Unique ownership prevents double accounting when a packet is deferred for
/// address resolution. Dropping any unsent packet releases its reservation.
#[derive(Debug)]
pub struct Reservation {
    budget: Option<Arc<Budget>>,
    direction: Direction,
    bytes: u64,
}

impl Reservation {
    /// Call only after the next owning transport accepts the packet, or after
    /// admitting an incoming packet to its destination.
    pub fn commit(mut self) {
        let budget = self.budget.take().expect("owned traffic reservation");
        let mut state = budget.state.lock();
        state.snapshot.reserved_bytes -= self.bytes;
        state.committed += self.bytes;
        let counter = match self.direction {
            Direction::Receive => &mut state.snapshot.received,
            Direction::Transmit => &mut state.snapshot.transmitted,
        };
        counter.packets = counter.packets.saturating_add(1);
        counter.bytes = counter.bytes.saturating_add(self.bytes);
        Budget::check_alert(&mut state);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(budget) = self.budget.take() {
            budget.state.lock().snapshot.reserved_bytes -= self.bytes;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn queued_packets_cannot_oversubscribe_and_cancellation_refunds_credit() {
        let budget = Budget::new();
        budget.configure(Some(100), Some(70)).unwrap();
        let queued = budget.reserve(Direction::Transmit, 60).unwrap();
        assert!(budget.reserve(Direction::Receive, 50).is_err());
        assert_eq!(budget.snapshot().transmitted.bytes, 0);
        assert_eq!(budget.snapshot().alerts, 0);
        drop(queued);
        budget.reserve(Direction::Receive, 70).unwrap().commit();
        budget.reserve(Direction::Transmit, 30).unwrap().commit();
        assert!(budget.reserve(Direction::Transmit, 1).is_err());
        assert_eq!(
            budget.snapshot(),
            Snapshot {
                received: Counter {
                    packets: 1,
                    bytes: 70
                },
                transmitted: Counter {
                    packets: 1,
                    bytes: 30
                },
                denied: Counter {
                    packets: 2,
                    bytes: 51
                },
                reserved_bytes: 0,
                alerts: 1,
            }
        );
    }
    #[test_case]
    fn updates_keep_in_flight_charges_and_failure_leaves_the_policy_unchanged() {
        let budget = Budget::new();
        let queued = budget.reserve(Direction::Transmit, 60).unwrap();
        budget.configure(Some(40), Some(20)).unwrap();
        assert!(budget.reserve(Direction::Receive, 1).is_err());
        queued.commit();
        assert_eq!(budget.snapshot().alerts, 1);
        budget.configure(Some(40), Some(20)).unwrap();
        assert!(budget.configure(None, Some(u64::MAX)).is_err());
        budget.reserve(Direction::Transmit, 40).unwrap().commit();
        assert_eq!(budget.snapshot().alerts, 2);
        assert!(budget.reserve(Direction::Transmit, 1).is_err());
        let independent = Budget::new();
        independent
            .reserve(Direction::Transmit, 100)
            .unwrap()
            .commit();
        assert_eq!(independent.snapshot().transmitted.bytes, 100);
        assert_eq!(budget.snapshot().transmitted.bytes, 100);
    }
}
