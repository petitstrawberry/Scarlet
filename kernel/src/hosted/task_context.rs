//! Task context at the hosted execution boundary.

use crate::{
    arch::{Arch, Mode, Trapframe},
    task::Task,
};

pub(crate) fn initial_kernel_sp(task: &Task) -> usize {
    task.get_kernel_stack_top()
}

pub(crate) fn trapframe(task: &Task) -> &mut Trapframe {
    let trapframe = task.kernel_context.lock().trapframe_ptr();
    // SAFETY: The Task owns the trapframe backing and the scheduler's
    // Task ownership serializes mutations.
    unsafe { &mut *trapframe }
}

pub(crate) fn configure_cpu(cpu: &mut Arch, task: &Task) {
    cpu.set_next_address_space(task.vm_manager.get_asid());
}

// Guest registers are captured at the worker boundary. Kernel continuations
// save the host ABI registers in context::switch_to.
pub(crate) fn save_kernel_context(_: usize, _: usize, _: &Task) {}
pub(crate) fn restore_kernel_context(_: usize, _: usize, _: &Task) {}

pub(crate) fn prepare_task_entry(_: Mode) {}
