#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_initcall,
    interrupt::{
        CpuId, InterruptError, InterruptId, InterruptManager, InterruptResult, Priority,
        controllers::ExternalInterruptController,
    },
};

// =============================================================================
// Register Offsets
// =============================================================================

/// AIC Info register - contains NR_HW in bits [15:0]
const AIC_INFO: usize = 0x0004;
/// AIC Configuration register
const AIC_CONFIG: usize = 0x0010;
/// AIC WHOAMI register - returns current CPU ID
const AIC_WHOAMI: usize = 0x2000;
/// AIC Event register - DIE in [31:24], type in [23:16], number in [15:0]
const AIC_EVENT: usize = 0x2004;
/// AIC IPI Send register
const AIC_IPI_SEND: usize = 0x2008;
/// AIC IPI Acknowledge register
const AIC_IPI_ACK: usize = 0x200C;
/// AIC IPI Mask Set register
const AIC_IPI_MASK_SET: usize = 0x2024;
/// AIC IPI Mask Clear register
const AIC_IPI_MASK_CLR: usize = 0x2028;
/// AIC Target CPU base - one 32-bit register per IRQ
const AIC_TARGET_CPU: usize = 0x3000;
/// AIC Software Set base - software trigger set
const AIC_SW_SET: usize = 0x4000;
/// AIC Software Clear base - software trigger clear
const AIC_SW_CLR: usize = 0x4080;
/// AIC Mask Set base - IRQ mask set
const AIC_MASK_SET: usize = 0x4100;
/// AIC Mask Clear base - IRQ mask clear
const AIC_MASK_CLR: usize = 0x4180;

// =============================================================================
// Event Types (from AIC_EVENT register)
// =============================================================================

/// No event pending
const AIC_EVENT_TYPE_NONE: u32 = 0;
/// Hardware IRQ event
const AIC_EVENT_TYPE_HW: u32 = 1;
/// IPI event
const AIC_EVENT_TYPE_IPI: u32 = 4;

/// IPI "other" type in event number field
const AIC_EVENT_IPI_OTHER: u32 = 1;
/// IPI "self" type in event number field
const AIC_EVENT_IPI_SELF: u32 = 2;

// =============================================================================
// IPI Bits
// =============================================================================

/// Bit for "other" IPI (from other CPUs)
const AIC_IPI_OTHER: u32 = 1 << 0;
/// Bit for "self" IPI
const AIC_IPI_SELF: u32 = 1 << 31;

// =============================================================================
// Constants
// =============================================================================

/// Maximum number of CPUs supported by AIC (31 bits in IPI SEND, bit 31 is self)
const AIC_MAX_CPUS: CpuId = 31;

// =============================================================================
// Helper Functions
// =============================================================================

/// Calculate the mask register offset for a given IRQ.
///
/// AIC uses 32 IRQs per 32-bit register.
#[inline]
const fn mask_reg(irq: InterruptId) -> usize {
    4 * (irq as usize >> 5)
}

/// Calculate the bit position within a mask register for a given IRQ.
#[inline]
const fn mask_bit(irq: InterruptId) -> u32 {
    1 << (irq & 0x1f)
}

// =============================================================================
// AIC Driver Structure
// =============================================================================

/// Apple Interrupt Controller (AIC) driver.
///
/// Implements the `ExternalInterruptController` trait for Apple Silicon SoCs.
pub struct Aic {
    /// Base address of the AIC MMIO region
    base_addr: usize,
    /// Number of hardware IRQs supported (read from AIC_INFO)
    num_irqs: InterruptId,
    /// Maximum number of CPUs supported
    max_cpus: CpuId,
}

impl Aic {
    /// Create a new AIC instance.
    ///
    /// Reads the number of hardware IRQs from the AIC_INFO register.
    ///
    /// # Arguments
    ///
    /// * `base_addr` - Virtual address of the AIC MMIO region
    /// * `max_cpus` - Maximum number of CPUs to support
    ///
    /// # Returns
    ///
    /// A new `Aic` instance with `num_irqs` populated from hardware.
    pub fn new(base_addr: usize, max_cpus: CpuId) -> Self {
        // Read AIC_INFO to get number of hardware IRQs
        let info = unsafe { mmio::read32(base_addr + AIC_INFO) };
        let num_irqs = info & 0xFFFF; // bits [15:0] = NR_HW

        scarlet::early_println!(
            "[AIC] new: base_addr={:#x}, num_irqs={}, max_cpus={}",
            base_addr,
            num_irqs,
            max_cpus
        );

        Self {
            base_addr,
            num_irqs,
            max_cpus: max_cpus.min(AIC_MAX_CPUS),
        }
    }

    /// Get the address of a register.
    #[inline]
    fn reg_addr(&self, offset: usize) -> usize {
        self.base_addr + offset
    }

    /// Validate an interrupt ID.
    ///
    /// AIC IRQs are 0-indexed, so valid IDs are 0..num_irqs.
    fn validate_interrupt_id(&self, interrupt_id: InterruptId) -> InterruptResult<()> {
        if interrupt_id >= self.num_irqs {
            Err(InterruptError::InvalidInterruptId)
        } else {
            Ok(())
        }
    }

    /// Validate a CPU ID.
    fn validate_cpu_id(&self, cpu_id: CpuId) -> InterruptResult<()> {
        if cpu_id >= self.max_cpus {
            Err(InterruptError::InvalidCpuId)
        } else {
            Ok(())
        }
    }

    /// Initialize the AIC hardware.
    ///
    /// - Masks all IRQs
    /// - Clears all software triggers
    /// - Sets default CPU affinity (CPU 0)
    /// - Masks IPIs initially
    fn init_hw(&self) {
        // Calculate number of mask registers needed
        let num_mask_regs = ((self.num_irqs as usize) + 31) / 32;

        // Mask all IRQs
        for i in 0..num_mask_regs {
            unsafe {
                mmio::write32(self.reg_addr(AIC_MASK_SET + i * 4), 0xFFFF_FFFF);
            }
        }

        // Clear all software triggers
        for i in 0..num_mask_regs {
            unsafe {
                mmio::write32(self.reg_addr(AIC_SW_CLR + i * 4), 0xFFFF_FFFF);
            }
        }

        for irq in 0..self.num_irqs {
            unsafe {
                mmio::write32(self.reg_addr(AIC_TARGET_CPU + irq as usize * 4), 1);
            }
        }

        // Mask IPIs initially (will be unmasked when needed)
        unsafe {
            mmio::write32(
                self.reg_addr(AIC_IPI_MASK_SET),
                AIC_IPI_SELF | AIC_IPI_OTHER,
            );
        }

        // Ack any pending IPIs
        unsafe {
            mmio::write32(self.reg_addr(AIC_IPI_ACK), AIC_IPI_SELF | AIC_IPI_OTHER);
        }

        scarlet::early_println!(
            "[AIC] init_hw: masked {} IRQs, cleared software triggers",
            self.num_irqs
        );
    }

    // =========================================================================
    // IPI Support Methods
    // =========================================================================

    /// Send an IPI to a specific CPU.
    ///
    /// # Arguments
    ///
    /// * `target_cpu` - CPU ID to send the IPI to
    pub fn send_ipi(&self, target_cpu: CpuId) {
        if target_cpu >= self.max_cpus {
            return;
        }
        let bit = AIC_IPI_SEND_CPU(target_cpu);
        unsafe {
            mmio::write32(self.reg_addr(AIC_IPI_SEND), bit);
        }
    }

    /// Send a "self" IPI to the current CPU.
    pub fn send_ipi_self(&self) {
        unsafe {
            mmio::write32(self.reg_addr(AIC_IPI_SEND), AIC_IPI_SELF);
        }
    }

    /// Acknowledge pending IPIs.
    ///
    /// Returns the IPI bits that were pending.
    pub fn ack_ipi(&self) -> u32 {
        unsafe {
            let pending = mmio::read32(self.reg_addr(AIC_IPI_ACK));
            mmio::write32(self.reg_addr(AIC_IPI_ACK), pending);
            pending
        }
    }

    /// Unmask IPIs for the current CPU.
    pub fn unmask_ipis(&self) {
        unsafe {
            mmio::write32(
                self.reg_addr(AIC_IPI_MASK_CLR),
                AIC_IPI_SELF | AIC_IPI_OTHER,
            );
        }
    }

    /// Mask IPIs for the current CPU.
    pub fn mask_ipis(&self) {
        unsafe {
            mmio::write32(
                self.reg_addr(AIC_IPI_MASK_SET),
                AIC_IPI_SELF | AIC_IPI_OTHER,
            );
        }
    }

    /// Get the current CPU ID from AIC_WHOAMI.
    pub fn whoami(&self) -> CpuId {
        unsafe { mmio::read32(self.reg_addr(AIC_WHOAMI)) }
    }
}

/// Helper to create CPU bit for IPI send register.
const fn AIC_IPI_SEND_CPU(cpu: CpuId) -> u32 {
    1u32 << cpu
}

// =============================================================================
// ExternalInterruptController Trait Implementation
// =============================================================================

impl ExternalInterruptController for Aic {
    /// Initialize the AIC.
    fn init(&mut self) -> InterruptResult<()> {
        scarlet::early_println!("[AIC] init: initializing hardware...");

        // Initialize hardware
        self.init_hw();

        // Verify CPU ID matches
        let cpu_id = self.whoami();
        scarlet::early_println!("[AIC] init: WHOAMI={}", cpu_id);

        // Unmask IPIs for the current CPU so they can be delivered
        self.unmask_ipis();
        scarlet::early_println!("[AIC] init: IPIs unmasked for CPU {}", cpu_id);

        Ok(())
    }

    /// Enable a specific interrupt for a CPU.
    ///
    /// This sets the CPU affinity and unmasks the IRQ.
    fn enable_interrupt(
        &mut self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        self.validate_cpu_id(cpu_id)?;

        // Set CPU affinity
        let target_addr = self.reg_addr(AIC_TARGET_CPU + interrupt_id as usize * 4);
        unsafe {
            mmio::write32(target_addr, 1 << cpu_id);
        }

        // Unmask the IRQ
        let mask_addr = self.reg_addr(AIC_MASK_CLR + mask_reg(interrupt_id));
        unsafe {
            mmio::write32(mask_addr, mask_bit(interrupt_id));
        }

        Ok(())
    }

    /// Disable a specific interrupt for a CPU.
    fn disable_interrupt(
        &mut self,
        interrupt_id: InterruptId,
        _cpu_id: CpuId,
    ) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;

        // Mask the IRQ
        let mask_addr = self.reg_addr(AIC_MASK_SET + mask_reg(interrupt_id));
        unsafe {
            mmio::write32(mask_addr, mask_bit(interrupt_id));
        }

        Ok(())
    }

    /// Set priority for a specific interrupt.
    ///
    /// AIC has automatic prioritization (lower IRQ = higher priority).
    /// This is a no-op that always succeeds.
    fn set_priority(
        &mut self,
        interrupt_id: InterruptId,
        _priority: Priority,
    ) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        // AIC has automatic prioritization, no per-IRQ priority registers
        Ok(())
    }

    /// Get priority for a specific interrupt.
    ///
    /// Returns 0 since AIC has automatic prioritization.
    fn get_priority(&self, interrupt_id: InterruptId) -> InterruptResult<Priority> {
        self.validate_interrupt_id(interrupt_id)?;
        // AIC has automatic prioritization, return 0 as default
        Ok(0)
    }

    /// Set priority threshold for a CPU.
    ///
    /// AIC does not support priority thresholds.
    /// This is a no-op that always succeeds.
    fn set_threshold(&mut self, cpu_id: CpuId, _threshold: Priority) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        // AIC does not support priority thresholds
        Ok(())
    }

    /// Get priority threshold for a CPU.
    ///
    /// Returns 0 since AIC does not support priority thresholds.
    fn get_threshold(&self, cpu_id: CpuId) -> InterruptResult<Priority> {
        self.validate_cpu_id(cpu_id)?;
        // AIC does not support priority thresholds
        Ok(0)
    }

    /// Claim an interrupt (acknowledge and get the interrupt ID).
    ///
    /// Reads the AIC_EVENT register to determine the interrupt type and number.
    /// For hardware IRQs, returns the interrupt ID.
    /// For IPIs, acknowledges them internally and returns None.
    fn claim_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<Option<InterruptId>> {
        self.validate_cpu_id(cpu_id)?;

        // Read the event register
        let event = unsafe { mmio::read32(self.reg_addr(AIC_EVENT)) };

        // Extract event type and number
        // EVENT format: [31:24]=DIE, [23:16]=TYPE, [15:0]=NUMBER
        let event_die = (event >> 24) & 0xFF;
        let event_type = (event >> 16) & 0xFF;
        let event_num = event & 0xFFFF;

        // For AIC v1 (single-die), DIE should always be 0
        // If DIE is non-zero on v1 hardware, treat as spurious
        if event_die != 0 {
            return Ok(None);
        }

        match event_type {
            AIC_EVENT_TYPE_HW => {
                // Hardware IRQ - the IRQ is auto-masked by hardware
                Ok(Some(event_num))
            }
            AIC_EVENT_TYPE_IPI => {
                // IPI - acknowledge it internally
                match event_num {
                    AIC_EVENT_IPI_OTHER | AIC_EVENT_IPI_SELF => {
                        // Ack the IPI
                        unsafe {
                            let ack_bit = if event_num == AIC_EVENT_IPI_OTHER {
                                AIC_IPI_OTHER
                            } else {
                                AIC_IPI_SELF
                            };
                            mmio::write32(self.reg_addr(AIC_IPI_ACK), ack_bit);
                            // Unmask the IPI to allow future IPIs
                            mmio::write32(self.reg_addr(AIC_IPI_MASK_CLR), ack_bit);
                        }
                    }
                    _ => {}
                }
                Ok(None)
            }
            AIC_EVENT_TYPE_NONE | _ => {
                // No event or unknown type
                Ok(None)
            }
        }
    }

    /// Complete an interrupt (signal that handling is finished).
    ///
    /// AIC auto-masks IRQs on delivery. To "complete" the interrupt,
    /// we unmask it to re-enable future occurrences.
    fn complete_interrupt(
        &mut self,
        cpu_id: CpuId,
        interrupt_id: InterruptId,
    ) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        self.validate_interrupt_id(interrupt_id)?;

        // Unmask the IRQ to re-enable it
        let mask_addr = self.reg_addr(AIC_MASK_CLR + mask_reg(interrupt_id));
        unsafe {
            mmio::write32(mask_addr, mask_bit(interrupt_id));
        }

        Ok(())
    }

    /// Check if a specific interrupt is pending.
    ///
    /// Note: AIC does not have a dedicated pending register like GIC.
    /// AIC is event-driven - use claim_interrupt() to discover pending events.
    /// This implementation conservatively returns false.
    fn is_pending(&self, interrupt_id: InterruptId) -> bool {
        if self.validate_interrupt_id(interrupt_id).is_err() {
            return false;
        }
        // AIC is event-driven without a dedicated pending register.
        // Return false conservatively - use claim_interrupt() to discover events.
        false
    }

    /// Get the maximum number of interrupts supported.
    fn max_interrupts(&self) -> InterruptId {
        self.num_irqs
    }

    /// Get the number of CPUs supported.
    fn max_cpus(&self) -> CpuId {
        self.max_cpus
    }
}

// =============================================================================
// Safety: AIC can be sent between threads safely
// =============================================================================

unsafe impl Send for Aic {}
unsafe impl Sync for Aic {}

// =============================================================================
// Platform Device Driver Registration
// =============================================================================

/// Probe function for AIC platform device.
fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    scarlet::early_println!("[AIC] probe: probing device {}", device.name());

    // Get memory resources
    let mem_resources: alloc::vec::Vec<_> = device
        .get_resources()
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    let paddr = mem_resources
        .get(0)
        .map(|r| r.start)
        .ok_or("No memory resource found for AIC")?;
    let size = mem_resources
        .get(0)
        .map(|r| r.end - r.start + 1)
        .unwrap_or(0x8000);

    scarlet::early_println!(
        "[AIC] probe: physical address={:#x}, size={:#x}",
        paddr,
        size
    );

    // Map the AIC MMIO region
    let base_addr = scarlet::vm::ioremap(paddr, size).map_err(|e| {
        scarlet::early_println!("[AIC] probe: ioremap failed: {}", e);
        e
    })?;

    scarlet::early_println!("[AIC] probe: mapped to virtual address={:#x}", base_addr);

    // Determine max_cpus from environment
    let max_cpus = scarlet::environment::MAX_NUM_CPUS as CpuId;

    scarlet::early_println!("[AIC] probe: max_cpus={}", max_cpus);

    let aic = Box::new(Aic::new(base_addr, max_cpus));

    // Register with interrupt manager
    match InterruptManager::with_manager(|manager| {
        manager
            .register_external_controller(aic)
            .map_err(|_| "Failed to register AIC")
    }) {
        Ok(()) => {
            scarlet::arch::interrupt::configure_timer_interrupt_route(
                scarlet::arch::interrupt::TimerInterruptRoute::FastInterrupt,
                None,
            );
            scarlet::early_println!("[AIC] probe: AIC registered successfully");
            Ok(())
        }
        Err(e) => {
            scarlet::early_println!("[AIC] probe: registration failed: {}", e);
            // Clean up the ioremap
            scarlet::vm::iounmap(base_addr);
            Err(e)
        }
    }
}

/// Remove function for AIC platform device.
fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    // Nothing to clean up currently
    Ok(())
}

/// Register the AIC driver with the device manager.
fn register_driver() {
    scarlet::early_println!("[AIC] register_driver: registering AIC driver");

    let driver = PlatformDeviceDriver::new(
        "apple,aic",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,aic",
            "apple,t8103-aic",
            "apple,t6000-aic",
            "apple,t8112-aic",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical);
}

early_initcall!(register_driver);

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_mask_reg_calculation() {
        // First register (IRQs 0-31)
        assert_eq!(mask_reg(0), 0);
        assert_eq!(mask_reg(1), 0);
        assert_eq!(mask_reg(31), 0);

        // Second register (IRQs 32-63)
        assert_eq!(mask_reg(32), 4);
        assert_eq!(mask_reg(63), 4);

        // Third register (IRQs 64-95)
        assert_eq!(mask_reg(64), 8);
        assert_eq!(mask_reg(95), 8);

        // IRQ 895 (in word 27)
        assert_eq!(mask_reg(895), 4 * 27);
    }

    #[test_case]
    fn test_mask_bit_calculation() {
        assert_eq!(mask_bit(0), 1);
        assert_eq!(mask_bit(1), 2);
        assert_eq!(mask_bit(2), 4);
        assert_eq!(mask_bit(31), 1 << 31);

        // Wraps to bit 0 of next word
        assert_eq!(mask_bit(32), 1);
        assert_eq!(mask_bit(33), 2);
        assert_eq!(mask_bit(63), 1 << 31);
    }

    #[test_case]
    fn test_event_type_extraction() {
        // Simulate AIC_EVENT register values
        let hw_event: u32 = (AIC_EVENT_TYPE_HW << 16) | 42;
        assert_eq!((hw_event >> 16) & 0xFFFF, AIC_EVENT_TYPE_HW);
        assert_eq!(hw_event & 0xFFFF, 42);

        let ipi_event: u32 = (AIC_EVENT_TYPE_IPI << 16) | AIC_EVENT_IPI_OTHER;
        assert_eq!((ipi_event >> 16) & 0xFFFF, AIC_EVENT_TYPE_IPI);
        assert_eq!(ipi_event & 0xFFFF, AIC_EVENT_IPI_OTHER);

        let no_event: u32 = 0;
        assert_eq!((no_event >> 16) & 0xFFFF, AIC_EVENT_TYPE_NONE);
    }

    #[test_case]
    fn test_ipi_send_cpu_bit() {
        assert_eq!(AIC_IPI_SEND_CPU(0), 1);
        assert_eq!(AIC_IPI_SEND_CPU(1), 2);
        assert_eq!(AIC_IPI_SEND_CPU(2), 4);
        assert_eq!(AIC_IPI_SEND_CPU(30), 1u32 << 30);
    }
}

#[used]
static SCARLET_DRIVER_APPLE_AIC_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
