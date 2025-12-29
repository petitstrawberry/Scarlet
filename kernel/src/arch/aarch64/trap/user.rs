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

            // Switch TTBR0_EL1 back to the kernel page table.
            // - Save current (user) TTBR0_EL1 into cpu.ttbr0 (offset 16)
            // - Save current (user) TTBR1_EL1 into cpu.scratch (offset 0)
            // - Load cpu.kernel_ttbr0 (offset 40) and install it into TTBR0_EL1
            mrs x15, ttbr0_el1
            str x15, [x16, #16]
            mrs x15, ttbr1_el1
            str x15, [x16, #0]
            ldr x17, [x16, #40]
            msr ttbr0_el1, x17
            msr ttbr1_el1, x17
            isb
            // Ensure the new TTBR0 takes effect for subsequent low-VA accesses
            // (kernel stack + trapframe live in low VA space).
            dsb ish
            tlbi vmalle1is
            dsb ish
            isb

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
        // x0 = trapframe pointer (kernel space address)
        // Strategy: mimic RISC-V exactly
        // 1. Restore all registers except x0
        // 2. Save x0's final value to Aarch64.scratch
        // 3. Load Aarch64 pointer to x0
        // 4. Swap TTBR0 using x0 as base for struct access
        // 5. Restore x0 from Aarch64.scratch

        // CRITICAL: Mask all interrupts
        msr daifset, #0xf

        // Restore ELR_EL1 and SP_EL0
        ldr x1, [x0, #256]       // ELR from trapframe.epc
        msr elr_el1, x1
        ldr x1, [x0, #248]       // SP from regs[31]
        msr sp_el0, x1

        // Configure SPSR for EL0 return
        mov x1, #0x0
        msr spsr_el1, x1

        // Save final user x0 and user x1 into per-CPU trampoline storage.
        // We must do this *before* restoring registers, because we will later
        // need temporary registers for the TTBR switch and cannot touch the
        // kernel trapframe after switching to the user TTBR.
        //
        // Use x2/x3/x4 as temporaries here; they will be restored from the
        // trapframe below.
        ldr x2, [x0, #0]         // x2 = final user x0
        ldr x3, [x0, #8]         // x3 = user x1
        mrs x4, tpidrro_el0      // x4 = Aarch64 pointer
        str x2, [x4, #0]         // Aarch64.scratch   = final x0
        str x3, [x4, #48]        // Aarch64.scratch_x1 = user x1

        // Restore x1-x30 (all registers except x0)
        ldp x1, x2, [x0, #8]
        ldp x3, x4, [x0, #24]
        ldp x5, x6, [x0, #40]
        ldp x7, x8, [x0, #56]
        ldp x9, x10, [x0, #72]
        ldp x11, x12, [x0, #88]
        ldp x13, x14, [x0, #104]
        ldr x15, [x0, #120]
        ldr x16, [x0, #128]
        ldr x17, [x0, #136]
        ldp x18, x19, [x0, #144]
        ldp x20, x21, [x0, #160]
        ldp x22, x23, [x0, #176]
        ldp x24, x25, [x0, #192]
        ldp x26, x27, [x0, #208]
        ldp x28, x29, [x0, #224]
        ldr x30, [x0, #240]

        // Get Aarch64 pointer (x0 loses trapframe ptr)
        mrs x0, tpidrro_el0

        // Load user TTBR0 from Aarch64.ttbr0 (offset 16) using x1 as temp.
        // This will clobber x1, so we restore x1 from Aarch64.scratch_x1 after.
        ldr x1, [x0, #16]
        
        // Switch page table
        msr ttbr0_el1, x1
        msr ttbr1_el1, x1
        isb
        dsb ish
        tlbi vmalle1is
        dsb ish
        isb

        // Restore x1 and x0 from trampoline scratch
        ldr x1, [x0, #48]
        ldr x0, [x0, #0]
        
        // All registers now restored, page table switched
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

    // Guard: never ERET to VA 0.
    // If EPC is 0 here, returning to EL0 will immediately take an instruction abort.
    // Prefer the per-task saved PC if available; otherwise fall back to the default
    // user entry used by AArch64 VCPU bring-up.
    if trapframe.epc == 0 {
        let fallback_pc = crate::task::mytask()
            .map(|t| t.vcpu.get_pc())
            .filter(|pc| *pc != 0)
            .unwrap_or(0x10000);
        crate::early_println!(
            "[aarch64] fixup user pc: epc=0 -> {:#x}",
            fallback_pc
        );
        trapframe.epc = fallback_pc;
    }

    // Policy: the kernel is responsible for providing a valid initial user stack pointer.
    // During bring-up we've observed non-canonical / out-of-range SP values reaching EL0.
    // Keep this fix strictly AArch64-local: canonicalize and clamp SP into the allocated
    // user stack VMA before the trampoline writes SP_EL0.
    {
        let raw_sp = trapframe.regs.reg[31];
        let mut sp = raw_sp & crate::environment::VMMAX;

        // USER_STACK_END is the exclusive top of the user stack.
        // Lower bounds may change at runtime if we grow the stack on-demand
        // in the data-abort handler, so don't clamp SP just because it moved down.
        let stack_end = crate::environment::USER_STACK_END;

        if sp >= stack_end {
            crate::early_println!(
                "[aarch64] fixup user sp: raw={:#x} canon={:#x} end={:#x}",
                raw_sp,
                sp,
                stack_end
            );
            // Keep some headroom below the exclusive end so early prologues that
            // use small positive offsets from SP don't touch `stack_end`.
            sp = stack_end - crate::environment::PAGE_SIZE;
        }

        // AArch64 ABI requires 16-byte alignment.
        sp &= !0xF;
        trapframe.regs.reg[31] = sp;
    }

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
