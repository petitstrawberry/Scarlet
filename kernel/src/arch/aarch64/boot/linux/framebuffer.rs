//! Allocation-free discovery of a firmware-owned scanout surface.

use crate::environment::PAGE_SIZE;
use crate::vm::vmem::PhysicalMemoryArea;

pub(super) struct BootFramebuffer {
    pub paddr: u64,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub red_low: bool,
    pub rotated: bool,
    pub area: PhysicalMemoryArea,
}

impl BootFramebuffer {
    pub fn parse(
        fdt: &fdt::Fdt<'_>,
        kernel: PhysicalMemoryArea,
        dtb: PhysicalMemoryArea,
        initramfs: Option<PhysicalMemoryArea>,
    ) -> Option<Self> {
        fdt.find_node("/chosen")?.children().find_map(|node| {
            if !node
                .compatible()?
                .all()
                .any(|name| name == "simple-framebuffer")
                || node
                    .property("status")
                    .is_some_and(|p| !matches!(p.as_str(), Some("okay" | "ok")))
            {
                return None;
            }
            let reg = node.reg()?.next()?;
            let paddr = reg.starting_address as usize as u64;
            let size = reg.size?;
            let width = node.property("width")?.as_usize()?;
            let height = node.property("height")?.as_usize()?;
            let stride = node.property("stride")?.as_usize()?;
            let red_low = match node.property("format")?.as_str()? {
                "a8b8g8r8" | "x8b8g8r8" => true,
                "a8r8g8b8" | "x8r8g8b8" => false,
                _ => return None,
            };
            let rotation = node
                .property("scarlet,rotation")
                .and_then(|p| p.as_usize())
                .unwrap_or(0);
            let bytes = stride.checked_mul(height)?;
            if paddr == 0
                || paddr & 3 != 0
                || stride & 3 != 0
                || !(64..=4096).contains(&width)
                || !(64..=4096).contains(&height)
                || stride < width.checked_mul(4)?
                || bytes > size
                || bytes > 16 * 1024 * 1024
                || !matches!(rotation, 0 | 3)
            {
                return None;
            }
            let end = paddr.checked_add(bytes as u64 - 1)?;
            let mask = PAGE_SIZE as u64 - 1;
            let area = PhysicalMemoryArea::new(paddr & !mask, end | mask);
            if [Some(kernel), Some(dtb), initramfs]
                .into_iter()
                .flatten()
                .any(|object| area.start <= object.end && object.start <= area.end)
            {
                return None;
            }
            Some(Self {
                paddr,
                width,
                height,
                stride,
                red_low,
                rotated: rotation == 3,
                area,
            })
        })
    }
}
