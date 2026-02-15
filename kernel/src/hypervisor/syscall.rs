//! Hypervisor system calls

use crate::arch::Trapframe;
use crate::hypervisor::types::{VcpuExit, VmExit};
use crate::hypervisor::vcpu::VcpuObject;
use crate::hypervisor::vm::GLOBAL_VM_MANAGER;
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

/// Run vCPU until exit.
/// a0 = vm_id, a1 = vcpu_id, a2 = exit_ptr (userspace pointer to VcpuExit)
pub fn sys_vcpu_run(trapframe: &mut Trapframe) -> usize {
    let vm_id = trapframe.get_arg(0) as u32;
    let vcpu_id = trapframe.get_arg(1) as u32;
    let exit_ptr = trapframe.get_arg(2);

    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let exit_kaddr = match task.vm_manager.translate_vaddr(exit_ptr) {
        Some(addr) => addr,
        None => return usize::MAX,
    };

    let vm = match GLOBAL_VM_MANAGER.get_vm_by_id(vm_id) {
        Some(vm) => vm,
        None => return usize::MAX,
    };

    let vcpu = match vm.get_vcpu(vcpu_id) {
        Some(v) => v,
        None => return usize::MAX,
    };

    let vm_exit = match vcpu.run() {
        Ok(exit) => exit,
        Err(_) => return usize::MAX,
    };

    let vcpu_exit = VcpuExit::from_vmexit(&vm_exit);
    unsafe {
        core::ptr::write(exit_kaddr as *mut VcpuExit, vcpu_exit);
    }

    trapframe.set_return_value(0);
    0
}
