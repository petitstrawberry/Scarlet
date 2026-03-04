//! ARM Generic Interrupt Controller (GIC) Implementation
//!
//! The GIC is responsible for managing external interrupts from devices and
//! routing them to different CPUs with priority support in AArch64 systems.

use crate::{
    arch::mmio,
    device::{
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    driver_initcall, early_initcall,
    interrupt::{
        CpuId, InterruptError, InterruptId, InterruptManager, InterruptResult, Priority,
        controllers::{ExternalInterruptController, LocalInterruptType},
    },
};
use alloc::{boxed::Box, vec};

/// GIC Distributor register offsets
const GICD_CTLR: usize = 0x0000; // Distributor Control Register
const GICD_TYPER: usize = 0x0004; // Interrupt Controller Type Register
const GICD_IIDR: usize = 0x0008; // Distributor Implementer Identification Register
const GICD_IGROUPR: usize = 0x0080; // Interrupt Group Registers
const GICD_ISENABLER: usize = 0x0100; // Interrupt Set-Enable Registers
const GICD_ICENABLER: usize = 0x0180; // Interrupt Clear-Enable Registers
const GICD_ISPENDR: usize = 0x0200; // Interrupt Set-Pending Registers
const GICD_ICPENDR: usize = 0x0280; // Interrupt Clear-Pending Registers
const GICD_ISACTIVER: usize = 0x0300; // Interrupt Set-Active Registers
const GICD_ICACTIVER: usize = 0x0380; // Interrupt Clear-Active Registers
const GICD_IPRIORITYR: usize = 0x0400; // Interrupt Priority Registers
const GICD_ITARGETSR: usize = 0x0800; // Interrupt Processor Targets Registers
const GICD_ICFGR: usize = 0x0C00; // Interrupt Configuration Registers
const GICD_SGIR: usize = 0x0F00; // Software Generated Interrupt Register

/// GIC CPU Interface register offsets
const GICC_CTLR: usize = 0x0000; // CPU Interface Control Register
const GICC_PMR: usize = 0x0004; // Interrupt Priority Mask Register
const GICC_BPR: usize = 0x0008; // Binary Point Register
const GICC_IAR: usize = 0x000C; // Interrupt Acknowledge Register
const GICC_EOIR: usize = 0x0010; // End of Interrupt Register
const GICC_RPR: usize = 0x0014; // Running Priority Register
const GICC_HPPIR: usize = 0x0018; // Highest Priority Pending Interrupt Register

/// Maximum number of interrupts supported by this GIC implementation
const MAX_INTERRUPTS: InterruptId = 1020;

/// Maximum number of CPUs supported by this GIC implementation
const MAX_CPUS: CpuId = 8;

/// ARM GIC Implementation
pub struct Gic {
    /// Base address of the GIC Distributor
    dist_base_addr: usize,
    /// Base address of the GIC CPU Interface
    cpu_base_addr: usize,
    /// Maximum number of interrupts this GIC supports
    max_interrupts: InterruptId,
    /// Maximum number of CPUs this GIC supports
    max_cpus: CpuId,

    /// Last acknowledged interrupt value (raw GICC_IAR) per CPU.
    ///
    /// GICv2 requires writing the same value returned by GICC_IAR back to
    /// GICC_EOIR to complete the interrupt (it includes CPUID bits).
    last_iar: [u32; MAX_CPUS as usize],
}

impl Gic {
    /// Create a new GIC instance
    ///
    /// # Arguments
    ///
    /// * `dist_base_addr` - Physical base address of the GIC Distributor
    /// * `cpu_base_addr` - Physical base address of the GIC CPU Interface
    /// * `max_interrupts` - Maximum interrupt ID supported (0-based)
    /// * `max_cpus` - Maximum number of CPUs supported
    pub fn new(
        dist_base_addr: usize,
        cpu_base_addr: usize,
        max_interrupts: InterruptId,
        max_cpus: CpuId,
    ) -> Self {
        Self {
            dist_base_addr,
            cpu_base_addr,
            max_interrupts: max_interrupts.min(MAX_INTERRUPTS),
            max_cpus: max_cpus.min(MAX_CPUS),
            last_iar: [0; MAX_CPUS as usize],
        }
    }

    /// Translate Device Tree interrupt metadata to actual GIC interrupt ID
    ///
    /// ARM GIC uses a 3-cell interrupt format in Device Tree:
    /// - Cell 0: Interrupt type (0 = SPI, 1 = PPI)
    /// - Cell 1: Interrupt number (within type)
    /// - Cell 2: Flags (edge/level triggered, polarity)
    ///
    /// The actual GIC interrupt ID is calculated based on the type:
    /// - SPI (Shared Peripheral Interrupt): 32 + number
    /// - PPI (Private Peripheral Interrupt): 16 + number
    ///
    /// # Arguments
    ///
    /// * `metadata` - Interrupt metadata from Device Tree
    ///
    /// # Returns
    ///
    /// The actual GIC interrupt ID to use for enabling/registering the interrupt
    pub fn translate_interrupt(
        &self,
        metadata: &crate::device::platform::resource::IrqMetadata,
    ) -> InterruptId {
        let irq_type = metadata.irq_type;
        let irq_number = metadata.irq_number;

        if irq_type == 0 {
            // SPI (Shared Peripheral Interrupt): base at 32
            32 + irq_number as InterruptId
        } else {
            // PPI (Private Peripheral Interrupt): base at 16
            16 + irq_number as InterruptId
        }
    }

    /// Get the address of a distributor register
    fn dist_reg_addr(&self, offset: usize) -> usize {
        self.dist_base_addr + offset
    }

    /// Get the address of a CPU interface register for a specific CPU
    fn cpu_reg_addr(&self, cpu_id: CpuId, offset: usize) -> usize {
        // For now, assume single CPU interface base
        // In multi-core systems, this might need adjustment
        self.cpu_base_addr + offset
    }

    /// Get the address of an interrupt enable register
    fn enable_addr(&self, interrupt_id: InterruptId) -> usize {
        let word_offset = interrupt_id / 32;
        self.dist_reg_addr(GICD_ISENABLER + (word_offset as usize * 4))
    }

    /// Get the address of an interrupt disable register
    fn disable_addr(&self, interrupt_id: InterruptId) -> usize {
        let word_offset = interrupt_id / 32;
        self.dist_reg_addr(GICD_ICENABLER + (word_offset as usize * 4))
    }

    /// Get the address of an interrupt priority register
    fn priority_addr(&self, interrupt_id: InterruptId) -> usize {
        self.dist_reg_addr(GICD_IPRIORITYR + interrupt_id as usize)
    }

    /// Get the address of an interrupt target register
    fn target_addr(&self, interrupt_id: InterruptId) -> usize {
        self.dist_reg_addr(GICD_ITARGETSR + interrupt_id as usize)
    }

    /// Validate interrupt ID
    fn validate_interrupt_id(&self, interrupt_id: InterruptId) -> InterruptResult<()> {
        if interrupt_id > self.max_interrupts {
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

    /// Initialize the GIC distributor
    fn init_distributor(&self) {
        unsafe {
            // Disable distributor while programming.
            mmio::write32(self.dist_reg_addr(GICD_CTLR), 0x0);

            // Put all interrupts into Group 1 (non-secure) so they can be delivered at EL1.
            // This is especially important for PPIs like the architected timer.
            let words = (self.max_interrupts as usize + 32) / 32;
            for i in 0..words {
                mmio::write32(self.dist_reg_addr(GICD_IGROUPR + i * 4), 0xFFFF_FFFF);
            }

            // Pre-enable the virtual timer PPI (ID 27).
            let timer_ppi = crate::drivers::pic::arm_generic_timer::CNTV_PPI_IRQ;
            let timer_ppi_bit = 1u32 << timer_ppi;
            mmio::write32(self.dist_reg_addr(GICD_ISENABLER), timer_ppi_bit);

            // Set timer PPI priority.
            // For Group 1 non-secure interrupts, use priority 0x80 (bit 7 set).
            mmio::write8(self.priority_addr(timer_ppi), 0x80);

            // Enable the distributor for both Group 0 and Group 1 interrupts.
            // Bit 0 = Enable Group 0, Bit 1 = Enable Group 1.
            // For EL1 non-secure, we need Group 1 enabled (bit 1).
            // DEBUG: Also enable Group 0 (bit 0) to test if interrupts are being sent as Group 0.
            mmio::write32(self.dist_reg_addr(GICD_CTLR), 0x3);
        }
    }

    /// Initialize the GIC CPU interface for a specific CPU
    fn init_cpu_interface(&self, cpu_id: CpuId) {
        // Set priority mask to allow all interrupts (lower priority = higher precedence)
        unsafe {
            mmio::write32(self.cpu_reg_addr(cpu_id, GICC_PMR), 0xFF);
            // No priority grouping.
            mmio::write32(self.cpu_reg_addr(cpu_id, GICC_BPR), 0x0);
            // Enable the CPU interface for both Group 0 and Group 1 interrupts.
            // Bit 0 = Enable Group 0 (FIQ), Bit 1 = Enable Group 1 (IRQ).
            // For EL1 non-secure, we want Group 1 interrupts (bit 0).
            // DEBUG: Also enable Group 0 to test if interrupts are coming as Group 0.
            mmio::write32(self.cpu_reg_addr(cpu_id, GICC_CTLR), 0x3);
        }
    }

    /// Send an Inter-Processor Interrupt (IPI)
    pub fn send_ipi(
        &self,
        target_cpu_id: CpuId,
        ipi_type: LocalInterruptType,
    ) -> InterruptResult<()> {
        self.validate_cpu_id(target_cpu_id)?;

        // Software Generated Interrupt Register format:
        // [31:26] reserved
        // [25:24] TargetListFilter
        // [23:16] CPUTargetList
        // [15] reserved
        // [14:0] INTID
        let cpu_target_list = 1u32 << (target_cpu_id + 16);
        let int_id = match ipi_type {
            LocalInterruptType::Timer => crate::drivers::pic::arm_generic_timer::CNTV_PPI_IRQ,
            LocalInterruptType::Software => 0, // Software Generated Interrupt
            LocalInterruptType::External => 1, // Software Generated Interrupt
        };

        let sgir_value = cpu_target_list | int_id;
        let sgir_addr = self.dist_reg_addr(GICD_SGIR);
        unsafe { mmio::write32(sgir_addr, sgir_value) }

        Ok(())
    }

    /// Initialize the GIC for a specific CPU
    pub fn init_for_cpu(&self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;

        // Initialize distributor (only once, typically on CPU 0)
        if cpu_id == 0 {
            self.init_distributor();
        }

        // Initialize CPU interface for this CPU
        self.init_cpu_interface(cpu_id);

        Ok(())
    }
}

impl ExternalInterruptController for Gic {
    fn init(&mut self) -> InterruptResult<()> {
        // Initialize distributor
        self.init_distributor();

        // Ensure the CPU interface is enabled for the boot CPU so PPIs/IRQs can be delivered.
        self.init_cpu_interface(0);
        Ok(())
    }

    fn enable_interrupt(
        &mut self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        self.validate_cpu_id(cpu_id)?;

        // Configure interrupt as Group 1
        // GICD_IGROUPR: 1 bit per interrupt, 1=Group 1, 0=Group 0
        let word_offset = interrupt_id / 32;
        let bit_offset = interrupt_id % 32;
        let igroupr_addr = self.dist_reg_addr(GICD_IGROUPR + (word_offset as usize * 4));
        unsafe {
            let current = mmio::read32(igroupr_addr);
            let new_value = current | (1u32 << bit_offset);
            mmio::write32(igroupr_addr, new_value);
        }

        // Set interrupt target to the specified CPU (SPIs only).
        // SGIs/PPIs (0-31) are banked per-CPU and their ITARGETSR is RO/ignored.
        if interrupt_id >= 32 {
            let target_addr = self.target_addr(interrupt_id);
            let cpu_mask = 1u8 << cpu_id;
            unsafe { mmio::write8(target_addr, cpu_mask) }
        }
        // Enable the interrupt
        let enable_addr = self.enable_addr(interrupt_id);
        let bit = 1u32 << (interrupt_id % 32);

        // GICD_ISENABLER is write-1-to-set, so just write the bit we want to enable
        unsafe { mmio::write32(enable_addr, bit) }

        Ok(())
    }

    fn disable_interrupt(
        &mut self,
        interrupt_id: InterruptId,
        _cpu_id: CpuId,
    ) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;

        // Disable the interrupt
        let disable_addr = self.disable_addr(interrupt_id);
        let bit = 1u32 << (interrupt_id % 32);
        unsafe { mmio::write32(disable_addr, bit) }

        Ok(())
    }

    fn set_priority(
        &mut self,
        interrupt_id: InterruptId,
        priority: Priority,
    ) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;

        // Set interrupt priority (higher value = lower priority in GIC)
        let priority_addr = self.priority_addr(interrupt_id);
        unsafe { mmio::write8(priority_addr, priority as u8) }

        Ok(())
    }

    fn get_priority(&self, interrupt_id: InterruptId) -> InterruptResult<Priority> {
        self.validate_interrupt_id(interrupt_id)?;

        let priority_addr = self.priority_addr(interrupt_id);
        let priority = unsafe { mmio::read8(priority_addr) };

        Ok(priority as Priority)
    }

    fn set_threshold(&mut self, cpu_id: CpuId, threshold: Priority) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;

        // Set priority mask register (threshold)
        let pmr_addr = self.cpu_reg_addr(cpu_id, GICC_PMR);
        unsafe { mmio::write32(pmr_addr, threshold as u32) }

        Ok(())
    }

    fn get_threshold(&self, cpu_id: CpuId) -> InterruptResult<Priority> {
        self.validate_cpu_id(cpu_id)?;

        let pmr_addr = self.cpu_reg_addr(cpu_id, GICC_PMR);
        let threshold = unsafe { mmio::read32(pmr_addr) };

        Ok(threshold as Priority)
    }

    fn claim_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<Option<InterruptId>> {
        self.validate_cpu_id(cpu_id)?;

        // Read interrupt acknowledge register
        let iar_addr = self.cpu_reg_addr(cpu_id, GICC_IAR);
        let iar = unsafe { mmio::read32(iar_addr) };

        // Remember the raw acknowledge value for correct EOI later.
        self.last_iar[cpu_id as usize] = iar;

        // Extract interrupt ID (bits 0-9)
        let interrupt_id = iar & 0x3FF;

        // Check for spurious interrupt (1023 or higher)
        if interrupt_id >= 1020 {
            // CRITICAL: Even for spurious interrupts, we must write EOI
            // Otherwise the GIC will not deliver the next interrupt
            let eoir_addr = self.cpu_reg_addr(cpu_id, GICC_EOIR);
            unsafe {
                mmio::write32(eoir_addr, iar);
            }
            Ok(None)
        } else {
            Ok(Some(interrupt_id))
        }
    }

    fn complete_interrupt(
        &mut self,
        cpu_id: CpuId,
        interrupt_id: InterruptId,
    ) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        self.validate_cpu_id(cpu_id)?;

        // Write to End of Interrupt Register
        let eoir_addr = self.cpu_reg_addr(cpu_id, GICC_EOIR);
        unsafe {
            // GICv2 expects the raw value read from GICC_IAR (includes CPUID bits).
            let iar = self.last_iar[cpu_id as usize];
            let eoi_value = if iar != 0 { iar } else { interrupt_id };
            mmio::write32(eoir_addr, eoi_value);
        }

        self.last_iar[cpu_id as usize] = 0;

        Ok(())
    }

    fn is_pending(&self, interrupt_id: InterruptId) -> bool {
        if self.validate_interrupt_id(interrupt_id).is_err() {
            return false;
        }

        // Check pending register
        let word_offset = interrupt_id / 32;
        let bit_offset = interrupt_id % 32;
        let pending_addr = self.dist_reg_addr(GICD_ISPENDR + (word_offset as usize * 4));

        let pending_word = unsafe { mmio::read32(pending_addr) };
        (pending_word & (1 << bit_offset)) != 0
    }

    fn max_interrupts(&self) -> InterruptId {
        self.max_interrupts
    }

    fn max_cpus(&self) -> CpuId {
        self.max_cpus
    }
}

unsafe impl Send for Gic {}
unsafe impl Sync for Gic {}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    // Extract distributor and CPU interface base addresses from device tree.
    // QEMU virt (GICv2) provides separate MMIO regions for distributor and CPU interface.
    let mem_resources: alloc::vec::Vec<_> = device
        .get_resources()
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    let dist_base_addr = match mem_resources.get(0) {
        Some(resource) => resource.start as usize,
        None => return Err("No memory resource found for GIC distributor"),
    };

    // Prefer the second MEM region for the CPU interface.
    // Fallback: common GICv2 layout uses dist + 0x10000.
    let cpu_base_addr = mem_resources
        .get(1)
        .map(|resource| resource.start as usize)
        .unwrap_or(dist_base_addr + 0x10000);

    // TODO: Parse actual interrupt count and CPU count from device tree
    let max_interrupts = 256; // Typical value
    let max_cpus = 4; // Typical value

    let gic = Box::new(Gic::new(
        dist_base_addr,
        cpu_base_addr,
        max_interrupts,
        max_cpus,
    ));

    // Register with interrupt manager
    InterruptManager::with_manager(|manager| {
        manager
            .register_external_controller(gic)
            .map_err(|_| "Failed to register GIC")?;
        Ok(())
    })?;

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "arm,gic-400",
        probe_fn,
        remove_fn,
        vec!["arm,gic-400", "arm,cortex-a15-gic", "arm,cortex-a9-gic"],
    );
    // Register the driver with the kernel
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical)
}

// driver_initcall!(register_driver);
early_initcall!(register_driver);
