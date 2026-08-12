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
use crate::arch::{Trapframe, get_cpu, get_kernel_trapvector_paddr, set_trapvector};
use crate::vm::get_trampoline_trap_vector;

#[unsafe(export_name = "aarch64_first_switch_to_user_naked")]
#[unsafe(naked)]
pub unsafe extern "C" fn aarch64_first_switch_to_user_naked(
    trapframe_addr: usize,
    trap_exit_addr: usize,
) -> ! {
    naked_asm!(
        r#"
        // x0 = trapframe_addr
        // x1 = trap_exit_addr
        mov x0, x0
        br  x1
        "#
    );
}

#[unsafe(link_section = ".trampoline.text.0")]
#[unsafe(export_name = "_user_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _user_trap_entry() {
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

        // Current EL with SPx (Kernel Re-entry, Invalid for User)
        // Sync
        b   .
        .space 124
        // IRQ
        b   .
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
            sub sp, sp, #304 // Allocate space for Trapframe (16-byte aligned)
            str x9, [sp, #72] // Save x9
            mov x9, #0         // Kind = Sync
            b   1f
        11: // IRQ
            sub sp, sp, #304 // Allocate space for Trapframe (16-byte aligned)
            str x9, [sp, #72] // Save x9
            mov x9, #1         // Kind = IRQ
            b   1f
        12: // FIQ
            sub sp, sp, #304 // Allocate space for Trapframe (16-byte aligned)
            str x9, [sp, #72] // Save x9
            mov x9, #2         // Kind = FIQ
            b   1f
        13: // SError
            sub sp, sp, #304 // Allocate space for Trapframe (16-byte aligned)
            str x9, [sp, #72] // Save x9
            mov x9, #3         // Kind = SError
            b   1f

        // Common User Trap Entry
        // PRE: x9=Kind, sp=Trapframe ptr
        1:
            // 1. Save Context
            // Trapframe layout is 304 bytes (16-byte aligned)
            stp x0, x1, [sp, #0]
            stp x2, x3, [sp, #16]
            stp x4, x5, [sp, #32]
            stp x6, x7, [sp, #48]
            str x8, [sp, #64]
            // x9 is saved above
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

            mrs x10, sp_el0 // load user SP
            str x10, [sp, #248] // Trapframe.sp = user SP
            mrs x10, elr_el1 // load user PC
            str x10, [sp, #256] // Trapframe.epc = user PC
            mrs x10, spsr_el1 // load user SPSR
            str x10, [sp, #264] // Trapframe.spsr = user SPSR

            // Save user thread pointer registers
            mrs x10, tpidr_el0 // load user TPIDR_EL0
            str x10, [sp, #272] // Trapframe.tpidr_el0 = user TPIDR_EL0
            mrs x10, tpidrro_el0 // load user TPIDRRO_EL0
            str x10, [sp, #280] // Trapframe.tpidrro_el0 = user TPIDRRO_EL0

            // Save ESR_EL1/FAR_EL1 before any kernel-side fault can overwrite them.
            mrs x10, esr_el1 // load user ESR_EL1
            str x10, [sp, #288] // Trapframe.esr_el1 = user ESR_EL1
            mrs x10, far_el1 // load user FAR_EL1
            str x10, [sp, #296] // Trapframe.far_el1 = user FAR_EL1

            // 2. Switch TTBR (User -> Kernel)
            mrs x10, tpidr_el1    // x10 = CPU struct ptr
            mrs x11, ttbr0_el1   // Save current TTBR0 (user)
            str x11, [x10, #16] // arch.ttbr0 = user TTBR
            
            ldr x11, [x10, #40] // arch.kernel_ttbr0
            msr ttbr0_el1, x11 // Switch to kernel TTBR

            isb
            tlbi vmalle1
            dsb nsh
            isb

            // 3. Call Handler
            // fn arch_user_trap_handler(tf: &mut Trapframe, kind: usize)
            mov x1, x9         // arg1 = Kind
            mov x0, sp          // arg0 = Trapframe ptr
            
            ldr x10, [x10, #32] // arch_user_trap_handler (trampoline target)
            br  x10
        "#
    );
}

#[unsafe(link_section = ".trampoline.text.1")]
#[unsafe(export_name = "_switch_to_user")]
#[unsafe(naked)]
pub extern "C" fn _switch_to_user(_trapframe: &mut Trapframe) -> ! {
    naked_asm!(
        r#"
        // x0 = trapframe pointer --> sp = trapframe pointer
        mov sp, x0 // Set SP to trapframe pointer for easy access

        // Restore system registers from trapframe
        // Layout (304 bytes):
        //   0..240: x0-x30
        //   248:    sp (SP_EL0)
        //   256:    epc (ELR_EL1)
        //   264:    spsr (SPSR_EL1)
        //   272:    tpidr_el0 (TLS)
        //   280:    tpidrro_el0 (read-only at EL0)
        //   288:    esr_el1 (not restore)
        //   296:    far_el1 (not restore)

        // Check SPSR mode bits [3:0] to decide TTBR switch
        ldr x9, [sp, #264]  // Load Trapframe.spsr
        and x9, x9, #0xF    // Extract mode bits
        cmp x9, #0x5        // EL1h?
        b.eq 2f             // Skip TTBR switch for kernel tasks
        cmp x9, #0x9        // EL2h?
        b.eq 2f             // Skip TTBR switch for VHE host kernel tasks

        // --- User task path: switch TTBR back to user ---
        mrs x10, tpidr_el1 // x10 = CPU struct ptr
        ldr x9, [x10, #16]  // user ttbr0
        msr ttbr0_el1, x9   // Switch to user TTBR
        isb
        tlbi vmalle1
        dsb nsh
        isb
        b   3f

        2: // --- Kernel task path: no TTBR switch needed ---

        3:
        // Restore SP/PC/SPSR
        ldr x9, [sp, #248]  // Load Trapframe.sp
        msr sp_el0, x9      // Restore SP_EL0
        ldr x9, [sp, #256]  // Load Trapframe.epc
        msr elr_el1, x9     // Restore ELR_EL1
        ldr x9, [sp, #264]  // Load Trapframe.spsr
        msr spsr_el1, x9    // Restore SPSR_EL1

        // Restore TLS
        ldr x9, [sp, #272]
        msr tpidr_el0, x9
        ldr x9, [sp, #280]
        msr tpidrro_el0, x9

        // Restore GPRs
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

        // Deallocate trapframe space
        add sp, sp, #304

        eret
        "#
    );
}

#[unsafe(export_name = "arch_switch_to_user")]
pub fn arch_switch_to_user(trapframe: &mut Trapframe) -> ! {
    let cpu_id = crate::arch::get_cpu().get_cpuid();
    crate::breadcrumb::drop(
        crate::breadcrumb::SWITCH_TO_USER,
        crate::sched::scheduler::current_task_id(cpu_id)
            .map(|t| t as u64)
            .unwrap_or(0),
        trapframe.elr,
    );
    let addr = trapframe as *mut Trapframe as usize;

    crate::arch::configure_user_entry(
        trapframe,
        crate::arch::UserEntryOptions {
            irq_policy: crate::arch::UserReturnIrqPolicy::Enable,
        },
    );

    // Calculate the address of _switch_to_user in the trampoline
    let trap_exit_offset = (_switch_to_user as *const () as usize)
        .wrapping_sub(_user_trap_entry as *const () as usize);
    let trampoline_base = get_trampoline_trap_vector();
    let trap_exit_addr = trampoline_base.wrapping_add(trap_exit_offset);

    // Determine VBAR based on target EL:
    // - EL0 (user task): trampoline vector (handles Lower EL traps)
    // - EL1 (kernel task like idle): kernel vector (handles Current EL traps)
    if crate::arch::is_privileged_return_mode(trapframe.spsr) {
        // EL1h/EL2h — kernel task: use kernel vector so privileged traps are handled
        set_trapvector(get_kernel_trapvector_paddr());
    } else {
        // EL0t — user task: use trampoline vector
        set_trapvector(trampoline_base);
    }

    crate::timer::publish_arch_timer_diagnostic(
        cpu_id,
        crate::arch::timer::diagnostic_snapshot(trapframe.spsr, trapframe.elr),
    );
    crate::breadcrumb::drop(
        crate::breadcrumb::USER_RETURN_READY,
        trapframe.elr,
        trapframe.spsr,
    );

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
    crate::breadcrumb::drop(
        crate::breadcrumb::USER_TRAP_ENTER,
        trap_kind as u64,
        trapframe.elr,
    );
    // We are now executing in EL1; switch VBAR_EL1 to the kernel vector so that
    // any exceptions/IRQs that occur while in kernel mode are handled by the
    // simple kernel trap routine.
    set_trapvector(get_kernel_trapvector_paddr());
    crate::breadcrumb::drop(
        crate::breadcrumb::USER_TRAP_VECTOR_READY,
        trap_kind as u64,
        trapframe.elr,
    );

    let cpu_id = get_cpu().get_cpuid();
    let first_traced_user_trap = if crate::sched::scheduler::DEBUG_FORK_TRACE_LOGGING {
        crate::sched::scheduler::current_task_id(cpu_id).filter(|&task_id| {
            crate::sched::scheduler::take_fork_trace_first_user_trap(cpu_id, task_id)
        })
    } else {
        None
    };
    if let Some(task_id) = first_traced_user_trap {
        let task_asid = crate::sched::scheduler::get_task_by_id(task_id)
            .map(|task| task.vm_manager.get_asid())
            .unwrap_or(0);
        crate::early_println!(
            "[fork-trace] child_task_id={} first-user-trap cpu={} kind={} elr={:#x} esr={:#x} far={:#x} task_asid={} user_ttbr={:#x}",
            task_id,
            cpu_id,
            trap_kind,
            trapframe.elr,
            trapframe.esr_el1,
            trapframe.far_el1,
            task_asid,
            get_cpu().get_ttbr0(),
        );
    }

    // trap_kind is now passed in x1 (argument 2), so no need to read from memory!
    if trap_kind == 1 || trap_kind == 2 {
        crate::breadcrumb::drop(
            crate::breadcrumb::USER_IRQ_DISPATCH,
            trap_kind as u64,
            trapframe.elr,
        );
        arch_irq_handler(trapframe, trap_kind);
        crate::breadcrumb::drop(
            crate::breadcrumb::IRQ_DISPATCH_DONE,
            trap_kind as u64,
            trapframe.elr,
        );
    } else {
        arch_exception_handler(trapframe, trap_kind);
    }

    if let Some(task_id) = first_traced_user_trap {
        crate::early_println!(
            "[fork-trace] child_task_id={} first-user-trap-done cpu={} current={:?} elr={:#x} user_ttbr={:#x}",
            task_id,
            cpu_id,
            crate::sched::scheduler::current_task_id(cpu_id),
            trapframe.elr,
            get_cpu().get_ttbr0(),
        );
    }

    if crate::sched::scheduler::may_schedule_from_interrupt(cpu_id)
        && crate::sched::scheduler::take_deferred_reschedule(cpu_id)
    {
        crate::sched::scheduler::schedule(trapframe);
    }

    arch_switch_to_user(trapframe);
}
