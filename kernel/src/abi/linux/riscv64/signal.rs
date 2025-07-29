//! Linux RISC-V 64 signal syscalls (stub)
//!
//! Implements rt_sigprocmask and rt_sigaction as stubs that always return 0.

use alloc::task;

use crate::abi::linux::riscv64::LinuxRiscv64Abi;
use crate::arch::Trapframe;
use crate::task::mytask;

/// Linux rt_sigaction system call implementation
/// 
/// Currently, this is a stub that does nothing and always returns 0.
pub fn sys_rt_sigaction(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    0
}

/// Linux rt_sigprocmask system call implementation
/// 
/// Currently, this is a stub that does nothing and always returns 0.
pub fn sys_rt_sigprocmask(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    0
}

