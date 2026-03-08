use limine::BaseRevision;
use limine::memory_map::{Entry, EntryType};
use limine::request::{
    DeviceTreeBlobRequest, ExecutableAddressRequest, FramebufferRequest, HhdmRequest,
    MemoryMapRequest, ModuleRequest, RequestsEndMarker, RequestsStartMarker,
};

use crate::vm::addr::virt_to_phys;
use crate::vm::vmem::MemoryArea;

#[unsafe(link_section = ".limine_requests_start")]
#[used]
static LIMINE_REQUESTS_START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub(crate) static BASE_REVISION: BaseRevision = BaseRevision::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub(crate) static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub(crate) static EXECUTABLE_ADDRESS_REQUEST: ExecutableAddressRequest =
    ExecutableAddressRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub(crate) static MEMMAP_REQUEST: MemoryMapRequest = MemoryMapRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub(crate) static DTB_REQUEST: DeviceTreeBlobRequest = DeviceTreeBlobRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub(crate) static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub(crate) static MODULE_REQUEST: ModuleRequest = ModuleRequest::new();

#[unsafe(link_section = ".limine_requests_end")]
#[used]
static LIMINE_REQUESTS_END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

pub(crate) fn ensure_base_revision_supported() {
    if !BASE_REVISION.is_supported() {
        panic!(
            "unsupported Limine base revision: {:?}",
            BASE_REVISION.loaded_revision()
        );
    }
}

pub(crate) fn response<T>(response: Option<&'static T>, name: &str) -> &'static T {
    response.unwrap_or_else(|| panic!("missing Limine response: {}", name))
}

pub(crate) fn select_usable_region(memmap: &[&Entry]) -> MemoryArea {
    let mut best: Option<MemoryArea> = None;

    for entry in memmap {
        if entry.entry_type != EntryType::USABLE {
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

pub(crate) fn module_area(
    module_response: Option<&'static limine::response::ModuleResponse>,
) -> Option<MemoryArea> {
    let file = module_response?.modules().first()?;
    let start = virt_to_phys(file.addr() as usize);
    let end = start + file.size() as usize - 1;
    Some(MemoryArea::new(start, end))
}

fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

pub(crate) fn reserve_front(area: MemoryArea, reserved_bytes: usize) -> MemoryArea {
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
