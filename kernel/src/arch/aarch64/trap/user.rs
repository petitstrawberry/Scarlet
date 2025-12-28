use core::arch::{asm, naked_asm};

use super::exception::arch_exception_handler;
use crate::arch::{Trapframe, set_trapvector};
use crate::vm::get_trampoline_trap_vector;

// Trap vector base in the trampoline. Must be 2KB-aligned for VBAR_EL1.
// We use this symbol as the "trapvector" base (mirroring RISC-V's _user_trap_entry usage).
#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_user_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _user_trap_entry() {
    unsafe {
        naked_asm!(
            r#"
        .align 11

        // Vector table (16 entries x 128 bytes = 2048 bytes)
        // For now, all entries branch to the same common handler.
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124

        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124

        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124

        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124

        // -----------------------------------------------------------------
        // Common entry: save regs to Trapframe on kernel stack and jump handler
        // -----------------------------------------------------------------
        1:
            // Disable interrupts
            msr daifset, #0xf

            // x16: cpu pointer (TPIDR_EL1 points to arch struct in trampoline)
            mrs x16, tpidr_el1

            // Swap TTBR0_EL1 with cpu.ttbr0 (offset 16)
            ldr x17, [x16, #16]
            mrs x15, ttbr0_el1
            msr ttbr0_el1, x17
            isb
            str x15, [x16, #16]

            // Switch to per-task kernel stack top from cpu.kernel_stack (offset 24)
            ldr x17, [x16, #24]
            mov sp, x17

            // Allocate trapframe (AArch64 Trapframe is 264 bytes)
            sub sp, sp, #264

            // Save x0-x30
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

            // Save user SP (SP_EL0) into regs[31] (offset 248)
            mrs x17, sp_el0
            str x17, [sp, #248]

            // Save ELR_EL1 into epc (offset 256)
            mrs x17, elr_el1
            str x17, [sp, #256]

            // Jump to trap handler: cpu.kernel_trap (offset 32)
            ldr x17, [x16, #32]
            mov x0, sp
            br  x17
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
        // x0 = trapframe

        // Restore ELR_EL1 from trapframe.epc (offset 256)
        ldr x1, [x0, #256]
        msr elr_el1, x1

        // Restore user SP (SP_EL0) from regs[31] (offset 248)
        ldr x1, [x0, #248]
        msr sp_el0, x1

        // Configure return to EL0t (User) for now.
        // M[3:0] = 0b0000 (EL0t)
        mov x1, #0
        msr spsr_el1, x1

        // Swap TTBR0_EL1 with cpu.ttbr0 (offset 16)
        mrs x16, tpidr_el1
        ldr x17, [x16, #16]
        mrs x15, ttbr0_el1
        msr ttbr0_el1, x17
        isb
        str x15, [x16, #16]

        // Restore x1-x30 first
        ldp x1, x2, [x0, #8]
        ldp x3, x4, [x0, #24]
        ldp x5, x6, [x0, #40]
        ldp x7, x8, [x0, #56]
        ldp x9, x10, [x0, #72]
        ldp x11, x12, [x0, #88]
        ldp x13, x14, [x0, #104]
        ldr x15, [x0, #112]
        ldr x16, [x0, #128]
        ldr x17, [x0, #136]
        ldp x18, x19, [x0, #144]
        ldp x20, x21, [x0, #160]
        ldp x22, x23, [x0, #176]
        ldp x24, x25, [x0, #192]
        ldp x26, x27, [x0, #208]
        ldp x28, x29, [x0, #224]
        ldr x30, [x0, #240]

        // Restore x0 last
        ldr x0, [x0, #0]

        eret
        "#
        );
    }
}

#[unsafe(export_name = "arch_user_trap_handler")]
pub extern "C" fn arch_user_trap_handler(addr: usize) -> ! {
    let trapframe: &mut Trapframe = unsafe { &mut *(addr as *mut Trapframe) };

    // Keep trap vector pointing to trampoline base.
    set_trapvector(get_trampoline_trap_vector());

    arch_exception_handler(trapframe);

    arch_switch_to_user_space(trapframe);
}

#[unsafe(export_name = "arch_switch_to_user_space")]
pub fn arch_switch_to_user_space(trapframe: &mut Trapframe) -> ! {
    let addr = trapframe as *mut Trapframe as usize;

    let trap_exit_offset = _user_trap_exit as usize - _user_trap_entry as usize;
    let trampoline_base = get_trampoline_trap_vector();
    let trap_exit_addr = trampoline_base + trap_exit_offset;

    set_trapvector(trampoline_base);

    unsafe {
        asm!(
            "mov x0, {tf}",
            "br {target}",
            tf = in(reg) addr,
            target = in(reg) trap_exit_addr,
            options(noreturn, nostack)
        );
    }
}
