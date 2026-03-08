use crate::abi::linux::riscv64::LinuxRiscv64Abi;
use crate::arch::Trapframe;
use crate::sync::waker::Waker;
use alloc::collections::BTreeMap;
use spin::{Mutex, Once};

// Minimal FUTEX op codes (match Linux)
const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;
// Extended ops commonly used by musl
const FUTEX_WAIT_BITSET: u32 = 9;
const FUTEX_WAKE_BITSET: u32 = 10;
const FUTEX_CMD_MASK: u32 = 0x3f; // per Linux uapi

// Global registry of futex wakers keyed by user address
static FUTEX_WAKERS: Once<Mutex<BTreeMap<usize, Waker>>> = Once::new();

fn init_futex_wakers() -> Mutex<BTreeMap<usize, Waker>> {
    Mutex::new(BTreeMap::new())
}

fn get_futex_waker(uaddr: usize) -> &'static Waker {
    let futex_map_mutex = FUTEX_WAKERS.call_once(init_futex_wakers);
    let mut map = futex_map_mutex.lock();
    if !map.contains_key(&uaddr) {
        // Leak a static name for diagnostics
        let name = alloc::format!("futex_{:#x}", uaddr);
        let static_name = alloc::boxed::Box::leak(name.into_boxed_str());
        map.insert(uaddr, Waker::new_interruptible(static_name));
    }
    unsafe {
        let ptr = map.get(&uaddr).unwrap() as *const Waker;
        &*ptr
    }
}

/// Wake up to `max` waiters on a futex at `uaddr`.
pub fn wake_address(uaddr: usize, max: usize) -> usize {
    let waker = get_futex_waker(uaddr);
    if max == 0 {
        return 0;
    }
    let mut woken = 0;
    if max == usize::MAX {
        // treat as wake_all
        woken = waker.wake_all();
    } else {
        for _ in 0..max {
            if waker.wake_one() {
                woken += 1;
            } else {
                break;
            }
        }
    }
    woken
}

/// Linux futex syscall (minimal implementation: WAIT/WAKE)
pub fn sys_futex(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
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

    let cmd = op_raw & FUTEX_CMD_MASK; // strip PRIVATE/other flags
    match cmd {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            // Validate user address and expected value
            let paddr = match task.vm_manager.translate_to_kva(uaddr) {
                Some(pa) => pa,
                None => return super::errno::to_result(super::errno::EFAULT),
            };
            let cur_val = unsafe { *(paddr as *const i32) };
            if cur_val != val {
                // Expected value mismatch -> EAGAIN (common benign fast-path)
                return super::errno::to_result(super::errno::EAGAIN);
            }

            // Block current task on futex queue
            let waker = get_futex_waker(uaddr);
            let tid = task.get_id();
            waker.wait(tid, trapframe);
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
