//! Boot-owned Sv32 tables. These are retained for secondary-hart entry.

use crate::environment::{KERNEL_DIRECT_MAP_SIZE, PAGE_SIZE};
use crate::vm::addr::virt_to_phys;
use crate::vm::direct_map::{DirectMapRegions, DirectMapWindow};

const ENTRIES: usize = 1024;
const SUPERPAGE: usize = PAGE_SIZE * ENTRIES;
// A misaligned physical direct-map origin can require one table for every
// superpage in the virtual window. This bound covers that entire window.
const TABLES: usize = KERNEL_DIRECT_MAP_SIZE / SUPERPAGE;

#[repr(C, align(4096))]
pub(super) struct Table([u32; ENTRIES]);

// Entry assembly writes this before init_bss; keep it outside the zeroed BSS.
#[unsafe(link_section = ".data.boot_tables")]
#[unsafe(no_mangle)]
static mut SBI_BOOT_ROOT: Table = Table([0; ENTRIES]);
static mut DIRECT_MAP_TABLES: [Table; TABLES] = [const { Table([0; ENTRIES]) }; TABLES];

/// Add only discovered, accessible RAM to the boot root before PMM use.
/// The BSP is the only running hart and owns these tables until publication.
pub(super) unsafe fn map_direct_map(
    window: DirectMapWindow,
    regions: &DirectMapRegions,
) -> Result<(), &'static str> {
    let root = unsafe { &mut *(&raw mut SBI_BOOT_ROOT) };
    let tables = unsafe { &mut *(&raw mut DIRECT_MAP_TABLES) };
    let tables_pa = virt_to_phys(tables.as_ptr() as usize);
    let mut used = 0;
    for index in 0..regions.len() {
        let area = regions.get(index).expect("direct-map region").area();
        let mut pa = area.start;
        while pa <= area.end {
            let va = window
                .phys_to_virt(crate::mem::address::PhysAddr::new(pa))
                .ok_or("boot direct-map address outside window")?
                .as_usize();
            let root_index = va >> 22;
            let leaf_index = (va >> 12) & (ENTRIES - 1);
            if root.0[root_index] == 0
                && va % SUPERPAGE == 0
                && pa % SUPERPAGE as u64 == 0
                && area.end - pa >= SUPERPAGE as u64 - 1
            {
                root.0[root_index] = leaf(pa, 0xc7)?;
                pa += SUPERPAGE as u64;
                continue;
            }
            if root.0[root_index] == 0 {
                let table = tables
                    .get_mut(used)
                    .ok_or("boot direct-map tables exhausted")?;
                let table_pa = virt_to_phys(table as *mut Table as usize);
                root.0[root_index] = leaf(table_pa, 1)?;
                used += 1;
            }
            if root.0[root_index] & 0xe != 0 {
                return Err("boot direct map overlaps another mapping");
            }
            let table_pa = ((root.0[root_index] >> 10) as u64) << 12;
            // These table pages belong to the image, whose linked alias is
            // already accessible; the direct map is not yet fully installed.
            let table_index = usize::try_from(
                table_pa
                    .checked_sub(tables_pa)
                    .ok_or("foreign boot page table")?
                    / PAGE_SIZE as u64,
            )
            .map_err(|_| "boot page-table index exceeds pointer width")?;
            let table = tables
                .get_mut(table_index)
                .ok_or("foreign boot page table")?;
            if table.0[leaf_index] != 0 {
                return Err("duplicate boot RAM mapping");
            }
            table.0[leaf_index] = leaf(pa, 0xc7)?;
            pa = pa
                .checked_add(PAGE_SIZE as u64)
                .ok_or("boot mapping end overflows")?;
        }
    }
    // SAFETY: All new entries are complete, valid supervisor translations.
    unsafe {
        core::arch::asm!("sfence.vma", options(nostack));
    }
    Ok(())
}

fn leaf(pa: u64, flags: u32) -> Result<u32, &'static str> {
    if pa >= (1u64 << 34) || pa & (PAGE_SIZE as u64 - 1) != 0 {
        return Err("physical address cannot be encoded in Sv32");
    }
    Ok(((pa >> 12) as u32) << 10 | flags)
}
