use crate::arch::{Trapframe, get_cpu};
use crate::interrupt::InterruptManager;

/// RISC-V S-mode interrupt causes
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

/// Handle software interrupt (IPI)
///
/// Clears the SSIP bit to acknowledge the interrupt and triggers an
/// immediate reschedule on this CPU so that newly-enqueued tasks are
/// picked up promptly.
///
/// Unlike timer ticks, an IPI-triggered reschedule bypasses the time-slice
/// check — the whole point is to wake the CPU from idle (or preempt the
/// idle task) as soon as new work arrives.
fn handle_software_interrupt(trapframe: &mut Trapframe) {
    // Acknowledge the IPI by clearing the supervisor software interrupt pending bit.
    crate::arch::riscv64::instruction::sbi::sbi_clear_ipi();

    // Trigger an immediate reschedule — skip time_slice decrement.
    let scheduler = crate::sched::scheduler::get_scheduler();
    scheduler.schedule(trapframe);
}

/// Handle timer interrupt from CLINT
fn handle_timer_interrupt(trapframe: &mut Trapframe) {
    // Increment the global tick counter
    crate::timer::tick(trapframe);
}

/// Handle external interrupt from PLIC
fn handle_external_interrupt(trapframe: &mut Trapframe) {
    // PLIC hardware operations require the physical CPU ID (hart ID),
    // not the abstract kernel CPU_ID.
    let physical_id = get_cpu().get_hartid() as u32;

    // Claim and handle external interrupt through PLIC
    match InterruptManager::with_manager(|mgr| mgr.claim_and_handle_external_interrupt(physical_id))
    {
        Ok(Some(interrupt_id)) => {
            // crate::early_println!("[interrupt] Handled external interrupt {} on hart {}", interrupt_id, physical_id);
        }
        Ok(None) => {
            crate::early_println!(
                "[interrupt] No pending external interrupt on hart {}",
                physical_id
            );
        }
        Err(e) => {
            crate::early_println!("[interrupt] Failed to handle external interrupt: {}", e);
        }
    }
}

/// Handle unknown interrupt
fn handle_unknown_interrupt(trapframe: &mut Trapframe, cause: usize) {
    crate::early_println!("[interrupt] Unknown interrupt trapframe: {:x?}", trapframe);
    panic!("Unknown interrupt cause: {}", cause);
}
