use crate::{mem::page::ContiguousPages, vm::vmem::MemoryAttribute};
use alloc::{boxed::Box, string::String, sync::Arc};

use crate::{
    device::{
        Device, DeviceInfo, DeviceType,
        graphics::{FramebufferConfig, GraphicsDevice, PixelFormat, output::DisplayRegion},
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_initcall,
    object::capability::{ControlOps, MemoryMappingOps, Selectable},
};

pub struct SimpleFramebufferDevice {
    name: &'static str,
    display_name: &'static str,
    config: FramebufferConfig,
    framebuffer_addr: u64,
    scanout_config: FramebufferConfig,
    scanout_addr: u64,
    shadow: Option<ContiguousPages>,
}

impl SimpleFramebufferDevice {
    fn new(
        name: &'static str,
        display_name: &'static str,
        config: FramebufferConfig,
        framebuffer_addr: u64,
        rotation: u32,
    ) -> Result<Self, &'static str> {
        let scanout_config = config.clone();
        let (config, shadow) = match rotation {
            0 => (config, None),
            3 => {
                if framebuffer_addr & 3 != 0 || config.stride & 3 != 0 {
                    return Err("Rotation requires aligned framebuffer pixels");
                }
                if !matches!(
                    config.format,
                    PixelFormat::RGBA8888
                        | PixelFormat::BGRA8888
                        | PixelFormat::XRGB8888
                        | PixelFormat::XBGR8888
                ) {
                    return Err("Rotation requires a 32-bit RGB framebuffer");
                }
                let config =
                    FramebufferConfig::new(config.height, config.width, PixelFormat::BGRA8888);
                let size = config.size();
                if size > 16 * 1024 * 1024 {
                    return Err("Rotated framebuffer is too large");
                }
                let pages = size.div_ceil(crate::environment::PAGE_SIZE);
                let shadow =
                    ContiguousPages::new(pages).ok_or("Cannot allocate rotated framebuffer")?;
                (config, Some(shadow))
            }
            _ => return Err("Unsupported framebuffer rotation"),
        };
        let exported_addr = shadow.as_ref().map_or(framebuffer_addr, |p| p.as_paddr());
        Ok(Self {
            name,
            display_name,
            config,
            framebuffer_addr: exported_addr,
            scanout_config,
            scanout_addr: framebuffer_addr,
            shadow,
        })
    }
}

impl Device for SimpleFramebufferDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Graphics
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn as_graphics_device(&self) -> Option<&dyn GraphicsDevice> {
        Some(self)
    }
}

impl ControlOps for SimpleFramebufferDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for SimpleFramebufferDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported by simple framebuffer device")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: u64, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for SimpleFramebufferDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

impl GraphicsDevice for SimpleFramebufferDevice {
    fn is_boot_framebuffer(&self) -> bool {
        true
    }

    fn get_display_name(&self) -> &'static str {
        self.display_name
    }

    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        Ok(self.config.clone())
    }

    fn get_framebuffer_address(&self) -> Result<u64, &'static str> {
        Ok(self.framebuffer_addr)
    }

    fn framebuffer_memory_attribute(&self) -> MemoryAttribute {
        if self.shadow.is_some() {
            MemoryAttribute::Normal
        } else {
            MemoryAttribute::DeviceBurstable
        }
    }

    fn present_framebuffer_region(
        &self,
        config: &FramebufferConfig,
        physical_addr: u64,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        if let Some(shadow) = &self.shadow {
            if physical_addr != self.framebuffer_addr
                || config.width != self.config.width
                || config.height != self.config.height
                || config.stride != self.config.stride
                || config.format != self.config.format
            {
                return Err("Unsupported rotated framebuffer backing");
            }
            let end_x = region
                .x
                .checked_add(region.width)
                .ok_or("Framebuffer region overflows")?;
            let end_y = region
                .y
                .checked_add(region.height)
                .ok_or("Framebuffer region overflows")?;
            if end_x > config.width || end_y > config.height {
                return Err("Framebuffer region exceeds the surface");
            }
            let scanout = crate::vm::addr::phys_to_virt(self.scanout_addr);
            let red_low = matches!(
                self.scanout_config.format,
                PixelFormat::RGBA8888 | PixelFormat::XRGB8888
            );
            for y in region.y..end_y {
                for x in region.x..end_x {
                    let source = shadow.as_vaddr() + (y * config.stride + x * 4) as usize;
                    let mut pixel = unsafe { core::ptr::read_volatile(source as *const u32) };
                    if red_low {
                        pixel =
                            (pixel & 0xff00ff00) | ((pixel & 0xff) << 16) | ((pixel >> 16) & 0xff);
                    }
                    let offset = (config.width - 1 - x) * self.scanout_config.stride + y * 4;
                    unsafe {
                        core::ptr::write_volatile(
                            (scanout + offset as usize) as *mut u32,
                            pixel | 0xff000000,
                        );
                    }
                }
            }
        }
        crate::earlyfb::deactivate();
        Ok(())
    }

    fn init_graphics(&self) -> Result<(), &'static str> {
        Ok(())
    }
}

fn property_u32(device: &PlatformDeviceInfo, name: &str) -> Result<u32, &'static str> {
    let property = device
        .property(name)
        .ok_or("Missing framebuffer property")?;
    let value = property
        .as_usize()
        .ok_or("Invalid framebuffer property value")?;
    u32::try_from(value).map_err(|_| "Framebuffer property out of range")
}

fn property_str<'a>(device: &'a PlatformDeviceInfo, name: &str) -> Result<&'a str, &'static str> {
    device
        .property(name)
        .and_then(|property| property.as_str())
        .ok_or("Missing framebuffer string property")
}

fn log_probe_properties(device: &PlatformDeviceInfo) {
    let status = device
        .property("status")
        .and_then(|property| property.as_str())
        .unwrap_or("<missing>");
    let width = device
        .property("width")
        .and_then(|property| property.as_usize());
    let height = device
        .property("height")
        .and_then(|property| property.as_usize());
    let stride = device
        .property("stride")
        .and_then(|property| property.as_usize());
    let format = device
        .property("format")
        .and_then(|property| property.as_str())
        .unwrap_or("<missing>");
    let mem_resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM));

    match mem_resource {
        Some(resource) => crate::println!(
            "[simplefb] probe name={} compatible={:?} status={} reg={:#x}..={:#x} width={:?} height={:?} stride={:?} format={}",
            device.name(),
            device.compatible(),
            status,
            resource.start,
            resource.end,
            width,
            height,
            stride,
            format,
        ),
        None => crate::println!(
            "[simplefb] probe name={} compatible={:?} status={} reg=<missing> width={:?} height={:?} stride={:?} format={}",
            device.name(),
            device.compatible(),
            status,
            width,
            height,
            stride,
            format,
        ),
    }
}

fn parse_pixel_format(device: &PlatformDeviceInfo) -> Result<PixelFormat, &'static str> {
    match property_str(device, "format")? {
        "a8r8g8b8" => Ok(PixelFormat::BGRA8888),
        "a8b8g8r8" => Ok(PixelFormat::RGBA8888),
        "x8r8g8b8" => Ok(PixelFormat::XBGR8888),
        "x8b8g8r8" => Ok(PixelFormat::XRGB8888),
        "x2r10g10b10" => Ok(PixelFormat::XRGB2101010),
        "r8g8b8" => Ok(PixelFormat::RGB888),
        "r5g6b5" => Ok(PixelFormat::RGB565),
        "a1r5g5b5" | "r5g5b5a1" => Ok(PixelFormat::ARGB1555),
        "x1r5g5b5" => Ok(PixelFormat::XRGB1555),
        _ => Err("Unsupported simple framebuffer pixel format"),
    }
}

fn framebuffer_resource(device: &PlatformDeviceInfo) -> Result<(u64, usize), &'static str> {
    let resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("No framebuffer memory resource found")?;
    Ok((resource.start, resource.size()?))
}

fn device_status_allows_probe(device: &PlatformDeviceInfo) -> bool {
    match device
        .property("status")
        .and_then(|property| property.as_str())
    {
        Some("disabled") => false,
        Some(_) | None => true,
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    if !device_status_allows_probe(device) {
        return Err("simple framebuffer is disabled");
    }

    let (framebuffer_addr, framebuffer_size) = framebuffer_resource(device)?;
    let width = property_u32(device, "width")?;
    let height = property_u32(device, "height")?;
    let stride = property_u32(device, "stride")?;
    let format = parse_pixel_format(device)?;
    let rotation = device
        .property("scarlet,rotation")
        .and_then(|p| p.as_usize())
        .unwrap_or(0);
    if !(64..=4096).contains(&width)
        || !(64..=4096).contains(&height)
        || stride < width * format.bytes_per_pixel() as u32
        || stride.checked_mul(height).is_none()
    {
        return Err("Invalid simple framebuffer dimensions or stride");
    }

    let config = FramebufferConfig {
        width,
        height,
        format,
        stride,
    };

    if config.size() > framebuffer_size {
        return Err("simple framebuffer memory resource is too small");
    }

    let display_name = match device.compatible().as_slice() {
        compatibles if compatibles.contains(&"apple,simple-framebuffer") => {
            "apple-simple-framebuffer"
        }
        _ => "simple-framebuffer",
    };

    let graphics_device = Arc::new(SimpleFramebufferDevice::new(
        device.name(),
        display_name,
        config,
        framebuffer_addr,
        u32::try_from(rotation).map_err(|_| "Invalid framebuffer rotation")?,
    )?);
    let registered_name = String::from(device.name());

    let device_id = DeviceManager::get_manager()
        .register_device_with_name(registered_name, graphics_device.clone());

    crate::device::graphics::manager::GraphicsManager::get_manager()
        .register_framebuffer_from_device(device_id, graphics_device)?;

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "simple-framebuffer",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,simple-framebuffer", "simple-framebuffer"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Late);
}

early_initcall!(register_driver);
