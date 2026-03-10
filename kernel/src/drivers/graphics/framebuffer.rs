//! Generic framebuffer-backed graphics device.

use core::any::Any;

use crate::{
    device::{
        Device, DeviceType,
        graphics::{FramebufferConfig, GraphicsDevice},
    },
    object::capability::{ControlOps, MemoryMappingOps, Selectable},
};

/// Graphics device backed by a pre-initialized framebuffer.
pub struct FramebufferDevice {
    display_name: &'static str,
    config: FramebufferConfig,
    framebuffer_addr: usize,
}

impl FramebufferDevice {
    /// Creates a framebuffer device from an existing framebuffer allocation.
    pub fn new(
        display_name: &'static str,
        config: FramebufferConfig,
        framebuffer_addr: usize,
    ) -> Result<Self, &'static str> {
        if framebuffer_addr == 0 {
            return Err("Framebuffer address must not be null");
        }

        Ok(Self {
            display_name,
            config,
            framebuffer_addr,
        })
    }

    fn page_aligned_size(&self) -> usize {
        (self.config.size() + crate::environment::PAGE_SIZE - 1)
            & !(crate::environment::PAGE_SIZE - 1)
    }
}

impl Device for FramebufferDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Graphics
    }

    fn name(&self) -> &'static str {
        self.display_name
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_graphics_device(&self) -> Option<&dyn GraphicsDevice> {
        Some(self)
    }
}

impl ControlOps for FramebufferDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for FramebufferDevice {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        if offset % crate::environment::PAGE_SIZE != 0 {
            return Err("Framebuffer mmap offset must be page-aligned");
        }
        if length % crate::environment::PAGE_SIZE != 0 {
            return Err("Framebuffer mmap length must be page-aligned");
        }

        let size = self.page_aligned_size();
        if offset >= size {
            return Err("Offset exceeds framebuffer size");
        }

        let available_size = size - offset;
        if length > available_size {
            return Err("Requested length exceeds available framebuffer size");
        }

        Ok((self.framebuffer_addr + offset, 0x3, true))
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        self.framebuffer_addr != 0 && self.page_aligned_size() > 0
    }
}

impl Selectable for FramebufferDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

impl GraphicsDevice for FramebufferDevice {
    fn get_display_name(&self) -> &'static str {
        self.display_name
    }

    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        Ok(self.config.clone())
    }

    fn get_framebuffer_address(&self) -> Result<usize, &'static str> {
        Ok(self.framebuffer_addr)
    }

    fn flush_framebuffer(
        &self,
        _x: u32,
        _y: u32,
        _width: u32,
        _height: u32,
    ) -> Result<(), &'static str> {
        Ok(())
    }

    fn init_graphics(&self) -> Result<(), &'static str> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        device::graphics::PixelFormat, object::capability::MemoryMappingOps, vm::addr::virt_to_phys,
    };

    #[test_case]
    fn test_framebuffer_device_preserves_stride() {
        let config = FramebufferConfig::with_stride(800, 600, PixelFormat::BGRA8888, 4096);
        let device = FramebufferDevice::new("framebuffer", config.clone(), 0x4000_0000)
            .expect("Framebuffer device should be created");

        let retrieved_config = device
            .get_framebuffer_config()
            .expect("Framebuffer config should be available");
        assert_eq!(retrieved_config.width, 800);
        assert_eq!(retrieved_config.height, 600);
        assert_eq!(retrieved_config.stride, 4096);
        assert_eq!(retrieved_config.format, PixelFormat::BGRA8888);
        assert_eq!(
            device
                .get_framebuffer_address()
                .expect("Framebuffer address should be available"),
            0x4000_0000
        );
    }

    #[test_case]
    fn test_framebuffer_device_supports_memory_mapping() {
        let config = FramebufferConfig::new(4, 4, PixelFormat::RGBA8888);
        let fb_size = config.size();
        let fb_pages =
            (fb_size + crate::environment::PAGE_SIZE - 1) / crate::environment::PAGE_SIZE;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        let device = FramebufferDevice::new("framebuffer", config, virt_to_phys(fb_addr))
            .expect("Framebuffer device should be created");

        assert!(device.supports_mmap());
        let (paddr, permissions, is_shared) = device
            .get_mapping_info(0, fb_pages * crate::environment::PAGE_SIZE)
            .expect("Framebuffer mapping should succeed");
        assert_eq!(paddr, virt_to_phys(fb_addr));
        assert_eq!(permissions, 0x3);
        assert!(is_shared);
    }

    #[test_case]
    fn test_framebuffer_device_rejects_unaligned_mappings() {
        let config = FramebufferConfig::new(4, 4, PixelFormat::RGBA8888);
        let device = FramebufferDevice::new("framebuffer", config, 0x4000_0000)
            .expect("Framebuffer device should be created");

        assert!(
            device
                .get_mapping_info(1, crate::environment::PAGE_SIZE)
                .is_err()
        );
        assert!(device.get_mapping_info(0, 1).is_err());
    }
}
