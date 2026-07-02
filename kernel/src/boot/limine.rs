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
use crate::vm::vmem::MemoryArea;

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

pub fn hhdm_physical_span(memmap: &[&memmap::Entry]) -> MemoryArea {
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
