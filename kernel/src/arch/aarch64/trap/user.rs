use core::arch::{asm, naked_asm};

use super::exception::arch_exception_handler;
use crate::arch::{Trapframe, set_trapvector};
use crate::vm::get_trampoline_trap_vector;

#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_user_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _user_trap_entry() {
    unsafe {
        naked_asm!(
            r#"
        .align 11
        // -----------------------------------------------------------------
        // VBAR_EL1 Vector Table (2048 bytes)
        // -----------------------------------------------------------------
        // Current EL with SP0
        b   2f
        .space 124
        b   2f
        .space 124
        b   2f
        .space 124
        b   2f
        .space 124

        // Current EL with SPx
        b   2f
        .space 124
        b   2f
        .space 124
        b   2f
        .space 124
        b   2f
        .space 124

        // Lower EL using AArch64 (User -> Kernel) <--- TARGET
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124

        // Lower EL using AArch32
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124

        // -----------------------------------------------------------------
        // 1: User -> Kernel Entry (Corresponds to RISC-V _user_trap_entry)
        // -----------------------------------------------------------------
        1:
            // Disable interrupts (RISC-V: csrci sstatus, 0x2)
            msr daifset, #0xf

            // [Context Swap Strategy]
            // RISC-V: csrrw a0, sscratch, a0
            // AArch64: cannot swap atomicaly.
            // 1. Hide user x16 in TPIDR_EL1 (system reg)
            // 2. Load Struct Base from TPIDRRO_EL0 to x16
            // 3. Save user x17 to Struct.scratch (memory)
            // Now x16 is our "a0" (base pointer).
            
            msr tpidr_el1, x16          // Save x16 temporarily
            mrs x16, tpidrro_el0        // Load Aarch64 struct ptr
            str x17, [x16, #0]          // Save x17 to scratch (offset 0)

            // Switch to Kernel Page Table (RISC-V: csrrw sp, satp, sp logic)
            // Save User TTBR0 (offset 16)
            mrs x17, ttbr0_el1
            str x17, [x16, #16]
            
            // Load Kernel TTBR0 (offset 40) and Switch
            ldr x17, [x16, #40]
            msr ttbr0_el1, x17
            
            // TLB Flush (sfence.vma equivalent)
            isb
            dsb ish
            tlbi vmalle1is
            dsb ish
            isb

            // Load Kernel Stack (RISC-V: ld sp, 24(a0))
            ldr x17, [x16, #24]
            mov sp, x17

            // Allocate Trapframe (RISC-V: addi sp, sp, -272)
            sub sp, sp, #272

            // Save General Registers
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

            // Save Special Registers (RISC-V: sepc, sstatus, etc)
            mrs x17, elr_el1            // PC
            str x17, [sp, #256]
            mrs x17, sp_el0             // User SP
            str x17, [sp, #248]
            mrs x17, spsr_el1           // PSTATE
            str x17, [sp, #264]

            // Restore and Save User x16, x17
            // User x16 is in TPIDR_EL1
            mrs x0, tpidr_el1
            str x0, [sp, #128]
            // User x17 is in Struct.scratch (offset 0)
            ldr x1, [x16, #0]
            str x1, [sp, #136]

            // Optimization: Set TPIDR_EL1 to Struct Ptr for kernel use
            msr tpidr_el1, x16

            // Jump to Rust Handler (RISC-V: ld ra, 32(a0) -> jr ra)
            ldr x17, [x16, #32]
            mov x0, sp                  // arg0: trapframe ptr
            br  x17

        // -----------------------------------------------------------------
        // 2: Kernel Re-entry (Nested Trap)
        // -----------------------------------------------------------------
        2:
            msr daifset, #0xf
            // (Minimal save for kernel panic/debug)
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

            mrs x16, tpidr_el1
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
        // x0 = trapframe pointer (Rust側から渡される)

        // Disable Interrupts
        msr daifset, #0xf

        // Restore System Registers (RISC-V: csrw sepc, t0)
        ldr x1, [x0, #256]      // ELR
        msr elr_el1, x1
        ldr x1, [x0, #248]      // SP_EL0
        msr sp_el0, x1
        ldr x1, [x0, #264]      // SPSR
        msr spsr_el1, x1

        // [PREPARE SCRATCH STRATEGY]
        // RISC-V uses sscratch swap. AArch64 needs a base pointer.
        // We need x0 and x1 free to switch Page Table.
        // 1. Get Struct Base (x16)
        // 2. Load NEW x0, x1 from Trapframe
        // 3. Store NEW x0 to Struct.scratch (Memory)
        // 4. Store NEW x1 to TPIDR_EL1 (Register - temporary storage)

        mrs x16, tpidrro_el0    // Get Struct Base (RISC-V: sscratch)

        ldr x2, [x0, #0]        // Load x0 from Trapframe (User x0)
        ldr x3, [x0, #8]        // Load x1 from Trapframe (User x1)

        str x2, [x16, #0]       // Save User x0 to scratch (Memory)
        msr tpidr_el1, x3       // Save User x1 to TPIDR_EL1 (Register)

        // Restore All Other Registers (x2-x30) from Trapframe
        // (RISC-V: ld x1, 8(a0) ... )
        ldp x2, x3, [x0, #16]
        ldp x4, x5, [x0, #32]
        ldp x6, x7, [x0, #48]
        ldp x8, x9, [x0, #64]
        ldp x10, x11, [x0, #80]
        ldp x12, x13, [x0, #96]
        ldp x14, x15, [x0, #112]
        ldp x16, x17, [x0, #128] // Restores User x16, x17
        ldp x18, x19, [x0, #144]
        ldp x20, x21, [x0, #160]
        ldp x22, x23, [x0, #176]
        ldp x24, x25, [x0, #192]
        ldp x26, x27, [x0, #208]
        ldp x28, x29, [x0, #224]
        ldr x30, [x0, #240]

        // Switch Page Table (Kernel -> User)
        // (RISC-V: ld t0, 16(a0) -> csrrw t0, satp, t0)
        
        mrs x0, tpidrro_el0     // Get Struct Base again (x0 is free/scratch)
        ldr x1, [x0, #16]       // Load User TTBR0 (x1 is free/scratch)
        msr ttbr0_el1, x1
        msr ttbr1_el1, x1
        isb
        dsb ish
        tlbi vmalle1is
        dsb ish
        isb

        // Final Restore of x0, x1
        // (RISC-V: ld t0, 0(a0) -> csrrw a0, sscratch, a0)
        
        // Restore x1 from TPIDR_EL1
        mrs x1, tpidr_el1
        
        // Restore x0 from Struct.scratch
        // Note: x0 holds Struct Base.
        // ldr x0, [x0] loads the value at address x0 into x0.
        // This overwrites the base pointer with User x0. Perfect.
        ldr x0, [x0, #0]

        // Return to User
        eret
        "#
        );
    }
}

#[unsafe(export_name = "arch_switch_to_user_space")]
pub fn arch_switch_to_user_space(trapframe: &mut Trapframe) -> ! {
    let addr = trapframe as *mut Trapframe as usize;

    let trap_exit_offset = _user_trap_exit as usize - _user_trap_entry as usize;
    let trampoline_base = get_trampoline_trap_vector();
    let trap_exit_addr = trampoline_base + trap_exit_offset;

    // トラップベクタをTrampolineに向ける
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

#[unsafe(export_name = "arch_user_trap_handler")]
pub extern "C" fn arch_user_trap_handler(addr: usize) -> ! {
    let trapframe: &mut Trapframe = unsafe { &mut *(addr as *mut Trapframe) };

    // Keep trap vector pointing to trampoline base.
    set_trapvector(get_trampoline_trap_vector());

    arch_exception_handler(trapframe);

    arch_switch_to_user_space(trapframe);
}
