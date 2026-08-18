//! ARM SMMU v2 identity-domain provider.
//!
//! The generic Scarlet IOMMU layer models domains and DMA mappings independently
//! of hardware. This provider supplies the AArch64 ARM SMMU v2 stream-routing
//! machinery required by firmware-described devices. Translated page-table
//! domains are deliberately left for a later extension; the initial provider
//! supports identity DMA without globally disabling the SMMU.

use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use core::arch::asm;

use crate::{
    arch::{self, mmio},
    device::{
        DeviceInfo,
        iommu::{
            IommuController, IommuDomain, IommuDomainConfig, IommuDomainType, IommuError,
            IommuMapFlags, IommuSpec, IommuStreamId, Iova, PhysAddr,
        },
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    driver_initcall, early_println,
    environment::PAGE_SIZE,
    sync::IrqSpinLock,
    vm,
};

const REGISTER_WINDOW_SIZE: usize = 0x10_0000;

const GLOBAL_CONTROL: usize = 0x000;
const ID_REGISTER_0: usize = 0x020;
const ID_REGISTER_1: usize = 0x024;
const GLOBAL_FAULT_STATUS: usize = 0x048;
const STREAM_MATCH_BASE: usize = 0x800;
const STREAM_CONTEXT_BASE: usize = 0xc00;

const ID0_STREAM_GROUP_COUNT: u32 = 0xff;
const ID1_LARGE_REGISTER_PAGE: u32 = 1 << 31;
const ID1_CONTEXT_PAGE_OFFSET: u32 = 0x7 << 28;
const ID1_CONTEXT_BANK_COUNT: u32 = 0xff;

const STREAM_MATCH_VALID: u32 = 1 << 31;
const STREAM_MATCH_MASK_SHIFT: u32 = 16;
const STREAM_MATCH_MASK: u32 = 0x7fff;
const STREAM_MATCH_ID: u32 = 0x7fff;

const STREAM_CONTEXT_TYPE_SHIFT: u32 = 16;
const STREAM_CONTEXT_TYPE_MASK: u32 = 0x3 << STREAM_CONTEXT_TYPE_SHIFT;
const STREAM_CONTEXT_TRANSLATE: u32 = 0;
const STREAM_CONTEXT_BYPASS: u32 = 1 << STREAM_CONTEXT_TYPE_SHIFT;
const STREAM_CONTEXT_BANK: u32 = 0xff;

const CONTEXT_ATTRIBUTE_BASE: usize = 0x000;
const CONTEXT_ATTRIBUTE_2_BASE: usize = 0x800;
const CONTEXT_ATTRIBUTE_TYPE: u32 = 0x3 << 16;
const CONTEXT_ATTRIBUTE_STAGE1: u32 = 1 << 16;
const CONTEXT_ATTRIBUTE_VMID: u32 = 0xff;
const CONTEXT_ATTRIBUTE_UNUSED_VMID: u32 = 0xff;
const CONTEXT_ATTRIBUTE_64BIT: u32 = 1;

const CONTEXT_CONTROL: usize = 0x000;
const CONTEXT_TTBR0: usize = 0x020;
const CONTEXT_TTBR1: usize = 0x028;
const CONTEXT_TCR: usize = 0x030;
const CONTEXT_MAIR0: usize = 0x038;
const CONTEXT_MAIR1: usize = 0x03c;
const CONTEXT_FAULT_STATUS: usize = 0x058;
const CONTEXT_CONTROL_STALL_ON_FAULT: u32 = 1 << 7;
const CONTEXT_CONTROL_FAULT_INTERRUPT: u32 = 1 << 6;
const CONTEXT_CONTROL_FAULT_REPORT: u32 = 1 << 5;

#[derive(Clone, Copy)]
struct RegisterWindow {
    base: usize,
}

impl RegisterWindow {
    const fn new(base: usize) -> Self {
        Self { base }
    }

    fn read(self, offset: usize) -> u32 {
        // SAFETY: the constructor receives an ioremap'd SMMU register window,
        // and all offsets in this driver are bounded by REGISTER_WINDOW_SIZE.
        unsafe { mmio::read32(self.base + offset) }
    }

    fn write(self, offset: usize, value: u32) {
        // SAFETY: see `read`; writes target the same mapped register window.
        unsafe { mmio::write32(self.base + offset, value) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IdentityRouting {
    FirmwarePassthrough,
    DirectBypass,
    DisabledContext,
}

impl IdentityRouting {
    const fn label(self) -> &'static str {
        match self {
            Self::FirmwarePassthrough => "firmware-passthrough",
            Self::DirectBypass => "s2cr-bypass",
            Self::DisabledContext => "disabled-context",
        }
    }
}

struct SmmuHardware {
    registers: RegisterWindow,
    register_page_shift: u32,
    context_page_base: usize,
    stream_group_count: usize,
    context_bank_count: usize,
    identity_routing: IdentityRouting,
    lock: IrqSpinLock<()>,
}

impl SmmuHardware {
    fn page_offset(&self, page: usize, offset: usize) -> usize {
        (page << self.register_page_shift) + offset
    }

    fn global_page_1_read(&self, offset: usize) -> u32 {
        self.registers.read(self.page_offset(1, offset))
    }

    fn global_page_1_write(&self, offset: usize, value: u32) {
        self.registers.write(self.page_offset(1, offset), value)
    }

    fn context_read(&self, context: usize, offset: usize) -> u32 {
        self.registers
            .read(self.page_offset(self.context_page_base + context, offset))
    }

    fn context_write(&self, context: usize, offset: usize, value: u32) {
        self.registers.write(
            self.page_offset(self.context_page_base + context, offset),
            value,
        )
    }

    fn allocate_disabled_context(&self) -> Result<usize, IommuError> {
        let _guard = self.lock.lock();
        for index in 0..self.context_bank_count {
            let attribute_offset = CONTEXT_ATTRIBUTE_BASE + index * 4;
            let inherited = self.global_page_1_read(attribute_offset);
            let context_type = inherited & CONTEXT_ATTRIBUTE_TYPE;
            let vmid = inherited & CONTEXT_ATTRIBUTE_VMID;
            let available = context_type == 0
                || (context_type == CONTEXT_ATTRIBUTE_STAGE1
                    && vmid == CONTEXT_ATTRIBUTE_UNUSED_VMID);
            if !available || self.context_is_routed(index) {
                continue;
            }

            let attribute = (inherited & !(CONTEXT_ATTRIBUTE_TYPE | CONTEXT_ATTRIBUTE_VMID))
                | CONTEXT_ATTRIBUTE_STAGE1;
            self.global_page_1_write(attribute_offset, attribute);

            let attribute_2_offset = CONTEXT_ATTRIBUTE_2_BASE + index * 4;
            let attribute_2 = self.global_page_1_read(attribute_2_offset) | CONTEXT_ATTRIBUTE_64BIT;
            self.global_page_1_write(attribute_2_offset, attribute_2);

            self.context_write(index, CONTEXT_TTBR0, 0);
            self.context_write(index, CONTEXT_TTBR1, 0);
            self.context_write(index, CONTEXT_TCR, 0);
            self.context_write(index, CONTEXT_MAIR0, 0);
            self.context_write(index, CONTEXT_MAIR1, 0);
            self.context_write(
                index,
                CONTEXT_CONTROL,
                CONTEXT_CONTROL_STALL_ON_FAULT
                    | CONTEXT_CONTROL_FAULT_INTERRUPT
                    | CONTEXT_CONTROL_FAULT_REPORT,
            );
            arch::io_wmb();

            early_println!(
                "[arm-smmu-v2] identity context {} ready: cbar={:#010x} sctlr={:#010x}",
                index,
                self.global_page_1_read(attribute_offset),
                self.context_read(index, CONTEXT_CONTROL),
            );
            return Ok(index);
        }
        Err(IommuError::DomainAllocationFailed)
    }

    fn context_is_routed(&self, context: usize) -> bool {
        (0..self.stream_group_count).any(|group| {
            let stream_match = self.registers.read(STREAM_MATCH_BASE + group * 4);
            if stream_match & STREAM_MATCH_VALID == 0 {
                return false;
            }
            let stream_context = self.registers.read(STREAM_CONTEXT_BASE + group * 4);
            stream_context & STREAM_CONTEXT_TYPE_MASK == STREAM_CONTEXT_TRANSLATE
                && stream_context & STREAM_CONTEXT_BANK == context as u32
        })
    }

    fn matching_stream_group(&self, stream_id: u32) -> Option<usize> {
        (0..self.stream_group_count).find(|index| {
            let value = self.registers.read(STREAM_MATCH_BASE + index * 4);
            if value & STREAM_MATCH_VALID == 0 {
                return false;
            }
            let mask = (value >> STREAM_MATCH_MASK_SHIFT) & STREAM_MATCH_MASK;
            let configured_id = value & STREAM_MATCH_ID;
            (stream_id & !mask) == (configured_id & !mask)
        })
    }

    fn unused_stream_group(&self) -> Option<usize> {
        (0..self.stream_group_count).find(|index| {
            self.registers.read(STREAM_MATCH_BASE + index * 4) & STREAM_MATCH_VALID == 0
        })
    }

    fn log_firmware_stream(&self, stream_id: u32) {
        let global_control = self.registers.read(GLOBAL_CONTROL);
        let global_fault = self.registers.read(GLOBAL_FAULT_STATUS);
        let Some(group) = self.matching_stream_group(stream_id) else {
            early_println!(
                "[arm-smmu-v2] SID {:#x} has no firmware SMR: scr0={:#010x} gfsr={:#010x}",
                stream_id,
                global_control,
                global_fault,
            );
            return;
        };

        let stream_match = self.registers.read(STREAM_MATCH_BASE + group * 4);
        let stream_context = self.registers.read(STREAM_CONTEXT_BASE + group * 4);
        if stream_context & STREAM_CONTEXT_TYPE_MASK != STREAM_CONTEXT_TRANSLATE {
            early_println!(
                "[arm-smmu-v2] SID {:#x} firmware route: SMR {} smr={:#010x} s2cr={:#010x} scr0={:#010x} gfsr={:#010x}",
                stream_id,
                group,
                stream_match,
                stream_context,
                global_control,
                global_fault,
            );
            return;
        }

        let context = (stream_context & STREAM_CONTEXT_BANK) as usize;
        if context >= self.context_bank_count {
            early_println!(
                "[arm-smmu-v2] SID {:#x} firmware route has invalid context {}: SMR {} smr={:#010x} s2cr={:#010x}",
                stream_id,
                context,
                group,
                stream_match,
                stream_context,
            );
            return;
        }
        early_println!(
            "[arm-smmu-v2] SID {:#x} firmware route: SMR {} smr={:#010x} s2cr={:#010x} CB {} cbar={:#010x} sctlr={:#010x} fsr={:#010x} scr0={:#010x} gfsr={:#010x}",
            stream_id,
            group,
            stream_match,
            stream_context,
            context,
            self.global_page_1_read(CONTEXT_ATTRIBUTE_BASE + context * 4),
            self.context_read(context, CONTEXT_CONTROL),
            self.context_read(context, CONTEXT_FAULT_STATUS),
            global_control,
            global_fault,
        );
    }

    fn attach_stream(&self, context: Option<usize>, stream_id: u32) -> Result<(), IommuError> {
        if stream_id > STREAM_MATCH_ID {
            return Err(IommuError::InvalidSpec);
        }

        if self.identity_routing == IdentityRouting::FirmwarePassthrough {
            self.log_firmware_stream(stream_id);
            return Ok(());
        }

        let _guard = self.lock.lock();
        let existing_group = self.matching_stream_group(stream_id);
        let group = existing_group
            .or_else(|| self.unused_stream_group())
            .ok_or(IommuError::AttachFailed)?;
        if existing_group.is_none() {
            self.registers.write(
                STREAM_MATCH_BASE + group * 4,
                STREAM_MATCH_VALID | stream_id,
            );
        }

        let route = match (self.identity_routing, context) {
            (IdentityRouting::FirmwarePassthrough, None) => return Ok(()),
            (IdentityRouting::DirectBypass, None) => STREAM_CONTEXT_BYPASS,
            (IdentityRouting::DisabledContext, Some(context)) => {
                STREAM_CONTEXT_TRANSLATE | (context as u32 & STREAM_CONTEXT_BANK)
            }
            _ => return Err(IommuError::AttachFailed),
        };
        self.registers.write(
            STREAM_CONTEXT_BASE + group * 4,
            route & (STREAM_CONTEXT_TYPE_MASK | STREAM_CONTEXT_BANK),
        );
        arch::io_wmb();

        early_println!(
            "[arm-smmu-v2] SID {:#x} routed by SMR {}: smr={:#010x} s2cr={:#010x}",
            stream_id,
            group,
            self.registers.read(STREAM_MATCH_BASE + group * 4),
            self.registers.read(STREAM_CONTEXT_BASE + group * 4),
        );
        Ok(())
    }
}

struct IdentityDomain {
    hardware: Arc<SmmuHardware>,
    context: Option<usize>,
}

impl IommuDomain for IdentityDomain {
    fn attach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
        self.hardware.attach_stream(self.context, stream.id)
    }

    fn detach_stream(&self, _stream: IommuStreamId) -> Result<(), IommuError> {
        Ok(())
    }

    fn map(
        &self,
        iova: Iova,
        paddr: PhysAddr,
        _len: usize,
        _flags: IommuMapFlags,
    ) -> Result<(), IommuError> {
        if iova != paddr as Iova {
            return Err(IommuError::MapFailed);
        }
        Ok(())
    }

    fn unmap(&self, _iova: Iova, _len: usize) -> Result<(), IommuError> {
        Ok(())
    }

    fn iova_to_phys(&self, iova: Iova) -> Option<PhysAddr> {
        usize::try_from(iova).ok()
    }

    fn page_size(&self) -> usize {
        PAGE_SIZE
    }

    fn flush(&self) -> Result<(), IommuError> {
        arch::io_wmb();
        let context_fault = self
            .context
            .map(|context| self.hardware.context_read(context, CONTEXT_FAULT_STATUS))
            .unwrap_or(0);
        early_println!(
            "[arm-smmu-v2] identity domain synchronized: gfsr={:#010x} fsr={:#010x}",
            self.hardware.registers.read(GLOBAL_FAULT_STATUS),
            context_fault,
        );
        Ok(())
    }
}

struct ArmSmmuV2 {
    hardware: Arc<SmmuHardware>,
}

impl IommuController for ArmSmmuV2 {
    fn name(&self) -> &'static str {
        "arm-smmu-v2"
    }

    fn alloc_domain(&self, config: IommuDomainConfig) -> Result<Arc<dyn IommuDomain>, IommuError> {
        if config.domain_type != IommuDomainType::Identity {
            return Err(IommuError::NotSupported);
        }
        let context = match self.hardware.identity_routing {
            IdentityRouting::FirmwarePassthrough | IdentityRouting::DirectBypass => None,
            IdentityRouting::DisabledContext => Some(self.hardware.allocate_disabled_context()?),
        };
        Ok(Arc::new(IdentityDomain {
            hardware: Arc::clone(&self.hardware),
            context,
        }))
    }

    fn stream_ids_from_fdt(&self, spec: &IommuSpec) -> Result<Vec<IommuStreamId>, IommuError> {
        let (base, mask) = match spec.cells.as_slice() {
            [base] => (*base, 0),
            [base, mask] => (*base, *mask),
            _ => return Err(IommuError::InvalidSpec),
        };
        expand_stream_mask(base, mask)
    }
}

fn expand_stream_mask(base: u32, mask: u32) -> Result<Vec<IommuStreamId>, IommuError> {
    if base > STREAM_MATCH_ID || mask > STREAM_MATCH_MASK {
        return Err(IommuError::InvalidSpec);
    }
    let variable_bit_count = mask.count_ones();
    if variable_bit_count > 8 {
        return Err(IommuError::InvalidSpec);
    }

    let variable_bits: Vec<u32> = (0..15).filter(|bit| mask & (1 << bit) != 0).collect();
    let mut streams = Vec::new();
    for combination in 0..(1u32 << variable_bit_count) {
        let mut id = base & !mask;
        for (combination_bit, stream_bit) in variable_bits.iter().enumerate() {
            if combination & (1 << combination_bit) != 0 {
                id |= 1 << stream_bit;
            }
        }
        streams.push(IommuStreamId {
            id,
            substream_id: None,
        });
    }
    Ok(streams)
}

fn read_phandle(device: &PlatformDeviceInfo) -> Result<u32, &'static str> {
    device
        .property("phandle")
        .or_else(|| device.property("linux,phandle"))
        .and_then(|property| property.as_usize())
        .and_then(|value| u32::try_from(value).ok())
        .ok_or("arm-smmu-v2: missing phandle")
}

fn current_exception_level() -> u64 {
    let current_el: u64;
    // SAFETY: CurrentEL is a read-only architectural status register available
    // at every AArch64 exception level.
    unsafe { asm!("mrs {value}, CurrentEL", value = out(reg) current_el, options(nomem, nostack)) };
    (current_el >> 2) & 0x3
}

fn probe(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let resource = device
        .get_resources()
        .iter()
        .find(|resource| resource.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("arm-smmu-v2: missing register resource")?;
    let resource_size = resource
        .end
        .checked_sub(resource.start)
        .and_then(|size| size.checked_add(1))
        .ok_or("arm-smmu-v2: invalid register resource")?;
    if resource_size < REGISTER_WINDOW_SIZE {
        return Err("arm-smmu-v2: register resource is too small");
    }

    let base = vm::ioremap(resource.start, REGISTER_WINDOW_SIZE)
        .map_err(|_| "arm-smmu-v2: ioremap failed")?;
    let registers = RegisterWindow::new(base);
    let id0 = registers.read(ID_REGISTER_0);
    let id1 = registers.read(ID_REGISTER_1);
    let register_page_shift = if id1 & ID1_LARGE_REGISTER_PAGE != 0 {
        16
    } else {
        12
    };
    let context_page_base = 1usize << (((id1 & ID1_CONTEXT_PAGE_OFFSET) >> 28) as usize + 1);
    let stream_group_count = (id0 & ID0_STREAM_GROUP_COUNT) as usize;
    let context_bank_count = (id1 & ID1_CONTEXT_BANK_COUNT) as usize;
    if stream_group_count == 0 || context_bank_count == 0 {
        return Err("arm-smmu-v2: invalid hardware capabilities");
    }

    let is_sc7180 = device.compatible().contains(&"qcom,sc7180-smmu-500");
    let identity_routing = if is_sc7180 && current_exception_level() >= 2 {
        // At EL2 the SC7180 alternate-firmware path already owns the physical
        // address space. Preserve its SMMU passthrough instead of touching
        // hypervisor-guarded stream routing registers.
        IdentityRouting::FirmwarePassthrough
    } else if is_sc7180 {
        // Some Qualcomm firmware environments turn a direct BYPASS S2CR write
        // into FAULT. Route through a stage-1 context with SCTLR.M left clear,
        // which preserves physical addresses without requesting BYPASS.
        IdentityRouting::DisabledContext
    } else {
        IdentityRouting::DirectBypass
    };

    let hardware = Arc::new(SmmuHardware {
        registers,
        register_page_shift,
        context_page_base,
        stream_group_count,
        context_bank_count,
        identity_routing,
        lock: IrqSpinLock::new(()),
    });
    let controller = Arc::new(ArmSmmuV2 { hardware });
    let phandle = read_phandle(device)?;
    DeviceManager::get_manager()
        .register_iommu_controller(phandle, controller as Arc<dyn IommuController>);

    early_println!(
        "[arm-smmu-v2] registered phandle={:#x} paddr={:#x} page={} SMRs={} CBs={} identity={} gfsr={:#010x}",
        phandle,
        resource.start,
        1usize << register_page_shift,
        stream_group_count,
        context_bank_count,
        identity_routing.label(),
        registers.read(GLOBAL_FAULT_STATUS),
    );
    Ok(())
}

fn remove(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "arm-smmu-v2",
        probe,
        remove,
        vec!["arm,mmu-500", "qcom,sc7180-smmu-500"],
    );
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical);
}

driver_initcall!(register_driver);

#[cfg(test)]
mod tests {
    use super::expand_stream_mask;

    #[test_case]
    fn expands_firmware_stream_mask() {
        let streams = expand_stream_mask(0x800, 0x2).expect("stream mask should decode");
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].id, 0x800);
        assert_eq!(streams[1].id, 0x802);
    }
}
