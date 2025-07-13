//! RISC-V Core Local Interrupt Controller (CLINT) Implementation
//!
//! The CLINT manages CPU-local interrupts such as timer interrupts and
//! software interrupts in RISC-V systems.

use crate::{device::{manager::{DeviceManager, DriverPriority}, platform::{resource::PlatformDeviceResourceType, PlatformDeviceDriver, PlatformDeviceInfo}}, driver_initcall, interrupt::{
    controllers::{LocalInterruptController, LocalInterruptType}, CpuId, InterruptError, InterruptManager, InterruptResult
}};
use alloc::{boxed::Box, vec};
use core::ptr::{read_volatile, write_volatile};

/// CLINT register offsets (relative to base address)
const CLINT_MSIP_OFFSET: usize = 0x0000;     // Software interrupt pending
const CLINT_MTIMECMP_OFFSET: usize = 0x4000; // Timer compare registers
const CLINT_MTIME_OFFSET: usize = 0xBFF8;    // Timer value

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
}

impl Clint {
    /// Create a new CLINT instance
    /// 
    /// # Arguments
    /// 
    /// * `base_addr` - Physical base address of the CLINT
    /// * `max_cpus` - Maximum number of CPUs supported
    /// 
    /// The base address is used to calculate all register addresses using
    /// relative offsets defined in the CLINT specification.
    pub fn new(base_addr: usize, max_cpus: CpuId) -> Self {
        Self {
            base_addr,
            max_cpus: max_cpus.min(MAX_CPUS),
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

impl LocalInterruptController for Clint {
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
    fn enable_interrupt(&mut self, cpu_id: CpuId, interrupt_type: LocalInterruptType) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;

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
    fn disable_interrupt(&mut self, cpu_id: CpuId, interrupt_type: LocalInterruptType) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;

        match interrupt_type {
            LocalInterruptType::Timer => {
                // Disable timer by setting mtimecmp to maximum value
                self.set_timer(cpu_id, u64::MAX)
            }
            LocalInterruptType::Software => {
                // Disable software interrupt by clearing MSIP
                self.clear_software_interrupt(cpu_id)
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
            LocalInterruptType::Timer => {
                let current_time = self.get_time();
                let compare_time = unsafe {
                    read_volatile(self.mtimecmp_addr(cpu_id) as *const u64)
                };
                current_time >= compare_time
            }
            LocalInterruptType::Software => {
                let msip = unsafe {
                    read_volatile(self.msip_addr(cpu_id) as *const u32)
                };
                (msip & 1) != 0
            }
            LocalInterruptType::External => false, // Not managed by CLINT
        }
    }

    /// Clear a pending local interrupt for a CPU
    fn clear_interrupt(&mut self, cpu_id: CpuId, interrupt_type: LocalInterruptType) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;

        match interrupt_type {
            LocalInterruptType::Timer => {
                // Clear timer interrupt by setting mtimecmp to future time
                let current_time = self.get_time();
                self.set_timer(cpu_id, current_time + 1000000) // 1M cycles in future
            }
            LocalInterruptType::Software => {
                self.clear_software_interrupt(cpu_id)
            }
            LocalInterruptType::External => {
                Err(InterruptError::NotSupported)
            }
        }
    }

    /// Send a software interrupt to a specific CPU
    fn send_software_interrupt(&mut self, target_cpu: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(target_cpu)?;

        let addr = self.msip_addr(target_cpu);
        unsafe {
            write_volatile(addr as *mut u32, 1);
        }

        Ok(())
    }

    /// Clear a software interrupt for a specific CPU
    fn clear_software_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
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
    fn set_timer(&mut self, cpu_id: CpuId, time: u64) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;

        // Set the timer compare register to the specified time using SBI
        crate::arch::riscv64::instruction::sbi::sbi_set_timer(time);
        
        Ok(())
    }

    /// Get current timer value
    fn get_time(&self) -> u64 {
        unsafe {
            read_volatile(self.mtime_addr() as *const u64)
        }
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
    let mem_res = res.iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("Memory resource not found")?;
    
    let base_addr = mem_res.start as usize;

    // Create CLINT controller
    let mut controller = Box::new(Clint::new(base_addr, 4)); // Example: 4 CPUs for QEMU virt
    
    // Initialize CLINT (Currently only initializes for CPU 0)
    if let Err(e) = controller.init(0) {
        crate::early_println!("[interrupt] Failed to initialize CLINT for CPU {}: {}", 0, e);
        return Err("Failed to initialize CLINT");
    }

    // Register with InterruptManager instead of DeviceManager
    match InterruptManager::global().lock().register_local_controller_for_range(controller, 0..4) {
        Ok(_) => {
            crate::early_println!("[interrupt] CLINT registered at base address: {:#x}", base_addr);
        },
        Err(e) => {
            crate::early_println!("[interrupt] Failed to register CLINT: {}", e);
            return Err("Failed to register CLINT");
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
    DeviceManager::get_mut_manager().register_driver(Box::new(driver), DriverPriority::Critical);
}

driver_initcall!(register_driver);


#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_clint_creation() {
        let clint = Clint::new(0x200_0000, 4);
        assert_eq!(clint.max_cpus, 4);
    }

    #[test_case]
    fn test_address_calculation() {
        let clint = Clint::new(0x200_0000, 4);
        
        // Test MSIP addresses
        assert_eq!(clint.msip_addr(0), 0x200_0000);
        assert_eq!(clint.msip_addr(1), 0x200_0004);
        assert_eq!(clint.msip_addr(3), 0x200_000C);
        
        // Test MTIMECMP addresses
        assert_eq!(clint.mtimecmp_addr(0), 0x200_4000);
        assert_eq!(clint.mtimecmp_addr(1), 0x200_4008);
        assert_eq!(clint.mtimecmp_addr(3), 0x200_4018);
        
        // Test MTIME address
        assert_eq!(clint.mtime_addr(), 0x200_BFF8);
    }

    #[test_case]
    fn test_different_base_address() {
        // Test with different base address to ensure base_addr is properly used
        let clint = Clint::new(0x300_0000, 4);
        
        // Test MSIP addresses with different base
        assert_eq!(clint.msip_addr(0), 0x300_0000);
        assert_eq!(clint.msip_addr(1), 0x300_0004);
        
        // Test MTIMECMP addresses with different base
        assert_eq!(clint.mtimecmp_addr(0), 0x300_4000);
        assert_eq!(clint.mtimecmp_addr(1), 0x300_4008);
        
        // Test MTIME address with different base
        assert_eq!(clint.mtime_addr(), 0x300_BFF8);
    }

    #[test_case]
    fn test_validation() {
        let clint = Clint::new(0x200_0000, 4);
        
        // Valid CPU IDs should pass
        assert!(clint.validate_cpu_id(0).is_ok());
        assert!(clint.validate_cpu_id(3).is_ok());
        
        // Invalid CPU IDs should fail
        assert!(clint.validate_cpu_id(4).is_err());
        assert!(clint.validate_cpu_id(100).is_err());
    }
}
