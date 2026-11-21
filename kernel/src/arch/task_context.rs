//! Task context at the bare-metal architecture boundary.

use crate::{
    arch::{Arch, Mode, Trapframe},
    breadcrumb,
    environment::{KERNEL_VM_STACK_END, PAGE_SIZE, TASK_KERNEL_STACK_SIZE},
    task::Task,
};

pub(crate) fn initial_kernel_sp(_: &Task) -> usize {
    KERNEL_VM_STACK_END + 1
}

pub(crate) fn trapframe(task: &Task) -> &mut Trapframe {
    let (_, base) = task
        .get_kernel_stack_window_base()
        .expect("get_trapframe: No kernel stack window mapped");
    let address = base + PAGE_SIZE + TASK_KERNEL_STACK_SIZE - core::mem::size_of::<Trapframe>();
    // SAFETY: The task's mapped stack contains its trapframe, and the
    // scheduler's Task ownership serializes mutations.
    unsafe { &mut *(address as *mut Trapframe) }
}

pub(crate) fn configure_cpu(cpu: &mut Arch, task: &Task) {
    let cpu_id = cpu.get_cpuid();
    let sp = if let Some((_, base)) = task.get_kernel_stack_window_base() {
        base + PAGE_SIZE + TASK_KERNEL_STACK_SIZE
    } else {
        task.get_kernel_stack_top()
    };

    let trampoline_arch = crate::vm::get_trampoline_arch(cpu_id);
    breadcrumb::drop_cpu(cpu_id, breadcrumb::TRAMP_GET, trampoline_arch as u64);
    crate::arch::set_arch(trampoline_arch);
    breadcrumb::drop_cpu(cpu_id, breadcrumb::SET_ARCH_DONE, sp as u64);
    cpu.set_kernel_stack(sp);
    breadcrumb::drop_cpu(cpu_id, breadcrumb::SETSP_DONE, sp as u64);
    cpu.set_trap_handler(crate::arch::get_user_trap_handler());
    breadcrumb::drop_cpu(cpu_id, breadcrumb::SETTH_DONE, 0);
    let asid = task.vm_manager.get_asid();
    breadcrumb::drop_cpu(cpu_id, breadcrumb::SETAS_ASID_DONE, asid as u64);
    cpu.set_next_address_space(asid);
}

pub(crate) fn save_kernel_context(_cpu_id: usize, _task_id: usize, _task: &Task) {
    #[cfg(feature = "user-fpu")]
    crate::arch::fpu::kernel_switch_out_user_fpu(&mut *_task.vcpu.lock());

    #[cfg(feature = "user-vector")]
    crate::arch::fpu::kernel_switch_out_user_vector(_cpu_id, _task_id, &mut *_task.vcpu.lock());
}

pub(crate) fn restore_kernel_context(_cpu_id: usize, _task_id: usize, _task: &Task) {
    crate::arch::set_trapvector(crate::arch::get_kernel_trapvector_paddr());

    #[cfg(feature = "user-fpu")]
    {
        breadcrumb::drop(breadcrumb::VCPU_LOCK_BEGIN, _cpu_id as u64, _task_id as u64);
        crate::arch::fpu::kernel_switch_in_user_fpu(&mut *_task.vcpu.lock());
        breadcrumb::drop(breadcrumb::VCPU_LOCK_DONE, _cpu_id as u64, _task_id as u64);
    }
}

pub(crate) fn prepare_task_entry(mode: Mode) {
    crate::arch::set_next_mode(mode);
    crate::arch::set_trapvector(crate::vm::get_trampoline_trap_vector());
}
