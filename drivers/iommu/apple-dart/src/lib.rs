#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use scarlet::sync::Mutex;

use scarlet::{
    arch::mmio,
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
    mem::pmm,
};

const DART_PARAMS1: usize = 0x00;
const DART_PARAMS2: usize = 0x04;
const DART_TCR: usize = 0x100;
const DART_TTBR: usize = 0x200;
const DART_ENABLE_STREAMS: usize = 0xfc;
const DART_DISABLE_STREAMS: usize = 0xfd;
const DART_STREAM_COMMAND: usize = 0x20;
const DART_ERROR_STATUS: usize = 0x40;

const DART_TCR_TRANSLATE_ENABLE: u32 = 1 << 7;
const DART_TCR_BYPASS_DART: u32 = 1 << 8;
const DART_TCR_BYPASS_DAPF: u32 = 1 << 12;

const DART_TTBR_VALID: u32 = 1 << 31;

const DART_PARAMS1_SHIFT: u32 = 24;
const DART_PARAMS1_MASK: u32 = 0xf << DART_PARAMS1_SHIFT;

const DART_STREAM_COMMAND_BUSY: u32 = 1 << 2;

const DART_PAGE_SHIFT: usize = 14;
const DART_PTE_COUNT: usize = 512;
const DART_TABLE_SIZE: usize = DART_PTE_COUNT * 8;

const DART_PTE_SUBPAGE_END_MASK: u64 = 0xfff << 40;
const DART_PTE_SUBPAGE_ALLOW_ALL: u64 = DART_PTE_SUBPAGE_END_MASK;

const DART_PTE_VALID: u64 = 1 << 0;
const DART_PTE_SP_DIS: u64 = 1 << 1;
const DART_PTE_NO_WRITE: u64 = 1 << 7;
const DART_PTE_NO_READ: u64 = 1 << 8;

const DART_PADDR_MASK: u64 = ((1u64 << 36) - 1) & !((1u64 << DART_PAGE_SHIFT) - 1);

const DART_STREAM_COMMAND_INV_ALL: u32 = 0;

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

    fn invalidate_all_tlbs(&self) {
        self.write32(DART_STREAM_COMMAND, DART_STREAM_COMMAND_INV_ALL);
        while self.read32(DART_STREAM_COMMAND) & DART_STREAM_COMMAND_BUSY != 0 {
            core::hint::spin_loop();
        }
    }

    fn enable_streams(&self, sid: usize) {
        self.write32(DART_ENABLE_STREAMS, sid as u32);
    }

    fn set_tcr(&self, sid: usize, val: u32) {
        self.write32(DART_TCR + sid * 4, val);
    }

    fn set_ttbr(&self, sid: usize, paddr: usize) {
        let val = (paddr >> 12) as u32 | DART_TTBR_VALID;
        self.write32(DART_TTBR + sid * 4, val);
    }

    /// Enable translated DMA for a stream ID.
    ///
    /// # Arguments
    ///
    /// * `sid` - Stream ID to configure.
    /// * `ttbr_paddr` - Physical address of the root page table.
    /// * `num_levels` - Number of page table levels used by the domain.
    pub fn enable_translation(&self, sid: usize, ttbr_paddr: usize, num_levels: u32) {
        self.set_ttbr(sid, ttbr_paddr);

        let level_bits = match num_levels {
            3 => 0b10,
            4 => 0b11,
            _ => 0b10,
        };
        let tcr = DART_TCR_TRANSLATE_ENABLE | level_bits;
        self.set_tcr(sid, tcr);
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

    fn flush(&self) -> Result<(), IommuError> {
        Ok(())
    }
}

/// Apple DART translation domain backed by a private page table.
pub struct DartDomain {
    dart: Arc<DartInstance>,
    page_table: Mutex<DartPageTable>,
    attached_streams: Mutex<alloc::vec::Vec<u32>>,
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
        let page_table = DartPageTable::new().map_err(|_| IommuError::DomainAllocationFailed)?;

        Ok(Self {
            dart,
            page_table: Mutex::new(page_table),
            attached_streams: Mutex::new(alloc::vec::Vec::new()),
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
        let root_paddr = self.page_table.lock().root_paddr();

        self.dart.enable_translation(sid, root_paddr, 3);

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
        iova: Iova,
        paddr: PhysAddr,
        len: usize,
        flags: IommuMapFlags,
    ) -> Result<(), IommuError> {
        self.page_table
            .lock()
            .map_contiguous(iova as usize, paddr, len, dart_pte_flags(flags))
            .map_err(|_| IommuError::MapFailed)?;
        self.flush()
    }

    fn unmap(&self, iova: Iova, len: usize) -> Result<(), IommuError> {
        let page_size = 1usize << DART_PAGE_SHIFT;
        let pages = len.div_ceil(page_size);
        let mut page_table = self.page_table.lock();

        for page in 0..pages {
            page_table.unmap_page(iova as usize + page * page_size);
        }

        drop(page_table);
        self.flush()
    }

    fn iova_to_phys(&self, _iova: Iova) -> Option<PhysAddr> {
        None
    }

    fn flush(&self) -> Result<(), IommuError> {
        self.dart.invalidate_all_tlbs();
        Ok(())
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
    root_paddr: usize,
    root_vaddr: usize,
}

impl DartPageTable {
    /// Allocate and zero a new root page table.
    ///
    /// # Returns
    ///
    /// A new page table, or an error if physical page allocation fails.
    pub fn new() -> Result<Self, &'static str> {
        let root_paddr = pmm::alloc_frame().ok_or("dart: failed to allocate root page table")?;
        let root_vaddr = scarlet::vm::phys_to_virt(root_paddr);
        unsafe {
            core::ptr::write_bytes(root_vaddr as *mut u8, 0, DART_TABLE_SIZE);
        }
        Ok(Self {
            root_paddr,
            root_vaddr,
        })
    }

    fn read_pte(table_vaddr: usize, index: usize) -> u64 {
        unsafe { core::ptr::read_volatile((table_vaddr + index * 8) as *const u64) }
    }

    fn write_pte(table_vaddr: usize, index: usize, pte: u64) {
        unsafe {
            core::ptr::write_volatile((table_vaddr + index * 8) as *mut u64, pte);
        }
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

        for level in 0..2 {
            let shift = DART_PAGE_SHIFT + (2 - level) * 9;
            let index = (iova >> shift) & (DART_PTE_COUNT - 1);

            let pte = Self::read_pte(table_vaddr, index);

            if level < 2 {
                if pte & DART_PTE_VALID == 0 {
                    let mid_paddr =
                        pmm::alloc_frame().ok_or("dart: failed to allocate page table")?;
                    let mid_vaddr = scarlet::vm::phys_to_virt(mid_paddr);
                    unsafe {
                        core::ptr::write_bytes(mid_vaddr as *mut u8, 0, DART_TABLE_SIZE);
                    }
                    // Format must match the read side below and the leaf PTE write:
                    // paddr bits live in DART_PADDR_MASK (bits 12..35), not as a
                    // frame number in the low bits. Using `>> DART_PAGE_SHIFT` here
                    // caused phys_to_virt() to receive a garbage address on the
                    // next map_page() that hit the same root entry (issue #480).
                    let mid_pte = (mid_paddr as u64 & DART_PADDR_MASK) | DART_PTE_VALID;
                    Self::write_pte(table_vaddr, index, mid_pte);
                    table_vaddr = mid_vaddr;
                } else {
                    let next_table_paddr =
                        (((pte & DART_PADDR_MASK) >> DART_PAGE_SHIFT) as usize) << DART_PAGE_SHIFT;
                    table_vaddr = scarlet::vm::phys_to_virt(next_table_paddr);
                }
            }
        }

        let leaf_index = (iova >> DART_PAGE_SHIFT) & (DART_PTE_COUNT - 1);
        let leaf_pte = (paddr as u64 & DART_PADDR_MASK) | DART_PTE_SUBPAGE_ALLOW_ALL | flags;
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
        for level in 0..2 {
            let shift = DART_PAGE_SHIFT + (2 - level) * 9;
            let index = (iova >> shift) & (DART_PTE_COUNT - 1);
            let pte = Self::read_pte(table_vaddr, index);
            if pte & DART_PTE_VALID == 0 {
                return;
            }
            if level < 2 {
                let next_table_paddr =
                    (((pte & DART_PADDR_MASK) >> DART_PAGE_SHIFT) as usize) << DART_PAGE_SHIFT;
                table_vaddr = scarlet::vm::phys_to_virt(next_table_paddr);
            }
        }
        let leaf_index = (iova >> DART_PAGE_SHIFT) & (DART_PTE_COUNT - 1);
        Self::write_pte(table_vaddr, leaf_index, 0);
    }

    /// Return the physical address of the root page table.
    ///
    /// # Returns
    ///
    /// Root page table physical address.
    pub fn root_paddr(&self) -> usize {
        self.root_paddr
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
        let page_size = 1usize << DART_PAGE_SHIFT;
        let pages = size.div_ceil(page_size);
        for i in 0..pages {
            self.map_page(iova + i * page_size, paddr + i * page_size, flags)?;
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
        dart.disable_translation(sid);
    }
    for sid in 0..dart.params.num_streams as usize {
        dart.enable_streams(sid);
    }

    dart.invalidate_all_tlbs();
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
}
