//! AArch64 interrupt trap handling

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::{Trapframe, get_cpu};
use crate::environment::MAX_NUM_CPUS;

const IRQ_PENDING_BIT: u64 = 1 << 7;
const IRQ_LIVENESS_INTERVAL: u64 = 500;

static TIMER_FIQ_COUNTS: [AtomicU64; MAX_NUM_CPUS] = [const { AtomicU64::new(0) }; MAX_NUM_CPUS];

fn can_schedule_from_interrupt(spsr: u64, current_is_idle: bool) -> bool {
    !crate::arch::is_privileged_return_mode(spsr) || current_is_idle
}

fn report_timer_fiq_irq_liveness(cpu_id: usize, trapframe: &Trapframe) {
    let count = TIMER_FIQ_COUNTS[cpu_id].fetch_add(1, Ordering::Relaxed) + 1;
    if count > 3 && !count.is_multiple_of(IRQ_LIVENESS_INTERVAL) {
        return;
    }

    let isr: u64;
    let daif: u64;
    let hcr: u64;
    let vgic_hcr: u64;
    let vgic_misr: u64;
    // SAFETY: The handler runs in privileged AArch64 context. Direct Limine
    // VHE boot executes at EL2, while non-VHE environments report zero for the
    // EL2-only diagnostic value without changing interrupt-controller state.
    unsafe {
        asm!(
            "mrs {isr}, isr_el1",
            "mrs {daif}, daif",
            isr = out(reg) isr,
            daif = out(reg) daif,
            options(nomem, nostack, preserves_flags)
        );
        if crate::arch::is_vhe_enabled() {
            asm!(
                "mrs {hcr}, hcr_el2",
                "mrs {vgic_hcr}, ich_hcr_el2",
                "mrs {vgic_misr}, ich_misr_el2",
                hcr = out(reg) hcr,
                vgic_hcr = out(reg) vgic_hcr,
                vgic_misr = out(reg) vgic_misr,
                options(nomem, nostack, preserves_flags)
            );
        } else {
            hcr = 0;
            vgic_hcr = 0;
            vgic_misr = 0;
        }
    }

    let current_task = crate::sched::scheduler::current_task_id(cpu_id);
    let current_is_idle = crate::sched::scheduler::current_task_is_idle(cpu_id);

    crate::early_println!(
        "[irq-liveness] cpu={} timer={} task={:?} idle={} irq_pending={} isr={:#x} daif={:#x} spsr={:#x} hcr={:#x} ich_hcr={:#x} ich_misr={:#x}",
        cpu_id,
        count,
        current_task,
        current_is_idle,
        isr & IRQ_PENDING_BIT != 0,
        isr,
        daif,
        trapframe.spsr,
        hcr,
        vgic_hcr,
        vgic_misr
    );
}

/// Handle an IRQ taken at EL1.
///
/// On QEMU virt (GICv2), the virtual timer arrives as a PPI (typically ID 27).
/// The generic InterruptManager path will acknowledge+EOI the interrupt, but the
/// kernel still needs to run the timer tick logic to advance scheduling.
pub fn arch_irq_handler(trapframe: &mut Trapframe, trap_kind: usize) {
    let cpu_id = get_cpu().get_cpuid() as u32;
    let from_kernel = crate::arch::is_privileged_return_mode(trapframe.spsr);
    let can_schedule = can_schedule_from_interrupt(
        trapframe.spsr,
        crate::sched::scheduler::current_task_is_idle(cpu_id as usize),
    ) && crate::sched::scheduler::may_schedule_from_interrupt(cpu_id as usize);
    let mut ran_scheduler = false;

    if trap_kind == 2 {
        if crate::arch::interrupt::is_arch_timer_pending() {
            report_timer_fiq_irq_liveness(cpu_id as usize, trapframe);
            crate::timer::tick_with_scheduler(trapframe, can_schedule);
            return;
        }
        if crate::arch::interrupt::timer_interrupt_route()
            == crate::arch::interrupt::TimerInterruptRoute::FastInterrupt
        {
            match crate::interrupt::InterruptManager::global().claim_fast_interrupt(cpu_id) {
                Ok(crate::interrupt::InterruptClaim::Reschedule) => {
                    crate::sched::scheduler::debug_log_reschedule_ipi(
                        cpu_id as usize,
                        from_kernel,
                        can_schedule,
                    );
                    if can_schedule {
                        let _ = crate::sched::scheduler::take_deferred_reschedule(cpu_id as usize);
                        crate::sched::scheduler::schedule(trapframe);
                    } else {
                        crate::sched::scheduler::defer_reschedule(cpu_id as usize);
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
                crate::sched::scheduler::debug_log_reschedule_ipi(
                    cpu_id as usize,
                    from_kernel,
                    can_schedule,
                );
                if can_schedule {
                    let _ = crate::sched::scheduler::take_deferred_reschedule(cpu_id as usize);
                    crate::sched::scheduler::schedule(trapframe);
                    ran_scheduler = true;
                } else {
                    crate::sched::scheduler::defer_reschedule(cpu_id as usize);
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
        && crate::sched::scheduler::may_schedule_from_interrupt(cpu_id)
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
