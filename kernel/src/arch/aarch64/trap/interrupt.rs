//! AArch64 interrupt trap handling

use crate::arch::{Trapframe, get_cpu};

const RESCHEDULE_SGI: u32 = 0;

/// Handle an IRQ taken at EL1.
///
/// On QEMU virt (GICv2), the virtual timer arrives as a PPI (typically ID 27).
/// The generic InterruptManager path will acknowledge+EOI the interrupt, but the
/// kernel still needs to run the timer tick logic to advance scheduling.
pub fn arch_irq_handler(trapframe: &mut Trapframe, trap_kind: usize) {
    let cpu_id = get_cpu().get_cpuid() as u32;
    let mut ran_scheduler = false;

    if trap_kind == 2 && crate::arch::interrupt::is_arch_timer_pending() {
        crate::timer::tick(trapframe);
        return;
    }

    let claimed =
        crate::interrupt::InterruptManager::global().claim_and_handle_external_interrupt(cpu_id);

    match claimed {
        Ok(Some(interrupt_id)) => {
            if interrupt_id == RESCHEDULE_SGI {
                crate::sched::scheduler::debug_log_reschedule_ipi(cpu_id as usize, false, true);
                crate::sched::scheduler::schedule(trapframe);
                ran_scheduler = true;
            } else if interrupt_id == crate::drivers::pic::arm_generic_timer::timer_ppi_irq() {
                crate::timer::tick(trapframe);
                ran_scheduler = true;
            }
        }
        Ok(None) => {
            if crate::arch::interrupt::is_arch_timer_pending() {
                crate::timer::tick(trapframe);
                ran_scheduler = true;
            }
        }
        Err(e) => {
            crate::println!(
                "[aarch64][irq] failed to claim/handle external interrupt: {}",
                e
            );
        }
    }

    let cpu_id = cpu_id as usize;
    if !ran_scheduler
        && crate::sched::scheduler::current_task_is_idle(cpu_id)
        && crate::sched::scheduler::has_ready_tasks(cpu_id)
    {
        crate::sched::scheduler::schedule(trapframe);
    }
}
