//! RISC-V Platform-Level Interrupt Controller (PLIC) Implementation
//!
//! The PLIC is responsible for managing external interrupts from devices and
//! routing them to different CPUs with priority support.

use crate::{device::{manager::{DeviceManager, DriverPriority}, platform::{resource::PlatformDeviceResourceType, PlatformDeviceDriver, PlatformDeviceInfo}}, driver_initcall, early_initcall, interrupt::{
    controllers::{ExternalInterruptController, LocalInterruptType}, CpuId, InterruptError, InterruptId, InterruptManager, InterruptResult, Priority
}};
use alloc::{boxed::Box, vec};
use core::ptr::{read_volatile, write_volatile};

/// PLIC register offsets
const PLIC_PRIORITY_BASE: usize = 0x0000_0000;
const PLIC_PENDING_BASE: usize = 0x0000_1000;
const PLIC_ENABLE_BASE: usize = 0x0000_2000;
const PLIC_THRESHOLD_BASE: usize = 0x0020_0000;
const PLIC_CLAIM_BASE: usize = 0x0020_0004;

/// PLIC context stride for enable registers (per context)
const PLIC_ENABLE_CONTEXT_STRIDE: usize = 0x80;
/// PLIC context stride for threshold/claim registers (per context)
const PLIC_CONTEXT_STRIDE: usize = 0x1000;

/// Maximum number of interrupts supported by this PLIC implementation
const MAX_INTERRUPTS: InterruptId = 1024;

/// Maximum number of CPUs supported by this PLIC implementation
const MAX_CPUS: CpuId = 15872; // RISC-V spec allows up to 15872 contexts

/// RISC-V PLIC Implementation
pub struct Plic {
    /// Base address of the PLIC
    base_addr: usize,
    /// Maximum number of interrupts this PLIC supports
    max_interrupts: InterruptId,
    /// Maximum number of CPUs this PLIC supports
    max_cpus: CpuId,
}

impl Plic {
    /// Create a new PLIC instance
    /// 
    /// # Arguments
    /// 
    /// * `base_addr` - Physical base address of the PLIC
    /// * `max_interrupts` - Maximum interrupt ID supported (1-based)
    /// * `max_cpus` - Maximum number of CPUs supported
    pub fn new(base_addr: usize, max_interrupts: InterruptId, max_cpus: CpuId) -> Self {
        Self {
            base_addr,
            max_interrupts: max_interrupts.min(MAX_INTERRUPTS),
            max_cpus: max_cpus.min(MAX_CPUS),
        }
    }

    /// Convert CPU ID to PLIC context ID for Supervisor mode.
    /// Hart 0 S-Mode -> Context 1, Hart 1 S-Mode -> Context 3, etc.
    fn context_id_for_cpu(&self, cpu_id: CpuId) -> usize {
        (cpu_id as usize * 2) + 1
    }

    /// Get the address of a priority register for an interrupt
    fn priority_addr(&self, interrupt_id: InterruptId) -> usize {
        self.base_addr + PLIC_PRIORITY_BASE + (interrupt_id as usize * 4)
    }

    /// Get the address of a pending register for an interrupt
    fn pending_addr(&self, interrupt_id: InterruptId) -> usize {
        let word_offset = interrupt_id / 32;
        self.base_addr + PLIC_PENDING_BASE + (word_offset as usize * 4)
    }

    /// Get the address of an enable register for a CPU and interrupt
    fn enable_addr(&self, cpu_id: CpuId, interrupt_id: InterruptId) -> usize {
        let word_offset = interrupt_id / 32;
        let context_id = self.context_id_for_cpu(cpu_id);
        let context_offset = context_id * PLIC_ENABLE_CONTEXT_STRIDE;
        self.base_addr + PLIC_ENABLE_BASE + context_offset + (word_offset as usize * 4)
    }

    /// Get the address of a threshold register for a CPU
    fn threshold_addr(&self, cpu_id: CpuId) -> usize {
        let context_id = self.context_id_for_cpu(cpu_id);
        let context_offset = context_id * PLIC_CONTEXT_STRIDE;
        self.base_addr + PLIC_THRESHOLD_BASE + context_offset
    }

    /// Get the address of a claim register for a CPU
    fn claim_addr(&self, cpu_id: CpuId) -> usize {
        let context_id = self.context_id_for_cpu(cpu_id);
        let context_offset = context_id * PLIC_CONTEXT_STRIDE;
        self.base_addr + PLIC_CLAIM_BASE + context_offset
    }

    /// Validate interrupt ID
    fn validate_interrupt_id(&self, interrupt_id: InterruptId) -> InterruptResult<()> {
        if interrupt_id == 0 || interrupt_id > self.max_interrupts {
            Err(InterruptError::InvalidInterruptId)
        } else {
            Ok(())
        }
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

impl ExternalInterruptController for Plic {
    /// Initialize the PLIC
    fn init(&mut self) -> InterruptResult<()> {
        // Disable all interrupts for all CPUs initially
        for cpu_id in 0..self.max_cpus {
            // Disable all interrupts for this CPU's context
            for word in 0..=(self.max_interrupts / 32) {
                let interrupt_id_base = word * 32;
                if interrupt_id_base > 0 { // Interrupt ID 0 is not used
                    let addr = self.enable_addr(cpu_id, interrupt_id_base);
                    unsafe { write_volatile(addr as *mut u32, 0); }
                }
            }
            // Set threshold to 0 (allow all priorities)
            let _ = self.set_threshold(cpu_id, 0);
        }

        // Set all interrupt priorities to 1 (lowest non-zero priority)
        for interrupt_id in 1..=self.max_interrupts {
            let _ = self.set_priority(interrupt_id, 1);
        }

        Ok(())
    }

    /// Enable a specific interrupt for a CPU
    fn enable_interrupt(&mut self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        self.validate_cpu_id(cpu_id)?;

        let addr = self.enable_addr(cpu_id, interrupt_id);
        let bit_offset = interrupt_id % 32;
        
        unsafe {
            let current = read_volatile(addr as *const u32);
            let new_value = current | (1 << bit_offset);
            write_volatile(addr as *mut u32, new_value);
        }

        Ok(())
    }

    /// Disable a specific interrupt for a CPU
    fn disable_interrupt(&mut self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        self.validate_cpu_id(cpu_id)?;

        let addr = self.enable_addr(cpu_id, interrupt_id);
        let bit_offset = interrupt_id % 32;
        
        unsafe {
            let current = read_volatile(addr as *const u32);
            let new_value = current & !(1 << bit_offset);
            write_volatile(addr as *mut u32, new_value);
        }

        Ok(())
    }

    /// Set priority for a specific interrupt
    fn set_priority(&mut self, interrupt_id: InterruptId, priority: Priority) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        
        if priority > 7 {
            return Err(InterruptError::InvalidPriority);
        }

        let addr = self.priority_addr(interrupt_id);
        unsafe {
            write_volatile(addr as *mut u32, priority);
        }

        Ok(())
    }

    /// Get priority for a specific interrupt
    fn get_priority(&self, interrupt_id: InterruptId) -> InterruptResult<Priority> {
        self.validate_interrupt_id(interrupt_id)?;

        let addr = self.priority_addr(interrupt_id);
        let priority = unsafe { read_volatile(addr as *const u32) };
        
        Ok(priority)
    }

    /// Set priority threshold for a CPU
    fn set_threshold(&mut self, cpu_id: CpuId, threshold: Priority) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        
        if threshold > 7 {
            return Err(InterruptError::InvalidPriority);
        }

        let addr = self.threshold_addr(cpu_id);
        unsafe {
            write_volatile(addr as *mut u32, threshold);
        }

        Ok(())
    }

    /// Get priority threshold for a CPU
    fn get_threshold(&self, cpu_id: CpuId) -> InterruptResult<Priority> {
        self.validate_cpu_id(cpu_id)?;

        let addr = self.threshold_addr(cpu_id);
        let threshold = unsafe { read_volatile(addr as *const u32) };
        
        Ok(threshold)
    }

    /// Claim an interrupt (acknowledge and get the interrupt ID)
    fn claim_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<Option<InterruptId>> {
        self.validate_cpu_id(cpu_id)?;

        let addr = self.claim_addr(cpu_id);
        let interrupt_id = unsafe { read_volatile(addr as *const u32) };
        
        if interrupt_id == 0 {
            Ok(None)
        } else {
            Ok(Some(interrupt_id))
        }
    }

    /// Complete an interrupt (signal that handling is finished)
    fn complete_interrupt(&mut self, cpu_id: CpuId, interrupt_id: InterruptId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        self.validate_interrupt_id(interrupt_id)?;

        let addr = self.claim_addr(cpu_id);
        unsafe {
            write_volatile(addr as *mut u32, interrupt_id);
        }

        Ok(())
    }

    /// Check if a specific interrupt is pending
    fn is_pending(&self, interrupt_id: InterruptId) -> bool {
        if self.validate_interrupt_id(interrupt_id).is_err() {
            return false;
        }

        let addr = self.pending_addr(interrupt_id);
        let bit_offset = interrupt_id % 32;
        
        unsafe {
            let pending_word = read_volatile(addr as *const u32);
            (pending_word & (1 << bit_offset)) != 0
        }
    }

    /// Get the maximum number of interrupts supported
    fn max_interrupts(&self) -> InterruptId {
        self.max_interrupts
    }

    /// Get the number of CPUs supported
    fn max_cpus(&self) -> CpuId {
        self.max_cpus
    }
}

unsafe impl Send for Plic {}
unsafe impl Sync for Plic {}

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

    let controller = Box::new(Plic::new(base_addr, 1023, 4)); // Example values for max interrupts and CPUs

    match InterruptManager::global().lock().register_external_controller(controller) {
        Ok(_) => {
            crate::early_println!("[interrupt] PLIC registered at base address: {:#x}", base_addr);
        },
        Err(e) => {
            crate::early_println!("[interrupt] Failed to register PLIC: {}", e);
            return Err("Failed to register PLIC");
        }
    }

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "riscv-plic",
        probe_fn,
        remove_fn,
        vec!["sifive,plic-1.0.0", "riscv,plic0"],
    );
    // Register the driver with the kernel
    DeviceManager::get_mut_manager().register_driver(Box::new(driver), DriverPriority::Critical)
}

// driver_initcall!(register_driver);
early_initcall!(register_driver);

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_plic_creation() {
        let plic = Plic::new(0x1000_0000, 100, 8);
        assert_eq!(plic.max_interrupts(), 100);
        assert_eq!(plic.max_cpus(), 8);
    }

    #[test_case]
    fn test_address_calculation() {
        let plic = Plic::new(0x1000_0000, 100, 8);
        
        // Test priority address
        assert_eq!(plic.priority_addr(1), 0x1000_0004);
        assert_eq!(plic.priority_addr(10), 0x1000_0028);
        
        // Test enable address for S-Mode
        // CPU 0 -> Context 1
        assert_eq!(plic.enable_addr(0, 10), 0x1000_2080);
        // CPU 1 -> Context 3
        assert_eq!(plic.enable_addr(1, 40), 0x1000_2184);
        
        // Test threshold address for S-Mode
        // CPU 0 -> Context 1
        assert_eq!(plic.threshold_addr(0), 0x1020_1000);
        // CPU 1 -> Context 3
        assert_eq!(plic.threshold_addr(1), 0x1020_3000);
        
        // Test claim address for S-Mode
        // CPU 0 -> Context 1
        assert_eq!(plic.claim_addr(0), 0x1020_1004);
        // CPU 1 -> Context 3
        assert_eq!(plic.claim_addr(1), 0x1020_3004);
    }

    #[test_case]
    fn test_validation() {
        let plic = Plic::new(0x1000_0000, 100, 8);
        
        // Valid IDs should pass
        assert!(plic.validate_interrupt_id(1).is_ok());
        assert!(plic.validate_interrupt_id(100).is_ok());
        assert!(plic.validate_cpu_id(0).is_ok());
        assert!(plic.validate_cpu_id(7).is_ok());
        
        // Invalid IDs should fail
        assert!(plic.validate_interrupt_id(0).is_err());
        assert!(plic.validate_interrupt_id(101).is_err());
        assert!(plic.validate_cpu_id(8).is_err());
    }
}
