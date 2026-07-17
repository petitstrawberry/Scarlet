//! Apple CPU-local sources routed through the shared FIQ line.

mod registers;

use core::sync::atomic::{AtomicU64, Ordering};

use registers::{El2State, LocalState};
use scarlet::interrupt::InterruptClaim;

const TIMER_STATE_MASK: u64 = 0b111;
const TIMER_FIRING: u64 = 0b101;
const TIMER_IMASK: u64 = 1 << 1;
const PMCR0_IMODE_FIQ: u64 = 4 << 8;
const PMCR0_IMODE_MASK: u64 = 7 << 8;
const PMCR0_IACT: u64 = 1 << 11;
const UPMCR0_IMODE_FIQ: u64 = 4 << 16;
const UPMCR0_IMODE_MASK: u64 = 7 << 16;
const UPMSR_IACT: u64 = 1;
const FAST_IPI_PENDING: u64 = 1;

const SOURCE_PHYSICAL_TIMER: u32 = 1 << 0;
const SOURCE_VIRTUAL_TIMER: u32 = 1 << 1;
const SOURCE_GUEST_PHYSICAL_TIMER: u32 = 1 << 2;
const SOURCE_GUEST_VIRTUAL_TIMER: u32 = 1 << 3;
const SOURCE_FAST_IPI: u32 = 1 << 4;
const SOURCE_CORE_PMU: u32 = 1 << 5;
const SOURCE_UNCORE_PMU: u32 = 1 << 6;
const MAX_FAST_IPI_CPUS: usize = 32;

static REPORTED_CPUS: AtomicU64 = AtomicU64::new(0);
static CPU_MPIDRS: [AtomicU64; MAX_FAST_IPI_CPUS] =
    [const { AtomicU64::new(0) }; MAX_FAST_IPI_CPUS];
static CPU_MPIDR_VALID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
struct FiqState {
    local: LocalState,
    el2: Option<El2State>,
}

impl FiqState {
    fn capture() -> Self {
        let vhe = scarlet::arch::is_vhe_enabled();
        Self {
            local: registers::read_local(vhe),
            el2: vhe.then(registers::read_el2),
        }
    }

    fn capture_traced(cpu_id: u32) -> Self {
        let vhe = scarlet::arch::is_vhe_enabled();
        Self {
            local: registers::read_local_traced(cpu_id, vhe),
            el2: vhe.then(registers::read_el2),
        }
    }

    fn pending_sources(&self) -> u32 {
        let mut sources =
            host_timer_sources(self.el2.is_some(), self.local.cntp_ctl, self.local.cntv_ctl);
        if self
            .el2
            .is_some_and(|state| timer_is_firing(state.guest_cntp_ctl))
        {
            sources |= SOURCE_GUEST_PHYSICAL_TIMER;
        }
        if self
            .el2
            .is_some_and(|state| timer_is_firing(state.guest_cntv_ctl))
        {
            sources |= SOURCE_GUEST_VIRTUAL_TIMER;
        }
        if self.local.fast_ipi_status & FAST_IPI_PENDING != 0 {
            sources |= SOURCE_FAST_IPI;
        }
        if self.local.pmcr0 & (PMCR0_IMODE_MASK | PMCR0_IACT) == PMCR0_IMODE_FIQ | PMCR0_IACT {
            sources |= SOURCE_CORE_PMU;
        }
        if self.local.upmcr0 & UPMCR0_IMODE_MASK == UPMCR0_IMODE_FIQ
            && self.local.upmsr & UPMSR_IACT != 0
        {
            sources |= SOURCE_UNCORE_PMU;
        }

        sources
    }
}

#[inline]
const fn timer_is_firing(control: u64) -> bool {
    control & TIMER_STATE_MASK == TIMER_FIRING
}

fn host_timer_sources(vhe: bool, physical: u64, virtual_: u64) -> u32 {
    match vhe {
        true if timer_is_firing(physical) => SOURCE_PHYSICAL_TIMER,
        false if timer_is_firing(virtual_) => SOURCE_VIRTUAL_TIMER,
        _ => 0,
    }
}

pub(super) fn prepare_cpu(cpu_id: u32) {
    scarlet::early_println!("[AIC] CPU {}: reading MPIDR", cpu_id);
    let cpu_index = cpu_id as usize;
    if cpu_index < MAX_FAST_IPI_CPUS {
        CPU_MPIDRS[cpu_index].store(registers::read_mpidr(), Ordering::Relaxed);
        CPU_MPIDR_VALID.fetch_or(1 << cpu_index, Ordering::Release);
    }

    scarlet::early_println!("[AIC] CPU {}: reading local FIQ state", cpu_id);
    let state = FiqState::capture_traced(cpu_id);
    scarlet::early_println!("[AIC] CPU {}: local FIQ state captured", cpu_id);

    if state.el2.is_some() {
        registers::mask_physical_timer(state.local.cntp_ctl | TIMER_IMASK);
        scarlet::early_println!("[AIC] CPU {}: physical host timer masked", cpu_id);
    } else {
        registers::mask_virtual_timer(state.local.cntv_ctl | TIMER_IMASK);
        scarlet::early_println!("[AIC] CPU {}: virtual host timer masked", cpu_id);
    }
    registers::clear_fast_ipi();
    scarlet::early_println!("[AIC] CPU {}: fast IPI cleared", cpu_id);
    registers::write_pmcr0(state.local.pmcr0 & !(PMCR0_IMODE_MASK | PMCR0_IACT));
    registers::write_upmcr0(state.local.upmcr0 & !UPMCR0_IMODE_MASK);
    scarlet::early_println!("[AIC] CPU {}: PMU FIQ sources disabled", cpu_id);

    if let Some(el2) = state.el2 {
        registers::mask_guest_timers(
            el2.guest_cntp_ctl | TIMER_IMASK,
            el2.guest_cntv_ctl | TIMER_IMASK,
        );
        registers::disable_vgic_maintenance(el2.vgic_hcr);
    }

    registers::synchronize();
    scarlet::early_println!("[AIC] CPU {}: FIQ state synchronized", cpu_id);
    report_prepared(cpu_id, &state);
}

pub(super) fn send_fast_ipi(target_cpu: u32) -> bool {
    let cpu_index = target_cpu as usize;
    if cpu_index >= MAX_FAST_IPI_CPUS
        || CPU_MPIDR_VALID.load(Ordering::Acquire) & (1 << cpu_index) == 0
    {
        scarlet::early_println!(
            "[AIC][IPI-SEND] rejected source_cpu={} target_cpu={} valid_mask={:#x}",
            scarlet::arch::get_cpu().get_cpuid(),
            target_cpu,
            CPU_MPIDR_VALID.load(Ordering::Relaxed)
        );
        return false;
    }

    let source_mpidr = registers::read_mpidr();
    let target_mpidr = CPU_MPIDRS[cpu_index].load(Ordering::Relaxed);
    scarlet::early_println!(
        "[AIC][IPI-SEND] source_cpu={} source_mpidr={:#x} target_cpu={} target_mpidr={:#x}",
        scarlet::arch::get_cpu().get_cpuid(),
        source_mpidr,
        target_cpu,
        target_mpidr
    );
    registers::send_fast_ipi(source_mpidr, target_mpidr);
    true
}

pub(super) fn claim_pending(cpu_id: u32) -> InterruptClaim {
    let state = FiqState::capture();
    let sources = state.pending_sources();
    // scarlet::early_println!(
    //     "[AIC][FIQ] non-timer claim cpu={} sources={:#x}",
    //     cpu_id,
    //     sources
    // );

    // Host timers are handled before this claim in the architecture trap
    // handler. If one becomes pending between that check and this snapshot,
    // leave the level asserted so the FIQ retriggers and the timer path can
    // rearm it. Masking it here would swallow the tick when an IPI is also
    // pending because InterruptClaim can report only the reschedule request.
    if sources & (SOURCE_GUEST_PHYSICAL_TIMER | SOURCE_GUEST_VIRTUAL_TIMER) != 0
        && let Some(el2) = state.el2
    {
        registers::mask_guest_timers(
            el2.guest_cntp_ctl | TIMER_IMASK,
            el2.guest_cntv_ctl | TIMER_IMASK,
        );
    }
    if sources & SOURCE_FAST_IPI != 0 {
        registers::clear_fast_ipi();
    }
    if sources & SOURCE_CORE_PMU != 0 {
        registers::write_pmcr0(state.local.pmcr0 & !(PMCR0_IMODE_MASK | PMCR0_IACT));
    }
    if sources & SOURCE_UNCORE_PMU != 0 {
        registers::write_upmcr0(state.local.upmcr0 & !UPMCR0_IMODE_MASK);
    }
    if sources != 0 {
        registers::synchronize();
    }

    report_once(cpu_id, sources, &state);
    if sources & SOURCE_FAST_IPI != 0 {
        InterruptClaim::Reschedule
    } else if sources != 0 {
        InterruptClaim::Handled
    } else {
        InterruptClaim::NotMine
    }
}

fn report_prepared(cpu_id: u32, state: &FiqState) {
    let el2 = state.el2.unwrap_or_default();
    scarlet::early_println!(
        "[AIC][FIQ] cpu={} prepared cntp={:#x} cntv={:#x} gpt={:#x} gvt={:#x} gate={:#x}",
        cpu_id,
        state.local.cntp_ctl,
        state.local.cntv_ctl,
        el2.guest_cntp_ctl,
        el2.guest_cntv_ctl,
        el2.vm_timer_enable
    );
    scarlet::early_println!(
        "[AIC][FIQ] cpu={} pre ipi={:#x} pmcr0={:#x} upmcr0={:#x} upmsr={:#x} hcr={:#x} cnthctl={:#x} ich_hcr={:#x} ich_misr={:#x}",
        cpu_id,
        state.local.fast_ipi_status,
        state.local.pmcr0,
        state.local.upmcr0,
        state.local.upmsr,
        el2.hcr,
        el2.cnthctl,
        el2.vgic_hcr,
        el2.vgic_misr
    );
}

fn report_once(cpu_id: u32, sources: u32, state: &FiqState) {
    if cpu_id >= u64::BITS {
        return;
    }
    let cpu_bit = 1u64 << cpu_id;
    if REPORTED_CPUS.fetch_or(cpu_bit, Ordering::Relaxed) & cpu_bit != 0 {
        return;
    }

    let el2 = state.el2.unwrap_or_default();
    scarlet::early_println!(
        "[AIC][FIQ] first cpu={} sources={:#x} elr={:#x} spsr={:#x} daif={:#x} isr={:#x}",
        cpu_id,
        sources,
        state.local.elr,
        state.local.spsr,
        state.local.daif,
        state.local.isr
    );
    scarlet::early_println!(
        "[AIC][FIQ] first cntp={:#x} cntv={:#x} gpt={:#x} gvt={:#x} gate={:#x} ipi={:#x} pmcr0={:#x} upmcr0={:#x} upmsr={:#x}",
        state.local.cntp_ctl,
        state.local.cntv_ctl,
        el2.guest_cntp_ctl,
        el2.guest_cntv_ctl,
        el2.vm_timer_enable,
        state.local.fast_ipi_status,
        state.local.pmcr0,
        state.local.upmcr0,
        state.local.upmsr
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn timer_is_firing_requires_enabled_unmasked_status() {
        assert!(timer_is_firing(TIMER_FIRING));
        assert!(!timer_is_firing(0b100));
        assert!(!timer_is_firing(0b111));
        assert!(!timer_is_firing(0b001));
    }

    #[test_case]
    fn host_sources_select_the_timer_used_by_the_kernel() {
        assert_eq!(
            host_timer_sources(true, TIMER_FIRING, TIMER_FIRING),
            SOURCE_PHYSICAL_TIMER
        );
        assert_eq!(
            host_timer_sources(false, TIMER_FIRING, TIMER_FIRING),
            SOURCE_VIRTUAL_TIMER
        );
        assert_eq!(host_timer_sources(true, 0, TIMER_FIRING), 0);
        assert_eq!(host_timer_sources(false, TIMER_FIRING, 0), 0);
    }
}
