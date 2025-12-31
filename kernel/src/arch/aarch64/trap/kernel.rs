//! AArch64 kernel trap vector
//!
//! RISC-V style: keep a dedicated kernel vector active while executing in EL1,
//! and switch VBAR_EL1 to the user/trampoline vector only right before
//! returning to EL0.

use core::arch::naked_asm;

#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_kernel_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _kernel_trap_entry() {
    unsafe {
        naked_asm!(
            r#"
        .align 11
        // -----------------------------------------------------------------
        // VBAR_EL1 Kernel Vector Table (2048 bytes)
        // -----------------------------------------------------------------
        // Current EL with SP0 (sync/irq/fiq/serror)
        b   30f
        .space 124
        b   31f
        .space 124
        b   32f
        .space 124
        b   33f
        .space 124

        // Current EL with SPx (sync/irq/fiq/serror)
        b   30f
        .space 124
        b   31f
        .space 124
        b   32f
        .space 124
        b   33f
        .space 124

        // Lower EL using AArch64 (should not happen while kernel vector is active)
        b   34f
        .space 124
        b   34f
        .space 124
        b   34f
        .space 124
        b   34f
        .space 124

        // Lower EL using AArch32 (not expected)
        b   34f
        .space 124
        b   34f
        .space 124
        b   34f
        .space 124
        b   34f
        .space 124

        // -----------------------------------------------------------------
        // Kernel trap entry: save registers on current kernel stack and
        // dispatch to arch_kernel_trap_handler.
        // Aarch64 per-CPU struct is at TPIDRRO_EL0.
        // trap_kind offset is +48.
        // -----------------------------------------------------------------
        30:
            msr daifset, #0xf
            msr spsel, #1
            mrs x16, tpidrro_el0
            mov x17, #0
            str x17, [x16, #48]
            b 35f

        31:
            msr daifset, #0xf
            msr spsel, #1
            mrs x16, tpidrro_el0
            mov x17, #1
            str x17, [x16, #48]
            b 35f

        32:
            msr daifset, #0xf
            msr spsel, #1
            mrs x16, tpidrro_el0
            mov x17, #2
            str x17, [x16, #48]
            b 35f

        33:
            msr daifset, #0xf
            msr spsel, #1
            mrs x16, tpidrro_el0
            mov x17, #3
            str x17, [x16, #48]
            b 35f

        // If we ever take a lower-EL trap with the kernel vector installed,
        // it means we failed to switch VBAR_EL1 before entering EL0.
        34:
            b 34b

        35:
            // Allocate and save trapframe on the current kernel stack
            sub sp, sp, #272
            stp x0, x1, [sp, #0]
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
            mrs x17, elr_el1
            str x17, [sp, #256]
            mrs x17, sp_el0
            str x17, [sp, #248]
            mrs x17, spsr_el1
            str x17, [sp, #264]

            // Call kernel trap handler (address in per-CPU struct at +32)
            mrs x16, tpidrro_el0
            ldr x17, [x16, #32]
            mov x0, sp
            br  x17
        "#
        );
    }
}
