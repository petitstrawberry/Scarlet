//! AArch64 interrupt trap handling

use core::arch::asm;
#[cfg(feature = "sync-debug")]
use core::sync::atomic::AtomicU32;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::{Trapframe, get_cpu};
use crate::environment::MAX_NUM_CPUS;

const IRQ_PENDING_BIT: u64 = 1 << 7;
const IRQ_LIVENESS_INTERVAL: u64 = 500;
const DEBUG_IRQ_LIVENESS_LOGGING: bool = false;

#[cfg(feature = "sync-debug")]
const IRQ_STORM_TRACKED_IRQS: usize = 1024;
#[cfg(feature = "sync-debug")]
const IRQ_STORM_SAMPLE_EVENTS: u64 = 256;
#[cfg(feature = "sync-debug")]
const IRQ_STORM_MIN_WINDOW_NS: u64 = 100_000_000;
#[cfg(feature = "sync-debug")]
const IRQ_STORM_RATE_THRESHOLD: u64 = 2_000;
#[cfg(feature = "sync-debug")]
const IRQ_STORM_REPORT_INTERVAL_NS: u64 = 1_000_000_000;

static TIMER_FIQ_COUNTS: [AtomicU64; MAX_NUM_CPUS] = [const { AtomicU64::new(0) }; MAX_NUM_CPUS];
static POST_CLAIM_TIMER_COUNTS: [AtomicU64; MAX_NUM_CPUS] =
    [const { AtomicU64::new(0) }; MAX_NUM_CPUS];

#[cfg(feature = "sync-debug")]
static IRQ_STORM_WINDOW_START_NS: [AtomicU64; MAX_NUM_CPUS] =
    [const { AtomicU64::new(0) }; MAX_NUM_CPUS];
#[cfg(feature = "sync-debug")]
static IRQ_STORM_WINDOW_TOTALS: [AtomicU64; MAX_NUM_CPUS] =
    [const { AtomicU64::new(0) }; MAX_NUM_CPUS];
#[cfg(feature = "sync-debug")]
static IRQ_STORM_LAST_REPORT_NS: [AtomicU64; MAX_NUM_CPUS] =
    [const { AtomicU64::new(0) }; MAX_NUM_CPUS];
#[cfg(feature = "sync-debug")]
static IRQ_STORM_COUNTS: [AtomicU32; MAX_NUM_CPUS * IRQ_STORM_TRACKED_IRQS] =
    [const { AtomicU32::new(0) }; MAX_NUM_CPUS * IRQ_STORM_TRACKED_IRQS];

#[cfg(feature = "sync-debug")]
fn irq_rate_per_second(events: u64, elapsed_ns: u64) -> u64 {
    if elapsed_ns == 0 {
        return u64::MAX;
    }
    ((u128::from(events) * 1_000_000_000u128) / u128::from(elapsed_ns)).min(u128::from(u64::MAX))
        as u64
}

#[cfg(feature = "sync-debug")]
fn record_external_irq_rate(cpu_id: usize, interrupt_id: u32) {
    if cpu_id >= MAX_NUM_CPUS {
        return;
    }

    // The architected timer is a deliberately high-rate PPI, not an external
    // device interrupt storm. Keep it out of this detector so normal scheduler
    // ticks cannot hide a genuinely stuck SPI. SGIs remain visible because an
    // IPI storm is independently actionable.
    if interrupt_id == crate::drivers::pic::arm_generic_timer::timer_ppi_irq() {
        return;
    }

    if let Ok(interrupt_index) = usize::try_from(interrupt_id)
        && interrupt_index < IRQ_STORM_TRACKED_IRQS
    {
        let index = cpu_id * IRQ_STORM_TRACKED_IRQS + interrupt_index;
        IRQ_STORM_COUNTS[index].fetch_add(1, Ordering::Relaxed);
    }

    let total = IRQ_STORM_WINDOW_TOTALS[cpu_id].fetch_add(1, Ordering::Relaxed) + 1;
    if total == 1 {
        IRQ_STORM_WINDOW_START_NS[cpu_id]
            .store(crate::timer::get_time_ns().max(1), Ordering::Relaxed);
        return;
    }
    if !total.is_multiple_of(IRQ_STORM_SAMPLE_EVENTS) {
        return;
    }

    let now_ns = crate::timer::get_time_ns();
    let start_ns = IRQ_STORM_WINDOW_START_NS[cpu_id].load(Ordering::Relaxed);
    let elapsed_ns = now_ns.saturating_sub(start_ns);
    if start_ns == 0 || elapsed_ns < IRQ_STORM_MIN_WINDOW_NS {
        return;
    }

    let observed_total = IRQ_STORM_WINDOW_TOTALS[cpu_id].swap(0, Ordering::Relaxed);
    IRQ_STORM_WINDOW_START_NS[cpu_id].store(now_ns.max(1), Ordering::Relaxed);

    let mut busiest_irq = u32::MAX;
    let mut busiest_count = 0u32;
    let first = cpu_id * IRQ_STORM_TRACKED_IRQS;
    for (offset, count) in IRQ_STORM_COUNTS[first..first + IRQ_STORM_TRACKED_IRQS]
        .iter()
        .enumerate()
    {
        let observed = count.swap(0, Ordering::Relaxed);
        if observed > busiest_count {
            busiest_count = observed;
            busiest_irq = offset as u32;
        }
    }

    let total_rate = irq_rate_per_second(observed_total, elapsed_ns);
    let busiest_rate = irq_rate_per_second(u64::from(busiest_count), elapsed_ns);
    if busiest_rate < IRQ_STORM_RATE_THRESHOLD {
        return;
    }

    let last_report_ns = IRQ_STORM_LAST_REPORT_NS[cpu_id].load(Ordering::Relaxed);
    if last_report_ns != 0 && now_ns.saturating_sub(last_report_ns) < IRQ_STORM_REPORT_INTERVAL_NS {
        return;
    }
    if IRQ_STORM_LAST_REPORT_NS[cpu_id]
        .compare_exchange(
            last_report_ns,
            now_ns.max(1),
            Ordering::AcqRel,
            Ordering::Relaxed,
        )
        .is_err()
    {
        return;
    }

    crate::emergency_println!(
        "[irq-storm] cpu={} source_rate={}/s total_rate={}/s window_us={} total={} busiest_irq={} busiest_count={} current_irq={}",
        cpu_id,
        busiest_rate,
        total_rate,
        elapsed_ns / 1_000,
        observed_total,
        busiest_irq,
        busiest_count,
        interrupt_id
    );
}

#[inline]
const fn fast_timer_pending(timer_pending_before: bool, timer_pending_after: bool) -> bool {
    timer_pending_before || timer_pending_after
}

fn can_schedule_from_interrupt(spsr: u64, current_is_idle: bool) -> bool {
    !crate::arch::is_privileged_return_mode(spsr) || current_is_idle
}

#[inline]
fn handle_observed_local_timer_irq(cpu_id: usize, trapframe: &Trapframe, from_kernel: bool) {
    crate::sched::scheduler::record_current_task_pc(cpu_id, trapframe.elr, from_kernel);
    crate::timer::handle_local_timer_irq();
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

fn report_post_claim_timer(cpu_id: usize) {
    let count = POST_CLAIM_TIMER_COUNTS[cpu_id].fetch_add(1, Ordering::Relaxed) + 1;
    if count <= 3 || count.is_power_of_two() {
        crate::early_println!(
            "[aarch64][fiq] timer became pending during fast claim cpu={} count={}",
            cpu_id,
            count
        );
    }
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
    crate::breadcrumb::drop(
        crate::breadcrumb::IRQ_ROUTE_READY,
        ((trap_kind as u64) << 32) | u64::from(cpu_id),
        u64::from(can_schedule) | (u64::from(from_kernel) << 1),
    );

    if trap_kind == 2 {
        let timer_pending_before = crate::arch::interrupt::is_arch_timer_pending();
        if crate::arch::interrupt::timer_interrupt_route()
            == crate::arch::interrupt::TimerInterruptRoute::FastInterrupt
        {
            // The architectural timer is a level-triggered FIQ source. Mask it
            // before claiming other Apple FIQ sources so the live level cannot
            // remain asserted while the handler enters the scheduler. The tick
            // path programs the next compare value before unmasking it again.
            if timer_pending_before {
                crate::arch::interrupt::disable_timer_source_interrupt();
            }
            let claim = crate::interrupt::InterruptManager::global().claim_fast_interrupt(cpu_id);
            let timer_pending_after = crate::arch::interrupt::is_arch_timer_pending();
            if timer_pending_after {
                crate::arch::interrupt::disable_timer_source_interrupt();
            }
            if DEBUG_IRQ_LIVENESS_LOGGING && timer_pending_after && !timer_pending_before {
                report_post_claim_timer(cpu_id as usize);
            }
            let timer_pending = fast_timer_pending(timer_pending_before, timer_pending_after);
            if timer_pending {
                if DEBUG_IRQ_LIVENESS_LOGGING {
                    report_timer_fiq_irq_liveness(cpu_id as usize, trapframe);
                }
                handle_observed_local_timer_irq(cpu_id as usize, trapframe, from_kernel);
            }

            match claim {
                Ok(crate::interrupt::InterruptClaim::Reschedule) => {
                    crate::sched::scheduler::acknowledge_reschedule_ipi(cpu_id as usize);
                    crate::sched::scheduler::debug_log_reschedule_ipi(
                        cpu_id as usize,
                        from_kernel,
                        can_schedule,
                    );
                    let timer_scheduled = if timer_pending {
                        crate::sched::scheduler::handle_timer_reschedule(
                            cpu_id as usize,
                            trapframe,
                            can_schedule,
                        )
                    } else {
                        false
                    };
                    if !timer_scheduled {
                        if can_schedule {
                            let _ =
                                crate::sched::scheduler::take_deferred_reschedule(cpu_id as usize);
                            crate::sched::scheduler::schedule(trapframe);
                        } else {
                            crate::sched::scheduler::defer_reschedule(cpu_id as usize);
                        }
                    }
                }
                Ok(_) => {
                    if timer_pending {
                        crate::sched::scheduler::handle_timer_reschedule(
                            cpu_id as usize,
                            trapframe,
                            can_schedule,
                        );
                    }
                }
                Err(e) => {
                    crate::println!("[aarch64][fiq] failed to claim fast interrupt: {}", e);
                    if timer_pending {
                        crate::sched::scheduler::handle_timer_reschedule(
                            cpu_id as usize,
                            trapframe,
                            can_schedule,
                        );
                    }
                }
            }
            return;
        }

        let timer_pending = timer_pending_before;
        if timer_pending {
            if DEBUG_IRQ_LIVENESS_LOGGING {
                report_timer_fiq_irq_liveness(cpu_id as usize, trapframe);
            }
            handle_observed_local_timer_irq(cpu_id as usize, trapframe, from_kernel);
            crate::sched::scheduler::handle_timer_reschedule(
                cpu_id as usize,
                trapframe,
                can_schedule,
            );
            return;
        }
    }

    crate::breadcrumb::drop(
        crate::breadcrumb::IRQ_CONTROLLER_WAIT,
        cpu_id as u64,
        trap_kind as u64,
    );
    let claimed = crate::interrupt::InterruptManager::global()
        .claim_and_handle_pending_external_interrupt(cpu_id);
    #[cfg(feature = "sync-debug")]
    if let Ok(Some(pending)) = &claimed {
        record_external_irq_rate(cpu_id as usize, pending.mapping.hwirq);
    }
    crate::breadcrumb::drop(
        crate::breadcrumb::IRQ_HANDLE_DONE,
        cpu_id as u64,
        match &claimed {
            Ok(Some(pending)) => u64::from(pending.mapping.virq),
            Ok(None) => u64::MAX,
            Err(_) => u64::MAX - 1,
        },
    );

    match claimed {
        Ok(Some(pending)) => {
            let interrupt_id = pending.mapping.virq;
            if crate::arch::interrupt::is_reschedule_interrupt(&pending) {
                crate::sched::scheduler::acknowledge_reschedule_ipi(cpu_id as usize);
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
                handle_observed_local_timer_irq(cpu_id as usize, trapframe, from_kernel);
                ran_scheduler = crate::sched::scheduler::handle_timer_reschedule(
                    cpu_id as usize,
                    trapframe,
                    can_schedule,
                );
            }
        }
        Ok(None) => {
            if crate::arch::interrupt::is_arch_timer_pending() {
                handle_observed_local_timer_irq(cpu_id as usize, trapframe, from_kernel);
                ran_scheduler = crate::sched::scheduler::handle_timer_reschedule(
                    cpu_id as usize,
                    trapframe,
                    can_schedule,
                );
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
    use super::{can_schedule_from_interrupt, fast_timer_pending};

    #[test_case]
    fn fast_timer_pending_includes_post_claim_transition() {
        assert!(!fast_timer_pending(false, false));
        assert!(fast_timer_pending(true, false));
        assert!(fast_timer_pending(false, true));
        assert!(fast_timer_pending(true, true));
    }

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
