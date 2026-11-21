use core::arch::asm;

#[inline(always)]
fn cache_line_bytes_dcache() -> usize {
    // CTR_EL0.DminLine is log2(number of 32-bit words in the smallest D-cache line).
    // LineBytes = 4 << DminLine.
    let ctr: u64;
    unsafe {
        asm!("mrs {0}, ctr_el0", out(reg) ctr, options(nostack));
    }
    let dminline = ((ctr >> 16) & 0xf) as usize;
    4usize << dminline
}

/// Clean D-cache to Point of Coherency (PoC) for the given virtual address range.
///
/// This is useful for ensuring page-table updates and CPU-written DMA buffers
/// become visible to hardware.
///
/// # Arguments
///
/// * `start_vaddr` - Kernel virtual address at the start of the range.
/// * `len` - Number of bytes to clean.
#[inline(always)]
pub fn clean_dcache_to_poc_range(start_vaddr: usize, len: usize) {
    if len == 0 {
        return;
    }

    let line = cache_line_bytes_dcache();
    let mut addr = start_vaddr & !(line - 1);
    let end = start_vaddr.saturating_add(len);

    unsafe {
        while addr < end {
            asm!("dc cvac, {0}", in(reg) addr, options(nostack));
            addr = addr.saturating_add(line);
        }
        // Ensure the clean completes before subsequent operations (e.g. TLBI).
        asm!("dsb sy", options(nostack));
    }
}

/// Clean and invalidate D-cache to Point of Coherency (PoC) for a virtual range.
///
/// Use this before reading memory that may have been modified through another
/// virtual alias, such as a userspace mapping of a kernel-owned shared page.
#[inline(always)]
pub fn clean_invalidate_dcache_to_poc_range(start_vaddr: usize, len: usize) {
    if len == 0 {
        return;
    }

    let line = cache_line_bytes_dcache();
    let mut addr = start_vaddr & !(line - 1);
    let end = start_vaddr.saturating_add(len);

    unsafe {
        while addr < end {
            asm!("dc civac, {0}", in(reg) addr, options(nostack));
            addr = addr.saturating_add(line);
        }
        asm!("dsb sy", options(nostack));
    }
}

/// Invalidate D-cache to Point of Coherency (PoC) for a virtual range.
///
/// Use this after device-written DMA has completed and before the CPU reads the
/// target memory. Dirty CPU lines for the range must already have been cleaned
/// before device ownership was granted.
///
/// # Arguments
///
/// * `start_vaddr` - Kernel virtual address at the start of the range.
/// * `len` - Number of bytes to invalidate.
#[inline(always)]
pub fn invalidate_dcache_to_poc_range(start_vaddr: usize, len: usize) {
    if len == 0 {
        return;
    }

    let line = cache_line_bytes_dcache();
    let mut addr = start_vaddr & !(line - 1);
    let end = start_vaddr.saturating_add(len);

    unsafe {
        while addr < end {
            asm!("dc ivac, {0}", in(reg) addr, options(nostack));
            addr = addr.saturating_add(line);
        }
        asm!("dsb sy", options(nostack));
    }
}

/// Clean D-cache to Point of Unification (PoU) for the given virtual address range.
///
/// Required when code is written via the data cache and will later be executed
/// (self-modifying code / JIT / loading user text). The canonical sequence is:
/// - clean D-cache to PoU over the modified range
/// - `dsb ishst`
/// - invalidate I-cache (`ic ivau` per-line or `ic iallu`)
/// - `dsb ish; isb`
#[inline(always)]
pub fn clean_dcache_to_pou_range(start_vaddr: usize, len: usize) {
    if len == 0 {
        return;
    }

    let line = cache_line_bytes_dcache();
    let mut addr = start_vaddr & !(line - 1);
    let end = start_vaddr.saturating_add(len);

    unsafe {
        while addr < end {
            asm!("dc cvau, {0}", in(reg) addr, options(nostack));
            addr = addr.saturating_add(line);
        }
        // Ensure the clean completes before subsequent I-cache invalidation.
        asm!("dsb ishst", options(nostack));
    }
}

/// Synchronize instruction fetch after writing executable memory.
///
/// # Arguments
///
/// * `start_vaddr` - Kernel virtual address used to write the executable bytes.
/// * `len` - Number of bytes written.
pub fn sync_icache_for_execution(start_vaddr: usize, len: usize) {
    if len == 0 {
        return;
    }

    clean_dcache_to_pou_range(start_vaddr, len);
    unsafe {
        asm!("ic ialluis", "dsb ish", "isb", options(nostack));
    }
}
