//! CPU scanout through Scarlet's ordinary graphics discovery and display VMAs.
use scarlet::{
    arch::Trapframe,
    device::{
        Device, DeviceType,
        graphics::{
            FramebufferConfig, GpuDisplayResource, GpuPresentOptions, GraphicsDevice,
            PresentationOwners, PresentationWait, output::DisplayRegion,
        },
    },
    environment::PAGE_SIZE,
    mem::page::ContiguousPages,
    object::capability::{
        ControlOps, MemoryMappingInfo, MemoryMappingOps,
        selectable::{ReadyInterest, SelectWaitOutcome, Selectable},
    },
    vm::vmem::MemoryAttribute,
};
use std::{
    any::Any,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// Only owned pixel snapshots cross to the event-loop thread.
pub(super) struct Frames {
    pixels: Vec<u8>,
    damage: Option<DisplayRegion>,
    stride: usize,
}

impl Frames {
    pub fn new(config: &FramebufferConfig) -> Self {
        Self {
            pixels: vec![0; config.size()],
            damage: None,
            stride: config.stride as usize,
        }
    }
    fn rows(&self, region: DisplayRegion) -> impl Iterator<Item = std::ops::Range<usize>> {
        let stride = self.stride;
        (region.y..region.y + region.height).map(move |y| {
            let start = y as usize * stride + region.x as usize * 4;
            start..start + region.width as usize * 4
        })
    }
    pub fn take(&mut self, out: &mut [u8]) -> Option<DisplayRegion> {
        let region = self.damage.take()?;
        for row in self.rows(region) {
            out[row.clone()].copy_from_slice(&self.pixels[row]);
        }
        Some(region)
    }
}

pub(super) struct Scanout {
    config: FramebufferConfig,
    pages: ContiguousPages,
    frames: Arc<Mutex<Frames>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Scanout {
    pub fn new(
        config: FramebufferConfig,
        frames: Arc<Mutex<Frames>>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, &'static str> {
        let alignment =
            scarlet_hosted_runtime::ram::host_page_size().map_err(|_| "Invalid host page size")?;
        let count = config.size().div_ceil(alignment) * (alignment / PAGE_SIZE);
        let pages =
            ContiguousPages::new_aligned(count, alignment).ok_or("Cannot allocate scanout")?;
        Ok(Self {
            config,
            pages,
            frames,
            wake,
        })
    }
}

macro_rules! graphics_endpoint {
    ($ty:ty) => {
        impl Device for $ty {
            fn device_type(&self) -> DeviceType {
                DeviceType::Graphics
            }
            fn name(&self) -> &'static str {
                "macos-display"
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
        impl ControlOps for $ty {
            fn control(&self, _: u32, _: usize) -> Result<i32, &'static str> {
                Err("Use the display endpoint")
            }
        }
        impl MemoryMappingOps for $ty {
            fn get_mapping_info(
                &self,
                _: usize,
                _: usize,
            ) -> Result<MemoryMappingInfo, &'static str> {
                Err("Use the display endpoint")
            }
            fn supports_mmap(&self) -> bool {
                false
            }
        }
        impl Selectable for $ty {
            fn wait_until_ready(
                &self,
                _: ReadyInterest,
                _: &mut Trapframe,
                _: Option<u64>,
                _: u64,
            ) -> SelectWaitOutcome {
                SelectWaitOutcome::Ready
            }
        }
    };
}
graphics_endpoint!(Scanout);
graphics_endpoint!(Output);
impl GraphicsDevice for Scanout {
    fn get_display_name(&self) -> &'static str {
        "Scarlet macOS window"
    }
    fn init_graphics(&self) -> Result<(), &'static str> {
        Ok(())
    }
    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        Ok(self.config.clone())
    }
    fn get_framebuffer_address(&self) -> Result<u64, &'static str> {
        Ok(self.pages.as_paddr())
    }
    fn framebuffer_mapping_size(&self, config: &FramebufferConfig) -> Result<usize, &'static str> {
        padded_size(config)
    }
    fn framebuffer_memory_attribute(&self) -> MemoryAttribute {
        MemoryAttribute::Normal
    }
    fn present_framebuffer_region(
        &self,
        config: &FramebufferConfig,
        address: u64,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        if address != self.pages.as_paddr()
            || config.width != self.config.width
            || config.height != self.config.height
            || config.stride != self.config.stride
            || config.format != self.config.format
        {
            return Err("Stale scanout backing");
        }
        if region
            .x
            .checked_add(region.width)
            .is_none_or(|x| x > config.width)
            || region
                .y
                .checked_add(region.height)
                .is_none_or(|y| y > config.height)
        {
            return Err("Damage outside scanout");
        }
        if region.width == 0 || region.height == 0 {
            return Ok(());
        }
        let mut frames = self.frames.lock().unwrap();
        let stride = frames.stride;
        for y in region.y..region.y + region.height {
            let start = y as usize * stride + region.x as usize * 4;
            // The driver owns these mapped pages for the lifetime of the copy.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.pages.as_ptr().cast::<u8>().add(start),
                    frames.pixels.as_mut_ptr().add(start),
                    region.width as usize * 4,
                );
            }
        }
        let wake = frames.damage.is_none();
        frames.damage = Some(frames.damage.map_or(region, |prev| {
            let x = prev.x.min(region.x);
            let y = prev.y.min(region.y);
            DisplayRegion::new(
                x,
                y,
                (prev.x + prev.width).max(region.x + region.width) - x,
                (prev.y + prev.height).max(region.y + region.height) - y,
            )
        }));
        drop(frames);
        if wake {
            (self.wake)();
        }
        Ok(())
    }
}

fn padded_size(config: &FramebufferConfig) -> Result<usize, &'static str> {
    let page =
        scarlet_hosted_runtime::ram::host_page_size().map_err(|_| "Invalid host page size")?;
    config
        .size()
        .checked_next_multiple_of(page)
        .ok_or("Scanout size overflow")
}

pub(super) fn dimensions(w: u32, h: u32) -> u64 {
    (u64::from(w) << 32) | u64::from(h)
}

pub(super) struct Output {
    gpu: crate::gpu::Endpoint,
    // GraphicsManager's original fb0 resource retains its boot-time physical address.
    _boot: Arc<Scanout>,
    current: scarlet::sync::IrqRwSpinLock<(u64, Arc<Scanout>)>,
    requested: Arc<AtomicU64>,
    proxy: winit::event_loop::EventLoopProxy<super::UiEvent>,
}
impl Output {
    pub fn new(
        scanout: Scanout,
        gpu: crate::gpu::Endpoint,
        requested: Arc<AtomicU64>,
        proxy: winit::event_loop::EventLoopProxy<super::UiEvent>,
    ) -> Self {
        let scanout = Arc::new(scanout);
        Self {
            gpu,
            _boot: scanout.clone(),
            current: scarlet::sync::IrqRwSpinLock::new((0, scanout)),
            requested,
            proxy,
        }
    }
    fn snapshot(&self) -> Arc<Scanout> {
        let desired = self.requested.load(Ordering::Acquire);
        let current = self.current.read().1.clone();
        if dimensions(current.config.width, current.config.height) == desired {
            return current;
        }
        let config = FramebufferConfig::new(
            (desired >> 32) as u32,
            desired as u32,
            scarlet::device::graphics::PixelFormat::BGRA8888,
        );
        let frames = Arc::new(Mutex::new(Frames::new(&config)));
        let Ok(next) = Scanout::new(config.clone(), frames.clone(), current.wake.clone()) else {
            return current;
        };
        let next = Arc::new(next);
        let generation = {
            let mut guard = self.current.write();
            if !Arc::ptr_eq(&guard.1, &current) || self.requested.load(Ordering::Acquire) != desired
            {
                return guard.1.clone();
            }
            guard.0 += 1;
            guard.1 = next.clone();
            guard.0
        };
        let _ = self.proxy.send_event(super::UiEvent::Mode {
            generation,
            config,
            frames,
        });
        next
    }
}
impl GraphicsDevice for Output {
    fn begin_gpu_resource_present(
        &self,
        resource: GpuDisplayResource,
        region: DisplayRegion,
        _options: GpuPresentOptions,
        owners: PresentationOwners,
    ) -> Result<Option<Box<dyn PresentationWait>>, &'static str> {
        self.gpu.begin_present(resource, region, owners)
    }

    fn get_display_name(&self) -> &'static str {
        "Scarlet macOS window"
    }
    fn init_graphics(&self) -> Result<(), &'static str> {
        Ok(())
    }
    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        self.snapshot().get_framebuffer_config()
    }
    fn get_framebuffer_address(&self) -> Result<u64, &'static str> {
        self.snapshot().get_framebuffer_address()
    }
    fn get_framebuffer_info(&self) -> Result<(FramebufferConfig, u64), &'static str> {
        self.snapshot().get_framebuffer_info()
    }
    fn framebuffer_mapping_size(&self, c: &FramebufferConfig) -> Result<usize, &'static str> {
        padded_size(c)
    }
    fn framebuffer_memory_attribute(&self) -> MemoryAttribute {
        MemoryAttribute::Normal
    }
    fn connector_mapping_device(
        &self,
        index: u32,
    ) -> Result<Option<Arc<dyn Device>>, &'static str> {
        if index != 0 {
            return Err("Unknown macOS connector");
        }
        Ok(Some(self.snapshot()))
    }
    fn present_framebuffer_region(
        &self,
        c: &FramebufferConfig,
        pa: u64,
        r: DisplayRegion,
    ) -> Result<(), &'static str> {
        let backing = self.current.read().1.clone();
        backing.present_framebuffer_region(c, pa, r)
    }
    fn present_current_framebuffer_region(
        &self,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        let backing = self.current.read().1.clone();
        // The caller may have clipped against the previous size during a resize.
        let w = backing
            .config
            .width
            .saturating_sub(region.x)
            .min(region.width);
        let h = backing
            .config
            .height
            .saturating_sub(region.y)
            .min(region.height);
        if w == 0 || h == 0 {
            return Ok(());
        }
        backing.present_framebuffer_region(
            &backing.config,
            backing.pages.as_paddr(),
            DisplayRegion::new(region.x, region.y, w, h),
        )
    }
}
