//! RISC-V Platform-Level Interrupt Controller (PLIC) Implementation
//!
//! The PLIC is responsible for managing external interrupts from devices and
//! routing them to different CPUs with priority support.

use crate::{
    device::{
        fdt::FdtManager,
        manager::{DeviceManager, DriverPriority},
        platform::{
            resource::PlatformDeviceResourceType, PlatformDeviceDriver, PlatformDeviceInfo,
        },
        DeviceInfo,
    },
    early_initcall,
    interrupt::{
        controllers::ExternalInterruptController, CpuId, InterruptError, InterruptId,
        InterruptManager, InterruptResult, Priority,
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
    fn init(&mut self) -> InterruptResult<()> {
        crate::early_println!(
            "[PLIC] init: max_cpus={}, max_interrupts={}, s_mode_contexts={:?}",
            self.max_cpus,
            self.max_interrupts,
            self.s_mode_contexts
        );

        // Establish a known baseline:
        // - Disable all interrupts for all contexts first.
        //   This prevents any firmware/previous-stage configuration from leaking into the kernel.
        //   Device drivers will later enable only what they need.
        let word_count = ((self.max_interrupts as usize) + 31) / 32;
        for cpu_id in 0..self.max_cpus {
            let context_id = self.context_id_for_cpu(cpu_id);
            let context_offset = context_id * PLIC_ENABLE_CONTEXT_STRIDE;
            for word in 0..word_count {
                let addr = self.base_addr + PLIC_ENABLE_BASE + context_offset + (word * 4);
                let verify = Self::mmio_write32_with_readback(addr, 0);
                if verify != 0 {
                    crate::early_println!(
                        "PLIC init: clear enable verify failed: cpu={}, context={}, addr={:#x}, read={}",
                        cpu_id,
                        context_id,
                        addr,
                        verify
                    );
                    return Err(InterruptError::HardwareError);
                }
            }
        }

        // Set threshold to 0 for all CPUs (allow all priorities)
        for cpu_id in 0..self.max_cpus {
            self.set_threshold(cpu_id, 0)?;
        }

        // Set all interrupt priorities to 1 (lowest non-zero priority)
        for interrupt_id in 1..=self.max_interrupts {
            self.set_priority(interrupt_id, 1)?;
        }

        Ok(())
    }

    /// Enable a specific interrupt for a CPU
    fn enable_interrupt(
        &mut self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
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
                crate::early_println!(
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
    fn disable_interrupt(
        &mut self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
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
            crate::early_println!(
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
            crate::early_println!(
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
    fn complete_interrupt(
        &mut self,
        cpu_id: CpuId,
        interrupt_id: InterruptId,
    ) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        self.validate_interrupt_id(interrupt_id)?;

        let addr = self.claim_addr(cpu_id);
        unsafe {
            write_volatile(addr as *mut u32, interrupt_id);
            crate::arch::mmio_fence();
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
    let mem_res = res
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("Memory resource not found")?;

    let base_addr = mem_res.start as usize;

    // Try to get PLIC configuration from FDT for proper context mapping
    let controller =
        if let Some((max_interrupts, s_mode_contexts)) = get_plic_config_from_fdt(device.name()) {
            crate::early_println!(
                "[interrupt] PLIC: FDT config found - ndev={}, contexts={:?}",
                max_interrupts,
                s_mode_contexts
            );
            Box::new(Plic::with_contexts(
                base_addr,
                max_interrupts,
                s_mode_contexts,
            ))
        } else {
            // Fallback to hardcoded values (TCG-style: M+S per hart)
            crate::early_println!(
                "[interrupt] PLIC: Using default config (1023 interrupts, 4 contexts)"
            );
            Box::new(Plic::new(base_addr, 1023, 4))
        };

    match InterruptManager::global()
        .lock()
        .register_external_controller(controller)
    {
        Ok(_) => {
            crate::early_println!(
                "[interrupt] PLIC registered at base address: {:#x}",
                base_addr
            );
        }
        Err(e) => {
            crate::early_println!("[interrupt] Failed to register PLIC: {}", e);
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
    let fdt_manager = FdtManager::get_manager();
    let fdt = fdt_manager.get_fdt()?;

    fn read_be_u32(bytes: &[u8]) -> Option<u32> {
        if bytes.len() < 4 {
            return None;
        }
        Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn get_u32_prop<'a, 'b>(node: &fdt::node::FdtNode<'a, 'b>, name: &str) -> Option<u32> {
        let prop = node.property(name)?;
        read_be_u32(prop.value)
    }

    fn find_node_by_phandle<'a>(
        fdt: &'a fdt::Fdt<'a>,
        phandle: u32,
    ) -> Option<fdt::node::FdtNode<'a, 'a>> {
        let mut stack: alloc::vec::Vec<fdt::node::FdtNode<'a, 'a>> = alloc::vec::Vec::new();
        stack.push(fdt.find_node("/")?);

        while let Some(node) = stack.pop() {
            if let Some(p) = get_u32_prop(&node, "phandle") {
                if p == phandle {
                    return Some(node);
                }
            }
            for child in node.children() {
                stack.push(child);
            }
        }

        None
    }

    // Find the PLIC node in /soc
    let soc = fdt.find_node("/soc")?;
    let plic_node = soc.children().find(|node| node.name == device_name)?;

    // Read riscv,ndev property for max interrupt count
    let max_interrupts = plic_node
        .property("riscv,ndev")
        .and_then(|prop| {
            if prop.value.len() >= 4 {
                Some(u32::from_be_bytes([
                    prop.value[0],
                    prop.value[1],
                    prop.value[2],
                    prop.value[3],
                ]))
            } else {
                None
            }
        })
        .unwrap_or(1023);

    // Read interrupts-extended property to find S-mode contexts.
    // The entry size depends on the referenced interrupt-controller node's
    // #interrupt-cells, so we must decode it dynamically.
    //
    // Typical RISC-V CPU interrupt controller uses #interrupt-cells = <1>
    // and provides irq_type values:
    // - 9  = Supervisor External Interrupt (SEI)
    // - 11 = Machine External Interrupt (MEI)
    let s_mode_contexts = plic_node
        .property("interrupts-extended")
        .map(|prop| {
            let mut contexts = Vec::new();
            let mut offset = 0usize;
            let mut context_id = 0usize;
            let bytes = prop.value;

            while offset + 4 <= bytes.len() {
                let phandle = match read_be_u32(&bytes[offset..offset + 4]) {
                    Some(v) => v,
                    None => break,
                };
                offset += 4;

                let intc_node = find_node_by_phandle(fdt, phandle);
                let interrupt_cells = intc_node
                    .as_ref()
                    .and_then(|n| get_u32_prop(n, "#interrupt-cells"))
                    .unwrap_or(1) as usize;

                if interrupt_cells == 0 {
                    break;
                }
                let needed = interrupt_cells.saturating_mul(4);
                if offset + needed > bytes.len() {
                    break;
                }

                // Interpret the first interrupt cell as the irq_type.
                let irq_type = read_be_u32(&bytes[offset..offset + 4]).unwrap_or(0);
                if irq_type == 9 {
                    contexts.push(context_id);
                }

                offset += needed;
                context_id += 1;
            }

            // Backward-compatible fallback: if decoding failed (e.g. phandle lookup),
            // fall back to fixed 2-cell entries.
            if contexts.is_empty() {
                for (idx, chunk) in bytes.chunks_exact(8).enumerate() {
                    let irq_type = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                    if irq_type == 9 {
                        contexts.push(idx);
                    }
                }
            }

            contexts
        })
        .unwrap_or_else(Vec::new);

    if s_mode_contexts.is_empty() {
        return None;
    }

    Some((max_interrupts, s_mode_contexts))
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
