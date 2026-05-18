use crate::abi::linux::generic::LinuxAbi;
use crate::arch::Trapframe;
use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    vec::Vec,
};
use spin::{Mutex, Once};

// Minimal FUTEX op codes (match Linux)
const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;
// Extended ops commonly used by musl
const FUTEX_WAIT_BITSET: u32 = 9;
const FUTEX_WAKE_BITSET: u32 = 10;
const FUTEX_CMD_MASK: u32 = 0x3f; // per Linux uapi

struct FutexQueue {
    waiters: Mutex<VecDeque<usize>>,
}

impl FutexQueue {
    const fn new() -> Self {
        Self {
            waiters: Mutex::new(VecDeque::new()),
        }
    }
}

// Global registry of futex wait queues keyed by user address.
static FUTEX_QUEUES: Once<Mutex<BTreeMap<usize, &'static FutexQueue>>> = Once::new();

fn init_futex_queues() -> Mutex<BTreeMap<usize, &'static FutexQueue>> {
    Mutex::new(BTreeMap::new())
}

fn get_futex_queue(uaddr: usize) -> &'static FutexQueue {
    let futex_map_mutex = FUTEX_QUEUES.call_once(init_futex_queues);
    let mut map = futex_map_mutex.lock();
    *map.entry(uaddr)
        .or_insert_with(|| Box::leak(Box::new(FutexQueue::new())))
}

/// Wake up to `max` waiters on a futex at `uaddr`.
pub fn wake_address(uaddr: usize, max: usize) -> usize {
    if max == 0 {
        return 0;
    }

    let queue = get_futex_queue(uaddr);
    let task_ids = {
        let mut waiters = queue.waiters.lock();
        let limit = max.min(waiters.len());
        let mut task_ids = Vec::with_capacity(limit);
        for _ in 0..limit {
            if let Some(task_id) = waiters.pop_front() {
                task_ids.push(task_id);
            }
        }
        task_ids
    };

    let mut woken = 0;
    for task_id in task_ids {
        if crate::sched::scheduler::wake_task(task_id) {
            woken += 1;
        }
    }
    woken
}

/// Linux futex syscall (minimal implementation: WAIT/WAKE)
pub fn sys_futex(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match crate::task::mytask() {
        Some(t) => t,
        None => return super::errno::to_result(super::errno::EPERM),
    };

    // args: uaddr, op, val, timeout, uaddr2, val3
    let uaddr = trapframe.get_arg(0) as usize;
    let op_raw = trapframe.get_arg(1) as u32;
    let val = trapframe.get_arg(2) as i32;
    let _timeout = trapframe.get_arg(3) as usize; // TODO: implement timeout
    let _uaddr2 = trapframe.get_arg(4) as usize;
    let _val3 = trapframe.get_arg(5) as u32; // e.g., bitset for *_BITSET ops

    // Always advance PC to avoid re-executing syscall on resume
    trapframe.increment_pc_next(task);

    if uaddr & 0x3 != 0 {
        return super::errno::to_result(super::errno::EINVAL);
    }

    let cmd = op_raw & FUTEX_CMD_MASK; // strip PRIVATE/CLOCK_REALTIME flags
    match cmd {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            let tid = task.get_id();
            let queue = get_futex_queue(uaddr);

            {
                let mut waiters = queue.waiters.lock();

                // Validate and compare while holding the futex queue lock. This
                // preserves futex semantics without remembering wakeups: a wake
                // racing after the compare either sees this waiter or the waiter
                // observes the changed value and returns EAGAIN.
                let paddr = match task.vm_manager.translate_to_kva(uaddr) {
                    Some(pa) => pa,
                    None => return super::errno::to_result(super::errno::EFAULT),
                };
                let cur_val = unsafe { core::ptr::read_volatile(paddr as *const i32) };
                if cur_val != val {
                    return super::errno::to_result(super::errno::EAGAIN);
                }

                task.set_state(crate::task::TaskState::Blocked(
                    crate::task::BlockedType::Interruptible,
                ));
                crate::sched::scheduler::mark_blocked(tid);
                if !waiters.contains(&tid) {
                    waiters.push_back(tid);
                }
            }

            crate::sched::scheduler::schedule(trapframe);
            // When resumed, report success (Linux may report -EINTR if interrupted; TBD)
            0
        }
        FUTEX_WAKE | FUTEX_WAKE_BITSET => {
            let max = if val < 0 { 0 } else { val as usize };
            let woken = wake_address(uaddr, max);
            // Return number of woken tasks
            woken
        }
        _ => {
            // Not implemented ops
            super::errno::to_result(super::errno::ENOSYS)
        }
    }
}
