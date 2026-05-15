use crate::arch::{Trapframe, get_cpu};

const SUPERVISOR_SOFTWARE_INTERRUPT: usize = 1;
const SUPERVISOR_TIMER_INTERRUPT: usize = 5;
const SUPERVISOR_EXTERNAL_INTERRUPT: usize = 9;

pub fn arch_interrupt_handler(trapframe: &mut Trapframe, cause: usize) {
    match cause {
        SUPERVISOR_SOFTWARE_INTERRUPT => handle_software_interrupt(trapframe),
        SUPERVISOR_TIMER_INTERRUPT => handle_timer_interrupt(trapframe),
        SUPERVISOR_EXTERNAL_INTERRUPT => handle_external_interrupt(trapframe),
        _ => handle_unknown_interrupt(trapframe, cause),
    }
}

fn handle_software_interrupt(trapframe: &mut Trapframe) {
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

    crate::sched::scheduler::schedule(trapframe);
}

/// Handle timer interrupt from CLINT
fn handle_timer_interrupt(trapframe: &mut Trapframe) {
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

    // Increment the global tick counter
    crate::timer::tick(trapframe);
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
    if crate::sched::scheduler::current_task_is_idle(cpu_id)
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
