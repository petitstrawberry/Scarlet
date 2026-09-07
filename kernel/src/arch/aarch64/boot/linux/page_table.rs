//! Allocator-independent AArch64 page tables for Linux Image entry.

use core::ptr::{read_volatile, write_volatile};

use crate::arch::aarch64::clean_dcache_to_poc_range;
use crate::environment::{PAGE_SIZE, SCARLET_HHDM_BASE};
use crate::vm::direct_map::DirectMapRegions;
use crate::vm::vmem::{MemoryArea, MemoryAttribute};

const ENTRY_COUNT: usize = 512;
const EARLY_TABLE_COUNT: usize = 512;
const PHYSICAL_ADDRESS_MASK: u64 = 0x0000_ffff_ffff_f000;
const TABLE_DESCRIPTOR: u64 = 0b11;
const BLOCK_DESCRIPTOR: u64 = 0b01;
const PAGE_DESCRIPTOR: u64 = 0b11;
const ACCESS_FLAG: u64 = 1 << 10;
const INNER_SHAREABLE: u64 = 0b11 << 8;
const UXN: u64 = 1 << 54;
const PXN: u64 = 1 << 53;

#[repr(C, align(4096))]
struct EarlyPageTablePool {
    tables: [[u64; ENTRY_COUNT]; EARLY_TABLE_COUNT],
    next: usize,
}

#[unsafe(link_section = ".bss.early_page_tables")]
static mut EARLY_PAGE_TABLES: EarlyPageTablePool = EarlyPageTablePool {
    tables: [[0; ENTRY_COUNT]; EARLY_TABLE_COUNT],
    next: 0,
};

/// Installs the temporary identity and higher-half mappings.
///
/// The linked kernel receives an executable identity alias so the bootstrap can
/// continue at its physical link address. The DTB and optional initramfs receive
/// NX identity aliases until they are relocated. Every direct-map region also
/// receives an NX HHDM alias.
///
/// # Arguments
///
/// * `regions` - Sparse, attribute-aware physical regions needed in the HHDM.
/// * `kernel_area` - Linked kernel image, including boot stack and early tables.
/// * `dtb_area` - Original firmware DTB read until relocation.
/// * `initramfs_area` - Optional original initramfs read until relocation.
///
/// # Returns
///
/// `Ok(())` after the MMU is active, or an error when the fixed page-table
/// pool cannot represent the supplied map.
pub fn install(
    regions: &DirectMapRegions,
    kernel_area: MemoryArea,
    dtb_area: MemoryArea,
    initramfs_area: Option<MemoryArea>,
) -> Result<(), &'static str> {
    // SAFETY: Linux Image entry is single-threaded, .bss has been cleared, and
    // no CPU can observe these tables until activate_early_boot_page_table().
    unsafe {
        reset_pool();
        let root = allocate_table()?;

        map_identity_area(root, kernel_area, true)?;
        map_identity_area(root, dtb_area, false)?;
        if let Some(area) = initramfs_area {
            map_identity_area(root, area, false)?;
        }

        for index in 0..regions.len() {
            let region = regions
                .get(index)
                .ok_or("early direct-map region index is invalid")?;
            let area = region.area();
            let size = area.size();

            let hhdm_start = SCARLET_HHDM_BASE
                .checked_add(area.start)
                .ok_or("early HHDM virtual address overflows")?;
            map_range(
                root,
                hhdm_start,
                area.start,
                size,
                region.memory_attribute(),
                false,
            )?;
        }

        clean_allocated_tables();
        crate::arch::aarch64::vm::mmu::activate_early_boot_page_table(root);
    }

    Ok(())
}

unsafe fn map_identity_area(
    root: usize,
    area: MemoryArea,
    executable: bool,
) -> Result<(), &'static str> {
    let start = area.start & !(PAGE_SIZE - 1);
    let end_exclusive = area
        .end
        .checked_add(1)
        .and_then(|value| value.checked_add(PAGE_SIZE - 1))
        .map(|value| value & !(PAGE_SIZE - 1))
        .ok_or("early identity range overflows")?;
    let size = end_exclusive
        .checked_sub(start)
        .ok_or("early identity range is invalid")?;

    // SAFETY: The caller exclusively owns root and the range is page-aligned.
    unsafe {
        map_range(
            root,
            start,
            start,
            size,
            MemoryAttribute::Normal,
            executable,
        )
    }
}

unsafe fn reset_pool() {
    let pool = &raw mut EARLY_PAGE_TABLES;
    // SAFETY: The caller owns the pool during single-CPU early boot.
    unsafe {
        core::ptr::write_bytes(
            core::ptr::addr_of_mut!((*pool).tables).cast::<u8>(),
            0,
            core::mem::size_of_val(&(*pool).tables),
        );
        (*pool).next = 0;
    }
}

unsafe fn allocate_table() -> Result<usize, &'static str> {
    let pool = &raw mut EARLY_PAGE_TABLES;
    // SAFETY: Allocation is serialized by the single boot CPU.
    unsafe {
        let index = (*pool).next;
        if index >= EARLY_TABLE_COUNT {
            return Err("early AArch64 page-table pool exhausted");
        }
        (*pool).next = index + 1;
        Ok(core::ptr::addr_of_mut!((*pool).tables[index]) as usize)
    }
}

unsafe fn clean_allocated_tables() {
    let pool = &raw const EARLY_PAGE_TABLES;
    // SAFETY: The pool remains exclusively owned and fully initialized here.
    unsafe {
        for index in 0..(*pool).next {
            clean_dcache_to_poc_range(
                core::ptr::addr_of!((*pool).tables[index]) as usize,
                PAGE_SIZE,
            );
        }
    }
}

unsafe fn map_range(
    root: usize,
    mut vaddr: usize,
    mut paddr: usize,
    mut size: usize,
    memory_attribute: MemoryAttribute,
    executable: bool,
) -> Result<(), &'static str> {
    if vaddr & (PAGE_SIZE - 1) != 0
        || paddr & (PAGE_SIZE - 1) != 0
        || size == 0
        || size & (PAGE_SIZE - 1) != 0
    {
        return Err("early AArch64 mapping is not page aligned");
    }

    while size != 0 {
        let level = best_level(vaddr, paddr, size);
        let chunk_size = level_size(level);
        // SAFETY: root belongs to the exclusively owned static table pool and
        // the range has been validated above.
        unsafe {
            map_leaf(root, vaddr, paddr, level, memory_attribute, executable)?;
        }
        vaddr = vaddr
            .checked_add(chunk_size)
            .ok_or("early mapping virtual address overflows")?;
        paddr = paddr
            .checked_add(chunk_size)
            .ok_or("early mapping physical address overflows")?;
        size -= chunk_size;
    }

    Ok(())
}

fn best_level(vaddr: usize, paddr: usize, size: usize) -> usize {
    for level in [2usize, 1] {
        let block_size = level_size(level);
        if size >= block_size
            && vaddr.is_multiple_of(block_size)
            && paddr.is_multiple_of(block_size)
        {
            return level;
        }
    }
    3
}

const fn level_size(level: usize) -> usize {
    match level {
        1 => 1 << 30,
        2 => 1 << 21,
        _ => 1 << 12,
    }
}

unsafe fn map_leaf(
    root: usize,
    vaddr: usize,
    paddr: usize,
    target_level: usize,
    memory_attribute: MemoryAttribute,
    executable: bool,
) -> Result<(), &'static str> {
    let mut table = root;
    for level in 0..target_level {
        let shift = 39 - level * 9;
        let index = (vaddr >> shift) & 0x1ff;
        let entry_ptr = (table as *mut u64).wrapping_add(index);
        // SAFETY: table points into the page-aligned static table pool.
        let entry = unsafe { read_volatile(entry_ptr) };
        if entry & 1 != 0 {
            if entry & 0b11 != TABLE_DESCRIPTOR {
                return Err("early mapping collides with a block descriptor");
            }
            table = (entry & PHYSICAL_ADDRESS_MASK) as usize;
            continue;
        }

        // SAFETY: Allocation and entry mutation are serialized by the boot CPU.
        let child = unsafe { allocate_table()? };
        unsafe {
            write_volatile(
                entry_ptr,
                (child as u64 & PHYSICAL_ADDRESS_MASK) | TABLE_DESCRIPTOR,
            );
        }
        table = child;
    }

    let shift = 39 - target_level * 9;
    let index = (vaddr >> shift) & 0x1ff;
    let entry_ptr = (table as *mut u64).wrapping_add(index);
    let descriptor = leaf_descriptor(paddr, target_level, memory_attribute, executable);
    // SAFETY: entry_ptr addresses the selected leaf in the static table pool.
    let old = unsafe { read_volatile(entry_ptr) };
    if old != 0 && old != descriptor {
        return Err("early mapping overlaps an incompatible leaf");
    }
    unsafe {
        write_volatile(entry_ptr, descriptor);
    }
    Ok(())
}

fn leaf_descriptor(
    paddr: usize,
    level: usize,
    memory_attribute: MemoryAttribute,
    executable: bool,
) -> u64 {
    let attr_index = match memory_attribute {
        MemoryAttribute::Normal => 0,
        MemoryAttribute::NonCacheable => 2,
        MemoryAttribute::DeviceBurstable => 3,
        MemoryAttribute::Device => 4,
    };
    let shareability = match memory_attribute {
        MemoryAttribute::Normal | MemoryAttribute::NonCacheable => INNER_SHAREABLE,
        MemoryAttribute::DeviceBurstable | MemoryAttribute::Device => 0,
    };
    let descriptor_type = if level == 3 {
        PAGE_DESCRIPTOR
    } else {
        BLOCK_DESCRIPTOR
    };
    let execute_never = UXN | if executable { 0 } else { PXN };

    (paddr as u64 & PHYSICAL_ADDRESS_MASK)
        | descriptor_type
        | ((attr_index as u64) << 2)
        | shareability
        | ACCESS_FLAG
        | execute_never
}
