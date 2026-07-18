use crate::arch::trap::{PRIV_S_MODE, prev_mode};
use crate::arch::{Trapframe, get_cpu};

const SUPERVISOR_SOFTWARE_INTERRUPT: usize = 1;
const SUPERVISOR_TIMER_INTERRUPT: usize = 5;
const SUPERVISOR_EXTERNAL_INTERRUPT: usize = 9;

pub fn arch_interrupt_handler(trapframe: &mut Trapframe, cause: usize) {
    let from_kernel = prev_mode() == PRIV_S_MODE;
    match cause {
        SUPERVISOR_SOFTWARE_INTERRUPT => handle_software_interrupt(trapframe, from_kernel),
        SUPERVISOR_TIMER_INTERRUPT => handle_timer_interrupt(trapframe, from_kernel),
        SUPERVISOR_EXTERNAL_INTERRUPT => handle_external_interrupt(trapframe),
        _ => handle_unknown_interrupt(trapframe, cause),
    }
}

fn can_schedule_from_interrupt(from_kernel: bool) -> bool {
    if !from_kernel {
        return true;
    }
    let cpu_id = get_cpu().get_cpuid();
    crate::sched::scheduler::current_task_is_idle(cpu_id)
}

fn handle_software_interrupt(trapframe: &mut Trapframe, from_kernel: bool) {
    // Clear SSIP (Supervisor Software Interrupt Pending) to prevent
    // re-triggering. SBI send_ipi sets MSIP via M-mode, which fires
    // SSIP in S-mode. We must clear it here.
    unsafe {
        core::arch::asm!(
            "csrc sip, {0}",
            in(reg) 1 << 1,
            options(nostack)
        );
    }

    let cpu_id = get_cpu().get_cpuid();
    crate::sched::scheduler::acknowledge_reschedule_ipi(cpu_id);
    let can_schedule = can_schedule_from_interrupt(from_kernel)
        && crate::sched::scheduler::may_schedule_from_interrupt(cpu_id);
    crate::sched::scheduler::debug_log_reschedule_ipi(cpu_id, from_kernel, can_schedule);

    if can_schedule {
        let _ = crate::sched::scheduler::take_deferred_reschedule(cpu_id);
        crate::sched::scheduler::schedule(trapframe);
    } else {
        crate::sched::scheduler::defer_reschedule(cpu_id);
    }
}

/// Handle timer interrupt from CLINT
fn handle_timer_interrupt(trapframe: &mut Trapframe, from_kernel: bool) {
    #[cfg(feature = "hypervisor")]
    {
        if crate::arch::hv::trap::is_from_guest() {
            use crate::arch::hv::switch::arch_guest_trap_exit;
            unsafe {
                arch_guest_trap_exit();
            }
            unreachable!();
        }
    }

    // Increment the global tick counter.  Only run scheduler accounting when
    // the trapframe is safe to store as the current task context.
    crate::timer::tick_with_scheduler(trapframe, can_schedule_from_interrupt(from_kernel));
}

/// Handle external interrupt from PLIC
fn handle_external_interrupt(trapframe: &mut Trapframe) {
    let cpu_id = get_cpu().get_cpuid() as u32;

    // Claim and handle external interrupt through PLIC
    match crate::interrupt::InterruptManager::global().claim_and_handle_external_interrupt(cpu_id) {
        Ok(Some(_interrupt_id)) => {
            // crate::println!("[interrupt] Handled external interrupt {} on CPU {}", interrupt_id, cpu_id);
        }
        Ok(None) => {
            crate::println!(
                "[interrupt] No pending external interrupt on CPU {}",
                cpu_id
            );
        }
        Err(e) => {
            crate::println!("[interrupt] Failed to handle external interrupt: {}", e);
        }
    }

    let cpu_id = cpu_id as usize;
    if crate::sched::scheduler::may_schedule_from_interrupt(cpu_id)
        && crate::sched::scheduler::current_task_is_idle(cpu_id)
        && crate::sched::scheduler::has_ready_tasks(cpu_id)
    {
        crate::sched::scheduler::schedule(trapframe);
    }
}

/// Handle unknown interrupt
fn handle_unknown_interrupt(trapframe: &mut Trapframe, cause: usize) {
    crate::println!("[interrupt] Unknown interrupt trapframe: {:x?}", trapframe);
    panic!("Unknown interrupt cause: {}", cause);
}
