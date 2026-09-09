//! RISC-V Platform-Level Interrupt Controller (PLIC) Implementation
//!
//! The PLIC is responsible for managing external interrupts from devices and
//! routing them to different CPUs with priority support.

use crate::{
    device::{
        DeviceInfo,
        fdt::FdtManager,
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_initcall,
    interrupt::{
        CpuId, InterruptError, InterruptId, InterruptResult, Priority,
        controllers::{
            ExternalInterruptController, InterruptControllerInitMode, IrqFlow, IrqMapping,
            PendingIrq,
        },
    },
};
use alloc::{boxed::Box, vec, vec::Vec};
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
    /// Maximum number of CPUs (harts) this PLIC supports
    max_cpus: CpuId,
    /// S-mode context ID for each CPU (hart).
    /// Index = CPU ID, Value = PLIC context ID for S-mode external interrupt.
    /// If None, use the default formula: (cpu_id * 2) + 1
    s_mode_contexts: Option<Vec<usize>>,
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
            s_mode_contexts: None,
        }
    }

    /// Create a new PLIC instance with explicit S-mode context mapping
    ///
    /// # Arguments
    ///
    /// * `base_addr` - Physical base address of the PLIC
    /// * `max_interrupts` - Maximum interrupt ID supported (1-based)
    /// * `s_mode_context_ids` - Vector mapping CPU ID -> PLIC context ID for S-mode
    pub fn with_contexts(
        base_addr: usize,
        max_interrupts: InterruptId,
        s_mode_context_ids: Vec<usize>,
    ) -> Self {
        let max_cpus = s_mode_context_ids.len() as CpuId;
        Self {
            base_addr,
            max_interrupts: max_interrupts.min(MAX_INTERRUPTS),
            max_cpus: max_cpus.min(MAX_CPUS),
            s_mode_contexts: Some(s_mode_context_ids),
        }
    }

    /// Convert CPU ID to PLIC context ID for Supervisor mode.
    /// If explicit mapping exists, use it; otherwise Hart 0 S-Mode -> Context 1, etc.
    fn context_id_for_cpu(&self, cpu_id: CpuId) -> usize {
        if let Some(ref contexts) = self.s_mode_contexts {
            contexts.get(cpu_id as usize).copied().unwrap_or(0)
        } else {
            // Default: Hart 0 S-Mode -> Context 1, Hart 1 S-Mode -> Context 3, etc.
            (cpu_id as usize * 2) + 1
        }
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

    /// MMIO write with readback verification
    #[inline(always)]
    fn mmio_write32_with_readback(addr: usize, value: u32) -> u32 {
        unsafe {
            write_volatile(addr as *mut u32, value);
            crate::arch::mmio_fence();
            read_volatile(addr as *const u32)
        }
    }
}

impl ExternalInterruptController for Plic {
    /// Initialize the PLIC
    fn init(&mut self, mode: InterruptControllerInitMode) -> InterruptResult<()> {
        debug_assert_eq!(mode, InterruptControllerInitMode::ColdBootReset);
        crate::println!(
            "[PLIC] init: max_cpus={}, max_interrupts={}, s_mode_contexts={:?}",
            self.max_cpus,
            self.max_interrupts,
            self.s_mode_contexts
        );

        // Linux assigns every PLIC source priority 1 and separately clears all
        // enables in each owned hart context. Priority 0 is the mask state used
        // by the per-IRQ runtime path, not the controller's cold baseline.
        for interrupt_id in 1..=self.max_interrupts {
            self.set_priority(interrupt_id, 1)?;
        }

        Ok(())
    }

    fn init_for_cpu(
        &mut self,
        cpu_id: CpuId,
        mode: InterruptControllerInitMode,
    ) -> InterruptResult<()> {
        debug_assert_eq!(mode, InterruptControllerInitMode::ColdBootReset);
        self.validate_cpu_id(cpu_id)?;

        let word_count = (self.max_interrupts as usize + 1).div_ceil(32);
        let context_id = self.context_id_for_cpu(cpu_id);
        let context_offset = context_id * PLIC_ENABLE_CONTEXT_STRIDE;

        for word in 0..word_count {
            let addr = self.base_addr + PLIC_ENABLE_BASE + context_offset + (word * 4);
            let verify = Self::mmio_write32_with_readback(addr, 0);
            if verify != 0 {
                crate::println!(
                    "PLIC init_for_cpu: clear enable verify failed: cpu={}, context={}, addr={:#x}, read={}",
                    cpu_id,
                    context_id,
                    addr,
                    verify
                );
                return Err(InterruptError::HardwareError);
            }
        }

        self.set_threshold(cpu_id, 0)
    }

    /// Enable a specific interrupt for a CPU
    fn enable_interrupt(&self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        self.validate_cpu_id(cpu_id)?;

        let context_id = self.context_id_for_cpu(cpu_id);
        let addr = self.enable_addr(cpu_id, interrupt_id);
        let bit_offset = interrupt_id % 32;

        unsafe {
            let current = read_volatile(addr as *const u32);
            let new_value = current | (1 << bit_offset);
            let verify = Self::mmio_write32_with_readback(addr, new_value);
            if verify != new_value {
                crate::println!(
                    "PLIC enable_interrupt verify failed: irq={}, cpu={}, context={}, addr={:#x}, bit={}, wrote={}, read={}",
                    interrupt_id,
                    cpu_id,
                    context_id,
                    addr,
                    bit_offset,
                    new_value,
                    verify
                );
                return Err(InterruptError::InvalidInterruptId);
            }
        }

        Ok(())
    }

    /// Disable a specific interrupt for a CPU
    fn disable_interrupt(&self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()> {
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

    fn mask_irq(&self, irq: &PendingIrq) -> InterruptResult<()> {
        self.disable_interrupt(irq.mapping.hwirq, irq.cpu_id)
    }

    fn unmask_irq(&self, irq: &PendingIrq) -> InterruptResult<()> {
        self.enable_interrupt(irq.mapping.hwirq, irq.cpu_id)
    }

    /// Set priority for a specific interrupt
    fn set_priority(
        &mut self,
        interrupt_id: InterruptId,
        priority: Priority,
    ) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;

        if priority > 7 {
            return Err(InterruptError::InvalidPriority);
        }

        let addr = self.priority_addr(interrupt_id);
        let verify = Self::mmio_write32_with_readback(addr, priority);
        if verify != priority {
            // Verification failed: MMIO write did not persist the expected value.
            // Return an InterruptError instead of panicking for consistent error handling.
            crate::println!(
                "PLIC set_priority verify failed: irq={}, addr={:#x}, wrote={}, read={}",
                interrupt_id,
                addr,
                priority,
                verify
            );
            return Err(InterruptError::InvalidPriority);
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
        let verify = Self::mmio_write32_with_readback(addr, threshold);

        if verify != threshold {
            // Verification failed: MMIO write did not persist the expected value.
            // Return an InterruptError instead of panicking for consistent error handling.
            crate::println!(
                "PLIC set_threshold verify failed: cpu={}, addr={:#x}, wrote={}, read={}",
                cpu_id,
                addr,
                threshold,
                verify
            );
            return Err(InterruptError::InvalidPriority);
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
    fn claim_interrupt(&self, cpu_id: CpuId) -> InterruptResult<Option<InterruptId>> {
        self.validate_cpu_id(cpu_id)?;

        let addr = self.claim_addr(cpu_id);
        let interrupt_id = unsafe { read_volatile(addr as *const u32) };

        if interrupt_id == 0 {
            Ok(None)
        } else {
            Ok(Some(interrupt_id))
        }
    }

    fn claim_pending_irq(&self, cpu_id: CpuId) -> InterruptResult<Option<PendingIrq>> {
        Ok(self
            .claim_interrupt(cpu_id)?
            .map(|interrupt_id| PendingIrq {
                mapping: IrqMapping::legacy(interrupt_id, IrqFlow::Level),
                cpu_id,
            }))
    }

    /// Complete an interrupt (signal that handling is finished)
    fn complete_interrupt(&self, cpu_id: CpuId, interrupt_id: InterruptId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        self.validate_interrupt_id(interrupt_id)?;

        let addr = self.claim_addr(cpu_id);
        unsafe {
            write_volatile(addr as *mut u32, interrupt_id);
            crate::arch::mmio_fence();
        }

        Ok(())
    }

    fn eoi_irq(&self, irq: &PendingIrq) -> InterruptResult<()> {
        self.complete_interrupt(irq.cpu_id, irq.mapping.hwirq)
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
    let mem_res = res
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("Memory resource not found")?;

    let paddr = mem_res.start;
    let size = mem_res.size()?;

    // Map the PLIC's physical MMIO region into the kernel virtual address space.
    let base_addr = crate::vm::ioremap(paddr, size).map_err(|e| {
        crate::println!(
            "[interrupt] PLIC ioremap({:#x}, {:#x}) failed: {}",
            paddr,
            size,
            e
        );
        e
    })?;

    let (max_interrupts, s_mode_contexts) = get_plic_config_from_fdt(device.name())
        .ok_or("PLIC requires valid CPU interrupt-context mappings")?;
    crate::println!(
        "[interrupt] PLIC: FDT config found - ndev={}, contexts={:?}",
        max_interrupts,
        s_mode_contexts
    );
    let controller = Box::new(Plic::with_contexts(
        base_addr,
        max_interrupts,
        s_mode_contexts,
    ));

    match crate::interrupt::InterruptManager::global().register_external_controller(controller) {
        Ok(_) => {
            crate::println!(
                "[interrupt] PLIC registered at base address: {:#x}",
                base_addr
            );
        }
        Err(e) => {
            crate::println!("[interrupt] Failed to register PLIC: {}", e);
            return Err("Failed to register PLIC");
        }
    }

    Ok(())
}

/// Extract PLIC configuration from FDT
///
/// Parses the `riscv,ndev` property for max interrupt count and
/// `interrupts-extended` property to determine S-mode context IDs per hart.
///
/// # Arguments
/// * `device_name` - The name of the PLIC device node (e.g., "plic@c000000")
///
/// # Returns
/// * `Some((max_interrupts, s_mode_contexts))` on success
/// * `None` if FDT is not available or properties cannot be read
fn get_plic_config_from_fdt(device_name: &str) -> Option<(InterruptId, Vec<usize>)> {
    let fdt = FdtManager::get_manager().get_fdt()?;
    let plic_node = fdt.all_nodes().find(|node| node.name == device_name)?;
    let max_interrupts =
        u32::from_be_bytes(plic_node.property("riscv,ndev")?.value.try_into().ok()?);
    let cpus = fdt.find_node("/cpus")?;
    let mut contexts = vec![None; crate::environment::MAX_NUM_CPUS];
    let mut offset = 0usize;
    let mut context_id = 0usize;
    let bytes = plic_node.property("interrupts-extended")?.value;
    while offset < bytes.len() {
        let phandle =
            u32::from_be_bytes(bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?);
        offset += 4;
        let intc = fdt.all_nodes().find(|node| {
            node.property("phandle")
                .is_some_and(|p| p.value == phandle.to_be_bytes())
        })?;
        // RISC-V per-CPU interrupt controllers encode a single interrupt ID.
        let cells = u32::from_be_bytes(intc.property("#interrupt-cells")?.value.try_into().ok()?);
        if cells != 1 {
            return None;
        }
        let irq = u32::from_be_bytes(bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?);
        offset += 4;
        if irq == 9 {
            let cpu = cpus.children().find(|cpu| {
                cpu.children().any(|child| {
                    child
                        .property("phandle")
                        .is_some_and(|p| p.value == phandle.to_be_bytes())
                })
            })?;
            let reg = cpu.raw_reg()?.next()?;
            let hart = usize::try_from(crate::device::fdt::decode_address(reg.address)?).ok()?;
            if let Some(logical) = crate::arch::riscv::cpu::logical_id(hart) {
                if contexts[logical].replace(context_id).is_some() {
                    return None;
                }
            }
        }
        context_id += 1;
    }
    let len = contexts.iter().rposition(Option::is_some)? + 1;
    contexts.truncate(len);
    // Reject incomplete mappings instead of silently directing a CPU at a
    // different hart's context. Firmware entry order has no meaning here.
    Some((
        max_interrupts,
        contexts.into_iter().collect::<Option<Vec<_>>>()?,
    ))
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
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical)
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
