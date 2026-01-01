//! Early (identity-mapped) MMU enable for AArch64.
//!
//! On some accelerators (e.g. HVF), exclusive accesses (LDXR/LDAXR, etc.) can
//! fault while the MMU is disabled because memory is treated as Device-type.
//! The kernel allocator (and many locks) rely on exclusives, so we enable a
//! minimal identity mapping early to make RAM Normal memory.

use core::arch::asm;

use crate::arch::aarch64::clean_dcache_to_poc_range;

#[repr(C, align(4096))]
struct PageTable([u64; 512]);

static mut EARLY_L0: PageTable = PageTable([0; 512]);
static mut EARLY_L1: PageTable = PageTable([0; 512]);

#[inline(always)]
unsafe fn zero_table(table: *mut u64) {
    // Avoid creating `&mut` references to `static mut` (Rust 2024).
    for i in 0..512usize {
        core::ptr::write_volatile(table.add(i), 0);
    }
}

#[inline(always)]
fn read_sctlr_el1() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, sctlr_el1", out(reg) val, options(nostack)) };
    val
}

#[inline(always)]
fn write_sctlr_el1(val: u64) {
    unsafe {
        asm!(
            "msr sctlr_el1, {0}",
            "dsb sy",
            "isb",
            in(reg) val,
            options(nostack)
        );
    }
}

/// Enable a minimal identity-mapped MMU configuration if MMU is currently off.
///
/// - Maps 0x0000_0000..0x3fff_ffff as Device (covers QEMU virt MMIO space).
/// - Maps RAM at 0x4000_0000.. up to `dram_end` as Normal memory (1GiB blocks).
///
/// This is intended to run before the first heap allocation.
pub fn enable_identity_mmu_if_disabled(dram_end: usize) {
    // If MMU already enabled, don't touch early mappings.
    if (read_sctlr_el1() & 1) != 0 {
        return;
    }

    const DESC_TABLE: u64 = 0b11;
    const DESC_BLOCK: u64 = 0b01;

    const AF: u64 = 1 << 10;

    const SH_NON: u64 = 0 << 8;
    const SH_INNER: u64 = 3 << 8;

    // AP bits (stage-1): 0b00 -> EL1 RW, EL0 no access.
    const AP_RW_EL1: u64 = 0 << 6;

    const ATTRINDX_DEVICE: u64 = 0 << 2;
    const ATTRINDX_NORMAL: u64 = 1 << 2;

    const PXN: u64 = 1 << 53;
    const UXN: u64 = 1 << 54;

    #[inline(always)]
    fn table_desc(paddr: usize) -> u64 {
        (paddr as u64 & 0x0000_ffff_ffff_f000) | DESC_TABLE
    }

    #[inline(always)]
    fn block_1g_desc(paddr: usize, attr: u64, sh: u64, xn: u64) -> u64 {
        // 1GiB block uses PA bits [47:30].
        (paddr as u64 & 0x0000_ffff_c000_0000) | DESC_BLOCK | attr | sh | AP_RW_EL1 | AF | xn
    }

    unsafe {
        // Clear tables.
        zero_table((&raw mut EARLY_L0.0[0]) as *mut u64);
        zero_table((&raw mut EARLY_L1.0[0]) as *mut u64);

        // L0[0] -> L1.
        core::ptr::write_volatile((&raw mut EARLY_L0.0[0]) as *mut u64, table_desc(&raw const EARLY_L1 as usize));

        // 0x0000_0000..0x3fff_ffff : device.
        core::ptr::write_volatile(
            (&raw mut EARLY_L1.0[0]) as *mut u64,
            block_1g_desc(0x0000_0000, ATTRINDX_DEVICE, SH_NON, PXN | UXN),
        );

        // 0x4000_0000.. : normal RAM.
        //
        // Older code mapped only the first ~2GiB of RAM. That breaks on larger
        // configurations (e.g. 16GiB) where the heap range can legitimately span
        // multiple 1GiB blocks.
        if dram_end >= 0x4000_0000 {
            let last_idx = (dram_end >> 30).min(511);
            for idx in 1..=last_idx {
                let base = (idx as usize) << 30;
                core::ptr::write_volatile(
                    (&raw mut EARLY_L1.0[idx]) as *mut u64,
                    block_1g_desc(base, ATTRINDX_NORMAL, SH_INNER, UXN),
                );
            }
        }

        // Make table writes visible to the hardware walker.
        clean_dcache_to_poc_range(&raw const EARLY_L0 as usize, core::mem::size_of::<PageTable>());
        clean_dcache_to_poc_range(&raw const EARLY_L1 as usize, core::mem::size_of::<PageTable>());

        // Program MAIR/TCR (match `arch/aarch64/vm/mmu/armv8_4k.rs`).
        let mair_val: u64 = 0x44ff00;
        asm!("msr mair_el1, {}", in(reg) mair_val, options(nostack));

        let tcr_val: u64 = 0xB5103510;
        asm!("msr tcr_el1, {}", in(reg) tcr_val, options(nostack));

        // TTBR0 points to EARLY_L0. (ASID=0)
        let ttbr0: u64 = (&raw const EARLY_L0 as usize as u64) & 0x0000_ffff_ffff_f000;
        asm!(
            "msr ttbr0_el1, {0}",
            "dsb sy",
            "isb",
            in(reg) ttbr0,
            options(nostack)
        );

        // Enable MMU (and caches) now that tables are in place.
        // This mirrors `init_mmu_registers` behavior.
        let mut sctlr = read_sctlr_el1();
        sctlr |= 1; // M
        sctlr |= 1 << 2; // C
        sctlr |= 1 << 12; // I
        write_sctlr_el1(sctlr);

        // Ensure a clean TLB state for the new translations.
        asm!("tlbi vmalle1is", "dsb ish", "isb", options(nostack));
    }
}
