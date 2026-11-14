//! Native FIFO priorities and scoped, independently revocable donations.

use crate::sync::{IrqSpinLock, sequence::IdSequence};
use alloc::{collections::BTreeMap, sync::Arc};

const MAX_DONATIONS: usize = 64;
static TOKENS: IdSequence = IdSequence::new();
static TICKETS: IdSequence = IdSequence::new();

struct State {
    base: u8,
    donations: BTreeMap<u64, u8>,
    ticket: u64,
    yielding: bool,
}

pub(crate) struct FixedPriorityState(IrqSpinLock<State>);

impl FixedPriorityState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self(IrqSpinLock::new(State {
            base: 0,
            donations: BTreeMap::new(),
            ticket: 0,
            yielding: false,
        })))
    }
    pub fn base(&self) -> u8 {
        self.0.lock().base
    }
    pub fn effective(&self) -> u8 {
        let state = self.0.lock();
        state
            .donations
            .values()
            .copied()
            .max()
            .unwrap_or(0)
            .max(state.base)
    }
    pub(crate) fn configure(&self, priority: u8) {
        let mut state = self.0.lock();
        if state.base != priority {
            state.base = priority;
            state.ticket = 0;
        }
    }
    pub(crate) fn enqueue_ticket(&self, fresh: bool) -> u64 {
        let mut state = self.0.lock();
        if fresh || state.ticket == 0 {
            state.ticket = TICKETS
                .reserve()
                .expect("FIFO ordering identities exhausted")
                .get();
        }
        state.ticket
    }
    pub(crate) fn request_yield(&self) {
        self.0.lock().yielding = true;
    }
    pub(crate) fn take_yield(&self) -> bool {
        let mut state = self.0.lock();
        let yielded = core::mem::take(&mut state.yielding);
        if yielded {
            state.ticket = 0;
        }
        yielded
    }
}

/// Dropping one donation preserves every other donor and the configured base.
/// The state outlives the lease, but the lease never keeps a task alive.
pub struct PriorityLease {
    state: Arc<FixedPriorityState>,
    task_id: usize,
    token: u64,
}

fn refresh(state: &Arc<FixedPriorityState>, task_id: usize, previous: u8) {
    if let Some(task) = crate::sched::scheduler::get_task_by_id(task_id)
        && Arc::ptr_eq(&task.fixed_priority, state)
    {
        crate::sched::scheduler::fixed_priority_changed(&task, previous);
    }
}

impl PriorityLease {
    pub fn enter(task: &super::Task, priority: u8) -> Result<Option<Self>, ()> {
        if priority == 0 {
            return Ok(None);
        }
        let previous = task.fixed_priority.effective();
        let token = TOKENS.reserve().ok_or(())?.get();
        {
            let mut state = task.fixed_priority.0.lock();
            if state.donations.len() >= MAX_DONATIONS {
                return Err(());
            }
            state.donations.insert(token, priority);
        }
        if task.fixed_priority.effective() != previous {
            refresh(
                &task.fixed_priority,
                task.registered_id().unwrap_or(0),
                previous,
            );
        }
        Ok(Some(Self {
            state: task.fixed_priority.clone(),
            task_id: task.registered_id().unwrap_or(0),
            token,
        }))
    }
}

impl Drop for PriorityLease {
    fn drop(&mut self) {
        let previous = self.state.effective();
        self.state.0.lock().donations.remove(&self.token);
        if self.state.effective() != previous {
            refresh(&self.state, self.task_id, previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn independent_donations_restore_updated_base_and_preserve_fifo_order() {
        let mut task =
            crate::task::Task::new("priority-donations".into(), 0, crate::task::TaskType::User);
        task.set_id(1);
        task.fixed_priority.configure(3);
        let first = PriorityLease::enter(&task, 7).unwrap();
        let second = PriorityLease::enter(&task, 5).unwrap();
        assert_eq!(task.fixed_priority.effective(), 7);
        task.fixed_priority.configure(4);
        drop(first);
        assert_eq!(task.fixed_priority.effective(), 5);
        drop(second);
        assert_eq!(task.fixed_priority.effective(), 4);
        let initial = task.fixed_priority.enqueue_ticket(true);
        assert_eq!(task.fixed_priority.enqueue_ticket(false), initial);
        task.fixed_priority.request_yield();
        assert!(task.fixed_priority.take_yield());
        assert!(task.fixed_priority.enqueue_ticket(false) > initial);
        let mut leases = alloc::vec::Vec::new();
        for _ in 0..MAX_DONATIONS {
            leases.push(PriorityLease::enter(&task, 9).unwrap());
        }
        assert!(PriorityLease::enter(&task, 10).is_err());
        drop(leases);
        assert_eq!(task.fixed_priority.effective(), 4);
    }
}
