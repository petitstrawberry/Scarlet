//! ARM SMMU v2 domain provider.
//!
//! The generic Scarlet IOMMU layer models domains and DMA mappings independently
//! of hardware. This provider supplies the AArch64 ARM SMMU v2 stream-routing
//! machinery required by firmware-described devices. It supports both the
//! firmware-compatible identity path and AArch64 stage-1 translated DMA.

use alloc::{boxed::Box, collections::BTreeMap, sync::Arc, vec, vec::Vec};
use core::arch::asm;

use crate::{
    arch::{self, mmio},
    device::{
        DeviceInfo,
        clk::ClkHandle,
        iommu::{
            IommuController, IommuDomain, IommuDomainConfig, IommuDomainType, IommuError,
            IommuMapFlags, IommuSpec, IommuStreamId, Iova, PhysAddr,
        },
        manager::{DeviceManager, DriverPriority, probe_defer},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    driver_initcall, early_println,
    environment::PAGE_SIZE,
    mem::page::{Page, allocate_raw_pages, free_raw_pages},
    sync::IrqSpinLock,
    vm::{
        self,
        addr::{phys_to_virt, virt_to_phys},
    },
};

const MINIMUM_REGISTER_WINDOW_SIZE: usize = 0x1000;

const GLOBAL_CONTROL: usize = 0x000;
const GLOBAL_CONTROL_CLIENT_POWER_DOWN: u32 = 1;
const GLOBAL_CONTROL_UNMATCHED_STREAM_FAULT: u32 = 1 << 10;
const ID_REGISTER_0: usize = 0x020;
const ID_REGISTER_1: usize = 0x024;
const ID_REGISTER_2: usize = 0x028;
const GLOBAL_FAULT_STATUS: usize = 0x048;
const GLOBAL_TLB_INVALIDATE_ALL_NONSECURE: usize = 0x068;
const GLOBAL_TLB_SYNC: usize = 0x070;
const GLOBAL_TLB_STATUS: usize = 0x074;
const TLB_STATUS_ACTIVE: u32 = 1;
const STREAM_MATCH_BASE: usize = 0x800;
const STREAM_CONTEXT_BASE: usize = 0xc00;

const ID0_STREAM_GROUP_COUNT: u32 = 0xff;
const ID0_STAGE1_TRANSLATION: u32 = 1 << 30;
const ID1_LARGE_REGISTER_PAGE: u32 = 1 << 31;
const ID1_CONTEXT_PAGE_OFFSET: u32 = 0x7 << 28;
const ID1_STAGE2_CONTEXT_BANK_COUNT: u32 = 0xff << 16;
const ID1_CONTEXT_BANK_COUNT: u32 = 0xff;
const ID2_4K_PAGE_TABLE: u32 = 1 << 12;
const ID2_VIRTUAL_ADDRESS_SIZE: u32 = 0xf << 8;
const ID2_OUTPUT_ADDRESS_SIZE: u32 = 0xf << 4;
const ID2_INPUT_ADDRESS_SIZE: u32 = 0xf;

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
const CONTEXT_ATTRIBUTE_64BIT: u32 = 1;

const CONTEXT_CONTROL: usize = 0x000;
const CONTEXT_TCR2: usize = 0x010;
const CONTEXT_CONTROL_MMU_ENABLE: u32 = 1;
const CONTEXT_TTBR0: usize = 0x020;
const CONTEXT_TTBR1: usize = 0x028;
const CONTEXT_TCR: usize = 0x030;
const CONTEXT_MAIR0: usize = 0x038;
const CONTEXT_MAIR1: usize = 0x03c;
const CONTEXT_FAULT_STATUS: usize = 0x058;
const CONTEXT_TLB_INVALIDATE_ASID: usize = 0x610;
const CONTEXT_TLB_SYNC: usize = 0x7f0;
const CONTEXT_TLB_STATUS: usize = 0x7f4;
const CONTEXT_CONTROL_STALL_ON_FAULT: u32 = 1 << 7;
const CONTEXT_CONTROL_FAULT_INTERRUPT: u32 = 1 << 6;
const CONTEXT_CONTROL_FAULT_REPORT: u32 = 1 << 5;
const CONTEXT_CONTROL_ASID_PRIVATE: u32 = 1 << 12;
const CONTEXT_CONTROL_ACCESS_FLAG_ENABLE: u32 = 1 << 2;
const CONTEXT_CONTROL_TRE: u32 = 1 << 1;

const MIN_DMA_IOVA_BITS: u32 = 32;
// The local 4 KiB page-table implementation starts at level 1.  It therefore
// covers at most bits 38:0; wider apertures require a level-0 table.
const MAX_THREE_LEVEL_IOVA_BITS: u32 = 39;
const TCR_INNER_SHAREABLE: u32 = 3 << 12;
const TCR_OUTER_WBWA: u32 = 1 << 10;
const TCR_INNER_WBWA: u32 = 1 << 8;
const TCR_DISABLE_TTBR1: u32 = 1 << 23;
const TCR2_16BIT_ASID: u32 = 1 << 4;
const TCR2_SEP_UPSTREAM: u32 = 7 << 15;
const MAIR_NORMAL_WBWA: u32 = 0xff;
const MAIR_NORMAL_NONCACHEABLE: u32 = 0x44 << 8;

const TABLE_VALID: u64 = 1;
const TABLE_DESCRIPTOR: u64 = 1 << 1;
const PTE_ACCESS_FLAG: u64 = 1 << 10;
const PTE_INNER_SHAREABLE: u64 = 3 << 8;
const PTE_READ_ONLY: u64 = 2 << 6;
const PTE_UNPRIVILEGED: u64 = 1 << 6;
const PTE_ATTR_NONCACHEABLE: u64 = 1 << 2;
const PTE_NOT_GLOBAL: u64 = 1 << 11;
const PTE_PXN: u64 = 1 << 53;
const PTE_UXN: u64 = 1 << 54;
const TABLE_ADDRESS_MASK: u64 = 0x0000_ffff_ffff_f000;
const TLB_SYNC_SPINS: usize = 1_000_000;

#[derive(Clone, Copy)]
struct RegisterWindow {
    base: usize,
}

impl RegisterWindow {
    const fn new(base: usize) -> Self {
        Self { base }
    }

    fn read(self, offset: usize) -> u32 {
        // SAFETY: probe validates the ID-derived global, stream, and context
        // register extents against the complete ioremap'd firmware resource.
        unsafe { mmio::read32(self.base + offset) }
    }

    fn write(self, offset: usize, value: u32) {
        // SAFETY: see `read`; writes target the same mapped register window.
        unsafe { mmio::write32(self.base + offset, value) }
    }

    fn read64(self, offset: usize) -> u64 {
        // SAFETY: see `read`; the SMMU's TTBR registers require 64-bit accesses.
        unsafe { mmio::read64(self.base + offset) }
    }

    fn write64(self, offset: usize, value: u64) {
        // SAFETY: see `read`; the SMMU's TTBR registers require 64-bit accesses.
        unsafe { mmio::write64(self.base + offset, value) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IdentityRouting {
    FirmwareUnmatchedBypass,
    DirectBypass,
    DisabledContext,
}

impl IdentityRouting {
    const fn label(self) -> &'static str {
        match self {
            Self::FirmwareUnmatchedBypass => "firmware-unmatched-bypass",
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
    stage1_context_start: usize,
    context_bank_count: usize,
    dma_supported: bool,
    dma_iova_address_bits: u32,
    dma_output_address_size: u32,
    dma_output_address_limit: u64,
    table_address_limit: u64,
    identity_routing: IdentityRouting,
    _clocks: EnabledClocks,
    _mmio: MmioMapping,
    lock: IrqSpinLock<()>,
    allocated_contexts: IrqSpinLock<Vec<bool>>,
    claimed_streams: IrqSpinLock<BTreeMap<u32, usize>>,
}

struct EnabledClocks(Vec<ClkHandle>);

impl Drop for EnabledClocks {
    fn drop(&mut self) {
        for clock in self.0.iter().rev() {
            clock.disable_unprepare();
        }
    }
}

/// Owns an ioremap allocation for the lifetime of the controller.
struct MmioMapping {
    base: usize,
}

impl Drop for MmioMapping {
    fn drop(&mut self) {
        vm::iounmap(self.base);
    }
}

#[derive(Clone, Copy)]
struct ContextSnapshot {
    cbar: u32,
    cba2r: u32,
    sctlr: u32,
    tcr2: u32,
    tcr: u32,
    ttbr0: u64,
    ttbr1: u64,
    mair0: u32,
    mair1: u32,
}

struct ContextLease {
    index: usize,
    snapshot: ContextSnapshot,
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

    fn context_write64(&self, context: usize, offset: usize, value: u64) {
        self.registers.write64(
            self.page_offset(self.context_page_base + context, offset),
            value,
        )
    }

    fn context_read64(&self, context: usize, offset: usize) -> u64 {
        self.registers
            .read64(self.page_offset(self.context_page_base + context, offset))
    }

    fn snapshot_context(&self, context: usize) -> ContextSnapshot {
        ContextSnapshot {
            cbar: self.global_page_1_read(CONTEXT_ATTRIBUTE_BASE + context * 4),
            cba2r: self.global_page_1_read(CONTEXT_ATTRIBUTE_2_BASE + context * 4),
            sctlr: self.context_read(context, CONTEXT_CONTROL),
            tcr2: self.context_read(context, CONTEXT_TCR2),
            tcr: self.context_read(context, CONTEXT_TCR),
            ttbr0: self.context_read64(context, CONTEXT_TTBR0),
            ttbr1: self.context_read64(context, CONTEXT_TTBR1),
            mair0: self.context_read(context, CONTEXT_MAIR0),
            mair1: self.context_read(context, CONTEXT_MAIR1),
        }
    }

    fn wait_for_tlb(&self, page: usize, status: usize) -> Result<(), IommuError> {
        for _ in 0..TLB_SYNC_SPINS {
            if self.registers.read(self.page_offset(page, status)) & TLB_STATUS_ACTIVE == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(IommuError::Busy)
    }

    fn invalidate_context(&self, context: usize, asid: u16) -> Result<(), IommuError> {
        arch::wmb();
        self.context_write(context, CONTEXT_TLB_INVALIDATE_ASID, u32::from(asid));
        self.context_write(context, CONTEXT_TLB_SYNC, 0);
        arch::io_wmb();
        self.wait_for_tlb(self.context_page_base + context, CONTEXT_TLB_STATUS)
    }

    fn invalidate_global(&self) -> Result<(), IommuError> {
        arch::wmb();
        self.registers.write(GLOBAL_TLB_INVALIDATE_ALL_NONSECURE, 0);
        self.registers.write(GLOBAL_TLB_SYNC, 0);
        arch::io_wmb();
        self.wait_for_tlb(0, GLOBAL_TLB_STATUS)
    }

    fn reserve_context(&self) -> Result<ContextLease, IommuError> {
        // Routing state is protected by `lock`. Always acquire it before the
        // allocation bitmap so selection is atomic with attach/detach and all
        // paths use one lock order.
        let _guard = self.lock.lock();
        let mut allocated = self.allocated_contexts.lock();
        let mut software_reserved = 0usize;
        let mut firmware_routed = 0usize;
        let mut enabled = 0usize;
        for index in self.stage1_context_start..self.context_bank_count {
            let snapshot = self.snapshot_context(index);
            if allocated[index] {
                software_reserved += 1;
                continue;
            }
            if self.context_is_routed(index) {
                firmware_routed += 1;
                continue;
            }
            if snapshot.sctlr & CONTEXT_CONTROL_MMU_ENABLE != 0 {
                enabled += 1;
                continue;
            }
            debug_assert!(context_bank_is_available(false, false, snapshot.sctlr));

            // Firmware may leave stale CBAR metadata in an otherwise disabled,
            // unrouted context bank. CBAR describes how a bank is configured;
            // it does not establish ownership. Reinitializing this bank is safe
            // because no valid SMR selects it and SCTLR.M is clear.
            allocated[index] = true;
            early_println!(
                "[arm-smmu-v2] context lease stage=reserved CB {}: cbar={:#010x} cba2r={:#010x} sctlr={:#010x}",
                index,
                snapshot.cbar,
                snapshot.cba2r,
                snapshot.sctlr,
            );
            return Ok(ContextLease { index, snapshot });
        }
        early_println!(
            "[arm-smmu-v2] domain alloc failed stage=context: range={}..{} software-reserved={} firmware-routed={} enabled={}",
            self.stage1_context_start,
            self.context_bank_count,
            software_reserved,
            firmware_routed,
            enabled,
        );
        Err(IommuError::DomainAllocationFailed)
    }

    fn release_context(&self, lease: &ContextLease) -> bool {
        let _guard = self.lock.lock();
        let context = lease.index;
        if self.context_is_routed(context) {
            early_println!(
                "[arm-smmu-v2] context lease stage=quarantined CB {}: stream route remains attached",
                context,
            );
            return false;
        }
        self.context_write(context, CONTEXT_CONTROL, 0);
        arch::io_wmb();
        if self
            .invalidate_context(context, context_asid(context))
            .is_err()
        {
            early_println!(
                "[arm-smmu-v2] context lease stage=quarantined CB {}: context TLBI sync failed",
                context,
            );
            return false;
        }

        // Restore the bank in the same fail-closed order used by Linux: keep
        // translation disabled, restore configuration and table state, make
        // stale global translations impossible, then restore SCTLR last.
        self.global_page_1_write(CONTEXT_ATTRIBUTE_2_BASE + context * 4, lease.snapshot.cba2r);
        self.global_page_1_write(CONTEXT_ATTRIBUTE_BASE + context * 4, lease.snapshot.cbar);
        self.context_write(context, CONTEXT_TCR2, lease.snapshot.tcr2);
        self.context_write(context, CONTEXT_TCR, lease.snapshot.tcr);
        self.context_write64(context, CONTEXT_TTBR0, lease.snapshot.ttbr0);
        self.context_write64(context, CONTEXT_TTBR1, lease.snapshot.ttbr1);
        self.context_write(context, CONTEXT_MAIR0, lease.snapshot.mair0);
        self.context_write(context, CONTEXT_MAIR1, lease.snapshot.mair1);
        arch::io_wmb();
        if self.invalidate_global().is_err() {
            early_println!(
                "[arm-smmu-v2] context lease stage=quarantined CB {}: global TLBI sync failed; SCTLR remains disabled",
                context,
            );
            return false;
        }
        self.context_write(context, CONTEXT_CONTROL, lease.snapshot.sctlr);
        arch::io_wmb();
        self.allocated_contexts.lock()[context] = false;
        early_println!(
            "[arm-smmu-v2] context lease stage=released CB {}: firmware snapshot restored",
            context,
        );
        true
    }

    fn allocate_disabled_context(&self) -> Result<ContextLease, IommuError> {
        let lease = self.reserve_context()?;
        let index = lease.index;
        {
            let _guard = self.lock.lock();
            self.context_write(index, CONTEXT_CONTROL, 0);
            arch::io_wmb();
            let attribute_offset = CONTEXT_ATTRIBUTE_BASE + index * 4;
            let inherited = self.global_page_1_read(attribute_offset);

            let attribute = (inherited & !(CONTEXT_ATTRIBUTE_TYPE | CONTEXT_ATTRIBUTE_VMID))
                | CONTEXT_ATTRIBUTE_STAGE1;
            let attribute_2_offset = CONTEXT_ATTRIBUTE_2_BASE + index * 4;
            let attribute_2 = self.global_page_1_read(attribute_2_offset) | CONTEXT_ATTRIBUTE_64BIT;
            self.global_page_1_write(attribute_2_offset, attribute_2);
            self.global_page_1_write(attribute_offset, attribute);
            self.context_write(index, CONTEXT_TCR2, 0);
            self.context_write(index, CONTEXT_TCR, 0);
            self.context_write64(index, CONTEXT_TTBR0, 0);
            self.context_write64(index, CONTEXT_TTBR1, 0);
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
        }
        Ok(lease)
    }

    fn configure_dma_context(
        &self,
        lease: &ContextLease,
        root: usize,
        iova_address_bits: u32,
    ) -> Result<(), IommuError> {
        if !(MIN_DMA_IOVA_BITS..=self.dma_iova_address_bits).contains(&iova_address_bits) {
            return Err(IommuError::MapFailed);
        }
        let _guard = self.lock.lock();
        let context = lease.index;
        let global_control = self.registers.read(GLOBAL_CONTROL);
        if global_control & GLOBAL_CONTROL_CLIENT_POWER_DOWN != 0 {
            self.registers.write(
                GLOBAL_CONTROL,
                global_control & !GLOBAL_CONTROL_CLIENT_POWER_DOWN,
            );
            arch::io_wmb();
            if self.registers.read(GLOBAL_CONTROL) & GLOBAL_CONTROL_CLIENT_POWER_DOWN != 0 {
                early_println!(
                    "[arm-smmu-v2] domain alloc failed stage=client-power: scr0-before={:#010x} scr0-after={:#010x}",
                    global_control,
                    self.registers.read(GLOBAL_CONTROL),
                );
                return Err(IommuError::DomainAllocationFailed);
            }
        }
        let cbar = CONTEXT_ATTRIBUTE_STAGE1 | (0xf << 12) | (3 << 8);
        self.context_write(context, CONTEXT_CONTROL, 0);
        arch::io_wmb();
        self.global_page_1_write(
            CONTEXT_ATTRIBUTE_2_BASE + context * 4,
            CONTEXT_ATTRIBUTE_64BIT,
        );
        self.global_page_1_write(CONTEXT_ATTRIBUTE_BASE + context * 4, cbar);
        self.context_write(
            context,
            CONTEXT_TCR2,
            TCR2_SEP_UPSTREAM | TCR2_16BIT_ASID | self.dma_output_address_size,
        );
        self.context_write(
            context,
            CONTEXT_TCR,
            TCR_DISABLE_TTBR1
                | TCR_INNER_SHAREABLE
                | TCR_OUTER_WBWA
                | TCR_INNER_WBWA
                | (u64::BITS - iova_address_bits),
        );
        self.context_write64(
            context,
            CONTEXT_TTBR0,
            root as u64 | (u64::from(context_asid(context)) << 48),
        );
        self.context_write64(context, CONTEXT_TTBR1, 0);
        self.context_write(
            context,
            CONTEXT_MAIR0,
            MAIR_NORMAL_WBWA | MAIR_NORMAL_NONCACHEABLE,
        );
        self.context_write(context, CONTEXT_MAIR1, 0);
        self.invalidate_context(context, context_asid(context))?;
        self.context_write(
            context,
            CONTEXT_CONTROL,
            CONTEXT_CONTROL_ASID_PRIVATE
                | CONTEXT_CONTROL_FAULT_INTERRUPT
                | CONTEXT_CONTROL_FAULT_REPORT
                | CONTEXT_CONTROL_ACCESS_FLAG_ENABLE
                | CONTEXT_CONTROL_TRE
                | CONTEXT_CONTROL_MMU_ENABLE,
        );
        arch::io_wmb();
        Ok(())
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

    fn validate_firmware_identity_route(&self, stream_id: u32) -> Result<(), IommuError> {
        let global_control = self.registers.read(GLOBAL_CONTROL);
        let global_fault = self.registers.read(GLOBAL_FAULT_STATUS);
        let Some(group) = self.matching_stream_group(stream_id) else {
            early_println!(
                "[arm-smmu-v2] SID {:#x} has no firmware SMR: scr0={:#010x} gfsr={:#010x}",
                stream_id,
                global_control,
                global_fault,
            );
            return Ok(());
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
            return if stream_context & STREAM_CONTEXT_TYPE_MASK == STREAM_CONTEXT_BYPASS {
                Ok(())
            } else {
                Err(IommuError::AttachFailed)
            };
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
            return Err(IommuError::AttachFailed);
        }
        let context_control = self.context_read(context, CONTEXT_CONTROL);
        early_println!(
            "[arm-smmu-v2] SID {:#x} firmware route: SMR {} smr={:#010x} s2cr={:#010x} CB {} cbar={:#010x} sctlr={:#010x} fsr={:#010x} scr0={:#010x} gfsr={:#010x}",
            stream_id,
            group,
            stream_match,
            stream_context,
            context,
            self.global_page_1_read(CONTEXT_ATTRIBUTE_BASE + context * 4),
            context_control,
            self.context_read(context, CONTEXT_FAULT_STATUS),
            global_control,
            global_fault,
        );
        if context_control & CONTEXT_CONTROL_MMU_ENABLE == 0 {
            Ok(())
        } else {
            Err(IommuError::AttachFailed)
        }
    }

    fn enable_firmware_unmatched_bypass_locked(&self, stream_id: u32) -> Result<(), IommuError> {
        self.validate_firmware_identity_route(stream_id)?;

        let inherited = self.registers.read(GLOBAL_CONTROL);
        let requested =
            inherited & !(GLOBAL_CONTROL_CLIENT_POWER_DOWN | GLOBAL_CONTROL_UNMATCHED_STREAM_FAULT);
        if requested != inherited {
            self.registers.write(GLOBAL_CONTROL, requested);
            arch::io_wmb();
        }
        let current = self.registers.read(GLOBAL_CONTROL);
        if current & (GLOBAL_CONTROL_CLIENT_POWER_DOWN | GLOBAL_CONTROL_UNMATCHED_STREAM_FAULT) != 0
        {
            return Err(IommuError::AttachFailed);
        }
        early_println!(
            "[arm-smmu-v2] SID {:#x} identity DMA enabled through unmatched bypass: scr0 {:#010x} -> {:#010x}",
            stream_id,
            inherited,
            current,
        );
        Ok(())
    }

    fn attach_stream(
        &self,
        context: Option<usize>,
        stream_id: u32,
        translated: bool,
    ) -> Result<StreamRoute, IommuError> {
        if stream_id > STREAM_MATCH_ID {
            return Err(IommuError::InvalidSpec);
        }

        if !translated && self.identity_routing == IdentityRouting::FirmwareUnmatchedBypass {
            let _guard = self.lock.lock();
            let mut claimed_streams = self.claimed_streams.lock();
            if claimed_streams.contains_key(&stream_id) {
                return Err(IommuError::AttachFailed);
            }
            self.enable_firmware_unmatched_bypass_locked(stream_id)?;
            claimed_streams.insert(stream_id, usize::MAX);
            return Ok(StreamRoute::untracked());
        }

        let _guard = self.lock.lock();
        let mut claimed_streams = self.claimed_streams.lock();
        if claimed_streams.contains_key(&stream_id) {
            return Err(IommuError::AttachFailed);
        }
        let global_control = self.registers.read(GLOBAL_CONTROL);
        if global_control & GLOBAL_CONTROL_CLIENT_POWER_DOWN != 0 {
            self.registers.write(
                GLOBAL_CONTROL,
                global_control & !GLOBAL_CONTROL_CLIENT_POWER_DOWN,
            );
            arch::io_wmb();
            if self.registers.read(GLOBAL_CONTROL) & GLOBAL_CONTROL_CLIENT_POWER_DOWN != 0 {
                return Err(IommuError::AttachFailed);
            }
        }
        let existing_group = self.matching_stream_group(stream_id);
        let group = existing_group
            .or_else(|| self.unused_stream_group())
            .ok_or(IommuError::AttachFailed)?;
        let old_smr = self.registers.read(STREAM_MATCH_BASE + group * 4);
        let old_s2cr = self.registers.read(STREAM_CONTEXT_BASE + group * 4);
        if existing_group.is_some()
            && (((old_smr >> STREAM_MATCH_MASK_SHIFT) & STREAM_MATCH_MASK) != 0
                || old_smr & STREAM_MATCH_ID != stream_id)
        {
            // Never redirect a firmware range mapping: doing so would move
            // sibling streams into this domain. An exact pre-existing SID can
            // be taken over and is restored verbatim on detach.
            return Err(IommuError::AttachFailed);
        }

        let route = if translated {
            let context = context.ok_or(IommuError::AttachFailed)?;
            STREAM_CONTEXT_TRANSLATE | (context as u32 & STREAM_CONTEXT_BANK)
        } else {
            match (self.identity_routing, context) {
                (IdentityRouting::FirmwareUnmatchedBypass, None) => {
                    return Ok(StreamRoute::untracked());
                }
                (IdentityRouting::DirectBypass, None) => STREAM_CONTEXT_BYPASS,
                (IdentityRouting::DisabledContext, Some(context)) => {
                    STREAM_CONTEXT_TRANSLATE | (context as u32 & STREAM_CONTEXT_BANK)
                }
                _ => return Err(IommuError::AttachFailed),
            }
        };
        self.registers.write(
            STREAM_CONTEXT_BASE + group * 4,
            route & (STREAM_CONTEXT_TYPE_MASK | STREAM_CONTEXT_BANK),
        );
        if existing_group.is_none() {
            self.registers.write(
                STREAM_MATCH_BASE + group * 4,
                STREAM_MATCH_VALID | stream_id,
            );
        }
        arch::io_wmb();
        claimed_streams.insert(stream_id, group);

        early_println!(
            "[arm-smmu-v2] SID {:#x} routed by SMR {}: smr={:#010x} s2cr={:#010x}",
            stream_id,
            group,
            self.registers.read(STREAM_MATCH_BASE + group * 4),
            self.registers.read(STREAM_CONTEXT_BASE + group * 4),
        );
        Ok(StreamRoute {
            group,
            old_smr,
            old_s2cr,
            installed_s2cr: route,
        })
    }

    fn detach_stream(&self, route: StreamRoute, stream_id: u32) -> Result<(), IommuError> {
        if route.group == usize::MAX {
            let _guard = self.lock.lock();
            let mut claimed_streams = self.claimed_streams.lock();
            if claimed_streams.get(&stream_id) != Some(&usize::MAX) {
                return Err(IommuError::AttachFailed);
            }
            claimed_streams.remove(&stream_id);
            return Ok(());
        }
        let _guard = self.lock.lock();
        let mut claimed_streams = self.claimed_streams.lock();
        if claimed_streams.get(&stream_id) != Some(&route.group) {
            return Err(IommuError::AttachFailed);
        }
        let smr = self.registers.read(STREAM_MATCH_BASE + route.group * 4);
        let s2cr = self.registers.read(STREAM_CONTEXT_BASE + route.group * 4);
        let route_is_installed = smr & STREAM_MATCH_VALID != 0
            && smr & STREAM_MATCH_ID == stream_id
            && s2cr & (STREAM_CONTEXT_TYPE_MASK | STREAM_CONTEXT_BANK) == route.installed_s2cr;
        let route_is_restored = smr == route.old_smr && s2cr == route.old_s2cr;
        if !route_is_installed && !route_is_restored {
            return Err(IommuError::AttachFailed);
        }
        if route_is_installed {
            self.registers
                .write(STREAM_MATCH_BASE + route.group * 4, route.old_smr);
            self.registers
                .write(STREAM_CONTEXT_BASE + route.group * 4, route.old_s2cr);
            arch::io_wmb();
        }
        self.invalidate_global()?;
        claimed_streams.remove(&stream_id);
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct StreamRoute {
    group: usize,
    old_smr: u32,
    old_s2cr: u32,
    installed_s2cr: u32,
}

impl StreamRoute {
    const fn untracked() -> Self {
        Self {
            group: usize::MAX,
            old_smr: 0,
            old_s2cr: 0,
            installed_s2cr: 0,
        }
    }
}

const fn context_asid(context: usize) -> u16 {
    context as u16 + 1
}

const fn id_size_to_bits(encoded: u32) -> u32 {
    match encoded {
        0 => 32,
        1 => 36,
        2 => 40,
        3 => 42,
        4 => 44,
        _ => 48,
    }
}

const fn normalized_address_size(encoded: u32) -> u32 {
    if encoded > 5 { 5 } else { encoded }
}

const fn address_limit(bits: u32) -> u64 {
    1u64 << bits
}

const fn context_bank_is_available(software_reserved: bool, routed: bool, sctlr: u32) -> bool {
    !software_reserved && !routed && sctlr & CONTEXT_CONTROL_MMU_ENABLE == 0
}

struct IdentityDomain {
    hardware: Arc<SmmuHardware>,
    context: Option<ContextLease>,
    streams: IrqSpinLock<BTreeMap<IommuStreamId, StreamRoute>>,
}

impl IommuDomain for IdentityDomain {
    fn attach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
        let mut streams = self.streams.lock();
        if streams.contains_key(&stream) {
            return Ok(());
        }
        let route = self.hardware.attach_stream(
            self.context.as_ref().map(|lease| lease.index),
            stream.id,
            false,
        )?;
        streams.insert(stream, route);
        Ok(())
    }

    fn detach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
        let mut streams = self.streams.lock();
        let route = *streams.get(&stream).ok_or(IommuError::AttachFailed)?;
        self.hardware.detach_stream(route, stream.id)?;
        streams.remove(&stream);
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
            .as_ref()
            .map(|lease| {
                self.hardware
                    .context_read(lease.index, CONTEXT_FAULT_STATUS)
            })
            .unwrap_or(0);
        early_println!(
            "[arm-smmu-v2] identity domain synchronized: gfsr={:#010x} fsr={:#010x}",
            self.hardware.registers.read(GLOBAL_FAULT_STATUS),
            context_fault,
        );
        Ok(())
    }
}

impl Drop for IdentityDomain {
    fn drop(&mut self) {
        let streams = core::mem::take(&mut *self.streams.lock());
        let mut restored_all = true;
        for (stream, route) in streams {
            if self.hardware.detach_stream(route, stream.id).is_err() {
                restored_all = false;
                early_println!(
                    "[arm-smmu-v2] failed to restore identity route for SID {:#x} during drop",
                    stream.id,
                );
            }
        }
        if let Some(lease) = self.context.as_ref() {
            if restored_all {
                if !self.hardware.release_context(lease) {
                    early_println!(
                        "[arm-smmu-v2] retaining identity CB {} after context restore failure",
                        lease.index,
                    );
                }
            } else {
                early_println!(
                    "[arm-smmu-v2] retaining identity CB {} after route teardown failure",
                    lease.index,
                );
            }
        }
    }
}

struct DmaPageTables {
    root: usize,
    pages: Vec<usize>,
    table_address_limit: u64,
}

impl DmaPageTables {
    fn new(table_address_limit: u64) -> Result<Self, IommuError> {
        let root = Self::allocate_table(table_address_limit)?;
        Ok(Self {
            root,
            pages: vec![root],
            table_address_limit,
        })
    }

    fn allocate_table(table_address_limit: u64) -> Result<usize, IommuError> {
        let page = allocate_raw_pages(1);
        if page.is_null() {
            early_println!(
                "[arm-smmu-v2] domain alloc failed stage=table-pmm: pages=1 limit={:#x}",
                table_address_limit,
            );
            return Err(IommuError::DomainAllocationFailed);
        }
        // SAFETY: `allocate_raw_pages(1)` returned exclusive ownership of one
        // PAGE_SIZE-aligned page. Translation tables must be zero before the
        // parent descriptor makes them visible to the SMMU walker.
        unsafe { core::ptr::write_bytes(page.cast::<u8>(), 0, PAGE_SIZE) };
        arch::clean_dcache_to_poc_range(page as usize, PAGE_SIZE);
        arch::wmb();
        let paddr = virt_to_phys(page as usize);
        if (paddr as u64)
            .checked_add(PAGE_SIZE as u64)
            .is_none_or(|end| end > table_address_limit)
        {
            early_println!(
                "[arm-smmu-v2] domain alloc failed stage=table-address: vaddr={:#x} paddr={:#x} end={:#x} limit={:#x}",
                page as usize,
                paddr,
                (paddr as u64)
                    .checked_add(PAGE_SIZE as u64)
                    .unwrap_or(u64::MAX),
                table_address_limit,
            );
            free_raw_pages(page, 1);
            return Err(IommuError::DomainAllocationFailed);
        }
        early_println!(
            "[arm-smmu-v2] domain alloc stage=table ready: vaddr={:#x} paddr={:#x} limit={:#x}",
            page as usize,
            paddr,
            table_address_limit,
        );
        Ok(paddr)
    }

    fn table_entry(table: usize, index: usize) -> *mut u64 {
        (phys_to_virt(table) as *mut u64).wrapping_add(index)
    }

    fn read_entry(table: usize, index: usize) -> u64 {
        // SAFETY: every table passed here is a live, page-aligned table page and
        // every AArch64 translation table contains exactly 512 u64 entries.
        unsafe { Self::table_entry(table, index).read() }
    }

    fn write_entry(table: usize, index: usize, value: u64) {
        // SAFETY: see `read_entry`; domain locking serializes all mutations.
        unsafe { Self::table_entry(table, index).write(value) }
    }

    fn next_table(&mut self, table: usize, index: usize) -> Result<usize, IommuError> {
        let entry = Self::read_entry(table, index);
        if entry & TABLE_VALID != 0 {
            if entry & TABLE_DESCRIPTOR == 0 {
                return Err(IommuError::MapFailed);
            }
            return Ok((entry & TABLE_ADDRESS_MASK) as usize);
        }
        let next = Self::allocate_table(self.table_address_limit)?;
        self.pages.push(next);
        Self::write_entry(table, index, next as u64 | TABLE_VALID | TABLE_DESCRIPTOR);
        arch::clean_dcache_to_poc_range(phys_to_virt(table), PAGE_SIZE);
        Ok(next)
    }

    fn leaf_table(&mut self, iova: Iova) -> Result<(usize, usize), IommuError> {
        let l1 = ((iova >> 30) & 0x1ff) as usize;
        let l2 = ((iova >> 21) & 0x1ff) as usize;
        let l3 = ((iova >> 12) & 0x1ff) as usize;
        let second = self.next_table(self.root, l1)?;
        let leaf = self.next_table(second, l2)?;
        Ok((leaf, l3))
    }

    fn lookup(&self, iova: Iova) -> Option<PhysAddr> {
        let mut table = self.root;
        for shift in [30, 21] {
            let entry = Self::read_entry(table, ((iova >> shift) & 0x1ff) as usize);
            if entry & (TABLE_VALID | TABLE_DESCRIPTOR) != TABLE_VALID | TABLE_DESCRIPTOR {
                return None;
            }
            table = (entry & TABLE_ADDRESS_MASK) as usize;
        }
        let entry = Self::read_entry(table, ((iova >> 12) & 0x1ff) as usize);
        if entry & (TABLE_VALID | TABLE_DESCRIPTOR) != TABLE_VALID | TABLE_DESCRIPTOR {
            return None;
        }
        Some((entry & TABLE_ADDRESS_MASK) as usize | (iova as usize & (PAGE_SIZE - 1)))
    }

    fn map_page(
        &mut self,
        iova: Iova,
        paddr: PhysAddr,
        flags: IommuMapFlags,
    ) -> Result<(), IommuError> {
        let (table, index) = self.leaf_table(iova)?;
        if Self::read_entry(table, index) & TABLE_VALID != 0 {
            return Err(IommuError::MapFailed);
        }
        let mut descriptor = paddr as u64
            | TABLE_VALID
            | TABLE_DESCRIPTOR
            | PTE_ACCESS_FLAG
            | PTE_INNER_SHAREABLE
            | PTE_UNPRIVILEGED
            | PTE_NOT_GLOBAL;
        if !flags.contains(IommuMapFlags::WRITE) {
            descriptor |= PTE_READ_ONLY;
        }
        if !flags.contains(IommuMapFlags::COHERENT) {
            descriptor |= PTE_ATTR_NONCACHEABLE;
        }
        if !flags.contains(IommuMapFlags::EXECUTE) {
            descriptor |= PTE_PXN | PTE_UXN;
        }
        Self::write_entry(table, index, descriptor);
        arch::clean_dcache_to_poc_range(phys_to_virt(table), PAGE_SIZE);
        Ok(())
    }

    fn unmap_page(&mut self, iova: Iova) -> Result<(), IommuError> {
        let (table, index) = self.leaf_table(iova)?;
        if Self::read_entry(table, index) & TABLE_VALID == 0 {
            return Err(IommuError::UnmapFailed);
        }
        Self::write_entry(table, index, 0);
        arch::clean_dcache_to_poc_range(phys_to_virt(table), PAGE_SIZE);
        Ok(())
    }
}

impl Drop for DmaPageTables {
    fn drop(&mut self) {
        for table in self.pages.drain(..) {
            free_raw_pages(phys_to_virt(table) as *mut Page, 1);
        }
    }
}

struct DmaDomain {
    hardware: Arc<SmmuHardware>,
    context: ContextLease,
    iova_base: Iova,
    iova_size: u64,
    iova_address_limit: u64,
    output_address_limit: u64,
    tables: IrqSpinLock<Option<DmaPageTables>>,
    streams: IrqSpinLock<BTreeMap<IommuStreamId, StreamRoute>>,
}

impl DmaDomain {
    fn validate_range(&self, iova: Iova, len: usize) -> Result<(), IommuError> {
        validate_dma_range(
            self.iova_base,
            self.iova_size,
            self.iova_address_limit,
            iova,
            len,
        )
    }
}

impl IommuDomain for DmaDomain {
    fn attach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
        if stream.substream_id.is_some() {
            return Err(IommuError::NotSupported);
        }
        let mut streams = self.streams.lock();
        if streams.contains_key(&stream) {
            return Ok(());
        }
        let group = self
            .hardware
            .attach_stream(Some(self.context.index), stream.id, true)?;
        streams.insert(stream, group);
        Ok(())
    }

    fn detach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
        let mut streams = self.streams.lock();
        let route = *streams.get(&stream).ok_or(IommuError::AttachFailed)?;
        self.hardware.detach_stream(route, stream.id)?;
        streams.remove(&stream);
        Ok(())
    }

    fn map(
        &self,
        iova: Iova,
        paddr: PhysAddr,
        len: usize,
        flags: IommuMapFlags,
    ) -> Result<(), IommuError> {
        if let Err(error) = self.validate_range(iova, len) {
            early_println!(
                "[arm-smmu-v2] DMA map rejected reason=iova-range iova={:#x} paddr={:#x} len={:#x} flags={:#x}",
                iova,
                paddr,
                len,
                flags.bits(),
            );
            return Err(error);
        }
        if paddr & (PAGE_SIZE - 1) != 0 {
            early_println!(
                "[arm-smmu-v2] DMA map rejected reason=paddr-alignment iova={:#x} paddr={:#x} len={:#x} flags={:#x}",
                iova,
                paddr,
                len,
                flags.bits(),
            );
            return Err(IommuError::MapFailed);
        }
        if (paddr as u64)
            .checked_add(len as u64)
            .is_none_or(|end| end > self.output_address_limit)
        {
            early_println!(
                "[arm-smmu-v2] DMA map rejected reason=paddr-range iova={:#x} paddr={:#x} len={:#x} flags={:#x} limit={:#x}",
                iova,
                paddr,
                len,
                flags.bits(),
                self.output_address_limit,
            );
            return Err(IommuError::MapFailed);
        }
        if !dma_permissions_valid(flags) {
            early_println!(
                "[arm-smmu-v2] DMA map rejected reason=permissions iova={:#x} paddr={:#x} len={:#x} flags={:#x}",
                iova,
                paddr,
                len,
                flags.bits(),
            );
            return Err(IommuError::MapFailed);
        }
        let mut tables = self.tables.lock();
        let tables = tables.as_mut().ok_or(IommuError::MapFailed)?;
        for offset in (0..len).step_by(PAGE_SIZE) {
            if tables.lookup(iova + offset as u64).is_some() {
                return Err(IommuError::MapFailed);
            }
        }
        let mut mapped = 0;
        for offset in (0..len).step_by(PAGE_SIZE) {
            if let Err(error) = tables.map_page(iova + offset as u64, paddr + offset, flags) {
                for rollback in (0..mapped).step_by(PAGE_SIZE) {
                    let _ = tables.unmap_page(iova + rollback as u64);
                }
                let _ = self
                    .hardware
                    .invalidate_context(self.context.index, context_asid(self.context.index));
                return Err(error);
            }
            mapped += PAGE_SIZE;
        }
        self.hardware
            .invalidate_context(self.context.index, context_asid(self.context.index))
    }

    fn unmap(&self, iova: Iova, len: usize) -> Result<(), IommuError> {
        self.validate_range(iova, len)
            .map_err(|_| IommuError::UnmapFailed)?;
        let mut tables = self.tables.lock();
        let tables = tables.as_mut().ok_or(IommuError::UnmapFailed)?;
        for offset in (0..len).step_by(PAGE_SIZE) {
            if tables.lookup(iova + offset as u64).is_none() {
                return Err(IommuError::UnmapFailed);
            }
        }
        for offset in (0..len).step_by(PAGE_SIZE) {
            tables.unmap_page(iova + offset as u64)?;
        }
        self.hardware
            .invalidate_context(self.context.index, context_asid(self.context.index))
    }

    fn iova_to_phys(&self, iova: Iova) -> Option<PhysAddr> {
        self.tables
            .lock()
            .as_ref()
            .and_then(|tables| tables.lookup(iova))
    }

    fn page_size(&self) -> usize {
        PAGE_SIZE
    }

    fn flush(&self) -> Result<(), IommuError> {
        self.hardware
            .invalidate_context(self.context.index, context_asid(self.context.index))
    }
}

impl Drop for DmaDomain {
    fn drop(&mut self) {
        let streams = core::mem::take(&mut *self.streams.lock());
        let mut restored_all = true;
        for (stream, route) in streams {
            if self.hardware.detach_stream(route, stream.id).is_err() {
                restored_all = false;
                early_println!(
                    "[arm-smmu-v2] failed to restore translated route for SID {:#x} during drop",
                    stream.id,
                );
            }
        }
        let released = restored_all && self.hardware.release_context(&self.context);
        if !released {
            // A route may still point at this context, or a global TLB
            // invalidation may still be outstanding.  Leaking one failed
            // domain is safer than reusing the context bank or freeing page
            // tables that live DMA could still walk.
            if let Some(tables) = self.tables.lock().take() {
                core::mem::forget(tables);
            }
            early_println!(
                "[arm-smmu-v2] retaining translated CB {} and page tables after teardown or restore failure",
                self.context.index,
            );
        }
    }
}

fn validate_dma_range(
    aperture_base: Iova,
    aperture_size: u64,
    iova_address_limit: u64,
    iova: Iova,
    len: usize,
) -> Result<(), IommuError> {
    if aperture_size == 0
        || aperture_base & (PAGE_SIZE as u64 - 1) != 0
        || aperture_size & (PAGE_SIZE as u64 - 1) != 0
        || len == 0
        || iova & (PAGE_SIZE as u64 - 1) != 0
        || len & (PAGE_SIZE - 1) != 0
    {
        return Err(IommuError::MapFailed);
    }
    let aperture_end = aperture_base
        .checked_add(aperture_size)
        .ok_or(IommuError::MapFailed)?;
    let end = iova.checked_add(len as u64).ok_or(IommuError::MapFailed)?;
    if aperture_end > iova_address_limit || iova < aperture_base || end > aperture_end {
        return Err(IommuError::MapFailed);
    }
    Ok(())
}

fn required_iova_address_bits(aperture_base: Iova, aperture_size: u64) -> Result<u32, IommuError> {
    if aperture_size == 0 {
        return Err(IommuError::MapFailed);
    }
    let aperture_end = aperture_base
        .checked_add(aperture_size)
        .ok_or(IommuError::MapFailed)?;
    let maximum_iova = aperture_end.checked_sub(1).ok_or(IommuError::MapFailed)?;
    Ok((u64::BITS - maximum_iova.leading_zeros()).max(MIN_DMA_IOVA_BITS))
}

fn dma_permissions_valid(flags: IommuMapFlags) -> bool {
    flags.contains(IommuMapFlags::READ) || flags.contains(IommuMapFlags::WRITE)
}

struct ArmSmmuV2 {
    hardware: Arc<SmmuHardware>,
}

impl IommuController for ArmSmmuV2 {
    fn name(&self) -> &'static str {
        "arm-smmu-v2"
    }

    fn alloc_domain(&self, config: IommuDomainConfig) -> Result<Arc<dyn IommuDomain>, IommuError> {
        if config.domain_type == IommuDomainType::Dma {
            early_println!(
                "[arm-smmu-v2] domain alloc begin type=DMA iova={:#x} size={:#x} table-limit={:#x} output-limit={:#x}",
                config.iova_base,
                config.iova_size,
                self.hardware.table_address_limit,
                self.hardware.dma_output_address_limit,
            );
            if !self.hardware.dma_supported {
                return Err(IommuError::NotSupported);
            }
            let iova_address_bits = required_iova_address_bits(config.iova_base, config.iova_size)?;
            if iova_address_bits > self.hardware.dma_iova_address_bits {
                early_println!(
                    "[arm-smmu-v2] domain alloc failed stage=iova-width: requested={}b supported={}b",
                    iova_address_bits,
                    self.hardware.dma_iova_address_bits,
                );
                return Err(IommuError::MapFailed);
            }
            let iova_address_limit = address_limit(iova_address_bits);
            validate_dma_range(
                config.iova_base,
                config.iova_size,
                iova_address_limit,
                config.iova_base,
                PAGE_SIZE,
            )?;
            let tables = DmaPageTables::new(self.hardware.table_address_limit)?;
            let context = self.hardware.reserve_context()?;
            if let Err(error) =
                self.hardware
                    .configure_dma_context(&context, tables.root, iova_address_bits)
            {
                early_println!(
                    "[arm-smmu-v2] domain alloc failed stage=context-config CB {} root={:#x}: {:?}",
                    context.index,
                    tables.root,
                    error,
                );
                if !self.hardware.release_context(&context) {
                    early_println!(
                        "[arm-smmu-v2] domain alloc rollback quarantined CB {} and page tables",
                        context.index,
                    );
                    // A failed synchronization means hardware quiescence is
                    // unproven. Keep the translation tables alive along with
                    // the quarantined context bank.
                    core::mem::forget(tables);
                }
                return Err(error);
            }
            early_println!(
                "[arm-smmu-v2] domain alloc complete type=DMA CB {} root={:#x} iova={}b",
                context.index,
                tables.root,
                iova_address_bits,
            );
            return Ok(Arc::new(DmaDomain {
                hardware: Arc::clone(&self.hardware),
                context,
                iova_base: config.iova_base,
                iova_size: config.iova_size,
                iova_address_limit,
                output_address_limit: self.hardware.dma_output_address_limit,
                tables: IrqSpinLock::new(Some(tables)),
                streams: IrqSpinLock::new(BTreeMap::new()),
            }));
        }
        if config.domain_type != IommuDomainType::Identity {
            return Err(IommuError::NotSupported);
        }
        let context = match self.hardware.identity_routing {
            IdentityRouting::FirmwareUnmatchedBypass | IdentityRouting::DirectBypass => None,
            IdentityRouting::DisabledContext => Some(self.hardware.allocate_disabled_context()?),
        };
        Ok(Arc::new(IdentityDomain {
            hardware: Arc::clone(&self.hardware),
            context,
            streams: IrqSpinLock::new(BTreeMap::new()),
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
    let manager = DeviceManager::get_manager();
    let phandle = read_phandle(device)?;
    let mut enabled_clocks = EnabledClocks(Vec::new());
    if let Some(property) = device.property("clock-names") {
        let names = property
            .as_string_list()
            .ok_or("arm-smmu-v2: malformed clock-names")?;
        for name in names {
            let clock = match manager.resolve_clk(device, name) {
                Ok(clock) => clock,
                Err("clk: provider not found") | Err("clk: clock not found") => {
                    return probe_defer();
                }
                Err(error) => return Err(error),
            };
            if let Err(error) = clock.prepare_enable() {
                return match error {
                    crate::device::clk::ClkError::ProviderNotFound
                    | crate::device::clk::ClkError::ClockNotFound => probe_defer(),
                    _ => Err("arm-smmu-v2: failed to enable required clock"),
                };
            }
            enabled_clocks.0.push(clock);
        }
    }

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
    if resource_size < MINIMUM_REGISTER_WINDOW_SIZE {
        return Err("arm-smmu-v2: register resource is too small");
    }

    let mapping = MmioMapping {
        base: vm::ioremap(resource.start, resource_size)
            .map_err(|_| "arm-smmu-v2: ioremap failed")?,
    };
    let base = mapping.base;
    let registers = RegisterWindow::new(base);
    let id0 = registers.read(ID_REGISTER_0);
    let id1 = registers.read(ID_REGISTER_1);
    let id2 = registers.read(ID_REGISTER_2);
    if (id0 == 0 && id1 == 0 && id2 == 0) || (id0 == u32::MAX && id1 == u32::MAX && id2 == u32::MAX)
    {
        early_println!("[arm-smmu-v2] controller is not powered yet, deferring");
        return probe_defer();
    }
    let register_page_shift = if id1 & ID1_LARGE_REGISTER_PAGE != 0 {
        16
    } else {
        12
    };
    let context_page_base = 1usize << (((id1 & ID1_CONTEXT_PAGE_OFFSET) >> 28) as usize + 1);
    let stream_group_count = (id0 & ID0_STREAM_GROUP_COUNT) as usize;
    let stage1_context_start = ((id1 & ID1_STAGE2_CONTEXT_BANK_COUNT) >> 16) as usize;
    let context_bank_count = (id1 & ID1_CONTEXT_BANK_COUNT) as usize;
    let page_size = 1usize << register_page_shift;
    let stream_register_end = STREAM_CONTEXT_BASE
        .checked_add(stream_group_count.saturating_sub(1).saturating_mul(4))
        .and_then(|offset| offset.checked_add(core::mem::size_of::<u32>()))
        .ok_or("arm-smmu-v2: stream register range overflows")?;
    let context_attribute_end = page_size
        .checked_add(CONTEXT_ATTRIBUTE_2_BASE)
        .and_then(|offset| {
            offset.checked_add(context_bank_count.saturating_sub(1).saturating_mul(4))
        })
        .and_then(|offset| offset.checked_add(core::mem::size_of::<u32>()))
        .ok_or("arm-smmu-v2: context attribute range overflows")?;
    let context_register_end = context_page_base
        .checked_add(context_bank_count.saturating_sub(1))
        .and_then(|page| page.checked_mul(page_size))
        .and_then(|offset| offset.checked_add(CONTEXT_TLB_STATUS))
        .and_then(|offset| offset.checked_add(core::mem::size_of::<u32>()))
        .ok_or("arm-smmu-v2: context register range overflows")?;
    let required_register_size = stream_register_end
        .max(context_attribute_end)
        .max(context_register_end);
    if required_register_size > resource_size {
        return Err("arm-smmu-v2: ID registers describe a window larger than firmware resource");
    }
    let supports_stage1 = id0 & ID0_STAGE1_TRANSLATION != 0;
    let supports_4k = id2 & ID2_4K_PAGE_TABLE != 0;
    let virtual_address_size = (id2 & ID2_VIRTUAL_ADDRESS_SIZE) >> 8;
    let table_address_size = (id2 & ID2_OUTPUT_ADDRESS_SIZE) >> 4;
    let dma_output_address_size = id2 & ID2_INPUT_ADDRESS_SIZE;
    let virtual_address_bits = id_size_to_bits(virtual_address_size);
    let table_address_bits = id_size_to_bits(table_address_size);
    let dma_output_address_bits = id_size_to_bits(dma_output_address_size);
    let dma_iova_address_bits = virtual_address_bits.min(MAX_THREE_LEVEL_IOVA_BITS);
    let dma_supported = supports_stage1
        && supports_4k
        && dma_iova_address_bits >= MIN_DMA_IOVA_BITS
        && stage1_context_start < context_bank_count;

    early_println!(
        "[arm-smmu-v2] capabilities: id0={:#010x} id1={:#010x} id2={:#010x} S1={} 4K={} VA={}b DMA-VA={}b IPA={}b PA={}b NUMS2CB={} NUMCB={} DMA={}",
        id0,
        id1,
        id2,
        supports_stage1,
        supports_4k,
        virtual_address_bits,
        dma_iova_address_bits,
        dma_output_address_bits,
        table_address_bits,
        stage1_context_start,
        context_bank_count,
        dma_supported,
    );

    if stream_group_count == 0 || context_bank_count == 0 {
        return Err("arm-smmu-v2: invalid hardware capabilities");
    }

    let is_sc7180 = device.compatible().contains(&"qcom,sc7180-smmu-500");
    let identity_routing = if is_sc7180 && current_exception_level() >= 2 {
        // At EL2, avoid hypervisor-guarded S2CR writes. Enable the client
        // interface lazily and use the architectural unmatched-stream bypass
        // for streams without inherited firmware routing.
        IdentityRouting::FirmwareUnmatchedBypass
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
        stage1_context_start,
        context_bank_count,
        dma_supported,
        dma_iova_address_bits,
        dma_output_address_size: normalized_address_size(dma_output_address_size),
        dma_output_address_limit: address_limit(dma_output_address_bits),
        table_address_limit: address_limit(table_address_bits),
        identity_routing,
        _clocks: enabled_clocks,
        _mmio: mapping,
        lock: IrqSpinLock::new(()),
        allocated_contexts: IrqSpinLock::new(vec![false; context_bank_count]),
        claimed_streams: IrqSpinLock::new(BTreeMap::new()),
    });
    let controller = Arc::new(ArmSmmuV2 { hardware });
    manager.register_iommu_controller(phandle, controller as Arc<dyn IommuController>);

    early_println!(
        "[arm-smmu-v2] registered phandle={:#x} paddr={:#x} page={} SMRs={} CBs={} S1-CBs={} DMA={} identity={} gfsr={:#010x}",
        phandle,
        resource.start,
        1usize << register_page_shift,
        stream_group_count,
        context_bank_count,
        context_bank_count.saturating_sub(stage1_context_start),
        dma_supported,
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
        vec!["arm,mmu-500", "qcom,sc7180-smmu-500", "qcom,smmu-v2"],
    );
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical);
}

driver_initcall!(register_driver);

#[cfg(test)]
mod tests {
    use super::{
        CONTEXT_CONTROL_MMU_ENABLE, context_bank_is_available, dma_permissions_valid,
        expand_stream_mask, required_iova_address_bits, validate_dma_range,
    };
    use crate::{
        device::iommu::{IommuError, IommuMapFlags},
        environment::PAGE_SIZE,
    };

    #[test_case]
    fn expands_firmware_stream_mask() {
        let streams = expand_stream_mask(0x800, 0x2).expect("stream mask should decode");
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].id, 0x800);
        assert_eq!(streams[1].id, 0x802);
    }

    #[test_case]
    fn validates_programmed_dma_aperture_and_alignment() {
        assert_eq!(
            validate_dma_range(0, 1u64 << 32, 1u64 << 32, 0x54000, PAGE_SIZE),
            Ok(())
        );
        assert_eq!(
            validate_dma_range(0, 1u64 << 32, 1u64 << 32, 0x54001, PAGE_SIZE),
            Err(IommuError::MapFailed)
        );
        assert_eq!(
            validate_dma_range(0, 1u64 << 32, 1u64 << 32, 1u64 << 32, PAGE_SIZE,),
            Err(IommuError::MapFailed)
        );
        assert_eq!(
            validate_dma_range(1u64 << 32, 1u64 << 32, 1u64 << 33, 1u64 << 32, PAGE_SIZE,),
            Ok(())
        );
        assert_eq!(
            validate_dma_range(1u64 << 32, 1u64 << 32, 1u64 << 32, 1u64 << 32, PAGE_SIZE,),
            Err(IommuError::MapFailed)
        );
    }

    #[test_case]
    fn derives_gpu_iova_width_from_aperture_end() {
        assert_eq!(required_iova_address_bits(0, 1u64 << 32), Ok(32));
        assert_eq!(required_iova_address_bits(1u64 << 32, 1u64 << 32), Ok(33));
        assert_eq!(required_iova_address_bits(0, 0), Err(IommuError::MapFailed));
    }

    #[test_case]
    fn accepts_read_or_write_dma_permissions() {
        assert!(dma_permissions_valid(IommuMapFlags::READ));
        assert!(dma_permissions_valid(IommuMapFlags::WRITE));
        assert!(dma_permissions_valid(
            IommuMapFlags::WRITE | IommuMapFlags::COHERENT
        ));
        assert!(!dma_permissions_valid(IommuMapFlags::COHERENT));
    }

    #[test_case]
    fn context_lease_requires_software_and_hardware_ownership_to_be_free() {
        assert!(context_bank_is_available(false, false, 0));
        assert!(!context_bank_is_available(true, false, 0));
        assert!(!context_bank_is_available(false, true, 0));
        assert!(!context_bank_is_available(
            false,
            false,
            CONTEXT_CONTROL_MMU_ENABLE
        ));
    }
}
