//! Coordinate group exit with synchronous driver transactions whose kernel
//! stacks own hardware locks. Admission and closing share a single atomic so
//! a new transaction cannot slip between the exit check and the drain.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sync::Waker;

const CLOSING: usize = 1 << (usize::BITS - 1);

pub(super) struct GroupExit {
    state: AtomicUsize,
    drained: Waker,
}

impl GroupExit {
    pub(super) fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
            drained: Waker::new_uninterruptible("thread_group_device_drain"),
        }
    }

    pub(super) fn started(&self) -> bool {
        self.state.load(Ordering::Acquire) & CLOSING != 0
    }

    /// Elect the teardown owner and stop admitting new transactions.
    pub(super) fn start(&self) -> bool {
        self.state.fetch_or(CLOSING, Ordering::AcqRel) & CLOSING == 0
    }

    pub(super) fn wait_for_transactions(&self) {
        while self.state.load(Ordering::Acquire) != CLOSING {
            let task = super::mytask().expect("draining device transactions requires a task");
            self.drained
                .wait_with_condition(task.get_id(), task.get_trapframe(), None, 0, || {
                    self.state.load(Ordering::Acquire) == CLOSING
                });
        }
    }

    fn admit(&self) -> Result<(), &'static str> {
        self.state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state < CLOSING - 1).then(|| state + 1)
            })
            .map_err(|_| "process is exiting")?;
        Ok(())
    }

    fn finish(&self) {
        if self.state.fetch_sub(1, Ordering::AcqRel) == CLOSING + 1 {
            self.drained.wake_all();
        }
    }
}

#[must_use]
pub(crate) struct DeviceTransaction(super::CurrentTaskRef);

impl Drop for DeviceTransaction {
    fn drop(&mut self) {
        self.0.group_exit.finish();
    }
}

/// Keep a userspace caller alive until a bounded synchronous device operation
/// releases its driver guards. Kernel workers already own their lifetime.
/// Do not use around waits for other userspace tasks or async completion tickets.
pub(crate) fn device_transaction() -> Result<Option<DeviceTransaction>, &'static str> {
    let Some(task) = super::mytask().filter(|task| task.task_type == super::TaskType::User) else {
        return Ok(None);
    };
    task.group_exit.admit()?;
    Ok(Some(DeviceTransaction(task)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn closing_a_group_drains_admitted_transactions_and_rejects_new_ones() {
        let group = GroupExit::new();
        group.admit().unwrap();
        group.admit().unwrap();
        assert!(group.start());
        assert!(!group.start());
        assert!(group.admit().is_err());
        group.finish();
        assert_eq!(group.state.load(Ordering::Acquire), CLOSING + 1);
        group.finish();
        group.wait_for_transactions();
        assert!(group.admit().is_err());
    }
}
