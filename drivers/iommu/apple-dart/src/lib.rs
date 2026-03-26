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
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
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

const DART_PAGE_SHIFT: usize = 12;
const DART_PTE_COUNT: usize = 512;
const DART_TABLE_SIZE: usize = DART_PTE_COUNT * 8;

const DART_PTE_VALID: u64 = 1 << 0;
const DART_PTE_SP_DIS: u64 = 1 << 1;

const DART_PADDR_MASK: u64 = ((1u64 << 36) - 1) & !((1u64 << DART_PAGE_SHIFT) - 1);

const DART_STREAM_COMMAND_INV_ALL: u32 = 0;

pub struct DartInstance {
    base_addr: usize,
    params: DartParams,
}

struct DartParams {
    page_shift: u32,
    supports_bypass: bool,
    num_streams: u32,
}

impl DartInstance {
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

    pub fn disable_translation(&self, sid: usize) {
        self.set_tcr(sid, 0);
    }

    pub fn enable_bypass(&self, sid: usize) {
        if !self.params.supports_bypass {
            self.enable_translation(sid, 0, 3);
            return;
        }
        let tcr = DART_TCR_BYPASS_DART | DART_TCR_BYPASS_DAPF;
        self.set_tcr(sid, tcr);
    }

    pub fn page_shift(&self) -> u32 {
        self.params.page_shift
    }
}

pub struct DartPageTable {
    root_paddr: usize,
    root_vaddr: usize,
}

impl DartPageTable {
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
                    let mid_pte = ((mid_paddr >> DART_PAGE_SHIFT) as u64) | DART_PTE_VALID;
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
        let leaf_pte = (paddr as u64 & DART_PADDR_MASK) | DART_PTE_SP_DIS | flags;
        Self::write_pte(table_vaddr, leaf_index, leaf_pte);
        Ok(())
    }

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

    pub fn root_paddr(&self) -> usize {
        self.root_paddr
    }

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

pub fn register_dart(instance: DartInstance, phandle: u32) -> u32 {
    let mut guard = DART_REGISTRY.lock();
    let id = guard.len() as u32;
    guard.push(DartEntry {
        instance: Arc::new(instance),
        phandle,
    });
    id
}

pub fn get_dart(id: u32) -> Option<Arc<DartInstance>> {
    let guard = DART_REGISTRY.lock();
    guard.get(id as usize).map(|e| Arc::clone(&e.instance))
}

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
pub fn force_link() {}
