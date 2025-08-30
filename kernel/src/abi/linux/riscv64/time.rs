//! Time-related system calls for Linux ABI on RISC-V 64
//! 
//! This module implements Linux time system calls for the Scarlet kernel,
//! providing compatibility with Linux userspace programs that need time information.

use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi, 
    arch::Trapframe, 
    time::{current_time, current_time_s},
    task::mytask,
    timer::ns_to_ticks,
};

/// Linux timespec structure (matches Linux userspace)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TimeSpec {
    pub tv_sec: i64,    // seconds
    pub tv_nsec: i64,   // nanoseconds
}

/// Linux clock IDs (subset of commonly used ones)
pub const CLOCK_REALTIME: i32 = 0;
pub const CLOCK_MONOTONIC: i32 = 1;
pub const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
pub const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
pub const CLOCK_MONOTONIC_RAW: i32 = 4;
pub const CLOCK_REALTIME_COARSE: i32 = 5;
pub const CLOCK_MONOTONIC_COARSE: i32 = 6;
pub const CLOCK_BOOTTIME: i32 = 7;

/// sys_clock_gettime - Get time from specified clock
/// 
/// Arguments:
/// - a0 (x10): clock_id - which clock to read from
/// - a1 (x11): timespec - pointer to timespec structure to fill
/// 
/// Returns:
/// - 0 on success
/// - -EINVAL (-22) for invalid clock_id
/// - -EFAULT (-14) for invalid timespec pointer
pub fn sys_clock_gettime(
    _abi: &mut LinuxRiscv64Abi, 
    trapframe: &mut Trapframe
) -> usize {
    let task = mytask().expect("No current task found");
    let clock_id = trapframe.get_arg(0) as i32;  // a0

    trapframe.increment_pc_next(&task);

    let timespec_ptr = match task.vm_manager.translate_vaddr(trapframe.get_arg(1)) {
        Some(ptr) => ptr as *mut TimeSpec,  // a1
        None => return (-14_isize) as usize, // -EFAULT
    };
    
    // Get the current time based on the clock type
    let timespec = match clock_id {
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE => {
            // For now, we use the same monotonic time for realtime
            // In a full implementation, this would be adjusted to Unix epoch
            let time_us = current_time();
            TimeSpec {
                tv_sec: (time_us / 1_000_000) as i64,
                tv_nsec: ((time_us % 1_000_000) * 1000) as i64,
            }
        },
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_MONOTONIC_COARSE => {
            // Monotonic time since boot
            let time_us = current_time();
            TimeSpec {
                tv_sec: (time_us / 1_000_000) as i64,
                tv_nsec: ((time_us % 1_000_000) * 1000) as i64,
            }
        },
        CLOCK_BOOTTIME => {
            // Boot time (same as monotonic for now)
            let time_us = current_time();
            TimeSpec {
                tv_sec: (time_us / 1_000_000) as i64,
                tv_nsec: ((time_us % 1_000_000) * 1000) as i64,
            }
        },
        CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            // CPU time for process/thread (simplified implementation)
            // In a full implementation, this would track actual CPU time
            let time_us = current_time();
            TimeSpec {
                tv_sec: (time_us / 1_000_000) as i64,
                tv_nsec: ((time_us % 1_000_000) * 1000) as i64,
            }
        },
        _ => {
            return (-22_isize) as usize; // -EINVAL
        }
    };
    
    // Write the timespec to user space
    unsafe {
        *timespec_ptr = timespec;
    }
    
    0 // Success
}

/// sys_nanosleep - Sleep for the specified time (Linux ABI)
///
/// Arguments:
/// - a0 (x10): rqtp - pointer to requested sleep time (struct __kernel_timespec __user *)
/// - a1 (x11): rmtp - pointer to remaining time (struct __kernel_timespec __user *)
///
/// Returns:
/// - 0 on success
/// - -EFAULT (-14) for invalid pointer
/// - -EINTR (-4) if interrupted by signal (not implemented, always 0)
pub fn sys_nanosleep(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    // Get current task
    let task = match mytask() {
        Some(task) => task,
        None => return (-14_isize) as usize, // -EFAULT
    };
    trapframe.increment_pc_next(&task);

    // Get user pointer to requested timespec
    let rqtp_ptr = trapframe.get_arg(0);
    let rmtp_ptr = trapframe.get_arg(1);
    let rqtp = match task.vm_manager.translate_vaddr(rqtp_ptr) {
        Some(ptr) => unsafe { &*(ptr as *const TimeSpec) },
        None => return (-14_isize) as usize, // -EFAULT
    };
    // Convert timespec to nanoseconds
    let ns = rqtp.tv_sec.saturating_mul(1_000_000_000).saturating_add(rqtp.tv_nsec);
    if ns <= 0 {
        return 0;
    }
    // Convert nanoseconds to kernel ticks
    let ticks = ns_to_ticks(ns as u64);
    trapframe.set_return_value(0); // Set return value to 0 (success)
    // Sleep the current task for the specified ticks
    task.sleep(trapframe, ticks);
    // If sleep is successful, this will not be reached. If interrupted, return -EINTR (not implemented)
    0
}

/// Linux sys_clock_getres implementation (stub)
///
/// Get clock resolution. This is a stub implementation that
/// returns a reasonable resolution for the specified clock.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: clk_id (clock ID)
///   - arg1: res (pointer to timespec structure for resolution)
///
/// Returns:
/// - 0 on success
/// - usize::MAX on error
pub fn sys_clock_getres(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let _clk_id = trapframe.get_arg(0) as i32;
    let res_ptr = trapframe.get_arg(1);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // If res pointer is provided, write resolution
    if res_ptr != 0 {
        if let Some(res_paddr) = task.vm_manager.translate_vaddr(res_ptr) {
            unsafe {
                // Write timespec structure with nanosecond resolution
                // struct timespec { long tv_sec; long tv_nsec; }
                let timespec = res_paddr as *mut [u64; 2];
                *timespec = [
                    0,         // tv_sec = 0
                    1_000_000, // tv_nsec = 1 millisecond (reasonable resolution)
                ];
            }
        }
    }

    0 // Always succeed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::Trapframe;
    use crate::abi::linux::riscv64::LinuxRiscv64Abi;
    use crate::task::mytask;

    #[test_case]
    fn test_timespec_size() {
        // Ensure TimeSpec matches Linux ABI
        assert_eq!(core::mem::size_of::<TimeSpec>(), 16);
        assert_eq!(core::mem::align_of::<TimeSpec>(), 8);
    }
    
    #[test_case] 
    fn test_clock_constants() {
        // Verify clock constants match Linux values
        assert_eq!(CLOCK_REALTIME, 0);
        assert_eq!(CLOCK_MONOTONIC, 1);
        assert_eq!(CLOCK_PROCESS_CPUTIME_ID, 2);
        assert_eq!(CLOCK_THREAD_CPUTIME_ID, 3);
    }
}
