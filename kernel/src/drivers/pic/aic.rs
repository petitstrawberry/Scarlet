//! Apple Interrupt Controller (AIC) implementation (AArch64)
//!
//! This driver supports the Apple interrupt controller as documented by Asahi
//! Linux and modeled after the upstream Linux `irq-apple-aic` driver. Scarlet
//! currently implements the wired hardware IRQ path needed for early bring-up
//! and device interrupts. Timer FIQ/IPI support remains handled elsewhere.

use crate::{
    arch::mmio,
    device::{
        DeviceInfo,
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo,
            resource::{IrqMetadata, PlatformDeviceResourceType},
        },
    },
    early_initcall,
    environment::MAX_NUM_CPUS,
    interrupt::{
        CpuId, InterruptError, InterruptId, InterruptManager, InterruptResult, Priority,
        controllers::ExternalInterruptController,
    },
};

use alloc::{boxed::Box, vec, vec::Vec};

/// AIC v1 information register.
const AIC_INFO: usize = 0x0004;
const AIC_INFO_NR_IRQ_MASK: u32 = 0x0000_FFFF;

/// AIC v1 event register offsets.
const AIC_EVENT: usize = 0x2004;
const AIC_TARGET_CPU: usize = 0x3000;
const AIC_MAX_IRQ: InterruptId = 0x400;

/// AIC v2 information/configuration register offsets.
const AIC2_INFO1: usize = 0x0004;
const AIC2_INFO3: usize = 0x000C;
const AIC2_CONFIG: usize = 0x0014;
const AIC2_CONFIG_ENABLE: u32 = 1 << 0;
const AIC2_IRQ_CFG: usize = 0x2000;
const AIC2_IRQ_CFG_TARGET_MASK: u32 = 0x0000_000F;

/// Common AIC event encoding.
const AIC_EVENT_DIE_SHIFT: u32 = 24;
const AIC_EVENT_DIE_MASK: u32 = 0xFF;
const AIC_EVENT_TYPE_SHIFT: u32 = 16;
const AIC_EVENT_TYPE_MASK: u32 = 0xFF;
const AIC_EVENT_NUM_MASK: u32 = 0xFFFF;
const AIC_EVENT_TYPE_IRQ: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AicVersion {
    V1,
    V2,
}

/// Apple interrupt controller implementation for the hardware IRQ path.
pub struct Aic {
    base_addr: usize,
    event_base_addr: usize,
    event_offset: usize,
    version: AicVersion,
    target_cpu_offset: Option<usize>,
    irq_cfg_offset: Option<usize>,
    sw_clr_offset: usize,
    mask_set_offset: usize,
    mask_clr_offset: usize,
    hw_state_offset: usize,
    die_stride: usize,
    num_irqs_per_die: InterruptId,
    max_irq_per_die: InterruptId,
    num_dies: u32,
    max_cpus: CpuId,
    enabled: Vec<u32>,
}

impl Aic {
    #[allow(clippy::too_many_arguments)]
    fn new(
        base_addr: usize,
        event_base_addr: usize,
        event_offset: usize,
        version: AicVersion,
        target_cpu_offset: Option<usize>,
        irq_cfg_offset: Option<usize>,
        sw_clr_offset: usize,
        mask_set_offset: usize,
        mask_clr_offset: usize,
        hw_state_offset: usize,
        die_stride: usize,
        num_irqs_per_die: InterruptId,
        max_irq_per_die: InterruptId,
        num_dies: u32,
        max_cpus: CpuId,
    ) -> Self {
        let words_per_die = ((max_irq_per_die as usize) + 31) / 32;
        Self {
            base_addr,
            event_base_addr,
            event_offset,
            version,
            target_cpu_offset,
            irq_cfg_offset,
            sw_clr_offset,
            mask_set_offset,
            mask_clr_offset,
            hw_state_offset,
            die_stride,
            num_irqs_per_die,
            max_irq_per_die,
            num_dies,
            max_cpus,
            enabled: vec![0; words_per_die * num_dies as usize],
        }
    }

    /// Translate Apple AIC device-tree IRQ metadata to a hardware IRQ number.
    ///
    /// AIC uses a GIC-like 3-cell format, but hardware IRQs keep their raw
    /// numbers instead of adding a controller-specific base offset.
    fn translate_irq_metadata(metadata: &IrqMetadata) -> Option<InterruptId> {
        if metadata.irq_type == 0 {
            Some(metadata.irq_number)
        } else {
            None
        }
    }

    fn words_per_die(&self) -> usize {
        ((self.max_irq_per_die as usize) + 31) / 32
    }

    fn total_interrupt_slots(&self) -> InterruptId {
        self.max_irq_per_die
            .saturating_mul(self.num_dies as InterruptId)
    }

    fn validate_cpu_id(&self, cpu_id: CpuId) -> InterruptResult<()> {
        if cpu_id >= self.max_cpus {
            Err(InterruptError::InvalidCpuId)
        } else {
            Ok(())
        }
    }

    fn interrupt_location(&self, interrupt_id: InterruptId) -> InterruptResult<(u32, InterruptId)> {
        if self.max_irq_per_die == 0 {
            return Err(InterruptError::InvalidInterruptId);
        }

        let die = interrupt_id / self.max_irq_per_die;
        let local_irq = interrupt_id % self.max_irq_per_die;
        if die >= self.num_dies || local_irq >= self.num_irqs_per_die {
            Err(InterruptError::InvalidInterruptId)
        } else {
            Ok((die, local_irq))
        }
    }

    fn reg_addr(&self, offset: usize) -> usize {
        self.base_addr + offset
    }

    fn die_reg_addr(&self, die: u32, offset: usize) -> usize {
        self.base_addr + offset + die as usize * self.die_stride
    }

    fn event_reg_addr(&self) -> usize {
        self.event_base_addr + self.event_offset
    }

    fn enabled_index(&self, die: u32, local_irq: InterruptId) -> (usize, u32) {
        let word = die as usize * self.words_per_die() + (local_irq as usize / 32);
        let bit = 1u32 << (local_irq % 32);
        (word, bit)
    }

    fn is_enabled(&self, die: u32, local_irq: InterruptId) -> bool {
        let (word, bit) = self.enabled_index(die, local_irq);
        (self.enabled[word] & bit) != 0
    }

    fn set_enabled(&mut self, die: u32, local_irq: InterruptId, enabled: bool) {
        let (word, bit) = self.enabled_index(die, local_irq);
        if enabled {
            self.enabled[word] |= bit;
        } else {
            self.enabled[word] &= !bit;
        }
    }

    fn set_target_cpu(&self, die: u32, local_irq: InterruptId, cpu_id: CpuId) {
        unsafe {
            match self.version {
                AicVersion::V1 => {
                    if let Some(offset) = self.target_cpu_offset {
                        mmio::write32(
                            self.die_reg_addr(die, offset + local_irq as usize * 4),
                            1u32 << cpu_id,
                        );
                    }
                }
                AicVersion::V2 => {
                    if let Some(offset) = self.irq_cfg_offset {
                        let addr = self.die_reg_addr(die, offset + local_irq as usize * 4);
                        let current = mmio::read32(addr);
                        let next = (current & !AIC2_IRQ_CFG_TARGET_MASK)
                            | ((cpu_id as u32) & AIC2_IRQ_CFG_TARGET_MASK);
                        mmio::write32(addr, next);
                    }
                }
            }
        }
    }

    fn unmask_interrupt(&self, die: u32, local_irq: InterruptId) {
        unsafe {
            mmio::write32(
                self.die_reg_addr(die, self.mask_clr_offset + (local_irq as usize / 32) * 4),
                1u32 << (local_irq % 32),
            );
        }
    }

    fn mask_interrupt(&self, die: u32, local_irq: InterruptId) {
        unsafe {
            mmio::write32(
                self.die_reg_addr(die, self.mask_set_offset + (local_irq as usize / 32) * 4),
                1u32 << (local_irq % 32),
            );
        }
    }

    fn decode_event(&self, event: u32) -> Option<InterruptId> {
        if event == 0 {
            return None;
        }

        let event_type = (event >> AIC_EVENT_TYPE_SHIFT) & AIC_EVENT_TYPE_MASK;
        if event_type != AIC_EVENT_TYPE_IRQ {
            return None;
        }

        let die = (event >> AIC_EVENT_DIE_SHIFT) & AIC_EVENT_DIE_MASK;
        let local_irq = event & AIC_EVENT_NUM_MASK;
        if die >= self.num_dies || local_irq >= self.num_irqs_per_die {
            return None;
        }

        die.checked_mul(self.max_irq_per_die)
            .and_then(|base| base.checked_add(local_irq))
    }

    fn init_masks(&mut self) {
        let words_per_die = self.words_per_die();
        for die in 0..self.num_dies {
            for word in 0..words_per_die {
                unsafe {
                    mmio::write32(
                        self.die_reg_addr(die, self.mask_set_offset + word * 4),
                        u32::MAX,
                    );
                    mmio::write32(
                        self.die_reg_addr(die, self.sw_clr_offset + word * 4),
                        u32::MAX,
                    );
                }
            }

            if self.version == AicVersion::V1 {
                for irq in 0..self.num_irqs_per_die {
                    self.set_target_cpu(die, irq, 0);
                }
            }
        }

        if self.version == AicVersion::V2 {
            unsafe {
                let config_addr = self.reg_addr(AIC2_CONFIG);
                let config = mmio::read32(config_addr);
                mmio::write32(config_addr, config | AIC2_CONFIG_ENABLE);
            }
        }
    }
}

impl ExternalInterruptController for Aic {
    fn init(&mut self) -> InterruptResult<()> {
        self.init_masks();
        Ok(())
    }

    fn enable_interrupt(
        &mut self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        let (die, local_irq) = self.interrupt_location(interrupt_id)?;

        self.set_target_cpu(die, local_irq, cpu_id);
        self.set_enabled(die, local_irq, true);
        self.unmask_interrupt(die, local_irq);
        Ok(())
    }

    fn disable_interrupt(
        &mut self,
        interrupt_id: InterruptId,
        _cpu_id: CpuId,
    ) -> InterruptResult<()> {
        let (die, local_irq) = self.interrupt_location(interrupt_id)?;
        self.set_enabled(die, local_irq, false);
        self.mask_interrupt(die, local_irq);
        Ok(())
    }

    fn set_priority(
        &mut self,
        interrupt_id: InterruptId,
        _priority: Priority,
    ) -> InterruptResult<()> {
        let _ = self.interrupt_location(interrupt_id)?;
        Ok(())
    }

    fn translate_interrupt(
        &self,
        interrupt_id: InterruptId,
        metadata: Option<&IrqMetadata>,
    ) -> InterruptResult<InterruptId> {
        match metadata {
            Some(metadata) => {
                Self::translate_irq_metadata(metadata).ok_or(InterruptError::NotSupported)
            }
            None => Ok(interrupt_id),
        }
    }

    fn get_priority(&self, interrupt_id: InterruptId) -> InterruptResult<Priority> {
        let _ = self.interrupt_location(interrupt_id)?;
        Ok(0)
    }

    fn set_threshold(&mut self, cpu_id: CpuId, _threshold: Priority) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        Ok(())
    }

    fn get_threshold(&self, cpu_id: CpuId) -> InterruptResult<Priority> {
        self.validate_cpu_id(cpu_id)?;
        Ok(0)
    }

    fn claim_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<Option<InterruptId>> {
        self.validate_cpu_id(cpu_id)?;

        loop {
            let event = unsafe { mmio::read32(self.event_reg_addr()) };
            if event == 0 {
                return Ok(None);
            }

            if let Some(interrupt_id) = self.decode_event(event) {
                return Ok(Some(interrupt_id));
            }
        }
    }

    fn complete_interrupt(
        &mut self,
        cpu_id: CpuId,
        interrupt_id: InterruptId,
    ) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        let (die, local_irq) = self.interrupt_location(interrupt_id)?;

        if self.is_enabled(die, local_irq) {
            self.unmask_interrupt(die, local_irq);
        }

        Ok(())
    }

    fn is_pending(&self, interrupt_id: InterruptId) -> bool {
        let Ok((die, local_irq)) = self.interrupt_location(interrupt_id) else {
            return false;
        };

        let pending_addr =
            self.die_reg_addr(die, self.hw_state_offset + (local_irq as usize / 32) * 4);
        let pending = unsafe { mmio::read32(pending_addr) };
        (pending & (1u32 << (local_irq % 32))) != 0
    }

    fn max_interrupts(&self) -> InterruptId {
        self.total_interrupt_slots().saturating_sub(1)
    }

    fn max_cpus(&self) -> CpuId {
        self.max_cpus
    }
}

unsafe impl Send for Aic {}
unsafe impl Sync for Aic {}

fn detect_version(device: &PlatformDeviceInfo) -> AicVersion {
    let compatible = device.compatible();
    if compatible.contains(&"apple,aic2") {
        AicVersion::V2
    } else {
        AicVersion::V1
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resources: Vec<_> = device
        .get_resources()
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    let base_resource = mem_resources
        .first()
        .ok_or("No memory resource found for Apple AIC")?;
    let base_paddr = base_resource.start;
    let base_size = base_resource.end - base_resource.start + 1;
    let base_addr = crate::vm::ioremap(base_paddr, base_size).map_err(|e| {
        crate::early_println!(
            "[interrupt] AIC base ioremap({:#x}, {:#x}) failed: {}",
            base_paddr,
            base_size,
            e
        );
        e
    })?;

    let version = detect_version(device);
    let (event_base_addr, event_offset) = match version {
        AicVersion::V1 => (base_addr, AIC_EVENT),
        AicVersion::V2 => {
            let event_resource = match mem_resources.get(1) {
                Some(resource) => *resource,
                None => {
                    crate::vm::iounmap(base_addr);
                    return Err("No event memory resource found for Apple AIC2");
                }
            };
            let event_paddr = event_resource.start;
            let event_size = event_resource.end - event_resource.start + 1;
            let event_base_addr = crate::vm::ioremap(event_paddr, event_size).map_err(|e| {
                crate::early_println!(
                    "[interrupt] AIC event ioremap({:#x}, {:#x}) failed: {}",
                    event_paddr,
                    event_size,
                    e
                );
                crate::vm::iounmap(base_addr);
                e
            })?;
            (event_base_addr, 0)
        }
    };

    let (
        target_cpu_offset,
        irq_cfg_offset,
        sw_clr_offset,
        mask_set_offset,
        mask_clr_offset,
        hw_state_offset,
        die_stride,
        num_irqs_per_die,
        max_irq_per_die,
        num_dies,
    ) = unsafe {
        match version {
            AicVersion::V1 => {
                let info = mmio::read32(base_addr + AIC_INFO);
                let num_irqs_per_die = info & AIC_INFO_NR_IRQ_MASK;
                let max_irq_per_die = AIC_MAX_IRQ;
                let words_per_die = ((max_irq_per_die as usize) + 31) / 32;
                let off = AIC_TARGET_CPU + max_irq_per_die as usize * 4;
                let sw_clr_offset = off + words_per_die * 4;
                let mask_set_offset = sw_clr_offset + words_per_die * 4;
                let mask_clr_offset = mask_set_offset + words_per_die * 4;
                let hw_state_offset = mask_clr_offset + words_per_die * 4;
                let die_stride = hw_state_offset + words_per_die * 4 - AIC_TARGET_CPU;
                (
                    Some(AIC_TARGET_CPU),
                    None,
                    sw_clr_offset,
                    mask_set_offset,
                    mask_clr_offset,
                    hw_state_offset,
                    die_stride,
                    num_irqs_per_die,
                    max_irq_per_die,
                    1,
                )
            }
            AicVersion::V2 => {
                let info1 = mmio::read32(base_addr + AIC2_INFO1);
                let info3 = mmio::read32(base_addr + AIC2_INFO3);
                let num_irqs_per_die = info1 & AIC_INFO_NR_IRQ_MASK;
                let max_irq_per_die = info3 & AIC_INFO_NR_IRQ_MASK;
                let num_dies = ((info1 >> 24) & 0xF) + 1;
                let words_per_die = ((max_irq_per_die as usize) + 31) / 32;
                let off = AIC2_IRQ_CFG + max_irq_per_die as usize * 4;
                let sw_clr_offset = off + words_per_die * 4;
                let mask_set_offset = sw_clr_offset + words_per_die * 4;
                let mask_clr_offset = mask_set_offset + words_per_die * 4;
                let hw_state_offset = mask_clr_offset + words_per_die * 4;
                let die_stride = hw_state_offset + words_per_die * 4 - AIC2_IRQ_CFG;
                (
                    None,
                    Some(AIC2_IRQ_CFG),
                    sw_clr_offset,
                    mask_set_offset,
                    mask_clr_offset,
                    hw_state_offset,
                    die_stride,
                    num_irqs_per_die,
                    max_irq_per_die,
                    num_dies,
                )
            }
        }
    };

    let aic = Box::new(Aic::new(
        base_addr,
        event_base_addr,
        event_offset,
        version,
        target_cpu_offset,
        irq_cfg_offset,
        sw_clr_offset,
        mask_set_offset,
        mask_clr_offset,
        hw_state_offset,
        die_stride,
        num_irqs_per_die,
        max_irq_per_die,
        num_dies,
        MAX_NUM_CPUS as CpuId,
    ));

    crate::early_println!(
        "[interrupt] Apple AIC selected: version={:?} base={:#x} event={:#x} irqs/die={} dies={}",
        version,
        base_addr,
        event_base_addr,
        num_irqs_per_die,
        num_dies
    );

    InterruptManager::with_manager(|manager| {
        manager.register_external_controller(aic).map_err(|e| {
            crate::early_println!("[interrupt] Failed to register Apple AIC: {}", e);
            "Failed to register Apple AIC"
        })?;
        Ok(())
    })?;
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple,aic",
        probe_fn,
        remove_fn,
        vec![
            "apple,aic",
            "apple,aic2",
            "apple,s5l8960x-aic",
            "apple,t7000-aic",
            "apple,s8000-aic",
            "apple,t8010-aic",
            "apple,t8015-aic",
            "apple,t8103-aic",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical)
}

early_initcall!(register_driver);

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_translate_hardware_irq_metadata() {
        let metadata = IrqMetadata {
            irq_type: 0,
            irq_number: 42,
            irq_flags: 4,
        };

        assert_eq!(Aic::translate_irq_metadata(&metadata), Some(42));
    }

    #[test_case]
    fn test_translate_fiq_metadata_returns_none() {
        let metadata = IrqMetadata {
            irq_type: 1,
            irq_number: 3,
            irq_flags: 4,
        };

        assert_eq!(Aic::translate_irq_metadata(&metadata), None);
    }

    #[test_case]
    fn test_decode_irq_event() {
        let controller = Aic::new(
            0,
            0,
            0,
            AicVersion::V1,
            Some(AIC_TARGET_CPU),
            None,
            0,
            0,
            0,
            0,
            0,
            128,
            1024,
            1,
            1,
        );

        let event = (AIC_EVENT_TYPE_IRQ << AIC_EVENT_TYPE_SHIFT) | 23;
        assert_eq!(controller.decode_event(event), Some(23));
    }

    #[test_case]
    fn test_decode_non_irq_event_is_ignored() {
        let controller = Aic::new(
            0,
            0,
            0,
            AicVersion::V1,
            Some(AIC_TARGET_CPU),
            None,
            0,
            0,
            0,
            0,
            0,
            128,
            1024,
            1,
            1,
        );

        let event = (4 << AIC_EVENT_TYPE_SHIFT) | 7;
        assert_eq!(controller.decode_event(event), None);
    }
}
