//! AArch64 User trap entry/exit trampoline (Optimized)
//!
//! Layout matches generic "Riscv64/Aarch64" struct:
//!   0: scratch (8)
//!   8: cpuid (8)
//!  16: ttbr0 / user_satp (8)
//!  24: kernel_stack (8)
//!  32: arch_user_trap_handler (8) - trampoline jumps here
//!  40: kernel_ttbr0 / kernel_satp (8)
//!  48: trap_kind (8) - UNUSED in this optimized asm (passed via register)

use core::arch::{asm, naked_asm};

use super::exception::arch_exception_handler;
use super::interrupt::arch_irq_handler;
use crate::arch::{Trapframe, get_current_cpu_id, get_kernel_trapvector_paddr, set_arch, set_trapvector};
use crate::vm::get_trampoline_trap_vector;

#[unsafe(export_name = "aarch64_first_switch_to_user_naked")]
#[unsafe(naked)]
pub unsafe extern "C" fn aarch64_first_switch_to_user_naked(
    kernel_sp: u64,
    trapframe_addr: usize,
    trap_exit_addr: usize,
) -> ! {
    naked_asm!(
        r#"
        // x0 = kernel_sp
        // x1 = trapframe_addr
        // x2 = trap_exit_addr
        mov sp, x0
        mov x0, x1
        br  x2
        "#
    );
}

#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_user_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _user_trap_entry() {
    unsafe {
        naked_asm!(
            r#"
        .align 11
        // -----------------------------------------------------------------
        // VBAR_EL1 Vector Table
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

        // Current EL with SPx (Kernel Re-entry, First timer interrupt only)
        // Sync
        b   .
        .space 124
        // IRQ
        b   11f
        .space 124
        // FIQ
        b   .
        .space 124
        // SError
        b   .
        .space 124

        // Lower EL using AArch64 (User -> Kernel)
        b   10f
        .space 124
        b   11f
        .space 124
        b   12f
        .space 124
        b   13f
        .space 124

        // Lower EL using AArch32 (Not supported, hang)
        b   .
        .space 124
        b   .
        .space 124
        b   .
        .space 124
        b   .
        .space 124

        // -----------------------------------------------------------------
        // User -> Kernel Entry (Lower EL)
        // -----------------------------------------------------------------
        10: // Sync
            stp x16, x17, [sp, #-16]! // Save x16, x17
            mrs x16, tpidr_el1 // x16 = CPU struct ptr
            mov x17, #0         // Kind = Sync
            b   1f
        11: // IRQ
            stp x16, x17, [sp, #-16]! // Save x16, x17
            mrs x16, tpidr_el1 // x16 = CPU struct ptr
            mov x17, #1         // Kind = IRQ
            b   1f
        12: // FIQ
            stp x16, x17, [sp, #-16]! // Save x16, x17
            mrs x16, tpidr_el1 // x16 = CPU struct ptr
            mov x17, #2         // Kind = FIQ
            b   1f
        13: // SError
            stp x16, x17, [sp, #-16]! // Save x16, x17
            mrs x16, tpidr_el1 // x16 = CPU struct ptr
            mov x17, #3         // Kind = SError
            b   1f

        // Common User Trap Entry
        // PRE: x16=Struct ptr, x17=Kind. User x16 at [x16,#48], user x17 at [x16,#0].
        1:
            // 1. Switch TTBR (User -> Kernel)
            mrs x18, ttbr0_el1
            str x18, [x16, #16] // arch.ttbr0 = user TTBR
            
            ldr x18, [x16, #40] // arch.kernel_ttbr0
            msr ttbr0_el1, x18

            isb
            tlbi vmalle1is
            dsb ish
            isb

            // 2. Save Context
            sub sp, sp, #272
            stp x0, x1, [sp, #0]
            stp x2, x3, [sp, #16]
            stp x4, x5, [sp, #32]
            stp x6, x7, [sp, #48]
            stp x8, x9, [sp, #64]
            stp x10, x11, [sp, #80]
            stp x12, x13, [sp, #96]
            stp x14, x15, [sp, #112]
            // x16, x17 saved later
            stp x18, x19, [sp, #144]
            stp x20, x21, [sp, #160]
            stp x22, x23, [sp, #176]
            stp x24, x25, [sp, #192]
            stp x26, x27, [sp, #208]
            stp x28, x29, [sp, #224]
            str x30, [sp, #240]

            mrs x18, elr_el1
            str x18, [sp, #256]
            mrs x18, sp_el0
            str x18, [sp, #248]
            mrs x18, spsr_el1
            str x18, [sp, #264]

            // Restore/Save x16, x17
            ldr x0, [x16, #48]  // User x16
            str x0, [sp, #128]
            ldr x1, [x16, #0]   // User x17
            str x1, [sp, #136]

            // 4. Call Handler
            // fn arch_user_trap_handler(tf: &mut Trapframe, kind: usize)
            mov x1, x17         // arg1 = Kind
            mov x0, sp          // arg0 = Trapframe ptr
            
            ldr x18, [x16, #32] // arch_user_trap_handler (trampoline target)
            br  x18
        "#
        );
    }
}

#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_user_trap_exit")]
#[unsafe(naked)]
pub extern "C" fn _user_trap_exit(_trapframe: &mut Trapframe) -> ! {
    unsafe {
        naked_asm!(
            r#"
        // x0 = trapframe pointer
        msr daifset, #0xf

        // Restore system registers from trapframe
        // Layout (272 bytes):
        //   0..240: x0-x30
        //   248:    sp (SP_EL0)
        //   256:    epc (ELR_EL1)
        //   264:    spsr (SPSR_EL1)
        ldr x1, [x0, #256]
        msr elr_el1, x1
        ldr x1, [x0, #248]
        msr sp_el0, x1
        ldr x1, [x0, #264]
        msr spsr_el1, x1

        // We must switch TTBR back to the user page table.
        // After the switch, kernel memory (including the trapframe) may not be accessible.
        // So we stash the necessary values in the trampoline-mapped CPU struct and sysregs.
        // CPU struct pointer is stored in TPIDR_EL1.
        // (TPIDRRO_EL0 is not initialized in our current bring-up.)
        mrs x16, tpidr_el1          // x16 = CPU struct ptr (clobbers user x16 for now)

        // Stash user x0/x1 and user x16 before restoring registers.
        ldr x2, [x0, #0]            // user x0
        str x2, [x16, #0]           // cpu.scratch = user x0
        ldr x3, [x0, #8]            // user x1
        str x3, [x16, #48]          // cpu.trap_kind (repurposed) = user x1
        ldr x3, [x0, #128]          // user x16
        msr tpidr_el0, x3           // TPIDR_EL0 = user x16

        // Restore GPRs except x0/x1 (handled last) and x16 (restored from TPIDR_EL0).
        ldp x2, x3, [x0, #16]
        ldp x4, x5, [x0, #32]
        ldp x6, x7, [x0, #48]
        ldp x8, x9, [x0, #64]
        ldp x10, x11, [x0, #80]
        ldp x12, x13, [x0, #96]
        ldp x14, x15, [x0, #112]
        ldr x17, [x0, #136]
        ldp x18, x19, [x0, #144]
        ldp x20, x21, [x0, #160]
        ldp x22, x23, [x0, #176]
        ldp x24, x25, [x0, #192]
        ldp x26, x27, [x0, #208]
        ldp x28, x29, [x0, #224]
        ldr x30, [x0, #240]

        // Switch TTBR back to user
        ldr x1, [x16, #16]          // user ttbr0
        msr ttbr0_el1, x1
        isb
        tlbi vmalle1is
        dsb ish
        isb

        // Restore remaining registers
        ldr x1, [x16, #48]          // user x1
        ldr x0, [x16, #0]           // user x0
        mrs x16, tpidr_el0          // user x16

        eret
        "#
        );
    }
}

#[unsafe(export_name = "arch_switch_to_user_space")]
pub fn arch_switch_to_user_space(trapframe: &mut Trapframe) -> ! {
    crate::arch::configure_user_entry(
        trapframe,
        crate::arch::UserEntryOptions {
            irq_policy: crate::arch::UserReturnIrqPolicy::Enable,
        },
    );

    let addr = trapframe as *mut Trapframe as usize;
    let trap_exit_offset = _user_trap_exit as usize - _user_trap_entry as usize;
    let trampoline_base = get_trampoline_trap_vector();
    let trap_exit_addr = trampoline_base + trap_exit_offset;

    let cpu_id = get_current_cpu_id();
    set_arch(crate::vm::get_trampoline_arch(cpu_id));
    
    // Trampolineベクタをセット
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

// TrapKindを引数で受け取るように変更
#[unsafe(export_name = "arch_user_trap_handler")]
pub extern "C" fn arch_user_trap_handler(trapframe: &mut Trapframe, trap_kind: usize) -> ! {
    // We are now executing in EL1; switch VBAR_EL1 to the kernel vector so that
    // any exceptions/IRQs that occur while in kernel mode are handled by the
    // simple kernel trap routine.
    set_trapvector(get_kernel_trapvector_paddr());

    // trap_kind is now passed in x1 (argument 2), so no need to read from memory!
    if trap_kind == 1 {
        arch_irq_handler(trapframe);
    } else {
        arch_exception_handler(trapframe);
    }

    arch_switch_to_user_space(trapframe);
}
