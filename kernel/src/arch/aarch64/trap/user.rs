use core::arch::{asm, naked_asm};

use super::exception::arch_exception_handler;
use crate::arch::{Trapframe, set_trapvector};
use crate::vm::get_trampoline_trap_vector;
use crate::early_println;

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
        // AArch64 vectors are grouped by origin:
        //  - current EL with SP0 (0x000)
        //  - current EL with SPx (0x200)
        //  - lower EL using AArch64 (0x400)
        //  - lower EL using AArch32 (0x600)
        //
        // We must NOT run the EL0 trampoline entry (TTBR0 swap + stack rewrite)
        // when the exception/IRQ happens in EL1 (kernel). Doing so breaks kernel
        // execution during WFI/idle and prevents timer IRQ bring-up.
        //
        // So: current-EL vectors branch to kernel path; lower-EL vectors branch
        // to user/trampoline path.

        // current EL, SP0
        b   2f
        .space 124
        b   2f
        .space 124
        b   2f
        .space 124
        b   2f
        .space 124

        // current EL, SPx
        b   2f
        .space 124
        b   2f
        .space 124
        b   2f
        .space 124
        b   2f
        .space 124

        // lower EL, AArch64
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124

        // lower EL, AArch32 (unused)
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124
        b   1f
        .space 124

        // -----------------------------------------------------------------
        // EL0 -> EL1 entry (user/trampoline path):
        // - mask interrupts
        // - swap TTBR0 to kernel mappings
        // - switch to cpu.kernel_stack (top)
        // - save regs into Trapframe
        // - tail-call cpu.kernel_trap
        // -----------------------------------------------------------------
        1:
            // Disable interrupts
            msr daifset, #0xf

            // x16: cpu pointer (TPIDRRO_EL0 points to arch struct in trampoline)
            // TPIDRRO_EL0 is read-only from EL0, so user-space cannot clobber it.
            mrs x16, tpidrro_el0

            // Swap TTBR0_EL1 with cpu.ttbr0 (offset 16)
            ldr x17, [x16, #16]
            mrs x15, ttbr0_el1
            msr ttbr0_el1, x17
            isb
            // Ensure the new TTBR0 takes effect for subsequent low-VA accesses
            // (kernel stack + trapframe live in low VA space).
            tlbi vmalle1
            dsb ish
            isb
            str x15, [x16, #16]

            // Switch to per-task kernel stack top from cpu.kernel_stack (offset 24)
            ldr x17, [x16, #24]
            mov sp, x17

            // Allocate trapframe (keep SP 16-byte aligned; Trapframe is 272 bytes)
            sub sp, sp, #272

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

        // -----------------------------------------------------------------
        // EL1 entry (kernel path):
        // - mask interrupts
        // - DO NOT touch TTBR0
        // - DO NOT rewrite SP (we must return to the interrupted kernel frame)
        // - save regs into Trapframe on current kernel stack
        // - call arch_kernel_trap_handler(tf)
        // - restore regs and ERET back to EL1
        // -----------------------------------------------------------------
        2:
            // Disable interrupts
            msr daifset, #0xf

            // Allocate trapframe (keep SP 16-byte aligned; Trapframe is 272 bytes)
            sub sp, sp, #272

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

            // Save SP_EL0 into regs[31] (offset 248) (best-effort)
            mrs x17, sp_el0
            str x17, [sp, #248]

            // Save ELR_EL1 into epc (offset 256)
            mrs x17, elr_el1
            str x17, [sp, #256]

            // Call the kernel trap handler via indirect branch
            // x16: cpu pointer (TPIDR_EL1 points to arch struct)
            mrs x16, tpidr_el1
            // Jump to trap handler: cpu.kernel_trap (offset 32)
            ldr x17, [x16, #32]
            mov x0, sp
            br  x17

            // Restore ELR_EL1 from trapframe.epc
            ldr x1, [sp, #256]
            msr elr_el1, x1

            // Restore x0-x30
            ldp x0, x1, [sp, #0]
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

            // Pop trapframe
            add sp, sp, #272

            eret
        "#
        );
    }
}

#[unsafe(export_name = "arch_kernel_trap_handler")]
pub extern "C" fn arch_kernel_trap_handler(addr: usize) {
    use core::sync::atomic::{AtomicUsize, Ordering};
    
    static CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
    static LAST_SP: AtomicUsize = AtomicUsize::new(0);
    
    let count = CALL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let current_sp = addr;
    let last_sp = LAST_SP.swap(current_sp, Ordering::Relaxed);
    
    if count <= 5 {
        crate::early_println!("[kernel_trap] call #{}, sp={:#x}, last_sp={:#x}, diff={}",
            count, current_sp, last_sp, current_sp as isize - last_sp as isize);
    }
    
    let trapframe: &mut Trapframe = unsafe { &mut *(addr as *mut Trapframe) };
    arch_exception_handler(trapframe);
}

#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_user_trap_exit")]
#[unsafe(naked)]
pub extern "C" fn _user_trap_exit(_trapframe: &mut Trapframe) -> ! {
    unsafe {
        naked_asm!(
            r#"
        // x0 = trapframe

        // CRITICAL: Mask all interrupts before setting up user context
        // This prevents nested interrupts during TTBR0 swap and register restore
        msr daifset, #0xf

        // Breadcrumb: reached _user_trap_exit (direct PL011 putc)
        mov w11, #'E'
        movz x10, #0x0900, lsl #16     // x10 = 0x0900_0000
        1:
            ldr w12, [x10, #0x18]      // UART_FR
            tst w12, #0x20             // TXFF
            b.ne 1b
        str w11, [x10, #0x0]           // UART_DR

        // Restore ELR_EL1 from trapframe.epc (offset 256)
        ldr x1, [x0, #256]
        msr elr_el1, x1

        // Restore user SP (SP_EL0) from regs[31] (offset 248)
        ldr x1, [x0, #248]
        msr sp_el0, x1

        // Configure return to EL0t (User)
        // M[3:0] = 0b0000 (EL0t), DAIF = 0 (interrupts enabled)
        mov x1, #0x0
        msr spsr_el1, x1

        // Swap TTBR0_EL1 with cpu.ttbr0 (offset 16)
        mrs x16, tpidrro_el0
        ldr x17, [x16, #16]
        mrs x15, ttbr0_el1
        msr ttbr0_el1, x17
        isb
        // Ensure the restored TTBR0 takes effect before returning to EL0.
        tlbi vmalle1
        dsb ish
        isb
        str x15, [x16, #16]

        // Breadcrumb: past TTBR swap (direct PL011 putc)
        mov w11, #'R'
        movz x10, #0x0900, lsl #16     // x10 = 0x0900_0000
        2:
            ldr w12, [x10, #0x18]      // UART_FR
            tst w12, #0x20             // TXFF
            b.ne 2b
        str w11, [x10, #0x0]           // UART_DR

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
    // Breadcrumb: we arrived in the trap handler from EL0.
    early_println!("[aarch64] trap_handler: entered, tf_addr={:#x}", addr);

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

    early_println!(
        "[aarch64] tramp calc: base={:#x} exit_off={:#x} exit_addr={:#x} sym_entry={:#x} sym_exit={:#x}",
        trampoline_base,
        trap_exit_offset,
        trap_exit_addr,
        _user_trap_entry as usize,
        _user_trap_exit as usize,
    );

    set_trapvector(trampoline_base);

    // Minimal breadcrumb so we can see we're about to ERET.
    // If we time out after this, the hang is likely in trampoline return or the first user instruction.
    unsafe {
        let current_el: usize;
        let tpidr_el1: usize;
        let tpidr_el0: usize;
        let tpidrro_el0: usize;
        let ttbr0_el1: usize;
        let ttbr1_el1: usize;
        let vbar_el1: usize;
        core::arch::asm!(
            "mrs {current_el}, CurrentEL",
            "mrs {tpidr_el1}, tpidr_el1",
            "mrs {tpidr_el0}, tpidr_el0",
            "mrs {tpidrro_el0}, tpidrro_el0",
            "mrs {ttbr0_el1}, ttbr0_el1",
            "mrs {ttbr1_el1}, ttbr1_el1",
            "mrs {vbar_el1}, vbar_el1",
            current_el = out(reg) current_el,
            tpidr_el1 = out(reg) tpidr_el1,
            tpidr_el0 = out(reg) tpidr_el0,
            tpidrro_el0 = out(reg) tpidrro_el0,
            ttbr0_el1 = out(reg) ttbr0_el1,
            ttbr1_el1 = out(reg) ttbr1_el1,
            vbar_el1 = out(reg) vbar_el1,
            options(nostack)
        );
        early_println!(
            "[aarch64] switch_to_user: el={:#x} tf={:#x} epc={:#x} sp={:#x} tramp={:#x} vbar={:#x} tpidr_el1={:#x} tpidr_el0={:#x} tpidrro_el0={:#x} ttbr0={:#x} ttbr1={:#x}",
            current_el,
            addr,
            trapframe.epc as usize,
            trapframe.regs.reg[31],
            trampoline_base,
            vbar_el1,
            tpidr_el1,
            tpidr_el0,
            tpidrro_el0,
            ttbr0_el1,
            ttbr1_el1,
        );
    }

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
