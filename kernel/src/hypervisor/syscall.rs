//! Hypervisor system calls

use crate::arch::Trapframe;
use crate::task::mytask;

pub const SYSCALL_HYPERVISOR_VM_CREATE: usize = 1100;
pub const SYSCALL_HYPERVISOR_VCPU_CREATE: usize = 1101;
pub const SYSCALL_VCPU_RUN: usize = 1102;

pub fn sys_hypervisor_vm_create(_trapframe: &mut Trapframe) -> usize {
    let _task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };
    usize::MAX
}

pub fn sys_hypervisor_vcpu_create(_trapframe: &mut Trapframe) -> usize {
    let _task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };
    usize::MAX
}

pub fn sys_vcpu_run(_trapframe: &mut Trapframe) -> usize {
    let _task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };
    usize::MAX
}
