//! AArch64 User trap entry/exit trampoline
//!
//! Strategy follows RISC-V implementation for consistency:
//! - TPIDRRO_EL0 holds the arch struct pointer (like sscratch)
//! - Entry: save context to kernel stack, switch TTBR, call handler
//! - Exit: restore context, switch to user TTBR, ERET

use core::arch::{asm, naked_asm};

use super::exception::arch_exception_handler;
use super::interrupt::arch_irq_handler;
use crate::arch::{Trapframe, get_current_cpu_id, set_arch, set_trapvector};
use crate::vm::get_trampoline_trap_vector;

/// User trap entry point and vector table
/// 
/// Aarch64 struct layout (offsets):
///   0: scratch (8 bytes)
///   8: cpuid (8 bytes)
///  16: ttbr0 / user TTBR (8 bytes)
///  24: kernel_stack (8 bytes)
///  32: kernel_trap_handler (8 bytes)
///  40: kernel_ttbr0 (8 bytes)
///  48: trap_kind (8 bytes)   // 0=sync,1=irq,2=fiq,3=serror
///  56: user_trap_handler (8 bytes)
#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_user_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _user_trap_entry() {
    unsafe {
        naked_asm!(
            r#"
        .align 11
        // -----------------------------------------------------------------
        // VBAR_EL1 Vector Table (2048 bytes total)
        // -----------------------------------------------------------------
        // Current EL with SP0 (sync/irq/fiq/serror)
        b   20f
        .space 124
        b   21f
        .space 124
        b   22f
        .space 124
        b   23f
        .space 124

        // Current EL with SPx (sync/irq/fiq/serror)
        b   20f
        .space 124
        b   21f
        .space 124
        b   22f
        .space 124
        b   23f
        .space 124

        // Lower EL using AArch64 - User -> Kernel (sync/irq/fiq/serror)
        b   10f
        .space 124
        b   11f
        .space 124
        b   12f
        .space 124
        b   13f
        .space 124

        // Lower EL using AArch32 (sync/irq/fiq/serror)
        b   15f
        .space 124
        b   15f
        .space 124
        b   15f
        .space 124
        b   15f
        .space 124

        // -----------------------------------------------------------------
        // 1_x: User -> Kernel Entry (Lower EL)
        // Record trap kind into arch.trap_kind (offset #8) so Rust can
        // reliably distinguish IRQ from synchronous exceptions.
        // -----------------------------------------------------------------
        // Current EL stubs (SP0/SPx): record trap kind then fallthrough into
        // the shared kernel entry (2:).
        20:
            msr daifset, #0xf
            mrs x16, tpidrro_el0
            mov x17, #0
            str x17, [x16, #48]         // arch.trap_kind = sync
            b 2f

        21:
            msr daifset, #0xf
            mrs x16, tpidrro_el0
            mov x17, #1
            str x17, [x16, #48]         // arch.trap_kind = irq
            b 2f

        22:
            msr daifset, #0xf
            mrs x16, tpidrro_el0
            mov x17, #2
            str x17, [x16, #48]         // arch.trap_kind = fiq
            b 2f

        23:
            msr daifset, #0xf
            mrs x16, tpidrro_el0
            mov x17, #3
            str x17, [x16, #48]         // arch.trap_kind = serror
            b 2f

        10:
            msr daifset, #0xf
            msr spsel, #1
            msr tpidr_el1, x16          // temp: user x16
            mrs x16, tpidrro_el0        // x16 = arch struct base
            str x17, [x16, #0]          // arch.scratch = user x17
            mov x17, #0
            str x17, [x16, #48]         // arch.trap_kind = sync
            b 14f

        11:
            msr daifset, #0xf
            msr spsel, #1
            msr tpidr_el1, x16
            mrs x16, tpidrro_el0
            str x17, [x16, #0]
            mov x17, #1
            str x17, [x16, #48]         // arch.trap_kind = irq
            b 14f

        12:
            msr daifset, #0xf
            msr spsel, #1
            msr tpidr_el1, x16
            mrs x16, tpidrro_el0
            str x17, [x16, #0]
            mov x17, #2
            str x17, [x16, #48]         // arch.trap_kind = fiq
            b 14f

        13:
            msr daifset, #0xf
            msr spsel, #1
            msr tpidr_el1, x16
            mrs x16, tpidrro_el0
            str x17, [x16, #0]
            mov x17, #3
            str x17, [x16, #48]         // arch.trap_kind = serror
            b 14f

        14:

            // RISC-V style: once we're in EL1, switch VBAR_EL1 to the kernel vector
            // so nested traps/IRQs use the kernel entry.
            adrp x17, _kernel_trap_entry
            add  x17, x17, :lo12:_kernel_trap_entry
            msr  vbar_el1, x17
            isb

            // Save user TTBR, switch to kernel TTBR
            mrs x17, ttbr0_el1
            str x17, [x16, #16]         // arch.ttbr0 = user TTBR
            ldr x17, [x16, #40]         // x17 = kernel TTBR
            msr ttbr0_el1, x17
            msr ttbr1_el1, x17
            isb
            tlbi vmalle1is
            dsb ish
            isb

            // Switch to kernel stack
            ldr x17, [x16, #24]
            mov sp, x17

            // Allocate Trapframe
            sub sp, sp, #272

            // Save general registers
            stp x0, x1, [sp, #0]
            stp x2, x3, [sp, #16]
            stp x4, x5, [sp, #32]
            stp x6, x7, [sp, #48]
            stp x8, x9, [sp, #64]
            stp x10, x11, [sp, #80]
            stp x12, x13, [sp, #96]
            stp x14, x15, [sp, #112]
            stp x18, x19, [sp, #144]
            stp x20, x21, [sp, #160]
            stp x22, x23, [sp, #176]
            stp x24, x25, [sp, #192]
            stp x26, x27, [sp, #208]
            stp x28, x29, [sp, #224]
            str x30, [sp, #240]

            // Save special registers
            mrs x17, elr_el1
            str x17, [sp, #256]
            mrs x17, sp_el0
            str x17, [sp, #248]
            mrs x17, spsr_el1
            str x17, [sp, #264]

            // Restore and save user x16/x17
            mrs x0, tpidr_el1
            str x0, [sp, #128]          // user x16
            ldr x1, [x16, #0]
            str x1, [sp, #136]          // user x17

            // Call Rust handler (EL0->EL1)
            ldr x17, [x16, #56]         // user_trap_handler
            mov x0, sp                  // arg0 = trapframe
            br  x17

        // Lower EL using AArch32 is not expected in this kernel.
        // Park the CPU here if it ever happens.
        15:
            b 15b

        // -----------------------------------------------------------------
        // 2: Kernel Re-entry (Nested Trap)
        // -----------------------------------------------------------------
        2:
            msr daifset, #0xf
            msr spsel, #1
            mrs x16, tpidrro_el0

            // Force kernel TTBRs
            ldr x17, [x16, #40]
            msr ttbr0_el1, x17
            msr ttbr1_el1, x17
            isb
            tlbi vmalle1is
            dsb ish
            isb

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

            // Call Rust handler (kernel-origin traps)
            ldr x17, [x16, #32]
            mov x0, sp
            br  x17
        "#
        );
    }
}

/// User trap exit point
#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_user_trap_exit")]
#[unsafe(naked)]
pub extern "C" fn _user_trap_exit(_trapframe: &mut Trapframe) -> ! {
    unsafe {
        naked_asm!(
            r#"
        // x0 = trapframe pointer
        msr daifset, #0xf
        msr spsel, #1

        // Ensure kernel TTBRs for trapframe access
        mov x19, x0              // keep trapframe pointer
        mrs x16, tpidrro_el0
        ldr x17, [x16, #40]
        msr ttbr0_el1, x17
        msr ttbr1_el1, x17
        isb
        tlbi vmalle1is
        dsb ish
        isb

        // Restore special registers
        ldr x1, [x19, #256]
        msr elr_el1, x1
        ldr x1, [x19, #248]
        msr sp_el0, x1
        ldr x1, [x19, #264]
        msr spsr_el1, x1

        // Save user x0/x1 for later
        ldr x2, [x19, #0]
        str x2, [x16, #0]        // arch.scratch = user x0
        ldr x2, [x19, #8]
        msr tpidr_el1, x2        // temp = user x1

        // Restore general registers
        mov x0, x19
        ldp x2, x3, [x0, #16]
        ldp x4, x5, [x0, #32]
        ldp x6, x7, [x0, #48]
        ldp x8, x9, [x0, #64]
        ldp x10, x11, [x0, #80]
        ldp x12, x13, [x0, #96]
        ldp x14, x15, [x0, #112]
        ldp x16, x17, [x0, #128]
        ldp x18, x19, [x0, #144]
        ldp x20, x21, [x0, #160]
        ldp x22, x23, [x0, #176]
        ldp x24, x25, [x0, #192]
        ldp x26, x27, [x0, #208]
        ldp x28, x29, [x0, #224]
        ldr x30, [x0, #240]

        // Switch to user TTBR (do last)
        mrs x0, tpidrro_el0
        ldr x1, [x0, #16]        // user TTBR
        msr ttbr0_el1, x1
        msr ttbr1_el1, x1
        isb
        tlbi vmalle1is
        dsb ish
        isb

        // Final restore of user x0/x1
        mrs x1, tpidr_el1        // user x1
        ldr x0, [x0, #0]         // user x0

        // RISC-V style: before returning to EL0, switch VBAR_EL1 back to the
        // user/trampoline vector.
        adrp x2, _user_trap_entry
        add  x2, x2, :lo12:_user_trap_entry
        msr  vbar_el1, x2
        isb

        // Return to user
        eret
        "#
        );
    }
}

/// Kernel trap exit point (return to EL1)
#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_kernel_trap_exit")]
#[unsafe(naked)]
pub extern "C" fn _kernel_trap_exit(_trapframe: &mut Trapframe) -> ! {
    unsafe {
        naked_asm!(
            r#"
        // x0 = trapframe pointer (on kernel stack)
        msr daifset, #0xf
        msr spsel, #1

        // Restore ELR/SPSR
        ldr x1, [x0, #256]
        msr elr_el1, x1
        ldr x1, [x0, #264]
        msr spsr_el1, x1

        // Restore general registers (except x0/x1 until the end)
        ldp x2, x3, [x0, #16]
        ldp x4, x5, [x0, #32]
        ldp x6, x7, [x0, #48]
        ldp x8, x9, [x0, #64]
        ldp x10, x11, [x0, #80]
        ldp x12, x13, [x0, #96]
        ldp x14, x15, [x0, #112]
        ldp x16, x17, [x0, #128]
        ldp x18, x19, [x0, #144]
        ldp x20, x21, [x0, #160]
        ldp x22, x23, [x0, #176]
        ldp x24, x25, [x0, #192]
        ldp x26, x27, [x0, #208]
        ldp x28, x29, [x0, #224]
        ldr x30, [x0, #240]

        // Restore original kernel SP and x0/x1, then return
        add sp, x0, #272
        ldp x0, x1, [x0, #0]
        eret
        "#
        );
    }
}

/// Switch to user space
#[unsafe(export_name = "arch_switch_to_user_space")]
pub fn arch_switch_to_user_space(trapframe: &mut Trapframe) -> ! {
    let addr = trapframe as *mut Trapframe as usize;

    let trap_exit_offset = _user_trap_exit as usize - _user_trap_entry as usize;
    let trampoline_base = get_trampoline_trap_vector();
    let trap_exit_addr = trampoline_base + trap_exit_offset;

    // Set arch struct pointer for trampoline access
    let cpu_id = get_current_cpu_id();
    set_arch(crate::vm::get_trampoline_arch(cpu_id));

    // Set trap vector
    set_trapvector(trampoline_base);

    unsafe {
        asm!(
            "mov x0, {tf_addr}",
            "br {target}",
            tf_addr = in(reg) addr,
            target = in(reg) trap_exit_addr,
            options(noreturn, nostack)
        );
    }
}

/// Return to EL1 kernel context using the trampoline exit.
fn arch_return_to_kernel(trapframe: &mut Trapframe) -> ! {
    let addr = trapframe as *mut Trapframe as usize;

    let trap_exit_offset = _kernel_trap_exit as usize - _user_trap_entry as usize;
    let trampoline_base = get_trampoline_trap_vector();
    let trap_exit_addr = trampoline_base + trap_exit_offset;

    // Ensure we keep using the trampoline vector (nested IRQs use it as well)
    set_trapvector(trampoline_base);

    unsafe {
        asm!(
            "mov x0, {tf_addr}",
            "br {target}",
            tf_addr = in(reg) addr,
            target = in(reg) trap_exit_addr,
            options(noreturn, nostack)
        );
    }
}

/// User trap handler entry point
#[unsafe(export_name = "arch_user_trap_handler")]
pub extern "C" fn arch_user_trap_handler(addr: usize) -> ! {
    let trapframe: &mut Trapframe = unsafe { &mut *(addr as *mut Trapframe) };

    // Decode previous exception level from SPSR_EL1.M.
    // EL0t == 0b0000.
    let from_el0 = (trapframe.spsr & 0xF) == 0;

    // IRQ/FIQ/SERROR don't have a reliable ESR_EL1 value.
    // Use the trap kind recorded by the trampoline entry.
    let arch_ptr: usize;
    unsafe {
        asm!("mrs {0}, tpidrro_el0", out(reg) arch_ptr, options(nostack));
    }
    let trap_kind_ptr = (arch_ptr + 48) as *const u64;
    let trap_kind = unsafe { core::ptr::read_volatile(trap_kind_ptr) };
    if trap_kind == 1 {
        arch_irq_handler(trapframe);
    } else {
        arch_exception_handler(trapframe);
    }

    if from_el0 {
        arch_switch_to_user_space(trapframe)
    } else {
        arch_return_to_kernel(trapframe)
    }
}

/// Kernel trap handler entry point.
///
/// This is invoked for Current-EL vectors and the nested-trap path (label `2:`).
#[unsafe(export_name = "arch_kernel_trap_handler")]
pub extern "C" fn arch_kernel_trap_handler(addr: usize) -> ! {
    let trapframe: &mut Trapframe = unsafe { &mut *(addr as *mut Trapframe) };

    // Use the trap kind recorded by the trampoline entry.
    let arch_ptr: usize;
    unsafe {
        asm!("mrs {0}, tpidrro_el0", out(reg) arch_ptr, options(nostack));
    }
    let trap_kind_ptr = (arch_ptr + 48) as *const u64;
    let trap_kind = unsafe { core::ptr::read_volatile(trap_kind_ptr) };

    if trap_kind == 1 {
        arch_irq_handler(trapframe);
    } else {
        arch_exception_handler(trapframe);
    }

    arch_return_to_kernel(trapframe)
}
