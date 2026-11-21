//! Hosted scheduler continuations and guest trapframe ownership.

use alloc::boxed::Box;
use core::{ffi::c_void, fmt};
use scarlet_hosted_runtime::context::TaskContext;

use crate::{arch::Trapframe, environment::TASK_KERNEL_STACK_SIZE};

pub struct KernelContext {
    host: TaskContext,
    trapframe: Box<Trapframe>,
}

impl KernelContext {
    pub fn new() -> Self {
        Self {
            host: TaskContext::new(TASK_KERNEL_STACK_SIZE, enter_task, core::ptr::null_mut())
                .expect("Cannot allocate a guarded hosted task stack"),
            trapframe: Box::new(Trapframe::new()),
        }
    }

    pub fn get_kernel_stack_top(&self) -> usize {
        self.host.stack_top()
    }

    pub(crate) fn trapframe_ptr(&mut self) -> *mut Trapframe {
        &raw mut *self.trapframe
    }
}

impl fmt::Debug for KernelContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KernelContext")
            .field("stack_top", &self.get_kernel_stack_top())
            .finish_non_exhaustive()
    }
}

unsafe extern "C" fn enter_task(_: *mut c_void) -> ! {
    crate::task::task_initial_kernel_entrypoint()
}

/// Switch using the scheduler's existing running-CPU ownership token.
///
/// # Safety
/// The scheduler must exclusively own both distinct continuations and keep
/// their storage alive across the switch.
pub unsafe fn switch_to(prev: *mut KernelContext, next: *const KernelContext) {
    // SAFETY: These borrows end before control transfers to another context.
    let previous = unsafe { (*prev).host.registers_ptr() };
    let next = unsafe { (*next).host.registers() };
    unsafe { scarlet_hosted_runtime::context::switch(previous, next) };
}
