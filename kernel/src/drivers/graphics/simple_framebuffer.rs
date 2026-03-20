use alloc::{boxed::Box, string::String, sync::Arc};

use crate::{
    device::{
        Device, DeviceInfo, DeviceType,
        graphics::{FramebufferConfig, GraphicsDevice, PixelFormat},
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
    framebuffer_addr: usize,
}

impl SimpleFramebufferDevice {
    fn new(
        name: &'static str,
        display_name: &'static str,
        config: FramebufferConfig,
        framebuffer_addr: usize,
    ) -> Self {
        Self {
            name,
            display_name,
            config,
            framebuffer_addr,
        }
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
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by simple framebuffer device")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

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
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

impl GraphicsDevice for SimpleFramebufferDevice {
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
        "a8r8g8b8" => Ok(PixelFormat::RGBA8888),
        "a8b8g8r8" => Ok(PixelFormat::BGRA8888),
        "x8r8g8b8" => Ok(PixelFormat::XRGB8888),
        "x8b8g8r8" => Ok(PixelFormat::XBGR8888),
        "x2r10g10b10" => Ok(PixelFormat::XRGB2101010),
        "r8g8b8" => Ok(PixelFormat::RGB888),
        "r5g6b5" => Ok(PixelFormat::RGB565),
        "a1r5g5b5" | "r5g5b5a1" => Ok(PixelFormat::ARGB1555),
        "x1r5g5b5" => Ok(PixelFormat::XRGB1555),
        _ => Err("Unsupported simple framebuffer pixel format"),
    }
}

fn framebuffer_resource(device: &PlatformDeviceInfo) -> Result<(usize, usize), &'static str> {
    let resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("No framebuffer memory resource found")?;
    Ok((resource.start, resource.end - resource.start + 1))
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
    ));
    let registered_name = String::from(device.name());

    let device_id = DeviceManager::get_manager()
        .register_device_with_name(registered_name, graphics_device.clone());

    crate::device::graphics::manager::GraphicsManager::get_manager()
        .register_framebuffer_from_device(device_id, graphics_device)?;

    if crate::earlyfb::is_initialized() {
        crate::earlyfb::deactivate();
    }

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
