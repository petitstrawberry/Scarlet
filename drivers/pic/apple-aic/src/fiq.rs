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
        Self {
            local: registers::read_local(),
            el2: scarlet::arch::is_vhe_enabled().then(registers::read_el2),
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
        true if timer_is_firing(virtual_) => SOURCE_VIRTUAL_TIMER,
        false if timer_is_firing(physical) => SOURCE_PHYSICAL_TIMER,
        _ => 0,
    }
}

pub(super) fn prepare_cpu(cpu_id: u32) {
    let cpu_index = cpu_id as usize;
    if cpu_index < MAX_FAST_IPI_CPUS {
        CPU_MPIDRS[cpu_index].store(registers::read_mpidr(), Ordering::Relaxed);
        CPU_MPIDR_VALID.fetch_or(1 << cpu_index, Ordering::Release);
    }

    let state = FiqState::capture();

    registers::mask_physical_timer(state.local.cntp_ctl | TIMER_IMASK);
    registers::mask_virtual_timer(state.local.cntv_ctl | TIMER_IMASK);
    registers::clear_fast_ipi();
    registers::write_pmcr0(state.local.pmcr0 & !(PMCR0_IMODE_MASK | PMCR0_IACT));
    registers::write_upmcr0(state.local.upmcr0 & !UPMCR0_IMODE_MASK);

    if let Some(el2) = state.el2 {
        registers::mask_guest_timers(
            el2.guest_cntp_ctl | TIMER_IMASK,
            el2.guest_cntv_ctl | TIMER_IMASK,
        );
        registers::disable_vgic_maintenance(el2.vgic_hcr);
    }

    registers::synchronize();
}

pub(super) fn send_fast_ipi(target_cpu: u32) -> bool {
    let cpu_index = target_cpu as usize;
    if cpu_index >= MAX_FAST_IPI_CPUS
        || CPU_MPIDR_VALID.load(Ordering::Acquire) & (1 << cpu_index) == 0
    {
        return false;
    }

    registers::send_fast_ipi(
        registers::read_mpidr(),
        CPU_MPIDRS[cpu_index].load(Ordering::Relaxed),
    );
    true
}

pub(super) fn claim_pending(cpu_id: u32) -> InterruptClaim {
    let state = FiqState::capture();
    let sources = state.pending_sources();

    if sources & SOURCE_PHYSICAL_TIMER != 0 {
        registers::mask_physical_timer(state.local.cntp_ctl | TIMER_IMASK);
    }
    if sources & SOURCE_VIRTUAL_TIMER != 0 {
        registers::mask_virtual_timer(state.local.cntv_ctl | TIMER_IMASK);
    }
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

    if sources & SOURCE_FAST_IPI != 0 {
        InterruptClaim::Reschedule
    } else if sources != 0 {
        InterruptClaim::Handled
    } else {
        InterruptClaim::NotMine
    }
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
    fn host_sources_exclude_the_selected_timer() {
        assert_eq!(
            host_timer_sources(true, TIMER_FIRING, TIMER_FIRING),
            SOURCE_VIRTUAL_TIMER
        );
        assert_eq!(
            host_timer_sources(false, TIMER_FIRING, TIMER_FIRING),
            SOURCE_PHYSICAL_TIMER
        );
    }
}
