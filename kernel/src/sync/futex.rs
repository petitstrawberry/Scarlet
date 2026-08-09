//! Scarlet Native private futex operations.
//!
//! The initial ABI intentionally supports process-private 32-bit futex words.
//! Threads in one process share a key made from their thread-group ID and the
//! userspace virtual address. Shared-memory futexes across processes require a
//! physical/backing-object key and are not part of this interface yet.

extern crate alloc;

use crate::arch::Trapframe;
use crate::sync::{Mutex, Once, Waker};
use crate::task::mytask;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};

const FUTEX_VALUE_CHANGED: usize = 1;
const FUTEX_TIMED_OUT: usize = 2;
const FUTEX_WAIT_FOREVER: usize = usize::MAX;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FutexKey {
    thread_group_id: usize,
    address: usize,
}

static FUTEX_WAITERS: Once<Mutex<BTreeMap<FutexKey, Arc<Waker>>>> = Once::new();

fn futex_waiters() -> &'static Mutex<BTreeMap<FutexKey, Arc<Waker>>> {
    FUTEX_WAITERS.call_once(|| Mutex::new(BTreeMap::new()))
}

fn futex_key(task: &crate::task::Task, address: usize) -> Option<FutexKey> {
    if address == 0 || !address.is_multiple_of(core::mem::align_of::<u32>()) {
        return None;
    }
    Some(FutexKey {
        thread_group_id: task.get_thread_group_id(),
        address,
    })
}

fn read_futex_value(task: &crate::task::Task, address: usize) -> Option<u32> {
    let kernel_address = task.vm_manager.translate_to_kva(address)?;
    if !kernel_address.is_multiple_of(core::mem::align_of::<u32>()) {
        return None;
    }

    // SAFETY: `translate_to_kva` verified that the caller's futex word is
    // mapped and readable. The ABI requires an `AtomicU32`, 32-bit alignment,
    // and a mapping that remains alive while a thread can wait on it.
    let value = unsafe { &*(kernel_address as *const AtomicU32) };
    Some(value.load(Ordering::Acquire))
}

fn waiter_for_key(key: FutexKey) -> Arc<Waker> {
    futex_waiters()
        .lock()
        .entry(key)
        .or_insert_with(|| Arc::new(Waker::new_uninterruptible("futex_wait")))
        .clone()
}

fn remove_unused_waiter(key: FutexKey) {
    let mut waiters = futex_waiters().lock();
    let remove = waiters
        .get(&key)
        .is_some_and(|waker| Arc::strong_count(waker) == 1 && waker.is_empty());
    if remove {
        waiters.remove(&key);
    }
}

/// Remove stale futex queue entries when a task exits permanently.
///
/// # Arguments
///
/// * `task_id` - Global task ID being torn down.
/// * `thread_group_id` - Process-private futex namespace of the task.
pub(crate) fn remove_task_waiter(task_id: usize, thread_group_id: usize) {
    let mut waiters = futex_waiters().lock();
    waiters.retain(|key, waker| {
        if key.thread_group_id == thread_group_id {
            let _ = waker.remove_terminated_task(task_id);
        }
        !waker.is_empty() || Arc::strong_count(waker) > 1
    });
}

/// Wait while a process-private 32-bit futex word equals an expected value.
///
/// Spurious wakeups are permitted. Userspace must re-check its atomic state in
/// a loop. Registering the keyed waker before the second value check closes the
/// unlock-before-wait race without polling a software timer.
///
/// # Arguments
///
/// * `trapframe.arg(0)` - Aligned userspace address of the `u32` futex word.
/// * `trapframe.arg(1)` - Expected 32-bit value.
/// * `trapframe.arg(2)` - Relative timeout in nanoseconds, or `usize::MAX`
///   to wait without a timeout.
///
/// # Returns
///
/// `0` after a wake, `1` when the word had already changed, `2` after a
/// timeout, or `usize::MAX` for an invalid address.
pub fn sys_futex_wait(trapframe: &mut Trapframe) -> usize {
    let Some(task) = mytask() else {
        return usize::MAX;
    };
    let address = trapframe.get_arg(0);
    let expected = trapframe.get_arg(1) as u32;
    let timeout_ns = trapframe.get_arg(2);
    trapframe.increment_pc_next(&task);

    let Some(key) = futex_key(&task, address) else {
        return usize::MAX;
    };
    let waiter = waiter_for_key(key);

    if read_futex_value(&task, address) != Some(expected) {
        drop(waiter);
        remove_unused_waiter(key);
        return FUTEX_VALUE_CHANGED;
    }

    let task_id = task.get_id();
    drop(task);
    let woken = if timeout_ns == FUTEX_WAIT_FOREVER {
        waiter.wait_owned(task_id, trapframe);
        true
    } else {
        waiter.wait_with_timeout_owned(task_id, trapframe, timeout_ns as u64)
    };
    remove_unused_waiter(key);
    if woken { 0 } else { FUTEX_TIMED_OUT }
}

/// Wake threads waiting on a process-private futex word.
///
/// # Arguments
///
/// * `trapframe.arg(0)` - Aligned userspace address of the `u32` futex word.
/// * `trapframe.arg(1)` - Maximum number of waiters to wake. Values greater
///   than one currently request a broadcast wake.
///
/// # Returns
///
/// Number of tasks made runnable, or `usize::MAX` for an invalid address.
pub fn sys_futex_wake(trapframe: &mut Trapframe) -> usize {
    let Some(task) = mytask() else {
        return usize::MAX;
    };
    let address = trapframe.get_arg(0);
    let max_count = trapframe.get_arg(1);
    trapframe.increment_pc_next(&task);

    let Some(key) = futex_key(&task, address) else {
        return usize::MAX;
    };
    if max_count == 0 {
        return 0;
    }

    let waiter = futex_waiters().lock().get(&key).cloned();
    let Some(waiter) = waiter else {
        return 0;
    };
    let woken = if max_count == 1 {
        usize::from(waiter.wake_one())
    } else {
        waiter.wake_all().min(max_count)
    };
    drop(waiter);
    remove_unused_waiter(key);
    woken
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn futex_keys_are_process_private() {
        let address = 0x4000;
        assert_ne!(
            FutexKey {
                thread_group_id: 1,
                address,
            },
            FutexKey {
                thread_group_id: 2,
                address,
            }
        );
    }
}
