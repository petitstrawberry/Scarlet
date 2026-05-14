//! SBI-based CLINT (Core Local Interruptor) driver for RISC-V architecture.
//!! This driver uses SBI calls to manage local interrupts such as
//! timer interrupts and software interrupts.

use core::arch::asm;

use alloc::boxed::Box;

use crate::{
    early_initcall,
    interrupt::{
        CpuId, InterruptError, InterruptResult,
        controllers::{LocalInterruptController, LocalInterruptType},
    },
};

struct SbiClint {
    max_cpus: usize,
    timebase_frequency_hz: u64,
}

impl SbiClint {
    /// Validate CPU ID
    fn validate_cpu_id(&self, cpu_id: CpuId) -> InterruptResult<()> {
        if cpu_id as usize >= self.max_cpus {
            Err(InterruptError::InvalidCpuId)
        } else {
            Ok(())
        }
    }
}

impl LocalInterruptController for SbiClint {
    /// Initialize the CLINT for a specific CPU
    fn init(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;

        // Clear software interrupt
        self.clear_software_interrupt(cpu_id)?;

        // Set timer to maximum value (effectively disable)
        self.set_timer(cpu_id, u64::MAX)?;

        Ok(())
    }

    /// Enable a specific local interrupt type for a CPU
    fn enable_interrupt(
        &self,
        _cpu_id: CpuId,
        interrupt_type: LocalInterruptType,
    ) -> InterruptResult<()> {
        match interrupt_type {
            LocalInterruptType::Timer => {
                // Timer interrupts are enabled by setting mtimecmp
                // This is done via set_timer() method
                Ok(())
            }
            LocalInterruptType::Software => {
                // Software interrupts are enabled by setting MSIP
                // This is done via send_software_interrupt() method
                Ok(())
            }
            LocalInterruptType::External => {
                // External interrupts are not managed by CLINT
                Err(InterruptError::NotSupported)
            }
        }
    }

    /// Disable a specific local interrupt type for a CPU
    fn disable_interrupt(
        &self,
        cpu_id: CpuId,
        interrupt_type: LocalInterruptType,
    ) -> InterruptResult<()> {
        match interrupt_type {
            LocalInterruptType::Timer => {
                // Disable timer by setting mtimecmp to maximum value
                self.set_timer(cpu_id, u64::MAX)
            }
            LocalInterruptType::Software => {
                self.validate_cpu_id(cpu_id)?;
                Ok(())
            }
            LocalInterruptType::External => {
                // External interrupts are not managed by CLINT
                Err(InterruptError::NotSupported)
            }
        }
    }

    /// Check if a specific local interrupt type is pending for a CPU
    fn is_pending(&self, cpu_id: CpuId, interrupt_type: LocalInterruptType) -> bool {
        if self.validate_cpu_id(cpu_id).is_err() {
            return false;
        }

        match interrupt_type {
            LocalInterruptType::Timer => false,
            LocalInterruptType::Software => false,
            LocalInterruptType::External => false, // Not managed by CLINT
        }
    }

    /// Clear a pending local interrupt for a CPU
    fn clear_interrupt(
        &mut self,
        cpu_id: CpuId,
        interrupt_type: LocalInterruptType,
    ) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;

        match interrupt_type {
            LocalInterruptType::Timer => {
                // Clear timer interrupt by setting mtimecmp to future time
                let current_time = self.get_time();
                self.set_timer(cpu_id, current_time + 1000000) // 1M cycles in future
            }
            LocalInterruptType::Software => self.clear_software_interrupt(cpu_id),
            LocalInterruptType::External => Err(InterruptError::NotSupported),
        }
    }

    /// Send a software interrupt to a specific CPU
    fn send_software_interrupt(&self, _target_cpu: CpuId) -> InterruptResult<()> {
        Ok(())
    }

    /// Clear a software interrupt for a specific CPU
    fn clear_software_interrupt(&mut self, _cpu_id: CpuId) -> InterruptResult<()> {
        // self.validate_cpu_id(cpu_id)?;

        // let addr = self.msip_addr(cpu_id);
        // unsafe {
        //     write_volatile(addr as *mut u32, 0);
        // }

        // TODO: Use SBI to clear software interrupt
        // For now, just return Ok

        Ok(())
    }

    /// Set timer interrupt for a specific CPU
    fn set_timer(&self, cpu_id: CpuId, time: u64) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;

        // Set the timer compare register to the specified time using SBI
        crate::arch::riscv64::instruction::sbi::sbi_set_timer(time);

        Ok(())
    }

    /// Get current timer value
    fn get_time(&self) -> u64 {
        let time: u64;
        unsafe {
            asm!(
                "rdtime {0}",
                out(reg) time,
            );
        }
        time
    }

    fn get_timer_frequency_hz(&self) -> u64 {
        self.timebase_frequency_hz
    }
}

unsafe impl Send for SbiClint {}
unsafe impl Sync for SbiClint {}

fn register_driver() {
    // Read the timebase frequency once from device tree
    // Prefer the timebase frequency provided by the device tree.
    // Fallback keeps QEMU virt default (10MHz) working even if FDT is unavailable.
    let timebase_frequency_hz =
        crate::arch::riscv64::fdt::timebase_frequency_hz_from_fdt().unwrap_or(10_000_000);

    // Create the SBI timer controller
    let mut controller = Box::new(SbiClint {
        max_cpus: 4,
        timebase_frequency_hz,
    });

    if let Err(e) = controller.init(0) {
        crate::early_println!(
            "[interrupt] Failed to initialize CLINT for CPU {}: {}",
            0,
            e
        );
    }

    // Register with InterruptManager instead of DeviceManager
    match crate::interrupt::InterruptManager::global()
        .register_local_controller_for_range(controller, 0..4)
    {
        Ok(_) => {}
        Err(e) => {
            crate::early_println!("[interrupt] Failed to register CLINT: {}", e);
        }
    }
}

early_initcall!(register_driver);
