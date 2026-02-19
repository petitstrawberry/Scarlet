//! Hypervisor system calls

use core::sync::atomic::compiler_fence;

use crate::arch::Trapframe;
use crate::hypervisor::types::VcpuExit;
use crate::hypervisor::vm::GLOBAL_VM_MANAGER;
use crate::object::KernelObject;
use crate::object::handle::HandleMetadata;
use crate::println;
use crate::task::mytask;

pub const SYSCALL_SHV_VM_CREATE: usize = 1100;
pub const SYSCALL_SHV_VCPU_CREATE: usize = 1101;
pub const SYSCALL_SHV_VCPU_RUN: usize = 1102;

/// Create a new VM and return a handle to it.
/// Returns handle number on success, usize::MAX on error.
pub fn sys_shv_vm_create(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Create a new VM via the global VM manager
    let vm = match GLOBAL_VM_MANAGER.create_vm() {
        Ok(vm) => vm,
        Err(_) => return usize::MAX,
    };

    // Insert VM into handle table with appropriate metadata
    let metadata = HandleMetadata::default();
    match task
        .handle_table
        .insert_with_metadata(KernelObject::HypervisorVm(vm), metadata)
    {
        Ok(handle) => handle as usize,
        Err(_) => usize::MAX,
    }
}

/// Create a vCPU on an existing VM.
/// a0 = vm_handle, a1 = vcpu_id
/// Returns vcpu_handle on success, usize::MAX on error.
pub fn sys_shv_vcpu_create(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let vm_handle = trapframe.get_arg(0) as u32;
    let vcpu_id = trapframe.get_arg(1) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Get the VM object from the handle table
    let vm = match task.handle_table.get(vm_handle) {
        Some(KernelObject::HypervisorVm(vm)) => vm,
        _ => return usize::MAX,
    };

    // Create a new vCPU on the VM
    let vcpu = match vm.create_vcpu(vcpu_id) {
        Ok(vcpu) => vcpu,
        Err(_) => return usize::MAX,
    };

    // Insert vCPU into handle table with appropriate metadata
    let metadata = HandleMetadata::default();
    match task
        .handle_table
        .insert_with_metadata(KernelObject::HypervisorVcpu(vcpu), metadata)
    {
        Ok(handle) => handle as usize,
        Err(_) => usize::MAX,
    }
}

/// Run vCPU until exit.
/// a0 = vcpu_handle, a1 = exit_ptr (userspace pointer to VcpuExit)
/// Returns 0 on success, usize::MAX on error.
pub fn sys_shv_vcpu_run(trapframe: &mut Trapframe) -> usize {
    // crate::early_println!("[sys_shv_vcpu_run] called, handle={}", trapframe.get_arg(0));

    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let vcpu_handle = trapframe.get_arg(0) as u32;
    let exit_ptr = trapframe.get_arg(1);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);
    compiler_fence(core::sync::atomic::Ordering::SeqCst);

    let exit_size = core::mem::size_of::<VcpuExit>();
    let exit_size_minus_one = match exit_size.checked_sub(1) {
        Some(size) => size,
        None => return usize::MAX,
    };
    let exit_end = match exit_ptr.checked_add(exit_size_minus_one) {
        Some(end) => end,
        None => return usize::MAX,
    };
    let exit_map = match task.vm_manager.search_memory_map(exit_ptr) {
        Some(map) => map,
        None => return usize::MAX,
    };
    if exit_end > exit_map.vmarea.end {
        return usize::MAX;
    }

    // Translate the exit pointer to kernel address
    let exit_kaddr = match task.vm_manager.translate_vaddr(exit_ptr) {
        Some(addr) => addr,
        None => return usize::MAX,
    };

    // Get the vCPU object from the handle table
    let vcpu = match task.handle_table.get(vcpu_handle) {
        Some(KernelObject::HypervisorVcpu(vcpu)) => vcpu,
        _ => return usize::MAX,
    };

    // crate::early_println!("[sys_shv_vcpu_run] calling vcpu.run()");

    // println!("[sys_shv_vcpu_run] before vcpu.run(), trapframe_addr={:x}, trapframe={:?}", trapframe as *const _ as usize, trapframe);

    // Run the vCPU
    let vm_exit = match vcpu.run() {
        Ok(exit) => exit,
        Err(e) => {
            crate::early_println!("[sys_shv_vcpu_run] vcpu.run() failed: {}", e);
            return usize::MAX;
        }
    };

    // println!("[sys_shv_vcpu_run] after vcpu.run(), vm_exit={:?}", vm_exit);
    // println!("[sys_shv_vcpu_run] trapframe_addr={:x}, trapframe={:?}", trapframe as *const _ as usize, trapframe);

    // Convert VmExit to VcpuExit and write to user space
    let vcpu_exit = VcpuExit::from_vmexit(&vm_exit);
    unsafe {
        core::ptr::write(exit_kaddr as *mut VcpuExit, vcpu_exit);
    }

    0
}
