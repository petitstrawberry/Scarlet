use limine::BaseRevision;
use limine::framebuffer::MemoryModel;
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
pub static BASE_REVISION: BaseRevision = BaseRevision::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static EXECUTABLE_ADDRESS_REQUEST: ExecutableAddressRequest = ExecutableAddressRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static MEMMAP_REQUEST: MemoryMapRequest = MemoryMapRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static DTB_REQUEST: DeviceTreeBlobRequest = DeviceTreeBlobRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
pub static MODULE_REQUEST: ModuleRequest = ModuleRequest::new();

#[unsafe(link_section = ".limine_requests_end")]
#[used]
static LIMINE_REQUESTS_END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

pub fn ensure_base_revision_supported() {
    if !BASE_REVISION.is_supported() {
        panic!(
            "unsupported Limine base revision: {:?}",
            BASE_REVISION.loaded_revision()
        );
    }
}

pub fn response<T>(response: Option<&'static T>, name: &str) -> &'static T {
    response.unwrap_or_else(|| panic!("missing Limine response: {}", name))
}

pub fn select_usable_region(memmap: &[&Entry]) -> MemoryArea {
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

pub fn module_area(
    module_response: Option<&'static limine::response::ModuleResponse>,
) -> Option<MemoryArea> {
    let file = module_response?.modules().first()?;
    let start = virt_to_phys(file.addr() as usize);
    let end = start + file.size() as usize - 1;
    Some(MemoryArea::new(start, end))
}

/// Limine framebuffer metadata exported for later graphics initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimineFramebufferInfo {
    pub addr: usize,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u16,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
}

fn filter_rgb_framebuffer(
    is_rgb: bool,
    info: LimineFramebufferInfo,
) -> Option<LimineFramebufferInfo> {
    if is_rgb { Some(info) } else { None }
}

/// Returns the first RGB Limine framebuffer in a kernel-friendly representation.
pub fn framebuffer_info() -> Option<LimineFramebufferInfo> {
    let response = FRAMEBUFFER_REQUEST.get_response()?;
    let framebuffer = response.framebuffers().next()?;
    filter_rgb_framebuffer(
        framebuffer.memory_model() == MemoryModel::RGB,
        LimineFramebufferInfo {
            addr: framebuffer.addr() as usize,
            width: framebuffer.width() as u32,
            height: framebuffer.height() as u32,
            pitch: framebuffer.pitch() as u32,
            bpp: framebuffer.bpp(),
            red_mask_size: framebuffer.red_mask_size(),
            red_mask_shift: framebuffer.red_mask_shift(),
            green_mask_size: framebuffer.green_mask_size(),
            green_mask_shift: framebuffer.green_mask_shift(),
            blue_mask_size: framebuffer.blue_mask_size(),
            blue_mask_shift: framebuffer.blue_mask_shift(),
        },
    )
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

#[cfg(test)]
mod tests {
    use super::{LimineFramebufferInfo, filter_rgb_framebuffer};

    fn sample_framebuffer_info() -> LimineFramebufferInfo {
        LimineFramebufferInfo {
            addr: 0x1234_5000,
            width: 1024,
            height: 768,
            pitch: 4096,
            bpp: 32,
            red_mask_size: 8,
            red_mask_shift: 16,
            green_mask_size: 8,
            green_mask_shift: 8,
            blue_mask_size: 8,
            blue_mask_shift: 0,
        }
    }

    #[test_case]
    fn test_framebuffer_info_from_raw_accepts_rgb() {
        let info = sample_framebuffer_info();
        assert_eq!(filter_rgb_framebuffer(true, info), Some(info));
    }

    #[test_case]
    fn test_framebuffer_info_from_raw_rejects_non_rgb() {
        assert_eq!(
            filter_rgb_framebuffer(false, sample_framebuffer_info()),
            None
        );
    }
}
