//! Per-thread native synchronous fault delivery. The alternate stack and
//! context are user memory; privileged state and FP/SIMD stay kernel-owned.

use super::ScarletAbi;
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::{
    arch::{Trapframe, fpu::FpuContext},
    task::Task,
};
use alloc::boxed::Box;

const SIZE: usize = core::mem::size_of::<scarlet_abi::RawNativeFaultContext>();

/// Task-local registration and at most one active synchronous fault.
#[derive(Default)]
pub(super) struct FaultState {
    registration: Option<(usize, usize)>, // handler, alternate-stack top
    active: Option<Box<ActiveFault>>,
}

struct ActiveFault {
    address: usize,
    trap: Trapframe,
    fpu: FpuContext,
    fpu_used: bool,
}

fn user_range(address: usize, size: usize) -> bool {
    address != 0
        && scarlet_abi::data_model::AbiDataModel::NATIVE
            .user_address(address as u64)
            .and_then(|value| value.check_range(size as u64))
            .is_ok()
}

impl ScarletAbi {
    /// Register a user handler and alternate stack, or unregister with all zeros.
    /// Returns zero on success and MAX for invalid or active registrations.
    pub(in crate::abi::scarlet) fn register_fault_handler(
        &mut self,
        task: &Task,
        trap: &mut Trapframe,
    ) -> usize {
        let (handler, base, size) = (trap.get_arg(0), trap.get_arg(1), trap.get_arg(2));
        if self.fault.active.is_some() {
            return usize::MAX;
        }
        if handler == 0 && base == 0 && size == 0 {
            self.fault.registration = None;
            return 0;
        }
        if handler & 3 != 0
            || !user_range(handler, 4)
            || size < 4096
            || !user_range(base, size)
            || task
                .vm_manager
                .translate_to_phys_with_access(
                    handler,
                    crate::object::capability::memory_mapping::AccessOp::Instruction,
                )
                .is_none()
        {
            return usize::MAX;
        }
        let Some(top) = base.checked_add(size) else {
            return usize::MAX;
        };
        self.fault.registration = Some((handler, top & !15));
        0
    }
    /// Arm an EL0 handler; nested faults or inaccessible stacks remain fatal.
    pub(super) fn deliver_fault(
        &mut self,
        task: &Task,
        trap: &mut Trapframe,
        address: usize,
        syndrome: u64,
    ) -> bool {
        if trap.spsr & 0xf != 0 {
            return false;
        }
        let Some((handler, top)) = self.fault.registration else {
            return false;
        };
        // A fault inside a fault handler follows the normal fatal path.
        if self.fault.active.is_some() {
            return false;
        }
        let Some(base) = top.checked_sub(SIZE) else {
            return false;
        };
        let mut words = [0u64; SIZE / 8];
        words[..4].copy_from_slice(&[1, SIZE as u64, address as u64, syndrome]);
        for i in 0..31 {
            words[4 + i] = trap.regs.reg[i] as u64;
        }
        words[35] = trap.sp;
        words[36] = trap.elr;
        words[37] = trap.spsr & 0xf000_0000;
        let mut bytes = [0u8; SIZE];
        for (i, word) in words.iter().enumerate() {
            bytes[i * 8..i * 8 + 8].copy_from_slice(&word.to_ne_bytes());
        }
        let (fpu, fpu_used) = {
            let mut vcpu = task.vcpu.lock();
            if vcpu.fpu_used && crate::arch::user_fpu_enabled() {
                // SAFETY: current task's FP/SIMD state is live and enabled.
                unsafe {
                    vcpu.fpu.save();
                }
            }
            (vcpu.fpu.clone(), vcpu.fpu_used)
        };
        if copy_to_user(task, base, &bytes).is_err() {
            return false;
        }
        self.fault.active = Some(Box::new(ActiveFault {
            address: base,
            trap: trap.clone(),
            fpu,
            fpu_used,
        }));
        trap.sp = base as u64;
        trap.elr = handler as u64;
        trap.regs.reg[0] = base;
        trap.regs.reg[30] = 0; // Handler must use NativeFaultReturn, never RET.
        true
    }
    /// Restore the active context with kernel-owned FP/TLS state. Returns the
    /// interrupted x0 on success and MAX for an invalid context.
    pub(in crate::abi::scarlet) fn return_from_fault(
        &mut self,
        task: &Task,
        trap: &mut Trapframe,
    ) -> usize {
        let address = trap.get_arg(0);
        let Some(active) = self.fault.active.as_ref() else {
            return usize::MAX;
        };
        if address != active.address {
            return usize::MAX;
        }
        let mut bytes = [0u8; SIZE];
        if copy_from_user(task, address, &mut bytes).is_err() {
            return usize::MAX;
        }
        let mut words = [0u64; SIZE / 8];
        for (i, word) in words.iter_mut().enumerate() {
            *word = u64::from_ne_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap());
        }
        if words[0] != 1
            || words[1] != SIZE as u64
            || words[35] & 15 != 0
            || words[36] & 3 != 0
            || !user_range(words[35] as usize, 1)
            || !user_range(words[36] as usize, 4)
        {
            return usize::MAX;
        }
        let active = self.fault.active.take().unwrap();
        *trap = active.trap;
        for i in 0..31 {
            trap.regs.reg[i] = words[4 + i] as usize;
        }
        trap.sp = words[35];
        trap.elr = words[36];
        trap.spsr = (trap.spsr & !0xf000_0000) | (words[37] & 0xf000_0000);
        let mut vcpu = task.vcpu.lock();
        vcpu.fpu = active.fpu;
        vcpu.fpu_used = active.fpu_used;
        if crate::arch::user_fpu_enabled() {
            crate::arch::fpu::set_user_fpu_enabled(vcpu.fpu_used);
            if vcpu.fpu_used {
                // SAFETY: restore the current task's saved state with FP enabled.
                unsafe {
                    vcpu.fpu.restore();
                }
            }
        }
        trap.get_return_value()
    }
}
