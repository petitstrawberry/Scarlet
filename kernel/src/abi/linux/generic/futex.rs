use crate::abi::linux::generic::LinuxAbi;
use crate::arch::Trapframe;
use crate::sync::{IrqSpinLock, Once};
use crate::timer::{TimerHandle, TimerHandler, add_timer, get_time_ns};
use alloc::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    vec::Vec,
};
use core::sync::atomic::{AtomicU8, Ordering};

// Minimal FUTEX op codes (match Linux)
const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;
// Extended ops commonly used by musl
const FUTEX_WAIT_BITSET: u32 = 9;
const FUTEX_WAKE_BITSET: u32 = 10;
const FUTEX_CMD_MASK: u32 = 0x3f; // per Linux uapi
const FUTEX_PRIVATE_FLAG: u32 = 0x80;
const FUTEX_CLOCK_REALTIME: u32 = 0x100;
const FUTEX_BITSET_MATCH_ANY: u32 = u32::MAX;
const NSEC_PER_SEC: i64 = 1_000_000_000;

const WAIT_PENDING: u8 = 0;
const WAIT_WAKE_IN_PROGRESS: u8 = 1;
const WAIT_TIMEOUT_IN_PROGRESS: u8 = 2;
const WAIT_WAKE_COMPLETE: u8 = 3;
const WAIT_TIMEOUT_COMPLETE: u8 = 4;
const WAIT_INTERRUPTED: u8 = 5;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum FutexKey {
    Private {
        thread_group_id: usize,
        address: usize,
    },
    Shared {
        physical_address: usize,
    },
}

struct FutexWaitOutcome {
    state: AtomicU8,
}

impl FutexWaitOutcome {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(WAIT_PENDING),
        }
    }

    fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) == WAIT_PENDING
    }

    fn try_wake(&self) -> bool {
        self.state
            .compare_exchange(
                WAIT_PENDING,
                WAIT_WAKE_IN_PROGRESS,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn complete_wake(&self) {
        debug_assert_eq!(self.state.load(Ordering::Acquire), WAIT_WAKE_IN_PROGRESS);
        self.state.store(WAIT_WAKE_COMPLETE, Ordering::Release);
    }

    fn try_timeout(&self) -> bool {
        self.state
            .compare_exchange(
                WAIT_PENDING,
                WAIT_TIMEOUT_IN_PROGRESS,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn complete_timeout(&self) {
        debug_assert_eq!(self.state.load(Ordering::Acquire), WAIT_TIMEOUT_IN_PROGRESS);
        self.state.store(WAIT_TIMEOUT_COMPLETE, Ordering::Release);
    }

    fn finish_after_resume(&self) -> FutexWaitResult {
        loop {
            match self.state.load(Ordering::Acquire) {
                WAIT_PENDING => {
                    let _ = self.state.compare_exchange(
                        WAIT_PENDING,
                        WAIT_INTERRUPTED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
                WAIT_WAKE_IN_PROGRESS | WAIT_TIMEOUT_IN_PROGRESS => core::hint::spin_loop(),
                WAIT_WAKE_COMPLETE => return FutexWaitResult::Woken,
                WAIT_TIMEOUT_COMPLETE => return FutexWaitResult::TimedOut,
                WAIT_INTERRUPTED => return FutexWaitResult::Interrupted,
                _ => unreachable!("invalid futex wait outcome"),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FutexWaitResult {
    Woken,
    TimedOut,
    Interrupted,
}

#[derive(Clone)]
struct FutexWaiter {
    task_id: usize,
    bitset: u32,
    outcome: Arc<FutexWaitOutcome>,
}

struct FutexQueue {
    waiters: IrqSpinLock<VecDeque<FutexWaiter>>,
}

impl FutexQueue {
    const fn new() -> Self {
        Self {
            waiters: IrqSpinLock::new(VecDeque::new()),
        }
    }
}

// Private futexes are scoped to one Linux thread group. Shared futexes are
// keyed by the translated backing address so different virtual mappings of
// the same shared page rendezvous on the same queue.
static FUTEX_QUEUES: Once<IrqSpinLock<BTreeMap<FutexKey, Arc<FutexQueue>>>> = Once::new();

fn init_futex_queues() -> IrqSpinLock<BTreeMap<FutexKey, Arc<FutexQueue>>> {
    IrqSpinLock::new(BTreeMap::new())
}

fn futex_key(task: &crate::task::Task, uaddr: usize, private: bool) -> Option<FutexKey> {
    if private {
        Some(FutexKey::Private {
            thread_group_id: task.get_thread_group_id(),
            address: uaddr,
        })
    } else {
        task.vm_manager
            .translate_to_phys(uaddr)
            .map(|physical_address| FutexKey::Shared { physical_address })
    }
}

fn get_futex_queue(key: FutexKey) -> Arc<FutexQueue> {
    let futex_map_mutex = FUTEX_QUEUES.call_once(init_futex_queues);
    let mut map = futex_map_mutex.lock();
    map.entry(key)
        .or_insert_with(|| Arc::new(FutexQueue::new()))
        .clone()
}

fn remove_unused_queue(key: FutexKey) {
    let futex_map_mutex = FUTEX_QUEUES.call_once(init_futex_queues);
    let mut map = futex_map_mutex.lock();
    let remove = map
        .get(&key)
        .is_some_and(|queue| Arc::strong_count(queue) == 1 && queue.waiters.lock().is_empty());
    if remove {
        map.remove(&key);
    }
}

fn remove_waiter(key: FutexKey, task_id: usize, outcome: &Arc<FutexWaitOutcome>) {
    let queue = {
        let map = FUTEX_QUEUES.call_once(init_futex_queues).lock();
        map.get(&key).cloned()
    };
    let Some(queue) = queue else {
        return;
    };
    queue
        .waiters
        .lock()
        .retain(|waiter| waiter.task_id != task_id || !Arc::ptr_eq(&waiter.outcome, outcome));
    drop(queue);
    remove_unused_queue(key);
}

fn wake_key(key: FutexKey, max: usize, wake_bitset: u32) -> usize {
    if max == 0 {
        return 0;
    }

    let queue = {
        let map = FUTEX_QUEUES.call_once(init_futex_queues).lock();
        map.get(&key).cloned()
    };
    let Some(queue) = queue else {
        return 0;
    };
    let selected = {
        let mut waiters = queue.waiters.lock();
        let mut selected = Vec::with_capacity(max.min(waiters.len()));
        let mut retained = VecDeque::with_capacity(waiters.len());
        while let Some(waiter) = waiters.pop_front() {
            if selected.len() < max && waiter.bitset & wake_bitset != 0 && waiter.outcome.try_wake()
            {
                selected.push(waiter);
            } else if waiter.outcome.is_pending() {
                retained.push_back(waiter);
            }
        }
        *waiters = retained;
        selected
    };

    for waiter in &selected {
        let _ = crate::sched::scheduler::wake_task(waiter.task_id);
        waiter.outcome.complete_wake();
    }
    let woken = selected.len();
    drop(queue);
    remove_unused_queue(key);
    woken
}

struct FutexTimeoutHandler {
    task_id: usize,
    key: FutexKey,
    outcome: Arc<FutexWaitOutcome>,
    timer_handle: IrqSpinLock<Option<TimerHandle>>,
}

impl TimerHandler for FutexTimeoutHandler {
    fn on_timer_expired(self: Arc<Self>, _context: usize) {
        let timeout_won = self.outcome.try_timeout();
        if timeout_won {
            remove_waiter(self.key, self.task_id, &self.outcome);
        }
        if let Some(task) = crate::sched::scheduler::get_task_by_id(self.task_id)
            && let Some(timer_handle) = self.timer_handle.lock().take()
        {
            task.finish_software_timer(timer_handle);
        }
        if timeout_won {
            let _ = crate::sched::scheduler::wake_task(self.task_id);
            self.outcome.complete_timeout();
        }
    }
}

fn read_timeout_ns(task: &crate::task::Task, userspace_address: usize) -> Result<u64, usize> {
    let mut bytes = [0_u8; core::mem::size_of::<i64>() * 2];
    crate::library::std::usercopy::copy_from_user(task, userspace_address, &mut bytes)
        .map_err(|_| super::errno::EFAULT)?;
    let seconds = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
    let nanoseconds = i64::from_ne_bytes(bytes[8..].try_into().unwrap());
    if seconds < 0 || !(0..NSEC_PER_SEC).contains(&nanoseconds) {
        return Err(super::errno::EINVAL);
    }
    Ok((seconds as u128)
        .saturating_mul(NSEC_PER_SEC as u128)
        .saturating_add(nanoseconds as u128)
        .min(u64::MAX as u128) as u64)
}

fn timeout_deadline(
    task: &crate::task::Task,
    cmd: u32,
    op_raw: u32,
    timeout_address: usize,
) -> Result<Option<u64>, usize> {
    if timeout_address == 0 {
        return Ok(None);
    }

    let timeout_ns = read_timeout_ns(task, timeout_address)?;
    let monotonic_now = get_time_ns();
    if cmd == FUTEX_WAIT {
        if op_raw & FUTEX_CLOCK_REALTIME != 0 {
            return Err(super::errno::ENOSYS);
        }
        return Ok(Some(monotonic_now.saturating_add(timeout_ns)));
    }

    if op_raw & FUTEX_CLOCK_REALTIME != 0 {
        let realtime_now = crate::time::system_time_ns().unwrap_or(monotonic_now);
        Ok(Some(
            monotonic_now.saturating_add(timeout_ns.saturating_sub(realtime_now)),
        ))
    } else {
        Ok(Some(timeout_ns))
    }
}

/// Wake futex waiters associated with a task-owned userspace address.
///
/// Linux clear-child-tid cleanup does not carry the original private/shared
/// futex flag. Try the process-private key first and then the shared backing
/// key so either form of join waiter can make progress.
///
/// # Arguments
///
/// * `task` - Task whose address space contains `uaddr`.
/// * `uaddr` - Aligned userspace futex-word address.
/// * `max` - Maximum number of waiters to wake.
///
/// # Returns
///
/// Number of tasks made runnable.
pub fn wake_task_address(task: &crate::task::Task, uaddr: usize, max: usize) -> usize {
    if max == 0 || uaddr & 0x3 != 0 {
        return 0;
    }

    let mut woken = 0;
    if let Some(private_key) = futex_key(task, uaddr, true) {
        woken += wake_key(private_key, max, FUTEX_BITSET_MATCH_ANY);
    }
    if woken < max
        && let Some(shared_key) = futex_key(task, uaddr, false)
    {
        woken += wake_key(shared_key, max - woken, FUTEX_BITSET_MATCH_ANY);
    }
    woken
}

/// Remove every Linux futex registration owned by a terminating task.
///
/// # Arguments
///
/// * `task_id` - Global task ID being removed.
pub(crate) fn remove_task_waiter(task_id: usize) {
    let futex_map_mutex = FUTEX_QUEUES.call_once(init_futex_queues);
    let mut map = futex_map_mutex.lock();
    map.retain(|_, queue| {
        let mut waiters = queue.waiters.lock();
        waiters.retain(|waiter| waiter.task_id != task_id);
        let retain_queue = !waiters.is_empty() || Arc::strong_count(queue) > 1;
        drop(waiters);
        retain_queue
    });
}

/// Handle Linux futex wait and wake operations.
///
/// # Arguments
///
/// * `_abi` - Linux ABI state for the calling task.
/// * `trapframe` - Register state containing the Linux futex arguments.
///
/// # Returns
///
/// Zero after a successful wait, the number of woken tasks after a wake, or a
/// negative Linux errno encoded as `usize`.
pub fn sys_futex(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match crate::task::mytask() {
        Some(t) => t,
        None => return super::errno::to_result(super::errno::EPERM),
    };

    // args: uaddr, op, val, timeout, uaddr2, val3
    let uaddr = trapframe.get_arg(0) as usize;
    let op_raw = trapframe.get_arg(1) as u32;
    let val = trapframe.get_arg(2) as i32;
    let timeout = trapframe.get_arg(3) as usize;
    let _uaddr2 = trapframe.get_arg(4) as usize;
    let val3 = trapframe.get_arg(5) as u32;

    // Always advance PC to avoid re-executing syscall on resume
    trapframe.increment_pc_next(&task);

    if uaddr & 0x3 != 0 {
        return super::errno::to_result(super::errno::EINVAL);
    }

    let cmd = op_raw & FUTEX_CMD_MASK; // strip PRIVATE/CLOCK_REALTIME flags
    let private = op_raw & FUTEX_PRIVATE_FLAG != 0;
    let key = match futex_key(&task, uaddr, private) {
        Some(key) => key,
        None => return super::errno::to_result(super::errno::EFAULT),
    };
    match cmd {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            let wait_bitset = if cmd == FUTEX_WAIT_BITSET {
                if val3 == 0 {
                    return super::errno::to_result(super::errno::EINVAL);
                }
                val3
            } else {
                FUTEX_BITSET_MATCH_ANY
            };
            let deadline = match timeout_deadline(&task, cmd, op_raw, timeout) {
                Ok(deadline) => deadline,
                Err(error) => return super::errno::to_result(error),
            };
            let tid = task.get_id();
            let queue = get_futex_queue(key);
            let outcome = Arc::new(FutexWaitOutcome::new());

            let wait_error = {
                let mut waiters = queue.waiters.lock();

                // Validate and compare while holding the futex queue lock. This
                // preserves futex semantics without remembering wakeups: a wake
                // racing after the compare either sees this waiter or the waiter
                // observes the changed value and returns EAGAIN.
                let paddr = match task.vm_manager.translate_to_kva(uaddr) {
                    Some(pa) => pa,
                    None => {
                        drop(waiters);
                        drop(queue);
                        remove_unused_queue(key);
                        return super::errno::to_result(super::errno::EFAULT);
                    }
                };
                let cur_val = unsafe { core::ptr::read_volatile(paddr as *const i32) };
                if cur_val != val {
                    Some(super::errno::EAGAIN)
                } else {
                    task.set_state(crate::task::TaskState::Blocked(
                        crate::task::BlockedType::Interruptible,
                    ));
                    crate::sched::scheduler::mark_blocked(tid);
                    waiters.push_back(FutexWaiter {
                        task_id: tid,
                        bitset: wait_bitset,
                        outcome: outcome.clone(),
                    });
                    None
                }
            };
            if let Some(error) = wait_error {
                drop(queue);
                remove_unused_queue(key);
                return super::errno::to_result(error);
            }

            let timer_handle = deadline.map(|deadline| {
                let handler = Arc::new(FutexTimeoutHandler {
                    task_id: tid,
                    key,
                    outcome: outcome.clone(),
                    timer_handle: IrqSpinLock::new(None),
                });
                let handler_ref: Arc<dyn TimerHandler> = handler.clone();
                let timer_handle = add_timer(
                    deadline,
                    crate::timer::TimerPrecision::Normal,
                    &handler_ref,
                    0,
                );
                *handler.timer_handle.lock() = Some(timer_handle);
                task.register_software_timer(timer_handle, handler_ref);
                timer_handle
            });

            crate::sched::scheduler::schedule(trapframe);
            let result = outcome.finish_after_resume();
            // A signal or process-control wake can resume the task without a
            // matching FUTEX_WAKE. Remove this exact registration so a later
            // wait at the same address cannot consume it.
            queue
                .waiters
                .lock()
                .retain(|waiter| waiter.task_id != tid || !Arc::ptr_eq(&waiter.outcome, &outcome));
            if let Some(timer_handle) = timer_handle {
                let _ = task.finish_software_timer(timer_handle);
            }
            drop(queue);
            remove_unused_queue(key);
            match result {
                FutexWaitResult::Woken => 0,
                FutexWaitResult::TimedOut => super::errno::to_result(super::errno::ETIMEDOUT),
                FutexWaitResult::Interrupted => super::errno::to_result(super::errno::EINTR),
            }
        }
        FUTEX_WAKE | FUTEX_WAKE_BITSET => {
            if val < 0 {
                return super::errno::to_result(super::errno::EINVAL);
            }
            let wake_bitset = if cmd == FUTEX_WAKE_BITSET {
                if val3 == 0 {
                    return super::errno::to_result(super::errno::EINVAL);
                }
                val3
            } else {
                FUTEX_BITSET_MATCH_ANY
            };
            let woken = wake_key(key, val as usize, wake_bitset);
            // Return number of woken tasks
            woken
        }
        _ => {
            // Not implemented ops
            super::errno::to_result(super::errno::ENOSYS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn private_futex_keys_are_isolated_by_thread_group() {
        let first = FutexKey::Private {
            thread_group_id: 10,
            address: 0x4000,
        };
        let second = FutexKey::Private {
            thread_group_id: 11,
            address: 0x4000,
        };
        assert_ne!(first, second);
    }

    #[test_case]
    fn shared_futex_keys_match_the_backing_address() {
        let first = FutexKey::Shared {
            physical_address: 0x8000,
        };
        let second = FutexKey::Shared {
            physical_address: 0x8000,
        };
        assert_eq!(first, second);
    }
}
