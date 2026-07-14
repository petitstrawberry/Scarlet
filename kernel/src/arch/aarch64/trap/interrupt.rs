//! AArch64 interrupt trap handling

use crate::arch::{Trapframe, get_cpu};

fn can_schedule_from_interrupt(spsr: u64, current_is_idle: bool) -> bool {
    !crate::arch::is_privileged_return_mode(spsr) || current_is_idle
}

/// Handle an IRQ taken at EL1.
///
/// On QEMU virt (GICv2), the virtual timer arrives as a PPI (typically ID 27).
/// The generic InterruptManager path will acknowledge+EOI the interrupt, but the
/// kernel still needs to run the timer tick logic to advance scheduling.
pub fn arch_irq_handler(trapframe: &mut Trapframe, trap_kind: usize) {
    let cpu_id = get_cpu().get_cpuid() as u32;
    let can_schedule = can_schedule_from_interrupt(
        trapframe.spsr,
        crate::sched::scheduler::current_task_is_idle(cpu_id as usize),
    );
    let mut ran_scheduler = false;

    if trap_kind == 2 {
        if crate::arch::interrupt::is_arch_timer_pending() {
            crate::timer::tick_with_scheduler(trapframe, can_schedule);
            return;
        }
        if crate::arch::interrupt::timer_interrupt_route()
            == crate::arch::interrupt::TimerInterruptRoute::FastInterrupt
        {
            match crate::interrupt::InterruptManager::global().claim_fast_interrupt(cpu_id) {
                Ok(crate::interrupt::InterruptClaim::Reschedule) => {
                    if can_schedule {
                        crate::sched::scheduler::schedule(trapframe);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    crate::println!("[aarch64][fiq] failed to claim fast interrupt: {}", e);
                }
            }
            return;
        }
    }

    let claimed = crate::interrupt::InterruptManager::global()
        .claim_and_handle_pending_external_interrupt(cpu_id);

    match claimed {
        Ok(Some(pending)) => {
            let interrupt_id = pending.mapping.virq;
            if crate::arch::interrupt::is_reschedule_interrupt(&pending) {
                if can_schedule {
                    crate::sched::scheduler::schedule(trapframe);
                    ran_scheduler = true;
                }
            } else if interrupt_id == crate::drivers::pic::arm_generic_timer::timer_ppi_irq() {
                crate::timer::tick_with_scheduler(trapframe, can_schedule);
                ran_scheduler = can_schedule;
            }
        }
        Ok(None) => {
            if crate::arch::interrupt::is_arch_timer_pending() {
                crate::timer::tick_with_scheduler(trapframe, can_schedule);
                ran_scheduler = can_schedule;
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

#[cfg(test)]
mod tests {
    use super::can_schedule_from_interrupt;

    #[test_case]
    fn user_interrupt_may_schedule() {
        assert!(can_schedule_from_interrupt(0, false));
    }

    #[test_case]
    fn busy_kernel_interrupt_must_not_schedule() {
        assert!(!can_schedule_from_interrupt(0x9, false));
    }

    #[test_case]
    fn idle_kernel_interrupt_may_schedule() {
        assert!(can_schedule_from_interrupt(0x9, true));
    }
}
