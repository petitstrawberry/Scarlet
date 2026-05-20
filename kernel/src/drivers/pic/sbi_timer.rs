//! SBI TIME timer controller for RISC-V.
//!
//! This is the firmware-backed timer fallback used when Sstc is unavailable.

use alloc::boxed::Box;
use core::arch::asm;

use crate::{
    early_initcall,
    interrupt::{CpuId, InterruptError, InterruptResult, controllers::TimerController},
};

struct SbiTimer {
    max_cpus: usize,
    timebase_frequency_hz: u64,
}

impl SbiTimer {
    /// Validate CPU ID
    fn validate_cpu_id(&self, cpu_id: CpuId) -> InterruptResult<()> {
        if cpu_id as usize >= self.max_cpus {
            Err(InterruptError::InvalidCpuId)
        } else {
            Ok(())
        }
    }
}

impl TimerController for SbiTimer {
    /// Initialize the SBI timer for a specific CPU.
    fn init(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        self.set_timer(cpu_id, u64::MAX)?;
        Ok(())
    }

    /// Enable timer interrupts for a CPU.
    fn enable_timer(&self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)
    }

    /// Disable timer interrupts for a CPU.
    fn disable_timer(&self, cpu_id: CpuId) -> InterruptResult<()> {
        self.set_timer(cpu_id, u64::MAX)
    }

    /// Check whether a timer interrupt is pending for a CPU.
    fn is_timer_pending(&self, cpu_id: CpuId) -> bool {
        self.validate_cpu_id(cpu_id).is_ok() && (read_sip() & (1 << 5)) != 0
    }

    /// Clear or acknowledge a timer interrupt for a CPU.
    fn clear_timer(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        let current_time = self.get_time();
        self.set_timer(cpu_id, current_time + 1_000_000)
    }

    /// Set the next timer compare value for a CPU.
    fn set_timer(&self, cpu_id: CpuId, time: u64) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        crate::arch::riscv64::instruction::sbi::sbi_set_timer(time);
        Ok(())
    }

    /// Get the current timer counter value.
    fn get_time(&self) -> u64 {
        read_rdtime()
    }

    /// Get the timer clock frequency.
    fn get_timer_frequency_hz(&self) -> u64 {
        self.timebase_frequency_hz
    }
}

unsafe impl Send for SbiTimer {}
unsafe impl Sync for SbiTimer {}

fn read_rdtime() -> u64 {
    let time: u64;
    // SAFETY: rdtime reads the architectural RISC-V time counter and has no
    // memory side effects.
    unsafe {
        asm!(
            "rdtime {0}",
            out(reg) time,
            options(nostack, nomem)
        );
    }
    time
}

fn read_sip() -> usize {
    let sip: usize;
    // SAFETY: sip is a supervisor CSR; reading it observes the current CPU's
    // pending interrupt bits and has no memory side effects.
    unsafe {
        asm!(
            "csrr {0}, sip",
            out(reg) sip,
            options(nostack, nomem)
        );
    }
    sip
}

fn register_driver() {
    if crate::arch::riscv64::fdt::all_cpus_have_isa_extension_from_fdt("sstc").unwrap_or(false) {
        crate::early_println!("[interrupt] RISC-V timer: skipping SBI TIME, Sstc is available");
        return;
    }

    let timebase_frequency_hz =
        crate::arch::riscv64::fdt::timebase_frequency_hz_from_fdt().unwrap_or(10_000_000);

    let controller = Box::new(SbiTimer {
        max_cpus: crate::environment::MAX_NUM_CPUS as usize,
        timebase_frequency_hz,
    });

    match crate::interrupt::InterruptManager::global().register_timer_controller_for_range(
        controller,
        0..(crate::environment::MAX_NUM_CPUS as CpuId),
    ) {
        Ok(_) => {
            crate::early_println!("[interrupt] RISC-V timer: using SBI TIME");
        }
        Err(e) => {
            crate::early_println!("[interrupt] Failed to register SBI timer: {}", e);
        }
    }
}

early_initcall!(register_driver);
