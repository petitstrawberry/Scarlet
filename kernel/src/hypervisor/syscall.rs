//! Hypervisor system calls (Scarlet Native API)
//!
//! Provides syscalls for creating VMs and vCPUs. Once a VM or vCPU handle is
//! obtained, further operations are performed through `HandleControl` (syscall 110)
//! which dispatches to `ControlOps` implementations on `VmObject` and `VcpuObject`.
//!
//! ## Syscalls
//!
//! - `HypervisorVmCreate` (1100): Create a new VM, returns a handle.
//! - `HypervisorVcpuCreate` (1101): Create a vCPU on an existing VM, returns a handle.
//!   - arg0 = VM handle
//!   - arg1 = vCPU ID
//!
//! ## VM ControlOps commands (via HandleControl on VM handle)
//!
//! - `SCTL_VM_SET_MEMORY_REGION` (0x01): Set memory region, arg = pointer to `ScarletVmMemoryRegion`.
//! - `SCTL_VM_GET_VCPU_COUNT` (0x02): Get number of vCPUs. Returns count.
//!
//! ## vCPU ControlOps commands (via HandleControl on vCPU handle)
//!
//! - `SCTL_VCPU_RUN` (0x01): Run vCPU, arg = pointer to `ScarletVcpuRunResult`. Returns 0.
//! - `SCTL_VCPU_GET_REGS` (0x02): Get registers, arg = pointer to `ScarletVcpuRegisters`. Returns 0.
//! - `SCTL_VCPU_SET_REGS` (0x03): Set registers, arg = pointer to `ScarletVcpuRegisters`. Returns 0.
//! - `SCTL_VCPU_GET_ONE_REG` (0x04): Get single register, arg = register index. Returns value.
//! - `SCTL_VCPU_SET_ONE_REG` (0x05): Set single register, arg = ptr to `ScarletVcpuOneReg`.

use crate::arch::Trapframe;
use crate::task::mytask;

/// sys_hypervisor_vm_create - Create a new virtual machine
///
/// # Arguments
///
/// None (no arguments from trapframe are used)
///
/// # Returns
///
/// * Handle number on success
/// * `usize::MAX` on error
#[cfg(feature = "hypervisor")]
pub fn sys_hypervisor_vm_create(trapframe: &mut Trapframe) -> usize {
    use crate::hypervisor::VmObject;
    use crate::object::KernelObject;

    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    trapframe.increment_pc_next(task);

    let vm = match VmObject::new() {
        Ok(vm) => vm,
        Err(_) => return usize::MAX,
    };

    let kernel_obj = KernelObject::HypervisorVm(vm);
    match task.handle_table.insert(kernel_obj) {
        Ok(handle) => handle as usize,
        Err(_) => usize::MAX,
    }
}

/// sys_hypervisor_vcpu_create - Create a vCPU on an existing VM
///
/// # Arguments
///
/// * arg0 - VM handle
/// * arg1 - vCPU ID
///
/// # Returns
///
/// * Handle number on success
/// * `usize::MAX` on error
#[cfg(feature = "hypervisor")]
pub fn sys_hypervisor_vcpu_create(trapframe: &mut Trapframe) -> usize {
    use crate::object::KernelObject;

    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let vm_handle = trapframe.get_arg(0) as u32;
    let vcpu_id = trapframe.get_arg(1) as u32;

    trapframe.increment_pc_next(task);

    let vm_arc = match task.handle_table.get(vm_handle) {
        Some(KernelObject::HypervisorVm(vm)) => vm,
        _ => return usize::MAX,
    };

    let vcpu = match vm_arc.create_vcpu(vcpu_id) {
        Ok(vcpu) => vcpu,
        Err(_) => return usize::MAX,
    };

    let kernel_obj = KernelObject::HypervisorVcpu(vcpu);
    match task.handle_table.insert(kernel_obj) {
        Ok(handle) => handle as usize,
        Err(_) => usize::MAX,
    }
}

/// Stub: hypervisor feature is disabled.
#[cfg(not(feature = "hypervisor"))]
pub fn sys_hypervisor_vm_create(trapframe: &mut Trapframe) -> usize {
    trapframe.increment_pc_next(mytask().unwrap());
    usize::MAX
}

/// Stub: hypervisor feature is disabled.
#[cfg(not(feature = "hypervisor"))]
pub fn sys_hypervisor_vcpu_create(trapframe: &mut Trapframe) -> usize {
    trapframe.increment_pc_next(mytask().unwrap());
    usize::MAX
}
