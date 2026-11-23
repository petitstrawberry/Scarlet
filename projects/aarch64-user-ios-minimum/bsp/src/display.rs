//! Connect a UIKit view to Scarlet's ordinary display endpoint.

#[path = "../../../shared/hosted/scanout.rs"]
mod scanout;

use scarlet::{
    arch::Trapframe,
    device::{
        Device, DeviceType,
        graphics::{
            FramebufferConfig, GpuDisplayResource, GpuPresentOptions, GraphicsDevice, PixelFormat,
            PresentationOwners, PresentationWait, output::DisplayRegion,
        },
        input::{
            event_device::{EventDevice, INPUT_CAP_INTERNAL, InputDeviceKind, InputDeviceMetadata},
            event_types::{EV_SW, EV_SYN},
            switch_codes::SW_TABLET_MODE,
            syn_codes::SYN_REPORT,
        },
        manager::DeviceManager,
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
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

pub(crate) type Damage = extern "C" fn();
const NAME: &str = "ios-display";
const LABEL: &str = "Scarlet iOS display";
const MAX_EDGE: u32 = 4096;

#[derive(Clone)]
pub(crate) struct Mode {
    scanout: Arc<scanout::Scanout>,
    pub(crate) frames: Arc<Mutex<scanout::Frames>>,
    pub(crate) generation: u64,
    pub(crate) texture: Option<wgpu::Texture>,
}

impl Mode {
    fn new(
        width: u32,
        height: u32,
        generation: u64,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, &'static str> {
        let config = FramebufferConfig::new(width, height, PixelFormat::BGRA8888);
        let frames = Arc::new(Mutex::new(scanout::Frames::new(&config)));
        let scanout = scanout::Scanout::new(NAME, LABEL, config, frames.clone(), wake)?;
        Ok(Self {
            scanout: Arc::new(scanout),
            frames,
            generation,
            texture: crate::metal::presenter()
                .map(|presenter| presenter.create_scanout(width, height)),
        })
    }
}

struct Output {
    // The graphics manager retains the original fb0 physical address.
    _boot: Arc<scanout::Scanout>,
    current: Mutex<Mode>,
    gpu: Option<crate::sgfx::Endpoint>,
}

static OUTPUT: OnceLock<Arc<Output>> = OnceLock::new();
static REQUESTED: AtomicU64 = AtomicU64::new(0);

fn dimensions(width: u32, height: u32) -> u64 {
    u64::from(width) << 32 | u64::from(height)
}

pub(crate) fn valid_size(width: u32, height: u32) -> bool {
    (2..=MAX_EDGE).contains(&width) && (2..=MAX_EDGE).contains(&height)
}

impl Output {
    fn snapshot(&self) -> Arc<scanout::Scanout> {
        let desired = REQUESTED.load(Ordering::Acquire);
        let current = self.current.lock().unwrap().clone();
        let config = &current.scanout.config;
        if dimensions(config.width, config.height) == desired {
            return current.scanout;
        }
        let Ok(next) = Mode::new(
            (desired >> 32) as u32,
            desired as u32,
            current.generation + 1,
            current.scanout.wake.clone(),
        ) else {
            return current.scanout;
        };
        let mut published = self.current.lock().unwrap();
        let mut mode_changed = false;
        if published.generation == current.generation
            && REQUESTED.load(Ordering::Acquire) == desired
        {
            if let (Some(presenter), Some(texture)) = (crate::metal::presenter(), &next.texture) {
                presenter.use_scanout(texture);
            }
            *published = next;
            mode_changed = true;
        }
        let scanout = published.scanout.clone();
        drop(published);
        if mode_changed && let Some(presenter) = crate::metal::presenter() {
            presenter.redraw();
        }
        scanout
    }
}

scanout::graphics_endpoint!(Output, |_: &Output| NAME);

impl GraphicsDevice for Output {
    fn begin_gpu_resource_present(
        &self,
        resource: GpuDisplayResource,
        region: DisplayRegion,
        _options: GpuPresentOptions,
        owners: PresentationOwners,
    ) -> Result<Option<Box<dyn PresentationWait>>, &'static str> {
        match &self.gpu {
            Some(endpoint) => endpoint.begin_present(resource, region, owners),
            None => Ok(None),
        }
    }

    fn get_display_name(&self) -> &'static str {
        LABEL
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

    fn framebuffer_mapping_size(&self, config: &FramebufferConfig) -> Result<usize, &'static str> {
        scanout::padded_size(config)
    }

    fn framebuffer_memory_attribute(&self) -> MemoryAttribute {
        MemoryAttribute::Normal
    }

    fn connector_mapping_device(
        &self,
        index: u32,
    ) -> Result<Option<Arc<dyn Device>>, &'static str> {
        if index != 0 {
            return Err("Unknown iOS connector");
        }
        Ok(Some(self.snapshot()))
    }

    fn present_framebuffer_region(
        &self,
        config: &FramebufferConfig,
        address: u64,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        let scanout = self.current.lock().unwrap().scanout.clone();
        scanout.present_framebuffer_region(config, address, region)
    }

    fn present_current_framebuffer_region(
        &self,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        let scanout = self.current.lock().unwrap().scanout.clone();
        // The caller may still be presenting damage from the previous mode.
        let width = region
            .width
            .min(scanout.config.width.saturating_sub(region.x));
        let height = region
            .height
            .min(scanout.config.height.saturating_sub(region.y));
        if width == 0 || height == 0 {
            return Ok(());
        }
        scanout.present_current_framebuffer_region(DisplayRegion::new(
            region.x, region.y, width, height,
        ))
    }
}

#[repr(C)]
pub struct Configuration {
    pub width: u32,
    pub height: u32,
    pub tablet: bool,
    pub damage: Damage,
}

impl Configuration {
    pub fn register(&self, manager: &DeviceManager) -> Result<(), &'static str> {
        let metadata = InputDeviceMetadata::new(InputDeviceKind::Switch, INPUT_CAP_INTERNAL)
            .with_switch(SW_TABLET_MODE)?;
        let posture = Arc::new(EventDevice::new_with_metadata("switch", metadata));
        posture.push_events(&[
            (EV_SW, SW_TABLET_MODE, i32::from(self.tablet)),
            (EV_SYN, SYN_REPORT, 0),
        ]);
        manager.register_device_with_name(posture.get_name().into(), posture);

        if self.width == 0 || self.height == 0 {
            return Ok(());
        }
        if !valid_size(self.width, self.height) {
            return Err("Invalid iOS scanout dimensions");
        }
        let _ = REQUESTED.compare_exchange(
            0,
            dimensions(self.width, self.height),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        let requested = REQUESTED.load(Ordering::Acquire);
        let damage = self.damage;
        let presenter = crate::metal::presenter();
        let mode = Mode::new(
            (requested >> 32) as u32,
            requested as u32,
            1,
            Arc::new(move || {
                if let Some(presenter) = presenter {
                    presenter.redraw();
                } else {
                    damage();
                }
            }),
        )?;
        let gpu = match (presenter, &mode.texture) {
            (Some(presenter), Some(texture)) => {
                presenter.use_scanout(texture);
                match presenter.endpoint().register() {
                    Ok(()) => Some(presenter.endpoint().clone()),
                    Err(error) => {
                        eprintln!("Scarlet iOS: cannot register SGFX: {error}");
                        None
                    }
                }
            }
            _ => None,
        };
        let output = Arc::new(Output {
            _boot: mode.scanout.clone(),
            current: Mutex::new(mode),
            gpu,
        });
        OUTPUT
            .set(output.clone())
            .map_err(|_| "iOS scanout is already registered")?;
        manager.register_device(output);
        crate::touch::register(manager)?;
        crate::buttons::register(manager)?;
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_ios_set_display_size(width: u32, height: u32) {
    if valid_size(width, height) {
        REQUESTED.store(dimensions(width, height), Ordering::Release);
    }
}

#[repr(C)]
#[derive(Default)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    pub generation: u64,
}

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_ios_frame_info() -> FrameInfo {
    OUTPUT.get().map_or_else(FrameInfo::default, |output| {
        let current = output.current.lock().unwrap();
        FrameInfo {
            width: current.scanout.config.width,
            height: current.scanout.config.height,
            generation: current.generation,
        }
    })
}

pub(crate) fn current_mode() -> Option<Mode> {
    OUTPUT
        .get()
        .map(|output| output.current.lock().unwrap().clone())
}

/// Copy current damage to the caller's persistent, tightly packed BGRA buffer.
/// # Safety
/// The buffer must be writable for `length` bytes and exclusively borrowed
/// until this call returns. UIKit keeps the same buffer until the next frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_ios_copy_frame(
    buffer: *mut u8,
    length: usize,
    generation: u64,
) -> bool {
    let Some(output) = OUTPUT.get() else {
        return false;
    };
    let mode = output.current.lock().unwrap().clone();
    if buffer.is_null() || length != mode.scanout.config.size() || mode.generation != generation {
        return false;
    }
    let pixels = unsafe { std::slice::from_raw_parts_mut(buffer, length) };
    let copied = mode.frames.lock().unwrap().take(pixels).is_some();
    copied
}
