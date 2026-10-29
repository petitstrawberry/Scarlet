//! Ordered, ABI-neutral observations of registered device endpoints.
use crate::{
    device::DeviceType,
    sync::{IrqSpinLock, waker::Waker},
};
use alloc::{
    collections::VecDeque,
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub id: usize,
    pub name: String,
    pub kind: DeviceType,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Added,
    Removed,
    Observed,
}
#[derive(Clone, Debug)]
pub struct Observation {
    pub sequence: u64,
    pub action: Action,
    pub endpoint: Endpoint,
}
impl Observation {
    fn cost(&self) -> usize {
        128 + self.endpoint.name.len()
    }
}
struct Queue {
    records: VecDeque<Observation>,
    bytes: usize,
    capacity: usize,
    overflow: bool,
}
pub struct Subscription {
    queue: IrqSpinLock<Queue>,
}
impl Subscription {
    pub fn resize(&self, capacity: usize) -> Result<(), ()> {
        if !(256..=64 * 1024 * 1024).contains(&capacity) {
            return Err(());
        }
        let mut queue = self.queue.lock();
        queue.capacity = capacity;
        while queue.bytes > capacity {
            if let Some(record) = queue.records.pop_front() {
                queue.bytes -= record.cost();
                queue.overflow = true;
            } else {
                break;
            }
        }
        Ok(())
    }
    pub fn capacity(&self) -> usize {
        self.queue.lock().capacity
    }
    pub fn pending(&self) -> bool {
        let queue = self.queue.lock();
        queue.overflow || !queue.records.is_empty()
    }
    pub fn next(&self) -> Result<Option<Observation>, ()> {
        let mut queue = self.queue.lock();
        if queue.overflow {
            queue.overflow = false;
            return Err(());
        }
        let record = queue.records.pop_front();
        if let Some(record) = &record {
            queue.bytes -= record.cost();
        }
        Ok(record)
    }
}
struct State {
    next: u64,
    subscribers: Vec<Weak<Subscription>>,
}
/// Consumers have independent bounded queues; overflow is reported explicitly.
/// Registry mutations publish under their registry locks and wake afterwards.
pub struct Journal {
    state: IrqSpinLock<State>,
    pub(crate) changed: Waker,
}
impl Journal {
    pub const fn new() -> Self {
        Self {
            state: IrqSpinLock::new(State {
                next: 1,
                subscribers: Vec::new(),
            }),
            changed: Waker::new_interruptible("device_lifecycle"),
        }
    }
    pub fn cursor(&self) -> u64 {
        self.state.lock().next
    }
    pub fn subscribe(&self) -> Arc<Subscription> {
        let subscription = Arc::new(Subscription {
            queue: IrqSpinLock::new(Queue {
                records: VecDeque::new(),
                bytes: 0,
                capacity: 64 * 1024,
                overflow: false,
            }),
        });
        let mut state = self.state.lock();
        state.subscribers.retain(|weak| weak.strong_count() != 0);
        state.subscribers.push(Arc::downgrade(&subscription));
        subscription
    }
    pub(crate) fn publish(&self, action: Action, endpoint: Endpoint) {
        let mut state = self.state.lock();
        let sequence = state.next;
        state.next = sequence
            .checked_add(1)
            .expect("device observation sequence exhausted");
        let record = Observation {
            sequence,
            action,
            endpoint,
        };
        let cost = record.cost();
        state.subscribers.retain(|weak| {
            let Some(subscriber) = weak.upgrade() else {
                return false;
            };
            let mut queue = subscriber.queue.lock();
            if cost > queue.capacity.saturating_sub(queue.bytes) {
                queue.overflow = true;
            } else {
                queue.bytes += cost;
                queue.records.push_back(record.clone());
            }
            true
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn observers_are_independent_and_overflow_is_explicit() {
        let journal = Journal::new();
        let first = journal.subscribe();
        let second = journal.subscribe();
        first.resize(256).unwrap();
        let endpoint = Endpoint {
            id: 1,
            name: "real".into(),
            kind: DeviceType::Char,
        };
        journal.publish(Action::Added, endpoint.clone());
        journal.publish(Action::Removed, endpoint.clone());
        assert!(first.next().is_err());
        assert_eq!(first.next().unwrap().unwrap().action, Action::Added);
        assert_eq!(second.next().unwrap().unwrap().action, Action::Added);
        assert_eq!(second.next().unwrap().unwrap().action, Action::Removed);
        assert!(!second.pending());
    }
}
