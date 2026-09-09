//! Translation through the kernel's published boot and runtime mappings.
//!
//! Layouts are immutable after one-time publication. The phase flag selects a
//! complete layout; physical addresses never require a machine-wide atomic.
//! These helpers describe mappings, not allocation ownership or access rights.

use core::sync::atomic::{AtomicU8, Ordering};

use crate::sync::{IrqSpinLock, IrqSpinLockGuard, Once};
use crate::vm::direct_map::{DirectMapRegions, DirectMapWindow};
use crate::vm::vmem::{MemoryAttribute, PhysicalMemoryArea};

pub use crate::mem::address::{PhysAddr, VirtAddr};

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KernelMemoryPhase {
    Uninitialized = 0,
    Bootloader = 1,
    BootKernel = 2,
    Runtime = 3,
}

/// A contiguous kernel image or heap mapping, with checked inclusive bounds.
#[derive(Clone, Copy, Debug)]
struct KernelMapping {
    phys_base: PhysAddr,
    virt_base: VirtAddr,
    size: usize,
}

impl KernelMapping {
    fn new(phys_base: PhysAddr, virt_base: VirtAddr, size: usize) -> Self {
        let last = size.checked_sub(1).expect("kernel mapping is empty");
        phys_base
            .checked_add(last as u64)
            .expect("kernel physical mapping overflows");
        virt_base
            .checked_add(last)
            .expect("kernel virtual mapping overflows");
        Self {
            phys_base,
            virt_base,
            size,
        }
    }

    fn virt_to_phys(self, vaddr: VirtAddr) -> Option<PhysAddr> {
        let offset = vaddr.checked_offset_from(self.virt_base)?;
        if offset >= self.size {
            return None;
        }
        self.phys_base.checked_add(offset as u64)
    }

    fn phys_to_virt(self, paddr: PhysAddr) -> Option<VirtAddr> {
        let offset = usize::try_from(paddr.checked_offset_from(self.phys_base)?).ok()?;
        if offset >= self.size {
            return None;
        }
        self.virt_base.checked_add(offset)
    }

    fn overlaps_virtual_range(self, base: VirtAddr, size: usize) -> bool {
        let last = base.checked_add(size - 1).expect("validated mapping");
        let own_last = self
            .virt_base
            .checked_add(self.size - 1)
            .expect("validated mapping");
        self.virt_base <= last && base <= own_last
    }
}

#[derive(Debug)]
struct BootLayout {
    kernel_image: KernelMapping,
    direct_map: DirectMapWindow,
}

/// Validated runtime mappings, prepared before changing page tables.
/// Fields are private so the handoff consumes the layout that was checked.
pub struct KernelRuntimeLayout {
    direct_map: DirectMapWindow,
    regions: IrqSpinLock<DirectMapRegions>,
    heap: KernelMapping,
}

struct KernelMemoryLayout {
    phase: AtomicU8,
    boot: Once<BootLayout>,
    runtime: Once<KernelRuntimeLayout>,
}

impl KernelMemoryLayout {
    const fn new() -> Self {
        Self {
            phase: AtomicU8::new(0),
            boot: Once::new(),
            runtime: Once::new(),
        }
    }

    fn phase(&self) -> KernelMemoryPhase {
        match self.phase.load(Ordering::Acquire) {
            0 => KernelMemoryPhase::Uninitialized,
            1 => KernelMemoryPhase::Bootloader,
            2 => KernelMemoryPhase::BootKernel,
            3 => KernelMemoryPhase::Runtime,
            _ => unreachable!("invalid kernel memory phase"),
        }
    }

    fn init_from_boot(&self, direct_map: DirectMapWindow, kernel_image: KernelMapping) {
        assert_eq!(self.phase(), KernelMemoryPhase::Uninitialized);
        self.boot
            .set(BootLayout {
                kernel_image,
                direct_map,
            })
            .expect("boot layout already published");
        self.phase
            .store(KernelMemoryPhase::Bootloader as u8, Ordering::Release);
    }

    fn prepare_runtime(
        &self,
        direct_map: DirectMapWindow,
        regions: DirectMapRegions,
        heap: KernelMapping,
    ) -> KernelRuntimeLayout {
        assert_eq!(self.phase(), KernelMemoryPhase::Bootloader);
        let bounds = regions
            .bounding_area()
            .expect("runtime direct map is empty");
        assert!(
            direct_map
                .phys_to_virt(PhysAddr::new(bounds.start))
                .is_some()
                && direct_map.phys_to_virt(PhysAddr::new(bounds.end)).is_some(),
            "runtime regions exceed the direct-map window"
        );
        let image = self.boot().kernel_image;
        assert!(
            !image.overlaps_virtual_range(direct_map.virtual_base(), direct_map.size()),
            "kernel image overlaps the direct-map virtual window"
        );
        assert!(
            !heap.overlaps_virtual_range(direct_map.virtual_base(), direct_map.size()),
            "kernel heap overlaps the direct-map virtual window"
        );
        assert!(
            !image.overlaps_virtual_range(heap.virt_base, heap.size),
            "kernel image overlaps the kernel heap"
        );
        let heap_area = PhysicalMemoryArea::new(
            heap.phys_base.as_u64(),
            heap.phys_base
                .checked_add(heap.size as u64 - 1)
                .unwrap()
                .as_u64(),
        );
        assert!(
            regions.contains_area_with_attribute(heap_area, MemoryAttribute::Normal),
            "kernel heap is not backed by direct-mapped RAM"
        );
        KernelRuntimeLayout {
            direct_map,
            regions: IrqSpinLock::new(regions),
            heap,
        }
    }

    fn transition_to_boot_kernel(&self, runtime: KernelRuntimeLayout) {
        assert_eq!(self.phase(), KernelMemoryPhase::Bootloader);
        assert!(
            self.runtime.set(runtime).is_ok(),
            "runtime layout already published"
        );
        self.phase
            .store(KernelMemoryPhase::BootKernel as u8, Ordering::Release);
    }

    fn finalize_runtime(&self) {
        self.phase
            .compare_exchange(
                KernelMemoryPhase::BootKernel as u8,
                KernelMemoryPhase::Runtime as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .expect("runtime layout must be installed before finalization");
    }

    fn boot(&self) -> &BootLayout {
        self.boot.get().expect("boot layout not initialized")
    }

    fn current_runtime(&self) -> Option<&KernelRuntimeLayout> {
        match self.phase() {
            KernelMemoryPhase::BootKernel | KernelMemoryPhase::Runtime => {
                Some(self.runtime.get().expect("runtime layout not initialized"))
            }
            _ => None,
        }
    }

    fn current_direct_map(&self) -> (DirectMapWindow, Option<&IrqSpinLock<DirectMapRegions>>) {
        match self.current_runtime() {
            Some(runtime) => (runtime.direct_map, Some(&runtime.regions)),
            None => (self.boot().direct_map, None),
        }
    }

    fn direct_map_phys(&self, vaddr: VirtAddr) -> Option<PhysAddr> {
        let (window, regions) = self.current_direct_map();
        let paddr = window.virt_to_phys(vaddr)?;
        if regions.is_some_and(|regions| !regions.lock().contains(paddr.as_u64())) {
            return None;
        }
        Some(paddr)
    }

    fn phys_to_current_virt(&self, paddr: PhysAddr) -> VirtAddr {
        let (window, regions) = self.current_direct_map();
        assert!(
            !regions.is_some_and(|regions| !regions.lock().contains(paddr.as_u64())),
            "physical address {:#x} is outside the sparse direct map",
            paddr
        );
        window
            .phys_to_virt(paddr)
            .expect("physical address is outside the direct-map window")
    }

    fn virt_to_current_phys(&self, vaddr: VirtAddr) -> Option<PhysAddr> {
        if let Some(paddr) = self.boot().kernel_image.virt_to_phys(vaddr) {
            return Some(paddr);
        }
        if let Some(paddr) = self
            .current_runtime()
            .and_then(|runtime| runtime.heap.virt_to_phys(vaddr))
        {
            return Some(paddr);
        }
        self.direct_map_phys(vaddr)
    }

    fn virt_to_boot_phys(&self, vaddr: VirtAddr) -> Option<PhysAddr> {
        self.boot()
            .kernel_image
            .virt_to_phys(vaddr)
            .or_else(|| self.boot().direct_map.virt_to_phys(vaddr))
    }
}

static KERNEL_MEMORY_LAYOUT: KernelMemoryLayout = KernelMemoryLayout::new();

fn layout() -> &'static KernelMemoryLayout {
    &KERNEL_MEMORY_LAYOUT
}

/// Publish the complete boot mapping before starting other CPUs or translating
/// addresses. The boot adapter must already have installed matching page tables.
/// This is a one-time operation; it never changes page tables itself.
pub fn init_boot_addressing(
    direct_map: DirectMapWindow,
    kernel_phys_base: PhysAddr,
    kernel_virt_base: VirtAddr,
    kernel_image_size: usize,
) {
    layout().init_from_boot(
        direct_map,
        KernelMapping::new(kernel_phys_base, kernel_virt_base, kernel_image_size),
    );
}

#[inline(always)]
pub fn address_translation_ready() -> bool {
    layout().phase() != KernelMemoryPhase::Uninitialized
}

/// Validate and prepare runtime metadata before the page-table handoff.
/// Rejects overlapping VA windows and regions the direct map cannot represent.
pub fn prepare_kernel_memory_layout(
    direct_map: DirectMapWindow,
    direct_map_regions: DirectMapRegions,
    heap_phys_base: PhysAddr,
    heap_virt_base: VirtAddr,
    heap_size: usize,
) -> KernelRuntimeLayout {
    layout().prepare_runtime(
        direct_map,
        direct_map_regions,
        KernelMapping::new(heap_phys_base, heap_virt_base, heap_size),
    )
}

/// Publish the prepared layout after the BSP page-table switch and pointer
/// relocation. The boot path serializes this one-time handoff; secondary CPUs
/// retain explicit boot helpers until they switch their own page tables.
pub fn transition_kernel_memory_layout(runtime: KernelRuntimeLayout) {
    layout().transition_to_boot_kernel(runtime);
}

pub fn finalize_runtime_memory_layout() {
    layout().finalize_runtime();
}

/// Broad inclusive physical bounds; sparse holes are not evidence of mapped RAM.
pub fn get_current_direct_map_phys_range() -> (u64, u64) {
    let area = match layout().current_runtime() {
        Some(runtime) => runtime
            .regions
            .lock()
            .bounding_area()
            .expect("runtime direct map is empty"),
        None => layout().boot().direct_map.physical_area(),
    };
    (area.start, area.end)
}

/// The original mapping is retained even after runtime publication.
pub fn get_boot_direct_map() -> DirectMapWindow {
    layout().boot().direct_map
}

/// Address arithmetic only; use sparse membership checks for mapped RAM.
pub fn get_current_direct_map() -> DirectMapWindow {
    layout().current_direct_map().0
}

pub fn runtime_direct_map_regions() -> Option<DirectMapRegions> {
    layout()
        .current_runtime()
        .map(|runtime| *runtime.regions.lock())
}

pub(crate) fn lock_runtime_direct_map_regions()
-> Result<IrqSpinLockGuard<'static, DirectMapRegions>, &'static str> {
    layout()
        .current_runtime()
        .map(|runtime| runtime.regions.lock())
        .ok_or("runtime direct-map regions not initialized")
}

/// Validate cache-attribute compatibility once the runtime sparse map is active.
pub fn validate_direct_map_alias(
    area: PhysicalMemoryArea,
    memory_attribute: MemoryAttribute,
) -> Result<(), &'static str> {
    if let Some(regions) = runtime_direct_map_regions() {
        regions.validate_alias(area, memory_attribute)
    } else {
        Ok(())
    }
}

pub fn get_heap_phys_layout() -> Option<(u64, usize, usize)> {
    layout().current_runtime().map(|runtime| {
        let heap = runtime.heap;
        (
            heap.phys_base.as_u64(),
            heap.virt_base.as_usize(),
            heap.size,
        )
    })
}

/// Translate with the boot mapping, even if another CPU published runtime state.
#[inline(always)]
pub fn boot_phys_to_virt(paddr: u64) -> usize {
    layout()
        .boot()
        .direct_map
        .phys_to_virt(PhysAddr::new(paddr))
        .expect("physical address is outside the boot direct map")
        .as_usize()
}

/// Translate a recorded kernel image, heap or current direct-map address.
/// This does not walk user or IOREMAP page tables.
#[inline(always)]
#[track_caller]
pub fn virt_to_phys(vaddr: usize) -> u64 {
    layout()
        .virt_to_current_phys(VirtAddr::new(vaddr))
        .unwrap_or_else(|| panic!("virt_to_phys: unmapped kernel virtual address {:#x}", vaddr))
        .as_u64()
}

/// Translate a boot-protocol pointer using retained boot metadata.
#[inline(always)]
#[track_caller]
pub fn boot_virt_to_phys(vaddr: usize) -> u64 {
    layout()
        .virt_to_boot_phys(VirtAddr::new(vaddr))
        .unwrap_or_else(|| {
            panic!(
                "boot_virt_to_phys: unmapped boot virtual address {:#x}",
                vaddr
            )
        })
        .as_u64()
}

/// Return the current direct-map address; panics for unmapped physical memory.
#[inline(always)]
pub fn phys_to_virt(paddr: u64) -> usize {
    layout()
        .phys_to_current_virt(PhysAddr::new(paddr))
        .as_usize()
}

#[inline(always)]
pub fn phys_to_kernel_virt(paddr: u64) -> usize {
    phys_to_virt(paddr)
}

#[inline(always)]
pub fn kernel_virt_to_phys(vaddr: usize) -> u64 {
    virt_to_phys(vaddr)
}

#[inline(always)]
pub fn phys_to_kernel_image_virt(paddr: u64) -> usize {
    layout()
        .boot()
        .kernel_image
        .phys_to_virt(PhysAddr::new(paddr))
        .expect("physical address is outside the kernel image")
        .as_usize()
}

#[inline(always)]
pub fn is_direct_mapped(vaddr: usize) -> bool {
    layout().direct_map_phys(VirtAddr::new(vaddr)).is_some()
}

impl PhysAddr {
    /// Translate through the current direct map; panics for unmapped memory.
    pub fn to_virt(self) -> VirtAddr {
        layout().phys_to_current_virt(self)
    }
}

impl VirtAddr {
    /// Translate a kernel image, heap or direct-map address.
    pub fn to_phys(self) -> PhysAddr {
        layout()
            .virt_to_current_phys(self)
            .expect("unmapped kernel virtual address")
    }
}

/// Calculate a position in the published runtime window, including explicit
/// initramfs mappings outside the sparse RAM set. Does not validate membership.
pub(crate) fn kernel_direct_map_vaddr(paddr: u64) -> usize {
    layout()
        .current_runtime()
        .expect("runtime layout is not active")
        .direct_map
        .phys_to_virt(PhysAddr::new(paddr))
        .expect("physical address exceeds the runtime window")
        .as_usize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn immutable_layout_handoff_preserves_boot_mapping_and_high_physical_bits() {
        let layout = KernelMemoryLayout::new();
        let base = PhysAddr::new(0x1_8000_0000);
        let boot = DirectMapWindow::new(base, VirtAddr::new(0x8000_0000), 0x10000).unwrap();
        let image = KernelMapping::new(
            PhysAddr::new(0x1_8001_0000),
            VirtAddr::new(0xb000_0000),
            0x1000,
        );
        layout.init_from_boot(boot, image);
        assert_eq!(
            layout.phys_to_current_virt(base),
            VirtAddr::new(0x8000_0000)
        );
        let runtime = DirectMapWindow::new(base, VirtAddr::new(0xc000_0000), 0x10000).unwrap();
        let mut regions = DirectMapRegions::new();
        regions
            .insert(
                PhysicalMemoryArea::new(base.as_u64(), base.as_u64() + 0x1fff),
                MemoryAttribute::Normal,
            )
            .unwrap();
        regions
            .insert(
                PhysicalMemoryArea::new(base.as_u64() + 0x3000, base.as_u64() + 0xffff),
                MemoryAttribute::Normal,
            )
            .unwrap();
        let heap = KernelMapping::new(base, VirtAddr::new(0x9000_0000), 0x1000);
        let prepared = layout.prepare_runtime(runtime, regions, heap);
        // Preparation does not select mappings that are not yet installed.
        assert_eq!(
            layout.phys_to_current_virt(base),
            VirtAddr::new(0x8000_0000)
        );
        layout.transition_to_boot_kernel(prepared);
        assert_eq!(
            layout.phys_to_current_virt(base),
            VirtAddr::new(0xc000_0000)
        );
        assert_eq!(
            layout.virt_to_current_phys(VirtAddr::new(0xc000_0001)),
            base.checked_add(1)
        );
        assert_eq!(
            layout.virt_to_current_phys(VirtAddr::new(0x9000_0001)),
            base.checked_add(1)
        );
        assert_eq!(
            layout.virt_to_current_phys(VirtAddr::new(0xb000_0001)),
            Some(PhysAddr::new(0x1_8001_0001))
        );
        assert_eq!(
            layout.virt_to_current_phys(VirtAddr::new(0x8000_0001)),
            None
        );
        assert_eq!(
            layout.virt_to_current_phys(VirtAddr::new(0xc000_2000)),
            None
        );
        assert_eq!(
            layout.virt_to_boot_phys(VirtAddr::new(0x8000_0001)),
            base.checked_add(1)
        );
        layout.finalize_runtime();
        assert_eq!(layout.boot().direct_map, boot);
        assert_eq!(layout.phase(), KernelMemoryPhase::Runtime);
    }
}
