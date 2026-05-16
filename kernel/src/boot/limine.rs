use limine::BaseRevision;
use limine::memmap;
use limine::request::{
    DtbRequest, ExecutableAddressRequest, FramebufferRequest, FramebufferResponse, HhdmRequest,
    MemmapRequest, MemmapResponse, ModulesRequest, ModulesResponse, MpRequest, MpResponse,
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
pub static MEMMAP_REQUEST: MemmapRequest = MemmapRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static DTB_REQUEST: DtbRequest = DtbRequest::new();

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
