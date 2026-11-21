//! Monotonic host clock and the logical CPU's absolute timer deadline.

use std::{sync::LazyLock, time::Instant};

static CLOCK_START: LazyLock<Instant> = LazyLock::new(Instant::now);

pub fn get_time() -> u64 {
    CLOCK_START.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

pub struct ArchTimer {
    deadline: u64,
}

impl ArchTimer {
    pub fn new() -> Self {
        Self { deadline: u64::MAX }
    }

    pub fn init(&self) {
        LazyLock::force(&CLOCK_START);
    }

    pub fn start(&mut self) {
        self.publish(Some(self.deadline));
    }

    pub fn stop(&mut self) {
        self.publish(None);
    }

    pub fn set_deadline_ns(&mut self, deadline: u64) {
        self.deadline = deadline.max(get_time().saturating_add(1));
    }

    pub fn get_time_ns(&self) -> u64 {
        get_time()
    }

    fn publish(&self, deadline: Option<u64>) {
        if let Some(cpu) = crate::arch::try_get_cpuid() {
            crate::hosted::interrupts::program_timer(cpu, deadline);
        }
    }
}

pub(crate) fn diagnostic_snapshot(
    return_spsr: u64,
    return_pc: u64,
) -> crate::timer::ArchTimerDiagnosticSnapshot {
    let cpu = crate::arch::get_cpu().get_cpuid();
    crate::timer::ArchTimerDiagnosticSnapshot {
        control: 0,
        counter: get_time(),
        compare: crate::timer::timer_programmed_deadline_ns(cpu).unwrap_or(u64::MAX),
        return_spsr,
        return_pc,
    }
}
