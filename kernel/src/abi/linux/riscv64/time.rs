//! Time-related system calls for Linux ABI on RISC-V 64
//! 
//! This module implements Linux time system calls for the Scarlet kernel,
//! providing compatibility with Linux userspace programs that need time information.

use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi, 
    arch::Trapframe, 
    time::{current_time, current_time_s},
    task::mytask
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

#[cfg(test)]
mod tests {
    use super::*;
    
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
