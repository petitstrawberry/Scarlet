//! AArch64 kernel trap vector
//!
//! Simple vector table for handling traps occurring within EL1 (kernel mode).
//! No context switching (TTBR/Stack) needed.

use super::exception::arch_exception_handler;
use super::interrupt::arch_irq_handler;
use crate::arch::Trapframe;
use core::arch::naked_asm;

// -------------------------------------------------------------------------
// Kernel Trap Vector
// -------------------------------------------------------------------------
#[unsafe(link_section = ".text")] // カーネル内コードなので通常の.textでOK
#[unsafe(export_name = "_kernel_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _kernel_trap_entry() {
    naked_asm!(
        r#"
        .align 11
        // -----------------------------------------------------------------
        // VBAR_EL1 Kernel Vector Table (2048 bytes)
        // -----------------------------------------------------------------
        // Current EL with SP0 (Invalid for OS use)
        b   .
        .space 124
        b   .
        .space 124
        b   .
        .space 124
        b   .
        .space 124

        // Current EL with SPx (Kernel -> Kernel Trap)
        // This is the main entry point for exceptions inside kernel.
        b   30f // Sync
        .space 124
        b   31f // IRQ
        .space 124
        b   32f // FIQ
        .space 124
        b   33f // SError
        .space 124

        // Lower EL (Should not happen if VBAR is managed correctly)
        // If this happens, it means we are in Userspace but VBAR points to Kernel Vector!
        b   .
        .space 124
        b   .
        .space 124
        b   .
        .space 124
        b   .
        .space 124

        // Lower EL using AArch32
        b   .
        .space 124
        b   .
        .space 124
        b   .
        .space 124
        b   .
        .space 124

        // -----------------------------------------------------------------
        // Entry Points
        // -----------------------------------------------------------------
        // [CRITICAL] Save registers to stack BEFORE clobbering them!
        
        30: // Sync
            msr daifset, #0xf       // Mask interrupts
            sub sp, sp, #304        // Alloc Trapframe
            stp x0, x1, [sp, #0]    // Save x0, x1 first
            mov x1, #0              // x1 (Arg2) = Kind: Sync
            b   40f

        31: // IRQ
            msr daifset, #0xf
            sub sp, sp, #304
            stp x0, x1, [sp, #0]
            mov x1, #1              // x1 (Arg2) = Kind: IRQ
            b   40f

        32: // FIQ
            msr daifset, #0xf
            sub sp, sp, #304
            stp x0, x1, [sp, #0]
            mov x1, #2              // x1 (Arg2) = Kind: FIQ
            b   40f

        33: // SError
            msr daifset, #0xf
            sub sp, sp, #304
            stp x0, x1, [sp, #0]
            mov x1, #3              // x1 (Arg2) = Kind: SError
            b   40f

        // -----------------------------------------------------------------
        // Common Handler
        // -----------------------------------------------------------------
        40: 
            // Save remaining GPRs
            stp x2, x3, [sp, #16]
            stp x4, x5, [sp, #32]
            stp x6, x7, [sp, #48]
            stp x8, x9, [sp, #64]
            stp x10, x11, [sp, #80]
            stp x12, x13, [sp, #96]
            stp x14, x15, [sp, #112]
            stp x16, x17, [sp, #128]
            stp x18, x19, [sp, #144]
            stp x20, x21, [sp, #160]
            stp x22, x23, [sp, #176]
            stp x24, x25, [sp, #192]
            stp x26, x27, [sp, #208]
            stp x28, x29, [sp, #224]
            str x30, [sp, #240]

            // Save captured kernel SP (SP_EL1 at exception entry)
            // Reuse Trapframe.tpidrro_el0 field for kernel traps (unused here).
            add x20, sp, #304
            str x20, [sp, #280]

            // Save System Registers
            mrs x19, elr_el1
            str x19, [sp, #256]
            mrs x19, sp_el0
            str x19, [sp, #248]
            mrs x19, spsr_el1
            str x19, [sp, #264]
            mrs x19, esr_el1
            str x19, [sp, #288]
            mrs x19, far_el1
            str x19, [sp, #296]

            // Call Rust Handler
            // fn arch_kernel_trap_handler(tf: &mut Trapframe, kind: usize)
            mov x0, sp              // x0 (Arg1) = &Trapframe
            
            // Branch with Link (bl) because we expect it to return!
            bl  arch_kernel_trap_handler

            // -------------------------------------------------------------
            // Exit Point (Restore Context)
            // -------------------------------------------------------------
            // Disable interrupts (just in case)
            msr daifset, #0xf

            // Restore System Registers
            ldr x19, [sp, #256]
            msr elr_el1, x19
            ldr x19, [sp, #248]
            msr sp_el0, x19
            ldr x19, [sp, #264]
            msr spsr_el1, x19

            // Restore GPRs
            ldp x2, x3, [sp, #16]
            ldp x4, x5, [sp, #32]
            ldp x6, x7, [sp, #48]
            ldp x8, x9, [sp, #64]
            ldp x10, x11, [sp, #80]
            ldp x12, x13, [sp, #96]
            ldp x14, x15, [sp, #112]
            ldp x16, x17, [sp, #128]
            ldp x18, x19, [sp, #144]
            ldp x20, x21, [sp, #160]
            ldp x22, x23, [sp, #176]
            ldp x24, x25, [sp, #192]
            ldp x26, x27, [sp, #208]
            ldp x28, x29, [sp, #224]
            ldr x30, [sp, #240]

            // Restore x0, x1 and Free Stack
            ldp x0, x1, [sp, #0]
            add sp, sp, #304

            eret
        "#
    );
}

// Rust側ハンドラ
// 戻り値なし(void)にする = 普通に関数から戻る
#[unsafe(export_name = "arch_kernel_trap_handler")]
pub extern "C" fn arch_kernel_trap_handler(trapframe: &mut Trapframe, trap_kind: usize) {
    if trap_kind == 1 || trap_kind == 2 {
        arch_irq_handler(trapframe, trap_kind);
    } else {
        // Exception (Sync/SError/FIQ)
        arch_exception_handler(trapframe, trap_kind);
    }
}

#[cfg(test)]
mod tests {
    #[test_case]
    fn kernel_trap_prologue_saves_x20_before_scratch_use() {
        let source = include_str!("kernel.rs");
        let save = source
            .find("stp x20, x21, [sp, #160]")
            .expect("kernel trap entry must save x20");
        let scratch = source
            .find("add x20, sp, #304")
            .expect("kernel trap entry must recover the pre-trap stack pointer");

        assert!(save < scratch);
        assert!(!source.contains("mov x20,\u{20}sp"));
    }
}
