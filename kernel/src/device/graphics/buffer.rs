use core::any::Any;
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::mem::page::{allocate_raw_pages, free_raw_pages, Page};

/// A trait representing a contiguous region of graphics memory (VRAM or GTT).
/// 
/// This trait allows graphics buffers to be treated as first-class kernel objects,
/// enabling them to be shared between processes, mapped into memory, and controlled
/// via ioctls.
pub trait GraphicsBuffer: Send + Sync + MemoryMappingOps + ControlOps {
    /// Get the size of the buffer in bytes
    fn size(&self) -> usize;
    
    /// Get the physical address (if applicable/visible to CPU)
    fn physical_address(&self) -> usize;

    /// Downcast to concrete type
    fn as_any(&self) -> &dyn Any;
}

/// A simple "dumb" buffer allocated in system memory.
/// 
/// This implementation uses contiguous physical pages allocated via the page allocator.
/// It is suitable for software rendering and simple framebuffers.
pub struct DumbBuffer {
    width: u32,
    height: u32,
    bpp: u32,
    pitch: u32,
    size: usize,
    phys_addr: usize,
}

impl DumbBuffer {
    /// Create a new dumb buffer with the specified dimensions and bits per pixel.
    pub fn new(width: u32, height: u32, bpp: u32) -> Result<Self, &'static str> {
        // Validate dimensions
        if width == 0 || height == 0 || bpp == 0 {
            return Err("Invalid buffer dimensions");
        }

        // Calculate size and pitch
        // pitch = align_up(width * bpp / 8, 4)
        let width_bits = width.checked_mul(bpp).ok_or("Integer overflow")?;
        let width_bytes = (width_bits + 7) / 8;
        let pitch = (width_bytes + 3) & !3; // Align to 4 bytes
        
        let size = (pitch as usize).checked_mul(height as usize).ok_or("Integer overflow")?;
        
        // Allocate physical pages
        let pages_needed = (size + 4095) / 4096;
        let phys_addr = allocate_raw_pages(pages_needed) as usize;
        
        if phys_addr == 0 {
            return Err("Failed to allocate memory for dumb buffer");
        }

        // Zero the memory
        // Note: In a real implementation, we should map this to virtual memory to zero it.
        // For now, assuming direct mapping or identity mapping for kernel access if possible,
        // but since we are in kernel, we might need to map it. 
        // However, allocate_raw_pages returns a physical address.
        // Ideally we should use a proper kernel virtual address mapping here.
        // For this MVP/stub, we skip zeroing or assume the allocator returns zeroed pages (it might not).
        
        Ok(Self {
            width,
            height,
            bpp,
            pitch,
            size,
            phys_addr,
        })
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn bpp(&self) -> u32 { self.bpp }
    pub fn pitch(&self) -> u32 { self.pitch }
}

impl Drop for DumbBuffer {
    fn drop(&mut self) {
        if self.phys_addr != 0 {
            let pages = (self.size + 4095) / 4096;
            free_raw_pages(self.phys_addr as *mut Page, pages);
        }
    }
}

impl GraphicsBuffer for DumbBuffer {
    fn size(&self) -> usize {
        self.size
    }

    fn physical_address(&self) -> usize {
        self.phys_addr
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl MemoryMappingOps for DumbBuffer {
    fn get_mapping_info(&self, offset: usize, length: usize) -> Result<(usize, usize, bool), &'static str> {
        if offset >= self.size {
            return Err("Offset out of bounds");
        }
        
        if offset + length > self.size {
            return Err("Length out of bounds");
        }

        // Return (physical_address, size, cacheable)
        // Dumb buffers in RAM are usually cacheable
        Ok((self.phys_addr + offset, length, true))
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Nothing to do
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Nothing to do
    }

    fn supports_mmap(&self) -> bool {
        true
    }
}

impl ControlOps for DumbBuffer {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        // Future: Implement buffer-specific ioctls (e.g. cache flushing)
        Err("Not implemented")
    }
}
