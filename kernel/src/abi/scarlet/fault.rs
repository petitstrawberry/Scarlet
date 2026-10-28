//! Native fault syscall entry points. Architecture-specific context encoding
//! and restoration live in the selected Scarlet ABI implementation.

use super::ScarletAbi;
use crate::{arch::Trapframe, task::mytask};
use scarlet_abi::Syscall;

// Called directly by ScarletAbi::handle_syscall while that ABI is borrowed.
// This avoids acquiring a second mutable reference to the task's ABI state.
pub(super) fn handle_syscall(
    abi: &mut ScarletAbi,
    trap: &mut Trapframe,
    number: usize,
) -> Option<usize> {
    if number != Syscall::NativeFaultRegister as usize
        && number != Syscall::NativeFaultReturn as usize
    {
        return None;
    }
    let Some(task) = mytask() else {
        return Some(usize::MAX);
    };
    trap.increment_pc_next(&task);
    Some(if number == Syscall::NativeFaultRegister as usize {
        abi.register_fault_handler(&task, trap)
    } else {
        abi.return_from_fault(&task, trap)
    })
}

// Explicit native-call encoding bypasses AbiModule::handle_syscall, so these
// table entries acquire the ABI once. Non-native default ABIs reject the call.
fn dispatch(trap: &mut Trapframe, number: Syscall) -> usize {
    let Some(task) = mytask() else {
        return usize::MAX;
    };
    task.with_default_abi_mut(|abi, _| {
        if let Some(abi) = abi.as_any_mut().downcast_mut::<ScarletAbi>() {
            handle_syscall(abi, trap, number as usize).unwrap()
        } else {
            trap.increment_pc_next(&task);
            usize::MAX
        }
    })
}

/// Register or remove the current native thread's synchronous fault handler.
pub fn sys_register(trap: &mut Trapframe) -> usize {
    dispatch(trap, Syscall::NativeFaultRegister)
}

/// Restore the active native fault context, or return MAX for an invalid one.
pub fn sys_return(trap: &mut Trapframe) -> usize {
    dispatch(trap, Syscall::NativeFaultReturn)
}
