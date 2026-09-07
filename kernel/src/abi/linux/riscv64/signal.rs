//! RISC-V 64 arch-specific signal handling
//!
//! Provides `setup_signal_handler` and `sys_rt_sigreturn` for RISC-V.
//! Signal types and generic logic live in `generic::signal`.

use crate::abi::linux::generic::signal::LinuxSignal;
use crate::abi::linux::riscv64::LinuxRiscv64Abi;
use crate::arch::Trapframe;
use crate::task::mytask;

/// Set up user-space signal handler execution with context save/restore.
///
/// Signal frame layout on user stack (RISC-V):
/// ```text
///   [frame_base + 0]:   trampoline (li a7, 139; ecall)
///   [frame_base + 8]:   signal number
///   [frame_base + 16]:  saved regs[0..31] (256 bytes)
///   [frame_base + 272]: saved epc (8 bytes)
/// ```
pub fn setup_signal_handler(trapframe: &mut Trapframe, handler_addr: usize, signal: LinuxSignal) {
    let mut sp = trapframe.regs.reg[2];

    // trampoline(8) + signo(8) + regs(32x8) + epc(8) = 280
    const SIGNAL_FRAME_SIZE: usize = 8 + 8 + (32 * 8) + 8;
    sp -= SIGNAL_FRAME_SIZE;
    sp &= !0xF;

    let frame_base = sp;

    let task = match mytask() {
        Some(t) => t,
        None => return,
    };

    unsafe {
        // Trampoline: addi a7, x0, 139 (0x08b00893) + ecall (0x00000073)
        let paddr = match task.vm_manager.translate_to_kva(frame_base) {
            Some(p) => p,
            None => return,
        };
        *(paddr as *mut u32) = 0x08b00893; // addi a7, x0, 139
        *((paddr as *mut u32).add(1)) = 0x00000073; // ecall

        let paddr = match task.vm_manager.translate_to_kva(frame_base + 8) {
            Some(p) => p,
            None => return,
        };
        *(paddr as *mut usize) = signal as usize;

        for i in 0..32 {
            let paddr = match task.vm_manager.translate_to_kva(frame_base + 16 + i * 8) {
                Some(p) => p,
                None => return,
            };
            *(paddr as *mut usize) = trapframe.regs.reg[i];
        }

        let paddr = match task.vm_manager.translate_to_kva(frame_base + 272) {
            Some(p) => p,
            None => return,
        };
        *(paddr as *mut u64) = trapframe.epc;
    }

    trapframe.epc = handler_addr as u64;
    trapframe.regs.reg[2] = sp;
    trapframe.regs.reg[10] = signal as usize; // a0 = signal number
    trapframe.regs.reg[1] = frame_base; // ra = trampoline (rt_sigreturn)
}

/// Linux sys_rt_sigreturn — Restore context saved by `setup_signal_handler`.
///
/// Reads the signal frame from the current user stack pointer and restores
/// all 32 general-purpose registers plus `epc`.
pub fn sys_rt_sigreturn(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let frame_base = trapframe.regs.reg[2]; // SP points to signal frame

    unsafe {
        for i in 0..32 {
            let paddr = match task.vm_manager.translate_to_kva(frame_base + 16 + i * 8) {
                Some(p) => p,
                None => return usize::MAX,
            };
            trapframe.regs.reg[i] = *(paddr as *const usize);
        }

        let paddr = match task.vm_manager.translate_to_kva(frame_base + 272) {
            Some(p) => p,
            None => return usize::MAX,
        };
        trapframe.epc = *(paddr as *const u64);
    }

    0
}

/// Arch-specific fallback syscall table for RISC-V 64.
pub fn dispatch_arch_syscall(
    abi: &mut LinuxRiscv64Abi,
    trapframe: &mut Trapframe,
    syscall_number: usize,
) -> Option<usize> {
    match syscall_number {
        139 => Some(sys_rt_sigreturn(abi, trapframe)),
        _ => None,
    }
}
