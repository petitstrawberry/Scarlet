//! RISC-V Supervisor-mode timer compare (Sstc) local timer driver.

use core::arch::asm;

use alloc::boxed::Box;

use crate::{
    early_initcall,
    interrupt::{CpuId, InterruptError, InterruptResult, controllers::TimerController},
};

struct SstcTimer {
    max_cpus: usize,
    timebase_frequency_hz: u64,
}

impl SstcTimer {
    /// Validate CPU ID
    fn validate_cpu_id(&self, cpu_id: CpuId) -> InterruptResult<()> {
        if cpu_id as usize >= self.max_cpus {
            Err(InterruptError::InvalidCpuId)
        } else {
            Ok(())
        }
    }
}

impl TimerController for SstcTimer {
    /// Initialize the Sstc timer for a specific CPU
    fn init(
        &mut self,
        cpu_id: CpuId,
        _mode: crate::interrupt::controllers::InterruptControllerInitMode,
    ) -> InterruptResult<()> {
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
        self.validate_cpu_id(cpu_id).is_ok() && self.get_time() >= read_stimecmp()
    }

    /// Clear or acknowledge a timer interrupt for a CPU.
    fn clear_timer(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        let current_time = self.get_time();
        self.set_timer(cpu_id, current_time + 1_000_000)
    }

    /// Set timer interrupt for a specific CPU
    fn set_timer(&self, cpu_id: CpuId, time: u64) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        write_stimecmp(time);
        Ok(())
    }

    /// Get current timer value
    fn get_time(&self) -> u64 {
        read_rdtime()
    }

    /// Returns the timer clock frequency in Hz
    fn get_timer_frequency_hz(&self) -> u64 {
        self.timebase_frequency_hz
    }
}

unsafe impl Send for SstcTimer {}
unsafe impl Sync for SstcTimer {}

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

fn read_stimecmp() -> u64 {
    let time: u64;
    // SAFETY: this driver registers only when FDT reports the Sstc extension.
    unsafe {
        asm!(
            "csrr {0}, stimecmp",
            out(reg) time,
            options(nostack, nomem)
        );
    }
    time
}

fn write_stimecmp(time: u64) {
    // SAFETY: this driver registers only when FDT reports the Sstc extension.
    unsafe {
        asm!(
            "csrw stimecmp, {0}",
            in(reg) time,
            options(nostack)
        );
    }
}

fn register_driver() {
    if !crate::arch::riscv64::fdt::all_cpus_have_isa_extension_from_fdt("sstc").unwrap_or(false) {
        return;
    }

    let timebase_frequency_hz =
        crate::arch::riscv64::fdt::timebase_frequency_hz_from_fdt().unwrap_or(10_000_000);

    let controller = Box::new(SstcTimer {
        max_cpus: crate::environment::MAX_NUM_CPUS as usize,
        timebase_frequency_hz,
    });

    match crate::interrupt::InterruptManager::global().register_timer_controller_for_range(
        controller,
        0..(crate::environment::MAX_NUM_CPUS as CpuId),
    ) {
        Ok(_) => {
            crate::early_println!("[interrupt] RISC-V timer: using Sstc stimecmp");
        }
        Err(e) => {
            crate::early_println!("[interrupt] Failed to register Sstc timer: {}", e);
        }
    }
}

early_initcall!(register_driver);
