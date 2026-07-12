#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::{
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
    early_println,
    environment::PAGE_SIZE,
    mem::page::ContiguousPages,
    time,
};

const DART_PARAMS1: usize = 0x00;
const DART_PARAMS2: usize = 0x04;
const DART_TCR: usize = 0x100;
const DART_TTBR: usize = 0x200;
const DART_TTBR_COUNT: usize = 4;
const DART_ENABLE_STREAMS: usize = 0xfc;
const DART_DISABLE_STREAMS: usize = 0xfd;
const DART_STREAM_COMMAND: usize = 0x20;
const DART_STREAM_SELECT: usize = 0x34;
const DART_ERROR_STATUS: usize = 0x40;

const DART_TCR_TRANSLATE_ENABLE: u32 = 1 << 7;
const DART_TCR_BYPASS_DART: u32 = 1 << 8;
const DART_TCR_BYPASS_DAPF: u32 = 1 << 12;

const DART_TTBR_VALID: u32 = 1 << 31;

const DART_PARAMS1_SHIFT: u32 = 24;
const DART_PARAMS1_MASK: u32 = 0xf << DART_PARAMS1_SHIFT;

const DART_STREAM_COMMAND_BUSY: u32 = 1 << 2;
const DART_STREAM_COMMAND_BUSY_TIMEOUT_US: usize = 100;

const DART_PAGE_SHIFT: usize = 14;
const DART_IAS_BITS: usize = 32;
const DART_IAS_MASK: usize = (1usize << DART_IAS_BITS) - 1;
const DART_PADDR_BITS: usize = 36;
const DART_PADDR_FIELD_SHIFT: usize = 12;
const DART_PTE_SIZE_SHIFT: usize = 3;

const DART_PTE_SUBPAGE_END_MASK: u64 = 0xfff << 40;
const DART_PTE_SUBPAGE_ALLOW_ALL: u64 = DART_PTE_SUBPAGE_END_MASK;

const DART_PTE_VALID: u64 = 1 << 0;
const DART_PTE_SP_DIS: u64 = 1 << 1;
const DART_PTE_NO_WRITE: u64 = 1 << 7;
const DART_PTE_NO_READ: u64 = 1 << 8;

const DART_PADDR_MASK: u64 =
    ((1u64 << DART_PADDR_BITS) - 1) & !((1u64 << DART_PADDR_FIELD_SHIFT) - 1);

const DART_STREAM_COMMAND_INV_ALL: u32 = 1 << 20;

const fn dart_ttbr_offset(sid: usize, index: usize) -> usize {
    DART_TTBR + (sid * DART_TTBR_COUNT + index) * core::mem::size_of::<u32>()
}

/// Apple DART IOMMU hardware instance.
#[derive(Clone)]
pub struct DartInstance {
    base_addr: usize,
    params: DartParams,
}

#[derive(Clone)]
struct DartParams {
    page_shift: u32,
    supports_bypass: bool,
    num_streams: u32,
}

impl DartInstance {
    /// Create a DART instance from a mapped MMIO base address.
    ///
    /// # Arguments
    ///
    /// * `base_addr` - Kernel virtual address of the DART MMIO region.
    ///
    /// # Returns
    ///
    /// Initialized DART instance with hardware parameters decoded.
    pub fn new(base_addr: usize) -> Self {
        let raw_params1 = unsafe { mmio::read32(base_addr + DART_PARAMS1) };
        let raw_params2 = unsafe { mmio::read32(base_addr + DART_PARAMS2) };

        let page_shift = (raw_params1 & DART_PARAMS1_MASK) >> DART_PARAMS1_SHIFT;
        let supports_bypass = raw_params2 & 1 != 0;

        let sid_width = ((raw_params1 >> 20) & 0xf) as u32;
        let num_streams = if sid_width > 0 && sid_width <= 8 {
            1u32 << sid_width
        } else {
            16
        };

        Self {
            base_addr,
            params: DartParams {
                page_shift,
                supports_bypass,
                num_streams,
            },
        }
    }

    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.base_addr + offset) }
    }

    #[inline]
    fn write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.base_addr + offset, val) }
    }

    fn invalidate_all_tlbs(&self) -> Result<(), IommuError> {
        arch::io_wmb();
        self.write32(DART_STREAM_SELECT, u32::MAX);
        self.write32(DART_STREAM_COMMAND, DART_STREAM_COMMAND_INV_ALL);
        for _ in 0..DART_STREAM_COMMAND_BUSY_TIMEOUT_US {
            if self.read32(DART_STREAM_COMMAND) & DART_STREAM_COMMAND_BUSY == 0 {
                return Ok(());
            }
            time::udelay(1);
        }
        Err(IommuError::Busy)
    }

    fn enable_streams(&self) {
        self.write32(DART_ENABLE_STREAMS, u32::MAX);
    }

    fn set_tcr(&self, sid: usize, val: u32) {
        self.write32(DART_TCR + sid * 4, val);
    }

    fn set_ttbr(&self, sid: usize, paddr: usize) {
        let val = (paddr >> 12) as u32 | DART_TTBR_VALID;
        self.write32(dart_ttbr_offset(sid, 0), val);
    }

    /// Enable translated DMA for a stream ID.
    ///
    /// # Arguments
    ///
    /// * `sid` - Stream ID to configure.
    /// * `ttbr_paddr` - Physical address of the root page table.
    /// * `num_levels` - Number of page table levels used by the domain.
    pub fn enable_translation(&self, sid: usize, ttbr_paddr: usize, num_levels: u32) {
        self.enable_streams();
        self.set_ttbr(sid, ttbr_paddr);
        let _ = num_levels;
        self.set_tcr(sid, DART_TCR_TRANSLATE_ENABLE);
        let _ = self.invalidate_all_tlbs();
    }

    /// Publish page-table updates and invalidate cached translations.
    ///
    /// Unlike [`Self::enable_translation`], this preserves the TTBR and TCR
    /// installed by firmware. This is required for locked DART streams.
    ///
    /// # Returns
    ///
    /// `Ok(())` after invalidation completes, or an IOMMU error on timeout.
    pub fn sync_page_tables(&self) -> Result<(), IommuError> {
        self.invalidate_all_tlbs()
    }

    /// Read the physical address of the root page table currently installed
    /// for a stream ID, or `None` when the TTBR is not valid.
    ///
    /// The TTBR register survives a `disable_translation` call — only the
    /// TCR is cleared — so this can be used to recover the page table that
    /// was installed by firmware or a previous boot stage.
    ///
    /// # Arguments
    ///
    /// * `sid` - Stream ID whose TTBR to read.
    ///
    /// # Returns
    ///
    /// Physical address of the L1 root page table, or `None` when TTBR
    /// is not valid.
    pub fn ttbr_paddr(&self, sid: usize) -> Option<usize> {
        let ttbr = self.read32(dart_ttbr_offset(sid, 0));
        if ttbr & DART_TTBR_VALID == 0 {
            return None;
        }
        let paddr_high = (ttbr & !DART_TTBR_VALID) as usize;
        Some(paddr_high << 12)
    }

    /// Disable translated DMA for a stream ID.
    ///
    /// # Arguments
    ///
    /// * `sid` - Stream ID to disable.
    pub fn disable_translation(&self, sid: usize) {
        self.set_tcr(sid, 0);
    }

    /// Enable hardware bypass mode for a stream ID when supported.
    ///
    /// # Arguments
    ///
    /// * `sid` - Stream ID to configure for bypass.
    pub fn enable_bypass(&self, sid: usize) {
        self.enable_streams();
        if !self.params.supports_bypass {
            self.enable_translation(sid, 0, 3);
            return;
        }
        let tcr = DART_TCR_BYPASS_DART | DART_TCR_BYPASS_DAPF;
        self.set_tcr(sid, tcr);
    }

    /// Return the hardware page shift reported by the DART.
    ///
    /// # Returns
    ///
    /// Page size shift value from the DART parameters register.
    pub fn page_shift(&self) -> u32 {
        self.params.page_shift
    }

    fn page_size_exceeds_cpu_page_size(&self) -> bool {
        (1usize << self.params.page_shift) > PAGE_SIZE
    }
}

impl IommuController for DartInstance {
    fn name(&self) -> &'static str {
        "apple-dart"
    }

    fn alloc_domain(&self, config: IommuDomainConfig) -> Result<Arc<dyn IommuDomain>, IommuError> {
        if self.params.supports_bypass
            && (matches!(config.domain_type, IommuDomainType::Identity)
                || self.page_size_exceeds_cpu_page_size())
        {
            return Ok(Arc::new(DartBypassDomain::new(Arc::new(self.clone()))));
        }
        if matches!(config.domain_type, IommuDomainType::Identity) {
            return Err(IommuError::NotSupported);
        }

        Ok(Arc::new(DartDomain::new(Arc::new(self.clone()))?))
    }

    fn stream_ids_from_fdt(
        &self,
        spec: &IommuSpec,
    ) -> Result<alloc::vec::Vec<IommuStreamId>, IommuError> {
        if spec.cells.is_empty() {
            return Err(IommuError::InvalidSpec);
        }

        if spec.cells.len() > 1 {
            early_println!(
                "[apple-dart] iommus spec for phandle {:#x} has {} cells; using first SID cell",
                spec.controller_phandle,
                spec.cells.len()
            );
        }

        Ok(alloc::vec![IommuStreamId {
            id: spec.cells[0],
            substream_id: None,
        }])
    }
}

struct DartBypassDomain {
    dart: Arc<DartInstance>,
    attached_streams: Mutex<alloc::vec::Vec<u32>>,
}

impl DartBypassDomain {
    fn new(dart: Arc<DartInstance>) -> Self {
        Self {
            dart,
            attached_streams: Mutex::new(alloc::vec::Vec::new()),
        }
    }

    fn validate_stream(&self, stream: IommuStreamId) -> Result<usize, IommuError> {
        if stream.substream_id.is_some() {
            return Err(IommuError::NotSupported);
        }

        if stream.id >= self.dart.params.num_streams {
            return Err(IommuError::AttachFailed);
        }

        Ok(stream.id as usize)
    }
}

impl IommuDomain for DartBypassDomain {
    fn attach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
        let sid = self.validate_stream(stream)?;
        self.dart.enable_bypass(sid);

        let mut attached_streams = self.attached_streams.lock();
        if !attached_streams.contains(&(sid as u32)) {
            attached_streams.push(sid as u32);
        }

        Ok(())
    }

    fn detach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
        let sid = self.validate_stream(stream)?;
        self.dart.disable_translation(sid);

        let mut attached_streams = self.attached_streams.lock();
        attached_streams.retain(|attached_sid| *attached_sid != sid as u32);

        Ok(())
    }

    fn map(
        &self,
        _iova: Iova,
        _paddr: PhysAddr,
        _len: usize,
        _flags: IommuMapFlags,
    ) -> Result<(), IommuError> {
        Ok(())
    }

    fn unmap(&self, _iova: Iova, _len: usize) -> Result<(), IommuError> {
        Ok(())
    }

    fn iova_to_phys(&self, iova: Iova) -> Option<PhysAddr> {
        Some(iova as PhysAddr)
    }

    fn page_size(&self) -> usize {
        PAGE_SIZE
    }

    fn flush(&self) -> Result<(), IommuError> {
        Ok(())
    }
}

/// Apple DART translation domain backed by an owned or firmware page table.
pub struct DartDomain {
    dart: Arc<DartInstance>,
    page_table: Mutex<DartPageTable>,
    attached_streams: Mutex<alloc::vec::Vec<u32>>,
    firmware_stream: Option<u32>,
}

impl DartDomain {
    /// Create a new DART translation domain.
    ///
    /// # Arguments
    ///
    /// * `dart` - DART hardware instance that will own the domain.
    ///
    /// # Returns
    ///
    /// A domain with a fresh root page table.
    pub fn new(dart: Arc<DartInstance>) -> Result<Self, IommuError> {
        let page_table = DartPageTable::new_for_page_shift(dart.page_shift())
            .map_err(|_| IommuError::DomainAllocationFailed)?;

        Ok(Self {
            dart,
            page_table: Mutex::new(page_table),
            attached_streams: Mutex::new(alloc::vec::Vec::new()),
            firmware_stream: None,
        })
    }

    /// Wrap the page table already installed for a firmware-owned stream.
    ///
    /// The existing TTBR is read from the DART and its physical root is
    /// accessed through Scarlet's runtime direct map. The TTBR and TCR are
    /// preserved; later mapping operations only update PTEs and invalidate
    /// cached translations, matching Asahi Linux's locked-DART path.
    ///
    /// # Arguments
    ///
    /// * `dart` - DART hardware instance containing the firmware stream.
    /// * `stream` - Firmware-owned stream whose existing TTBR should be used.
    ///
    /// # Returns
    ///
    /// A domain backed by the existing page table, or an IOMMU error when the
    /// stream is invalid or has no valid TTBR.
    pub fn wrap_existing(
        dart: Arc<DartInstance>,
        stream: IommuStreamId,
    ) -> Result<Self, IommuError> {
        if stream.substream_id.is_some() {
            return Err(IommuError::NotSupported);
        }
        if stream.id >= dart.params.num_streams {
            return Err(IommuError::AttachFailed);
        }

        let root_paddr = dart
            .ttbr_paddr(stream.id as usize)
            .ok_or(IommuError::DomainAllocationFailed)?;
        let page_table = DartPageTable::wrap_existing(root_paddr, dart.page_shift())
            .map_err(|_| IommuError::DomainAllocationFailed)?;

        Ok(Self {
            dart,
            page_table: Mutex::new(page_table),
            attached_streams: Mutex::new(alloc::vec![stream.id]),
            firmware_stream: Some(stream.id),
        })
    }

    fn validate_stream(&self, stream: IommuStreamId) -> Result<usize, IommuError> {
        if stream.substream_id.is_some() {
            return Err(IommuError::NotSupported);
        }

        if stream.id >= self.dart.params.num_streams {
            return Err(IommuError::AttachFailed);
        }

        Ok(stream.id as usize)
    }
}

impl IommuDomain for DartDomain {
    fn attach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
        let sid = self.validate_stream(stream)?;
        if let Some(firmware_stream) = self.firmware_stream {
            if stream.id != firmware_stream {
                return Err(IommuError::AttachFailed);
            }
            let mut attached_streams = self.attached_streams.lock();
            if !attached_streams.contains(&stream.id) {
                attached_streams.push(stream.id);
            }
            return Ok(());
        }

        let page_table = self.page_table.lock();
        let root_paddr = page_table.root_paddr();
        let levels = page_table.translation_levels();

        self.dart.enable_translation(sid, root_paddr, levels);

        let mut attached_streams = self.attached_streams.lock();
        if !attached_streams.contains(&(sid as u32)) {
            attached_streams.push(sid as u32);
        }

        Ok(())
    }

    fn detach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
        let sid = self.validate_stream(stream)?;
        if self.firmware_stream.is_none() {
            self.dart.disable_translation(sid);
        }

        let mut attached_streams = self.attached_streams.lock();
        attached_streams.retain(|attached_sid| *attached_sid != sid as u32);

        Ok(())
    }

    fn map(
        &self,
        iova: Iova,
        paddr: PhysAddr,
        len: usize,
        flags: IommuMapFlags,
    ) -> Result<(), IommuError> {
        self.page_table
            .lock()
            .map_contiguous(
                (iova as usize) & DART_IAS_MASK,
                paddr,
                len,
                dart_pte_flags(flags),
            )
            .map_err(|_| IommuError::MapFailed)?;
        self.flush()
    }

    fn unmap(&self, iova: Iova, len: usize) -> Result<(), IommuError> {
        let page_size = self.page_size();
        let pages = len.div_ceil(page_size);
        let mut page_table = self.page_table.lock();
        let iova = (iova as usize) & DART_IAS_MASK;

        for page in 0..pages {
            page_table.unmap_page(iova + page * page_size);
        }

        drop(page_table);
        self.flush()
    }

    fn iova_to_phys(&self, _iova: Iova) -> Option<PhysAddr> {
        None
    }

    fn page_size(&self) -> usize {
        self.page_table.lock().page_size()
    }

    fn flush(&self) -> Result<(), IommuError> {
        self.dart.sync_page_tables()
    }
}

fn dart_pte_flags(flags: IommuMapFlags) -> u64 {
    let mut pte = DART_PTE_VALID | DART_PTE_SP_DIS;
    if !flags.contains(IommuMapFlags::READ) {
        pte |= DART_PTE_NO_READ;
    }
    if !flags.contains(IommuMapFlags::WRITE) {
        pte |= DART_PTE_NO_WRITE;
    }
    pte
}

/// Apple DART three-level page table.
pub struct DartPageTable {
    root_vaddr: usize,
    root_paddr: usize,
    tables: Vec<ContiguousPages>,
    page_shift: usize,
    bits_per_level: usize,
    table_levels: usize,
    pte_count: usize,
    table_size: usize,
}

impl DartPageTable {
    /// Allocate and zero a new root page table.
    ///
    /// # Returns
    ///
    /// A new page table, or an error if physical page allocation fails.
    pub fn new() -> Result<Self, &'static str> {
        Self::new_for_page_shift(DART_PAGE_SHIFT as u32)
    }

    /// Allocate and zero a new root page table for a DART page size.
    ///
    /// # Arguments
    ///
    /// * `page_shift` - Hardware DART page shift reported by PARAMS1.
    ///
    /// # Returns
    ///
    /// A new page table, or an error if physical page allocation fails.
    pub fn new_for_page_shift(page_shift: u32) -> Result<Self, &'static str> {
        let page_shift = page_shift as usize;
        if page_shift < DART_PTE_SIZE_SHIFT || page_shift < PAGE_SIZE.trailing_zeros() as usize {
            return Err("dart: invalid page shift");
        }

        let bits_per_level = page_shift - DART_PTE_SIZE_SHIFT;
        if bits_per_level == 0 || DART_IAS_BITS <= page_shift {
            return Err("dart: invalid page table geometry");
        }

        let table_size = 1usize
            .checked_shl(page_shift as u32)
            .ok_or("dart: page table size overflow")?;
        let pte_count = table_size / core::mem::size_of::<u64>();
        let iova_index_bits = DART_IAS_BITS - page_shift;
        let table_levels = core::cmp::max(2, iova_index_bits.div_ceil(bits_per_level));
        let root = Self::allocate_table(table_size)?;
        let root_vaddr = root.as_vaddr();
        let root_paddr = root.as_paddr();
        core::mem::forget(root);

        Ok(Self {
            root_vaddr,
            root_paddr,
            tables: Vec::new(),
            page_shift,
            bits_per_level,
            table_levels,
            pte_count,
            table_size,
        })
    }

    /// Wrap a pre-existing root page table installed by firmware.
    ///
    /// The root table at `root_paddr` must be reachable through the kernel
    /// runtime direct map. New leaf and intermediate entries are added
    /// in-place, matching Asahi Linux's locked-DART page-table handling.
    ///
    /// # Arguments
    ///
    /// * `root_paddr` - Physical address of the existing L1 root table.
    /// * `page_shift` - Hardware DART page shift reported by PARAMS1.
    ///
    /// # Returns
    ///
    /// A page table that reads and writes through the existing root.
    pub fn wrap_existing(root_paddr: usize, page_shift: u32) -> Result<Self, &'static str> {
        let page_shift = page_shift as usize;
        if page_shift < DART_PTE_SIZE_SHIFT || page_shift < PAGE_SIZE.trailing_zeros() as usize {
            return Err("dart: invalid page shift");
        }

        let bits_per_level = page_shift - DART_PTE_SIZE_SHIFT;
        if bits_per_level == 0 || DART_IAS_BITS <= page_shift {
            return Err("dart: invalid page table geometry");
        }

        let table_size = 1usize
            .checked_shl(page_shift as u32)
            .ok_or("dart: page table size overflow")?;
        let pte_count = table_size / core::mem::size_of::<u64>();
        let iova_index_bits = DART_IAS_BITS - page_shift;
        let table_levels = core::cmp::max(2, iova_index_bits.div_ceil(bits_per_level));

        Ok(Self {
            root_vaddr: scarlet::vm::phys_to_virt(root_paddr),
            root_paddr,
            tables: Vec::new(),
            page_shift,
            bits_per_level,
            table_levels,
            pte_count,
            table_size,
        })
    }

    fn allocate_table(table_size: usize) -> Result<ContiguousPages, &'static str> {
        let pages = table_size.div_ceil(PAGE_SIZE);
        ContiguousPages::new_aligned(pages, table_size).ok_or("dart: failed to allocate page table")
    }

    fn read_pte(table_vaddr: usize, index: usize) -> u64 {
        unsafe { core::ptr::read_volatile((table_vaddr + index * 8) as *const u64) }
    }

    fn write_pte(table_vaddr: usize, index: usize, pte: u64) {
        unsafe {
            core::ptr::write_volatile((table_vaddr + index * 8) as *mut u64, pte);
        }
    }

    fn paddr_to_pte(paddr: usize) -> u64 {
        paddr as u64 & DART_PADDR_MASK
    }

    fn pte_to_paddr(pte: u64) -> usize {
        (pte & DART_PADDR_MASK) as usize
    }

    fn table_index(&self, iova: usize, level: usize) -> usize {
        (iova >> (self.page_shift + level * self.bits_per_level)) & (self.pte_count - 1)
    }

    fn leaf_index(&self, iova: usize) -> usize {
        (iova >> self.page_shift) & (self.pte_count - 1)
    }

    /// Map one DART page.
    ///
    /// # Arguments
    ///
    /// * `iova` - I/O virtual address to map.
    /// * `paddr` - Physical address backing the mapping.
    /// * `flags` - DART PTE flags to apply.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the mapping is installed.
    pub fn map_page(&mut self, iova: usize, paddr: usize, flags: u64) -> Result<(), &'static str> {
        let mut table_vaddr = self.root_vaddr;
        for level in (1..self.table_levels).rev() {
            let index = self.table_index(iova, level);
            let pte = Self::read_pte(table_vaddr, index);

            if pte & DART_PTE_VALID == 0 {
                let table = Self::allocate_table(self.table_size)?;
                let next_vaddr = table.as_vaddr();
                let next_paddr = table.as_paddr();
                let table_pte = Self::paddr_to_pte(next_paddr) | DART_PTE_VALID;
                self.tables.push(table);
                Self::write_pte(table_vaddr, index, table_pte);
                table_vaddr = next_vaddr;
            } else {
                let next_table_paddr = Self::pte_to_paddr(pte);
                table_vaddr = scarlet::vm::phys_to_virt(next_table_paddr);
            }
        }

        let leaf_index = self.leaf_index(iova);
        let leaf_pte = Self::paddr_to_pte(paddr) | DART_PTE_SUBPAGE_ALLOW_ALL | flags;
        Self::write_pte(table_vaddr, leaf_index, leaf_pte);
        Ok(())
    }

    /// Unmap one DART page if present.
    ///
    /// # Arguments
    ///
    /// * `iova` - I/O virtual address to unmap.
    pub fn unmap_page(&mut self, iova: usize) {
        let mut table_vaddr = self.root_vaddr;
        for level in (1..self.table_levels).rev() {
            let index = self.table_index(iova, level);
            let pte = Self::read_pte(table_vaddr, index);
            if pte & DART_PTE_VALID == 0 {
                return;
            }
            let next_table_paddr = Self::pte_to_paddr(pte);
            table_vaddr = scarlet::vm::phys_to_virt(next_table_paddr);
        }
        let leaf_index = self.leaf_index(iova);
        Self::write_pte(table_vaddr, leaf_index, 0);
    }

    /// Resolve an IOVA through the current DART page table.
    ///
    /// # Arguments
    ///
    /// * `iova` - Device virtual address to resolve.
    ///
    /// # Returns
    ///
    /// The mapped physical address, including the page offset, when present.
    pub fn translate_iova(&self, iova: usize) -> Option<usize> {
        let mut table_vaddr = self.root_vaddr;
        for level in (1..self.table_levels).rev() {
            let pte = Self::read_pte(table_vaddr, self.table_index(iova, level));
            if pte & DART_PTE_VALID == 0 {
                return None;
            }
            table_vaddr = scarlet::vm::phys_to_virt(Self::pte_to_paddr(pte));
        }

        let pte = Self::read_pte(table_vaddr, self.leaf_index(iova));
        if pte & DART_PTE_VALID == 0 {
            return None;
        }
        Some(Self::pte_to_paddr(pte) | (iova & (self.page_size() - 1)))
    }

    /// Return the physical address of the root page table.
    ///
    /// # Returns
    ///
    /// Root page table physical address.
    pub fn root_paddr(&self) -> usize {
        self.root_paddr
    }

    /// Return the page size used by this DART page table.
    ///
    /// # Returns
    ///
    /// DART mapping granule in bytes.
    pub fn page_size(&self) -> usize {
        1usize << self.page_shift
    }

    /// Return the number of translation levels expected by the DART TCR.
    ///
    /// # Returns
    ///
    /// Table depth including the top TTBR-selected level.
    pub fn translation_levels(&self) -> u32 {
        (self.table_levels + 1) as u32
    }

    /// Map a contiguous IOVA range to contiguous physical pages.
    ///
    /// # Arguments
    ///
    /// * `iova` - Start I/O virtual address.
    /// * `paddr` - Start physical address.
    /// * `size` - Mapping size in bytes.
    /// * `flags` - DART PTE flags to apply.
    ///
    /// # Returns
    ///
    /// `Ok(())` when every page is mapped.
    pub fn map_contiguous(
        &mut self,
        iova: usize,
        paddr: usize,
        size: usize,
        flags: u64,
    ) -> Result<(), &'static str> {
        let page_size = self.page_size();
        let pages = size.div_ceil(page_size);
        for i in 0..pages {
            self.map_page(iova + i * page_size, paddr + i * page_size, flags)?;
        }
        Ok(())
    }

    /// Import all valid leaf mappings from a pre-existing page table.
    ///
    /// Walks every level of the page table rooted at `root_paddr` and
    /// recreates each valid leaf PTE inside this [`DartPageTable`]. This
    /// is used to carry forward firmware/bootloader DART mappings (e.g.
    /// coprocessor firmware segments) when replacing the TTBR with a
    /// kernel-owned page table.
    ///
    /// # Arguments
    ///
    /// * `root_paddr` - Physical address of the existing L1 root table.
    ///
    /// # Returns
    ///
    /// Number of leaf entries that were imported.
    pub fn import_from_existing(&mut self, root_paddr: usize) -> Result<usize, &'static str> {
        let mut imported = 0;
        self.import_level(root_paddr, self.table_levels, 0, &mut imported)?;
        Ok(imported)
    }

    fn import_level(
        &mut self,
        table_paddr: usize,
        level: usize,
        iova_base: usize,
        imported: &mut usize,
    ) -> Result<(), &'static str> {
        let table_vaddr = scarlet::vm::phys_to_virt(table_paddr);

        let shift = if level <= 1 {
            self.page_shift
        } else {
            self.page_shift + (level - 1) * self.bits_per_level
        };

        for i in 0..self.pte_count {
            let pte = Self::read_pte(table_vaddr, i);
            if pte & DART_PTE_VALID == 0 {
                continue;
            }

            let iova = iova_base | (i << shift);

            if level <= 1 {
                let leaf_paddr = Self::pte_to_paddr(pte);
                let leaf_flags = pte & !DART_PADDR_MASK;
                self.map_page(iova, leaf_paddr, leaf_flags)?;
                *imported += 1;
            } else {
                let next_paddr = Self::pte_to_paddr(pte);
                self.import_level(next_paddr, level - 1, iova, imported)?;
            }
        }
        Ok(())
    }
}

struct DartEntry {
    instance: Arc<DartInstance>,
    phandle: u32,
}

static DART_REGISTRY: Mutex<alloc::vec::Vec<DartEntry>> = Mutex::new(alloc::vec::Vec::new());

/// Register a DART instance in the legacy Apple DART registry.
///
/// # Arguments
///
/// * `instance` - DART hardware instance to register.
/// * `phandle` - Firmware phandle for this DART node.
///
/// # Returns
///
/// Numeric legacy registry identifier assigned to the DART instance.
pub fn register_dart(instance: DartInstance, phandle: u32) -> u32 {
    let mut guard = DART_REGISTRY.lock();
    let id = guard.len() as u32;
    guard.push(DartEntry {
        instance: Arc::new(instance),
        phandle,
    });
    id
}

/// Look up a DART instance by legacy registry identifier.
///
/// # Arguments
///
/// * `id` - Legacy DART registry identifier.
///
/// # Returns
///
/// Registered DART instance, or `None` if the identifier is unknown.
pub fn get_dart(id: u32) -> Option<Arc<DartInstance>> {
    let guard = DART_REGISTRY.lock();
    guard.get(id as usize).map(|e| Arc::clone(&e.instance))
}

/// Look up a DART instance by firmware phandle.
///
/// # Arguments
///
/// * `phandle` - Firmware phandle for a DART node.
///
/// # Returns
///
/// Registered DART instance, or `None` if no DART uses `phandle`.
pub fn get_dart_by_phandle(phandle: u32) -> Option<Arc<DartInstance>> {
    let guard = DART_REGISTRY.lock();
    guard
        .iter()
        .find(|e| e.phandle == phandle)
        .map(|e| Arc::clone(&e.instance))
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resource = device
        .get_resources()
        .iter()
        .find(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-dart: no memory resource found")?;

    let paddr = mem_resource.start;
    let size = mem_resource.end - mem_resource.start + 1;

    early_println!(
        "[apple-dart] probing {} at paddr={:#x}, size={:#x}",
        device.name(),
        paddr,
        size
    );

    let base_addr = scarlet::vm::ioremap(paddr, size).map_err(|_| "dart: ioremap failed")?;
    let dart = DartInstance::new(base_addr);

    early_println!(
        "[apple-dart] page_shift={}, bypass={}",
        dart.params.page_shift,
        dart.params.supports_bypass
    );

    for sid in 0..dart.params.num_streams as usize {
        let ttbr = dart.read32(dart_ttbr_offset(sid, 0));
        if ttbr & DART_TTBR_VALID != 0 {
            continue;
        }
        dart.disable_translation(sid);
    }
    dart.enable_streams();

    dart.invalidate_all_tlbs()
        .map_err(|_| "apple-dart: timed out invalidating TLBs during probe")?;
    dart.write32(DART_ERROR_STATUS, 0xffff_ffff);

    let phandle = device
        .property("phandle")
        .and_then(|p| p.as_usize())
        .map(|v| v as u32)
        .or_else(|| {
            device
                .property("linux,phandle")
                .and_then(|p| p.as_usize())
                .map(|v| v as u32)
        })
        .unwrap_or(0);

    let id = register_dart(dart, phandle);
    if let Some(controller) = get_dart(id) {
        DeviceManager::get_manager().register_iommu_controller(phandle, controller);
    }
    early_println!("[apple-dart] initialized (id={})", id);
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_dart_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-dart",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-dart",
            "apple,dart",
            "apple,t6000-dart",
            "apple,t8112-dart",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_dart_driver);

#[used]
static SCARLET_DRIVER_APPLE_DART_ANCHOR: fn() = force_link;

#[inline(never)]
/// Force linker retention for the Apple DART driver crate.
pub fn force_link() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_stream_ids_from_fdt_single_cell() {
        let dart = DartInstance {
            base_addr: 0,
            params: DartParams {
                page_shift: DART_PAGE_SHIFT as u32,
                supports_bypass: false,
                num_streams: 256,
            },
        };
        let spec = IommuSpec {
            controller_phandle: 0x40,
            cells: alloc::vec![0x17],
        };

        assert_eq!(
            dart.stream_ids_from_fdt(&spec).unwrap(),
            alloc::vec![IommuStreamId {
                id: 0x17,
                substream_id: None,
            }]
        );
    }

    #[test_case]
    fn test_stream_ids_from_fdt_multi_cell_uses_first() {
        let dart = DartInstance {
            base_addr: 0,
            params: DartParams {
                page_shift: DART_PAGE_SHIFT as u32,
                supports_bypass: false,
                num_streams: 256,
            },
        };
        let spec = IommuSpec {
            controller_phandle: 0x40,
            cells: alloc::vec![0x22, 0x33],
        };

        assert_eq!(dart.stream_ids_from_fdt(&spec).unwrap()[0].id, 0x22);
    }

    #[test_case]
    fn test_dart_pte_flags_encode_permissions() {
        let flags = IommuMapFlags::READ | IommuMapFlags::WRITE | IommuMapFlags::COHERENT;

        assert_eq!(dart_pte_flags(flags), DART_PTE_VALID | DART_PTE_SP_DIS);
        assert_eq!(
            dart_pte_flags(IommuMapFlags::READ),
            DART_PTE_VALID | DART_PTE_SP_DIS | DART_PTE_NO_WRITE
        );
        assert_eq!(
            dart_pte_flags(IommuMapFlags::WRITE),
            DART_PTE_VALID | DART_PTE_SP_DIS | DART_PTE_NO_READ
        );
    }

    #[test_case]
    fn test_dart_pte_preserves_4k_aligned_physical_address() {
        let paddr = 0x800b_1000usize;
        let pte = DartPageTable::paddr_to_pte(paddr) | DART_PTE_VALID;

        assert_eq!(DartPageTable::pte_to_paddr(pte), paddr);
    }
}
