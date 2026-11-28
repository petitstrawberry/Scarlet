//! Presents Scarlet's retained scanout on the SwiftUI view's Metal layer.
use crate::{display, sgfx};
use std::{
    ffi::c_void,
    sync::{Arc, Mutex, OnceLock, mpsc},
    thread,
    time::Duration,
};
use wgpu::*;

const SGFX_IRQ: u32 = 23;
const FORMAT: TextureFormat = TextureFormat::Bgra8Unorm;
const DRAWABLE_RETRY: Duration = Duration::from_millis(16);
type Viewport = Arc<Mutex<Option<(u32, u32)>>>;

static PRESENTER: OnceLock<Presenter> = OnceLock::new();

pub(crate) struct Presenter {
    device: Device,
    queue: Queue,
    worker: Arc<sgfx::Worker>,
    endpoint: sgfx::Endpoint,
    gate: Arc<sgfx::SubmissionGate>,
    viewport: Viewport,
    wake: mpsc::SyncSender<()>,
}

pub(crate) fn presenter() -> Option<&'static Presenter> {
    PRESENTER.get()
}

impl Presenter {
    fn open(layer: *mut c_void, width: u32, height: u32) -> Result<Self, String> {
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::METAL,
            ..Default::default()
        });
        // Swift creates the CAMetalLayer on the main thread and keeps it
        // alive for the session. wgpu retains it while the surface exists.
        let surface = unsafe {
            instance.create_surface_unsafe(SurfaceTargetUnsafe::CoreAnimationLayer(layer))
        }
        .map_err(|error| error.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .ok_or("No Metal adapter for the Scarlet view")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&DeviceDescriptor::default(), None))
                .map_err(|error| error.to_string())?;
        let capabilities = surface.get_capabilities(&adapter);
        if !capabilities.formats.contains(&FORMAT)
            || !capabilities.usages.contains(TextureUsages::COPY_DST)
            || !capabilities
                .alpha_modes
                .contains(&CompositeAlphaMode::Opaque)
        {
            return Err("Metal drawable does not support opaque BGRA scanout".into());
        }
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_DST,
            format: FORMAT,
            width,
            height,
            present_mode: PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: CompositeAlphaMode::Opaque,
            view_formats: vec![],
        };
        let (wake, pending) = mpsc::sync_channel(1);
        let gpu_wake = wake.clone();
        let scanout = sgfx::create_scanout(&device, FORMAT, width, height);
        let gate = Arc::new(sgfx::SubmissionGate::new());
        let (worker, endpoint) = sgfx::Worker::start(
            device.clone(),
            queue.clone(),
            scanout,
            sgfx::Config {
                irq: SGFX_IRQ,
                trace: None,
            },
            Some(gate.clone()),
            move || {
                let _ = gpu_wake.try_send(());
            },
        )?;
        let worker = Arc::new(worker);
        let viewport = Viewport::default();
        let mut renderer = Renderer {
            surface,
            config,
            configured: false,
            device: device.clone(),
            queue: queue.clone(),
            worker: worker.clone(),
            gate: gate.clone(),
            viewport: viewport.clone(),
            pixels: Vec::new(),
            generation: 0,
            have_frame: false,
            gpu_frame: false,
            saw_gpu: false,
        };
        thread::Builder::new()
            .name("scarlet-metal-present".into())
            .spawn(move || renderer.run(pending))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            device,
            queue,
            worker,
            endpoint,
            gate,
            viewport,
            wake,
        })
    }

    pub(crate) fn create_scanout(&self, width: u32, height: u32) -> Texture {
        sgfx::create_scanout(&self.device, FORMAT, width, height)
    }

    pub(crate) fn use_scanout(&self, scanout: &Texture) {
        self.worker.resize(scanout);
    }

    pub(crate) fn endpoint(&self) -> &sgfx::Endpoint {
        &self.endpoint
    }

    pub(crate) fn redraw(&self) {
        let _ = self.wake.try_send(());
    }
}

struct Renderer {
    surface: Surface<'static>,
    config: SurfaceConfiguration,
    configured: bool,
    device: Device,
    queue: Queue,
    worker: Arc<sgfx::Worker>,
    gate: Arc<sgfx::SubmissionGate>,
    viewport: Viewport,
    pixels: Vec<u8>,
    generation: u64,
    have_frame: bool,
    gpu_frame: bool,
    saw_gpu: bool,
}

impl Renderer {
    fn run(&mut self, pending: mpsc::Receiver<()>) {
        let mut retry = false;
        loop {
            let wake = if retry {
                !matches!(
                    pending.recv_timeout(DRAWABLE_RETRY),
                    Err(mpsc::RecvTimeoutError::Disconnected)
                )
            } else {
                pending.recv().is_ok()
            };
            if !wake {
                break;
            }
            retry = self.draw().unwrap_or_else(|error| {
                eprintln!("Scarlet Metal display: {error}");
                false
            });
        }
    }

    fn draw(&mut self) -> Result<bool, String> {
        let Some((width, height)) = *self.viewport.lock().unwrap() else {
            return Ok(false);
        };
        let Some(mode) = display::current_mode() else {
            return Ok(false);
        };
        let Some(texture) = &mode.texture else {
            return Ok(false);
        };
        // A corresponding guest mode will arrive after the window layout.
        if texture.width() != ((width + 1) & !1) || texture.height() != ((height + 1) & !1) {
            return Ok(false);
        }
        let gate = self.gate.clone();
        if !gate
            .while_active(|| self.prepare(&mode, texture, width, height))
            .transpose()?
            .unwrap_or(false)
        {
            return Ok(false);
        }
        // Drawable acquisition may block on Core Animation. Leave the
        // lifecycle gate available to UIKit while waiting for it.
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(SurfaceError::Timeout) => return Ok(true),
            Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                self.configured = false;
                return Ok(true);
            }
            Err(error) => return Err(error.to_string()),
        };
        gate.while_active(|| self.present(texture, frame, width, height));
        Ok(false)
    }

    fn prepare(
        &mut self,
        mode: &display::Mode,
        texture: &Texture,
        width: u32,
        height: u32,
    ) -> Result<bool, String> {
        if self.generation != mode.generation {
            self.generation = mode.generation;
            self.pixels.clear();
            self.pixels
                .resize((texture.width() * texture.height() * 4) as usize, 0);
            self.have_frame = false;
            self.gpu_frame = false;
        }
        if let Some(damage) = mode.frames.lock().unwrap().take(&mut self.pixels) {
            sgfx::upload_scanout(
                &self.queue,
                texture,
                &mut self.have_frame,
                &self.pixels,
                damage,
            )?;
        }
        self.gpu_frame |= self.worker.take_present(texture);
        self.have_frame |= self.gpu_frame;
        if !self.have_frame {
            return Ok(false);
        }
        if !self.configured || (self.config.width, self.config.height) != (width, height) {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
            self.configured = true;
        }
        Ok(true)
    }

    fn present(&mut self, texture: &Texture, frame: SurfaceTexture, width: u32, height: u32) {
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor::default());
        encoder.copy_texture_to_texture(
            texture.as_image_copy(),
            frame.texture.as_image_copy(),
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        frame.present();
        if self.gpu_frame && !self.saw_gpu {
            self.saw_gpu = true;
            eprintln!("Scarlet iOS: presented SGFX frame on Metal ({width}x{height})");
        }
        self.gpu_frame = false;
    }
}

/// Swift calls this on the main thread with its session-owned CAMetalLayer.
#[unsafe(no_mangle)]
pub extern "C" fn scarlet_ios_open_metal(layer: *mut c_void, width: u32, height: u32) -> bool {
    if presenter().is_some() {
        return true;
    }
    if layer.is_null() || !display::valid_size(width, height) {
        return false;
    }
    match Presenter::open(layer, width, height) {
        Ok(presenter) => PRESENTER.set(presenter).is_ok(),
        Err(error) => {
            eprintln!("Scarlet iOS: {error}");
            false
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_ios_metal_viewport(width: u32, height: u32) {
    if let Some(presenter) = presenter() {
        *presenter.viewport.lock().unwrap() =
            display::valid_size(width, height).then_some((width, height));
        presenter.redraw();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_ios_metal_set_active(active: bool) {
    if let Some(presenter) = presenter() {
        presenter
            .gate
            .set_active(active, &presenter.device, &presenter.queue);
        if active {
            presenter.redraw();
        }
    }
}
