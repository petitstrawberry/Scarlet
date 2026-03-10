//! Limine framebuffer-backed graphics device.

use alloc::{string::ToString, sync::Arc};
use core::any::Any;

use crate::{
    boot::limine::{LimineFramebufferInfo, framebuffer_info},
    device::{
        Device, DeviceType,
        graphics::{FramebufferConfig, GraphicsDevice, PixelFormat},
        manager::DeviceManager,
    },
    object::capability::{ControlOps, MemoryMappingOps, Selectable},
    vm::addr::virt_to_phys,
};

const DEVICE_NAME: &str = "limine-framebuffer";

fn bytes_per_pixel(bpp: u16) -> usize {
    (bpp as usize).div_ceil(8)
}

fn pixel_format_from_info(info: &LimineFramebufferInfo) -> Result<PixelFormat, &'static str> {
    match (
        bytes_per_pixel(info.bpp),
        info.red_mask_size,
        info.red_mask_shift,
        info.green_mask_size,
        info.green_mask_shift,
        info.blue_mask_size,
        info.blue_mask_shift,
    ) {
        (4, 8, 0, 8, 8, 8, 16) => Ok(PixelFormat::RGBA8888),
        (4, 8, 16, 8, 8, 8, 0) => Ok(PixelFormat::BGRA8888),
        (3, 8, 0, 8, 8, 8, 16) => Ok(PixelFormat::RGB888),
        (2, 5, 11, 6, 5, 5, 0) => Ok(PixelFormat::RGB565),
        _ => Err("Unsupported Limine framebuffer pixel format"),
    }
}

/// Registers the Limine boot framebuffer as a graphics device when available.
pub fn register_boot_framebuffer() -> Result<Option<usize>, &'static str> {
    let Some(info) = framebuffer_info() else {
        return Ok(None);
    };

    let device_manager = DeviceManager::get_manager();
    if let Some(device_id) = device_manager.get_device_id_by_name(DEVICE_NAME) {
        return Ok(Some(device_id));
    }

    let device: Arc<dyn Device> = Arc::new(LimineFramebufferDevice::new(info)?);
    Ok(Some(device_manager.register_device_with_name(
        DEVICE_NAME.to_string(),
        device,
    )))
}

/// Graphics device backed by the boot framebuffer provided by Limine.
pub struct LimineFramebufferDevice {
    config: FramebufferConfig,
    framebuffer_addr: usize,
}

impl LimineFramebufferDevice {
    /// Creates a framebuffer device from Limine-provided metadata.
    pub fn new(info: LimineFramebufferInfo) -> Result<Self, &'static str> {
        let format = pixel_format_from_info(&info)?;
        let minimum_pitch = info.width * format.bytes_per_pixel() as u32;
        if info.pitch < minimum_pitch {
            return Err("Limine framebuffer pitch is smaller than the logical width");
        }

        Ok(Self {
            config: FramebufferConfig::with_stride(info.width, info.height, format, info.pitch),
            framebuffer_addr: virt_to_phys(info.addr),
        })
    }
}

impl Device for LimineFramebufferDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Graphics
    }

    fn name(&self) -> &'static str {
        DEVICE_NAME
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

impl ControlOps for LimineFramebufferDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for LimineFramebufferDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by Limine framebuffer device")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for LimineFramebufferDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

impl GraphicsDevice for LimineFramebufferDevice {
    fn get_display_name(&self) -> &'static str {
        DEVICE_NAME
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

    fn sample_info(
        pitch: u32,
        bpp: u16,
        red_mask_shift: u8,
        green_mask_shift: u8,
        blue_mask_shift: u8,
    ) -> LimineFramebufferInfo {
        LimineFramebufferInfo {
            addr: 0x4000_0000,
            width: 800,
            height: 600,
            pitch,
            bpp,
            red_mask_size: if bpp == 16 { 5 } else { 8 },
            red_mask_shift,
            green_mask_size: if bpp == 16 { 6 } else { 8 },
            green_mask_shift,
            blue_mask_size: if bpp == 16 { 5 } else { 8 },
            blue_mask_shift,
        }
    }

    #[test_case]
    fn test_limine_framebuffer_uses_reported_pitch() {
        let info = sample_info(4096, 32, 16, 8, 0);
        let device = LimineFramebufferDevice::new(info).expect("Limine framebuffer should work");

        let config = device
            .get_framebuffer_config()
            .expect("Framebuffer config should be available");
        assert_eq!(config.width, 800);
        assert_eq!(config.height, 600);
        assert_eq!(config.format, PixelFormat::BGRA8888);
        assert_eq!(config.stride, 4096);
        assert_eq!(config.size(), 4096 * 600);
    }

    #[test_case]
    fn test_limine_framebuffer_supports_rgba8888() {
        let info = sample_info(800 * 4, 32, 0, 8, 16);
        let device = LimineFramebufferDevice::new(info).expect("RGBA8888 should be supported");

        assert_eq!(
            device
                .get_framebuffer_config()
                .expect("Framebuffer config should be available")
                .format,
            PixelFormat::RGBA8888
        );
        assert_eq!(
            device
                .get_framebuffer_address()
                .expect("Framebuffer address should be available"),
            info.addr
        );
    }

    #[test_case]
    fn test_limine_framebuffer_rejects_unknown_formats() {
        let info = sample_info(800 * 3, 24, 16, 8, 0);
        assert!(LimineFramebufferDevice::new(info).is_err());
    }
}
