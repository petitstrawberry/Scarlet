//! RISC-V Core Local Interrupt Controller (CLINT) Implementation
//!
//! The CLINT manages CPU-local interrupts such as timer interrupts and
//! software interrupts in RISC-V systems.

use crate::{
    device::{
        manager::{DeviceManager, DriverPriority},
        platform::{
            resource::PlatformDeviceResourceType, PlatformDeviceDriver, PlatformDeviceInfo,
        },
    },
    driver_initcall,
    interrupt::{
        controllers::{SoftwareInterruptController, TimerController},
        CpuId, InterruptError, InterruptResult,
    },
};
use alloc::{boxed::Box, vec};
use core::ptr::{read_volatile, write_volatile};

/// CLINT register offsets (relative to base address)
const CLINT_MSIP_OFFSET: usize = 0x0000; // Software interrupt pending
const CLINT_MTIMECMP_OFFSET: usize = 0x4000; // Timer compare registers
const CLINT_MTIME_OFFSET: usize = 0xBFF8; // Timer value

/// CLINT register stride per CPU
const CLINT_MSIP_STRIDE: usize = 4;
const CLINT_MTIMECMP_STRIDE: usize = 8;

/// Maximum number of CPUs supported by this CLINT implementation
const MAX_CPUS: CpuId = 4095;

/// RISC-V CLINT Implementation
pub struct Clint {
    /// Base address of the CLINT
    base_addr: usize,
    /// Maximum number of CPUs this CLINT supports
    max_cpus: CpuId,
    /// Cached timebase frequency in Hz
    timebase_frequency_hz: u64,
}

impl Clint {
    /// Create a new CLINT instance
    ///
    /// # Arguments
    ///
    /// * `base_addr` - Physical base address of the CLINT
    /// * `max_cpus` - Maximum number of CPUs supported
    /// * `timebase_frequency_hz` - Timebase frequency in Hz
    ///
    /// The base address is used to calculate all register addresses using
    /// relative offsets defined in the CLINT specification.
    pub fn new(base_addr: usize, max_cpus: CpuId, timebase_frequency_hz: u64) -> Self {
        Self {
            base_addr,
            max_cpus: max_cpus.min(MAX_CPUS),
            timebase_frequency_hz,
        }
    }

    /// Get the address of the software interrupt pending register for a CPU
    fn msip_addr(&self, cpu_id: CpuId) -> usize {
        self.base_addr + CLINT_MSIP_OFFSET + (cpu_id as usize * CLINT_MSIP_STRIDE)
    }

    /// Get the address of the timer compare register for a CPU
    fn mtimecmp_addr(&self, cpu_id: CpuId) -> usize {
        self.base_addr + CLINT_MTIMECMP_OFFSET + (cpu_id as usize * CLINT_MTIMECMP_STRIDE)
    }

    /// Get the address of the timer value register
    fn mtime_addr(&self) -> usize {
        self.base_addr + CLINT_MTIME_OFFSET
    }

    /// Validate CPU ID
    fn validate_cpu_id(&self, cpu_id: CpuId) -> InterruptResult<()> {
        if cpu_id >= self.max_cpus {
            Err(InterruptError::InvalidCpuId)
        } else {
            Ok(())
        }
    }
}

impl TimerController for Clint {
    /// Initialize the CLINT for a specific CPU
    fn init(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        self.set_timer(cpu_id, u64::MAX)?;
        Ok(())
    }

    /// Enable timer interrupts for a CPU
    fn enable_timer(&self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)
    }

    /// Disable timer interrupts for a CPU
    fn disable_timer(&self, cpu_id: CpuId) -> InterruptResult<()> {
        self.set_timer(cpu_id, u64::MAX)
    }

    /// Check whether a timer interrupt is pending for a CPU
    fn is_timer_pending(&self, cpu_id: CpuId) -> bool {
        if self.validate_cpu_id(cpu_id).is_err() {
            return false;
        }

        let current_time = self.get_time();
        let compare_time = unsafe { read_volatile(self.mtimecmp_addr(cpu_id) as *const u64) };
        current_time >= compare_time
    }

    /// Clear a pending timer interrupt for a CPU
    fn clear_timer(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        let current_time = self.get_time();
        self.set_timer(cpu_id, current_time + 1000000)
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
        unsafe { read_volatile(self.mtime_addr() as *const u64) }
    }

    fn get_timer_frequency_hz(&self) -> u64 {
        self.timebase_frequency_hz
    }
}

impl SoftwareInterruptController for Clint {
    /// Initialize software interrupt state for a specific CPU
    fn init(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
        self.clear_software_interrupt(cpu_id)
    }

    /// Enable software interrupts for a CPU
    fn enable_software_interrupt(&self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)
    }

    /// Disable software interrupts for a CPU
    fn disable_software_interrupt(&self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        let addr = self.msip_addr(cpu_id);
        unsafe {
            write_volatile(addr as *mut u32, 0);
        }
        Ok(())
    }

    /// Check whether a software interrupt is pending for a CPU
    fn is_software_interrupt_pending(&self, cpu_id: CpuId) -> bool {
        if self.validate_cpu_id(cpu_id).is_err() {
            return false;
        }

        let msip = unsafe { read_volatile(self.msip_addr(cpu_id) as *const u32) };
        (msip & 1) != 0
    }

    /// Send a software interrupt to a specific CPU
    fn send_software_interrupt(&self, target_cpu: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(target_cpu)?;

        let addr = self.msip_addr(target_cpu);
        unsafe {
            write_volatile(addr as *mut u32, 1);
        }

        Ok(())
    }

    /// Clear a software interrupt for a specific CPU
    fn clear_software_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
        self.disable_software_interrupt(cpu_id)
    }
}

unsafe impl Send for Clint {}
unsafe impl Sync for Clint {}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let res = device.get_resources();
    if res.is_empty() {
        return Err("No resources found");
    }

    // Get memory region resource (res_type == PlatformDeviceResourceType::MEM)
    let mem_res = res
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("Memory resource not found")?;

    let paddr = mem_res.start;
    let size = mem_res.end - mem_res.start + 1;

    // Map the CLINT's physical MMIO region into the kernel virtual address space.
    let base_addr = crate::vm::ioremap(paddr, size).map_err(|e| {
        crate::early_println!(
            "[interrupt] CLINT ioremap({:#x}, {:#x}) failed: {}",
            paddr,
            size,
            e
        );
        e
    })?;

    // Read the timebase frequency once from device tree
    // Prefer the timebase frequency provided by the device tree.
    // Fallback keeps QEMU virt default (10MHz) working even if FDT is unavailable.
    let timebase_frequency_hz =
        crate::arch::riscv64::fdt::timebase_frequency_hz_from_fdt().unwrap_or(10_000_000);

    // Create CLINT controllers
    let mut timer_controller = Box::new(Clint::new(base_addr, crate::environment::MAX_NUM_CPUS as CpuId, timebase_frequency_hz));
    let mut software_controller = Box::new(Clint::new(base_addr, crate::environment::MAX_NUM_CPUS as CpuId, timebase_frequency_hz));

    // Initialize CLINT (Currently only initializes for CPU 0)
    if let Err(e) = TimerController::init(timer_controller.as_mut(), 0) {
        crate::early_println!(
            "[interrupt] Failed to initialize CLINT for CPU {}: {}",
            0,
            e
        );
        return Err("Failed to initialize CLINT");
    }
    if let Err(e) = SoftwareInterruptController::init(software_controller.as_mut(), 0) {
        crate::early_println!(
            "[interrupt] Failed to initialize CLINT software interrupt for CPU {}: {}",
            0,
            e
        );
        return Err("Failed to initialize CLINT software interrupt");
    }

    // Register with InterruptManager instead of DeviceManager
    match crate::interrupt::InterruptManager::global()
        .register_timer_controller_for_range(timer_controller, 0..(crate::environment::MAX_NUM_CPUS as CpuId))
    {
        Ok(_) => {
            crate::early_println!(
                "[interrupt] CLINT registered at base address: {:#x}",
                base_addr
            );
        }
        Err(e) => {
            crate::early_println!("[interrupt] Failed to register CLINT: {}", e);
            return Err("Failed to register CLINT");
        }
    }
    match crate::interrupt::InterruptManager::global()
        .register_software_interrupt_controller_for_range(software_controller, 0..(crate::environment::MAX_NUM_CPUS as CpuId))
    {
        Ok(_) => {}
        Err(e) => {
            crate::early_println!("[interrupt] Failed to register CLINT software interrupt: {}", e);
            return Err("Failed to register CLINT software interrupt");
        }
    }

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "riscv-clint",
        probe_fn,
        remove_fn,
        vec!["sifive,clint0", "riscv,clint0"],
    );
    // Register the driver with the kernel
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical);
}

driver_initcall!(register_driver);

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_clint_creation() {
        let clint = Clint::new(0x200_0000, crate::environment::MAX_NUM_CPUS as CpuId, 10_000_000);
        assert_eq!(clint.max_cpus, crate::environment::MAX_NUM_CPUS as CpuId);
    }

    #[test_case]
    fn test_address_calculation() {
        let clint = Clint::new(0x200_0000, crate::environment::MAX_NUM_CPUS as CpuId, 10_000_000);

        assert_eq!(clint.msip_addr(0), 0x200_0000);
        assert_eq!(clint.msip_addr(1), 0x200_0004);
        assert_eq!(clint.msip_addr(3), 0x200_000C);

        assert_eq!(clint.mtimecmp_addr(0), 0x200_4000);
        assert_eq!(clint.mtimecmp_addr(1), 0x200_4008);
        assert_eq!(clint.mtimecmp_addr(3), 0x200_4018);

        assert_eq!(clint.mtime_addr(), 0x200_BFF8);
    }

    #[test_case]
    fn test_different_base_address() {
        let clint = Clint::new(0x300_0000, crate::environment::MAX_NUM_CPUS as CpuId, 10_000_000);

        assert_eq!(clint.msip_addr(0), 0x300_0000);
        assert_eq!(clint.msip_addr(1), 0x300_0004);

        assert_eq!(clint.mtimecmp_addr(0), 0x300_4000);
        assert_eq!(clint.mtimecmp_addr(1), 0x300_4008);

        assert_eq!(clint.mtime_addr(), 0x300_BFF8);
    }

    #[test_case]
    fn test_validation() {
        let clint = Clint::new(0x200_0000, crate::environment::MAX_NUM_CPUS as CpuId, 10_000_000);

        assert!(clint.validate_cpu_id(0).is_ok());
        assert!(clint.validate_cpu_id(crate::environment::MAX_NUM_CPUS as CpuId - 1).is_ok());

        assert!(clint.validate_cpu_id(crate::environment::MAX_NUM_CPUS as CpuId).is_err());
        assert!(clint.validate_cpu_id(100).is_err());
    }
}
