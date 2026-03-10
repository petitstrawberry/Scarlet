//! Limine boot framebuffer registration.

use alloc::{string::ToString, sync::Arc};

use crate::{
    boot::limine::{LimineFramebufferInfo, framebuffer_info},
    device::{
        Device,
        graphics::{FramebufferConfig, PixelFormat},
        manager::DeviceManager,
    },
    drivers::graphics::framebuffer::FramebufferDevice,
    vm::addr::virt_to_phys,
};

const DEVICE_NAME: &str = "boot-framebuffer";

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

fn framebuffer_config_from_info(
    info: LimineFramebufferInfo,
) -> Result<FramebufferConfig, &'static str> {
    let format = pixel_format_from_info(&info)?;
    let minimum_pitch = info.width * format.bytes_per_pixel() as u32;
    if info.pitch < minimum_pitch {
        return Err("Limine framebuffer pitch is smaller than the minimum required pitch");
    }

    Ok(FramebufferConfig::with_stride(
        info.width,
        info.height,
        format,
        info.pitch,
    ))
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

    let config = framebuffer_config_from_info(info)?;
    let device: Arc<dyn Device> = Arc::new(FramebufferDevice::new(
        "framebuffer",
        config,
        virt_to_phys(info.addr),
    )?);
    Ok(Some(device_manager.register_device_with_name(
        DEVICE_NAME.to_string(),
        device,
    )))
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
    fn test_framebuffer_config_from_info_supports_rgb_formats() {
        let bgra = framebuffer_config_from_info(sample_info(4096, 32, 16, 8, 0))
            .expect("BGRA8888 should be supported");
        assert_eq!(bgra.format, PixelFormat::BGRA8888);
        assert_eq!(bgra.stride, 4096);

        let rgba = framebuffer_config_from_info(sample_info(800 * 4, 32, 0, 8, 16))
            .expect("RGBA8888 should be supported");
        assert_eq!(rgba.format, PixelFormat::RGBA8888);

        let rgb888 = framebuffer_config_from_info(sample_info(800 * 3, 24, 0, 8, 16))
            .expect("RGB888 should be supported");
        assert_eq!(rgb888.format, PixelFormat::RGB888);

        let rgb565 = framebuffer_config_from_info(sample_info(800 * 2, 16, 11, 5, 0))
            .expect("RGB565 should be supported");
        assert_eq!(rgb565.format, PixelFormat::RGB565);
    }

    #[test_case]
    fn test_framebuffer_config_from_info_rejects_invalid_formats() {
        assert!(framebuffer_config_from_info(sample_info(800 * 3, 24, 16, 8, 0)).is_err());
        assert!(framebuffer_config_from_info(sample_info(800 * 4 - 4, 32, 16, 8, 0)).is_err());
    }
}
