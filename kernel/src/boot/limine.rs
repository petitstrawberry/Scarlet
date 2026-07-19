use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
use limine::BaseRevision;
use limine::memmap;

use limine::request::{
    DateAtBootRequest, DtbRequest, ExecutableAddressRequest, ExecutableCmdlineRequest,
    FramebufferRequest, FramebufferResponse, HhdmRequest, MemmapRequest, ModulesRequest,
    ModulesResponse, MpRequest,
};
use limine::{RequestsEndMarker, RequestsStartMarker};

use crate::vm::addr::boot_virt_to_phys;
use crate::vm::direct_map::{DirectMapRegion, DirectMapRegions};
use crate::vm::vmem::{MemoryArea, MemoryAttribute};

#[unsafe(link_section = ".limine_requests_start")]
#[used]
static LIMINE_REQUESTS_START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static BASE_REVISION: BaseRevision = BaseRevision::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static EXECUTABLE_ADDRESS_REQUEST: ExecutableAddressRequest = ExecutableAddressRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static EXECUTABLE_CMDLINE_REQUEST: ExecutableCmdlineRequest = ExecutableCmdlineRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static MEMMAP_REQUEST: MemmapRequest = MemmapRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static DTB_REQUEST: DtbRequest = DtbRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static DATE_AT_BOOT_REQUEST: DateAtBootRequest = DateAtBootRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static MODULE_REQUEST: ModulesRequest = ModulesRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static MP_REQUEST: MpRequest = MpRequest::new(0);

#[unsafe(link_section = ".limine_requests_end")]
#[used]
static LIMINE_REQUESTS_END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

pub fn ensure_base_revision_supported() {
    if !BASE_REVISION.is_supported() {
        panic!(
            "unsupported Limine base revision: {:?}",
            BASE_REVISION.actual_revision()
        );
    }
}

/// Cached wall-clock nanoseconds from Limine's `Date at Boot`.
///
/// `u64::MAX` is the "not captured" sentinel.
static DATE_AT_BOOT_NS: AtomicU64 = AtomicU64::new(u64::MAX);
const CMDLINE_BUFFER_SIZE: usize = 4096;

struct CmdlineBuffer(UnsafeCell<[u8; CMDLINE_BUFFER_SIZE]>);

// SAFETY: The command line buffer is written once during single-threaded boot
// before scheduler startup, then read immutably through the returned string.
unsafe impl Sync for CmdlineBuffer {}

static CMDLINE_BUFFER: CmdlineBuffer = CmdlineBuffer(UnsafeCell::new([0; CMDLINE_BUFFER_SIZE]));

fn cache_cmdline(cmdline: &str) -> Option<&'static str> {
    if cmdline.is_empty() {
        return None;
    }

    let bytes = cmdline.as_bytes();
    let len = bytes.len().min(CMDLINE_BUFFER_SIZE - 1);
    if len < bytes.len() {
        crate::early_println!(
            "[boot] Limine cmdline truncated from {} to {} bytes",
            bytes.len(),
            len
        );
    }

    let buffer = unsafe { &mut *CMDLINE_BUFFER.0.get() };
    buffer[..len].copy_from_slice(&bytes[..len]);
    buffer[len] = 0;

    Some(unsafe { core::str::from_utf8_unchecked(&buffer[..len]) })
}

pub fn boot_cmdline(fdt_cmdline: Option<&'static str>) -> Option<&'static str> {
    if let Some(cmdline) = EXECUTABLE_CMDLINE_REQUEST
        .response()
        .and_then(|response| cache_cmdline(response.cmdline()))
    {
        Some(cmdline)
    } else {
        fdt_cmdline
    }
}

/// Read Limine's `Date at Boot` response and cache the wall-clock nanoseconds.
///
/// Must be called once from the boot entry, before the page-table switch: the
/// Limine response pointer is only valid in the bootloader's address space, so
/// it cannot be dereferenced later from `start_kernel`. No-op if the
/// bootloader provided no response.
pub fn capture_date_at_boot() {
    let Some(resp) = DATE_AT_BOOT_REQUEST.response() else {
        crate::early_println!(
            "[boot] Limine Date at Boot: no response (request unfulfilled by bootloader)"
        );
        return;
    };
    let secs = resp.timestamp;
    crate::early_println!(
        "[boot] Limine Date at Boot: timestamp = {} (0x{:x})",
        secs,
        secs
    );
    if secs > 0 {
        if let Some(ns) = (secs as u64).checked_mul(1_000_000_000) {
            DATE_AT_BOOT_NS.store(ns, Ordering::SeqCst);
        } else {
            crate::early_println!("[boot] Limine Date at Boot: timestamp overflow, ignored");
        }
    } else if secs == 0 {
        crate::early_println!("[boot] Limine Date at Boot: zero timestamp, ignored");
    } else {
        crate::early_println!("[boot] Limine Date at Boot: negative timestamp, ignored");
    }
}

/// Cached wall-clock nanoseconds since the Unix epoch from Limine's
/// `Date at Boot`. Returns `None` if not captured (e.g. non-EFI boot). The
/// value has ~1s granularity (Limine exposes a whole-second timestamp).
pub fn date_at_boot_ns() -> Option<u64> {
    let ns = DATE_AT_BOOT_NS.load(Ordering::Acquire);
    if ns == u64::MAX { None } else { Some(ns) }
}

pub fn response<T>(response: Option<&'static T>, name: &str) -> &'static T {
    response.unwrap_or_else(|| panic!("missing Limine response: {}", name))
}

pub fn select_usable_region(memmap: &[&memmap::Entry]) -> MemoryArea {
    let mut best: Option<MemoryArea> = None;

    for entry in memmap {
        if entry.type_ != memmap::MEMMAP_USABLE {
            continue;
        }

        let area = MemoryArea::new(
            entry.base as usize,
            (entry.base + entry.length - 1) as usize,
        );
        best = match best {
            Some(current) if current.size() >= area.size() => Some(current),
            _ => Some(area),
        };
    }

    best.expect("no usable Limine memmap region")
}

/// Returns Limine's broad bootloader direct-map bounds.
///
/// These bounds describe the page tables owned by Limine and can include
/// reserved ranges and holes. They must only be used for bootloader-phase
/// address translation, never for Scarlet's runtime mappings.
///
/// # Arguments
///
/// * `memmap` - Limine physical memory-map entries.
///
/// # Returns
///
/// The inclusive physical bounds covered by Limine's direct map.
pub fn bootloader_hhdm_physical_bound(memmap: &[&memmap::Entry]) -> MemoryArea {
    let mut start = usize::MAX;
    let mut end = 0usize;

    for entry in memmap {
        if entry.length == 0 {
            continue;
        }

        let entry_start = entry.base as usize;
        let entry_end = (entry.base + entry.length - 1) as usize;
        start = start.min(entry_start);
        end = end.max(entry_end);
    }

    if start == usize::MAX {
        panic!("no Limine memmap entries available for HHDM span");
    }

    MemoryArea::new(start, end)
}

/// Builds Scarlet's sparse runtime direct-map regions from Limine's memory map.
///
/// Only usable RAM and executable/module memory receive Normal aliases. A
/// framebuffer, when supplied, is carved out as explicit DeviceBurstable pages
/// so it never overlaps a Normal direct-map alias.
///
/// # Arguments
///
/// * `memmap` - Limine physical memory-map entries.
/// * `framebuffer` - Optional inclusive framebuffer physical range.
///
/// # Returns
///
/// A fixed-capacity sparse direct-map set, or an error for malformed Limine
/// ranges, capacity exhaustion, or incompatible overlapping attributes.
pub fn runtime_direct_map_regions(
    memmap: &[&memmap::Entry],
    framebuffer: Option<MemoryArea>,
) -> Result<DirectMapRegions, &'static str> {
    let framebuffer_region = framebuffer
        .map(|area| DirectMapRegion::new(area, MemoryAttribute::DeviceBurstable))
        .transpose()?;
    let mut regions = DirectMapRegions::new();

    if let Some(region) = framebuffer_region {
        regions.insert(region.area(), region.memory_attribute())?;
    }

    for entry in memmap {
        if entry.length == 0
            || (entry.type_ != memmap::MEMMAP_USABLE
                && entry.type_ != memmap::MEMMAP_EXECUTABLE_AND_MODULES)
        {
            continue;
        }

        let entry_start = usize::try_from(entry.base)
            .map_err(|_| "Limine direct-map region start does not fit usize")?;
        let entry_end_exclusive = entry
            .base
            .checked_add(entry.length)
            .ok_or("Limine direct-map region end overflows")?;
        let entry_end = usize::try_from(
            entry_end_exclusive
                .checked_sub(1)
                .ok_or("Limine direct-map region is empty")?,
        )
        .map_err(|_| "Limine direct-map region end does not fit usize")?;
        let normal_region = DirectMapRegion::new(
            MemoryArea::new(entry_start, entry_end),
            MemoryAttribute::Normal,
        )?;

        if let Some(framebuffer_region) = framebuffer_region {
            insert_normal_region_excluding(
                &mut regions,
                normal_region.area(),
                framebuffer_region.area(),
            )?;
        } else {
            regions.insert(normal_region.area(), MemoryAttribute::Normal)?;
        }
    }

    if regions.is_empty() {
        return Err("Limine did not provide runtime direct-map regions");
    }

    Ok(regions)
}

fn insert_normal_region_excluding(
    regions: &mut DirectMapRegions,
    normal: MemoryArea,
    excluded: MemoryArea,
) -> Result<(), &'static str> {
    if normal.end < excluded.start || excluded.end < normal.start {
        return regions.insert(normal, MemoryAttribute::Normal);
    }

    if normal.start < excluded.start {
        let before_end = excluded
            .start
            .checked_sub(1)
            .ok_or("framebuffer exclusion underflows")?;
        regions.insert(
            MemoryArea::new(normal.start, before_end),
            MemoryAttribute::Normal,
        )?;
    }
    if excluded.end < normal.end {
        let after_start = excluded
            .end
            .checked_add(1)
            .ok_or("framebuffer exclusion overflows")?;
        regions.insert(
            MemoryArea::new(after_start, normal.end),
            MemoryAttribute::Normal,
        )?;
    }

    Ok(())
}

pub fn module_area(module_response: Option<&'static ModulesResponse>) -> Option<MemoryArea> {
    let file = module_response?.modules().first()?;
    let start = boot_virt_to_phys(file.data().as_ptr() as usize);
    let end = start + file.data().len() - 1;
    Some(MemoryArea::new(start, end))
}

fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

pub fn reserve_front(area: MemoryArea, reserved_bytes: usize) -> MemoryArea {
    let reserved_start = align_up(area.start, 4096);
    let reserved_end = align_up(reserved_start + reserved_bytes, 4096);

    if reserved_end > area.end {
        panic!(
            "insufficient usable memory after reserving {:#x} bytes from {:#x}..={:#x}",
            reserved_bytes, area.start, area.end
        );
    }

    MemoryArea::new(reserved_end, area.end)
}

/// Get framebuffer physical memory area from Limine response.
///
/// Returns the framebuffer's physical address range for use in early console
/// after page table transition.
pub fn framebuffer_area(fb_response: Option<&'static FramebufferResponse>) -> Option<MemoryArea> {
    let fb = fb_response?.framebuffers().first()?;
    let addr = fb.address() as usize;
    // Calculate size: pitch * height (pitch is bytes per row)
    let size = fb.pitch as usize * fb.height as usize;
    let start = boot_virt_to_phys(addr);
    let end = start + size - 1;
    Some(MemoryArea::new(start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn runtime_direct_map_selects_ram_and_carves_out_framebuffer() {
        let usable = memmap::Entry {
            base: 0x1000,
            length: 0x4000,
            type_: memmap::MEMMAP_USABLE,
        };
        let module = memmap::Entry {
            base: 0x8000,
            length: 0x1000,
            type_: memmap::MEMMAP_EXECUTABLE_AND_MODULES,
        };
        let reserved = memmap::Entry {
            base: 0xa000,
            length: 0x1000,
            type_: memmap::MEMMAP_RESERVED,
        };
        let entries = [&usable, &module, &reserved];
        let framebuffer = MemoryArea::new(0x2000, 0x2fff);

        let regions = runtime_direct_map_regions(&entries, Some(framebuffer)).unwrap();

        assert!(regions.contains_area_with_attribute(
            MemoryArea::new(0x1000, 0x1fff),
            MemoryAttribute::Normal,
        ));
        assert!(
            regions.contains_area_with_attribute(framebuffer, MemoryAttribute::DeviceBurstable,)
        );
        assert!(regions.contains_area_with_attribute(
            MemoryArea::new(0x3000, 0x4fff),
            MemoryAttribute::Normal,
        ));
        assert!(regions.contains_area_with_attribute(
            MemoryArea::new(0x8000, 0x8fff),
            MemoryAttribute::Normal,
        ));
        assert!(!regions.contains(0xa000));
        assert!(
            regions
                .validate_alias(framebuffer, MemoryAttribute::Normal)
                .is_err()
        );
    }
}
