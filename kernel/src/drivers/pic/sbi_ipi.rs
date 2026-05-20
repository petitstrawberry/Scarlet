//! SBI IPI software interrupt controller for RISC-V.

use alloc::boxed::Box;

use crate::{
    early_initcall,
    interrupt::{CpuId, InterruptError, InterruptResult, controllers::SoftwareInterruptController},
};

struct SbiIpi {
    max_cpus: usize,
}

impl SbiIpi {
    /// Validate CPU ID
    fn validate_cpu_id(&self, cpu_id: CpuId) -> InterruptResult<()> {
        if cpu_id as usize >= self.max_cpus {
            Err(InterruptError::InvalidCpuId)
        } else {
            Ok(())
        }
    }
}

impl SoftwareInterruptController for SbiIpi {
    /// Initialize software interrupt state for a CPU.
    fn init(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
        self.clear_software_interrupt(cpu_id)
    }

    /// Enable software interrupts for a CPU.
    fn enable_software_interrupt(&self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)
    }

    /// Disable software interrupts for a CPU.
    fn disable_software_interrupt(&self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)
    }

    /// Check whether a software interrupt is pending for a CPU.
    fn is_software_interrupt_pending(&self, cpu_id: CpuId) -> bool {
        self.validate_cpu_id(cpu_id).is_ok() && (read_sip() & (1 << 1)) != 0
    }

    /// Clear a software interrupt for a CPU.
    fn clear_software_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        clear_ssip();
        Ok(())
    }

    /// Send a software interrupt to a CPU.
    fn send_software_interrupt(&self, target_cpu: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(target_cpu)?;
        crate::arch::riscv64::instruction::sbi::sbi_send_ipi(1 << target_cpu, 0);
        Ok(())
    }
}

unsafe impl Send for SbiIpi {}
unsafe impl Sync for SbiIpi {}

fn clear_ssip() {
    // SAFETY: clearing sip.SSIP acknowledges the current CPU's supervisor
    // software interrupt. This is the architectural RISC-V clear path used by
    // the trap handler as well.
    unsafe {
        core::arch::asm!(
            "csrc sip, {0}",
            in(reg) 1 << 1,
            options(nostack)
        );
    }
}

fn read_sip() -> usize {
    let sip: usize;
    // SAFETY: sip is a supervisor CSR; reading it observes the current CPU's
    // pending interrupt bits and has no memory side effects.
    unsafe {
        core::arch::asm!(
            "csrr {0}, sip",
            out(reg) sip,
            options(nostack, nomem)
        );
    }
    sip
}

fn register_driver() {
    let controller = Box::new(SbiIpi {
        max_cpus: crate::environment::MAX_NUM_CPUS as usize,
    });

    match crate::interrupt::InterruptManager::global()
        .register_software_interrupt_controller_for_range(
            controller,
            0..(crate::environment::MAX_NUM_CPUS as CpuId),
        ) {
        Ok(_) => {
            crate::early_println!("[interrupt] RISC-V IPI: using SBI IPI");
        }
        Err(e) => {
            crate::early_println!("[interrupt] Failed to register SBI IPI: {}", e);
        }
    }
}

early_initcall!(register_driver);
