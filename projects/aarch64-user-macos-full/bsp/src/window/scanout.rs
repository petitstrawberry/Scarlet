//! macOS display endpoint with a resizable shared CPU scanout.
use scarlet::{
    arch::Trapframe,
    device::{
        Device, DeviceType,
        graphics::{
            FramebufferConfig, GpuDisplayResource, GpuPresentOptions, GraphicsDevice,
            PresentationOwners, PresentationWait, output::DisplayRegion,
        },
    },
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

#[path = "../../../../shared/hosted/scanout.rs"]
mod common;
pub(super) use common::{Frames, Scanout};
use common::{graphics_endpoint, padded_size};

graphics_endpoint!(Output, |_: &Output| "macos-display");

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
        let Ok(next) = Scanout::new(
            "macos-display",
            "Scarlet macOS window",
            config.clone(),
            frames.clone(),
            current.wake.clone(),
        ) else {
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
