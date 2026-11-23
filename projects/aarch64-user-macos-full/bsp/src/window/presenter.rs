use scarlet::device::graphics::output::DisplayRegion;
use std::sync::Arc;
use wgpu::*;
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    window::Window,
};

pub(super) struct Presenter {
    gpu: super::gpu::WindowGpu,
    acquisition: super::drawable::Acquisition,
    awaiting_drawable: bool,
    surface: Arc<Surface<'static>>,
    config: SurfaceConfiguration,
    configured: bool,
    device: Device,
    queue: Queue,
    texture: Texture,
    bindings: BindGroup,
    pipeline: RenderPipeline,
    have_frame: bool,
}

pub(super) struct Viewport {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Viewport {
    pub(super) fn guest_position(
        &self,
        position: PhysicalPosition<f64>,
        scanout: (u32, u32),
    ) -> Option<(i32, i32)> {
        let x = (position.x - f64::from(self.x)) / f64::from(self.width);
        let y = (position.y - f64::from(self.y)) / f64::from(self.height);
        ((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y)).then_some((
            (x * f64::from(scanout.0)) as i32,
            (y * f64::from(scanout.1)) as i32,
        ))
    }
}

pub(super) enum Draw {
    Complete,
    Retry,
}

impl Presenter {
    pub(super) fn open(
        window: Arc<Window>,
        width: u32,
        height: u32,
        wake: impl Fn() + Send + Sync + 'static,
        deliver: impl Fn(super::drawable::Drawable) + Send + 'static,
    ) -> Result<(Self, crate::gpu::Endpoint), String> {
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::METAL,
            ..Default::default()
        });
        let size = window.inner_size();
        let surface = Arc::new(instance.create_surface(window).map_err(|e| e.to_string())?);
        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .ok_or("no Metal adapter for the Scarlet window")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&DeviceDescriptor::default(), None))
                .map_err(|e| e.to_string())?;
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("no Metal surface configuration for the Scarlet window")?;
        let capabilities = surface.get_capabilities(&adapter);
        if let Some(format) = capabilities
            .formats
            .iter()
            .copied()
            .find(TextureFormat::is_srgb)
        {
            config.format = format;
        }
        // The scanout stores BGRA bytes. An opaque, format-matched drawable can
        // display them directly, including framebuffers whose alpha is unused.
        if matches!(
            config.format,
            TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb
        ) && capabilities.usages.contains(TextureUsages::COPY_DST)
            && capabilities
                .alpha_modes
                .contains(&CompositeAlphaMode::Opaque)
        {
            config.alpha_mode = CompositeAlphaMode::Opaque;
            config.usage |= TextureUsages::COPY_DST;
        }
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Scarlet scanout"),
            source: ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Scarlet scanout"),
            layout: None,
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                targets: &[Some(config.format.into())],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });
        let (texture, bindings) = Self::scanout(&device, &pipeline, config.format, width, height);
        let (gpu, endpoint) = super::gpu::WindowGpu::start(
            device.clone(),
            queue.clone(),
            texture.clone(),
            crate::gpu::config(),
            None,
            wake,
        )?;
        Ok((
            Self {
                gpu,
                acquisition: super::drawable::Acquisition::new(surface.clone(), deliver)?,
                awaiting_drawable: false,
                surface,
                config,
                configured: false,
                device,
                queue,
                texture,
                bindings,
                pipeline,
                have_frame: false,
            },
            endpoint,
        ))
    }

    fn scanout(
        device: &Device,
        pipeline: &RenderPipeline,
        format: TextureFormat,
        width: u32,
        height: u32,
    ) -> (Texture, BindGroup) {
        let texture = crate::gpu::create_scanout(device, format, width, height);
        let view = texture.create_view(&TextureViewDescriptor::default());
        let sampler = device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        let bindings = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Scarlet scanout"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&sampler),
                },
            ],
        });
        (texture, bindings)
    }
    pub(super) fn replace_scanout(&mut self, width: u32, height: u32) {
        (self.texture, self.bindings) = Self::scanout(
            &self.device,
            &self.pipeline,
            self.config.format,
            width,
            height,
        );
        self.gpu.resize(&self.texture);
        self.have_frame = false;
    }
    pub(super) fn scanout_size(&self) -> (u32, u32) {
        (self.texture.width(), self.texture.height())
    }

    pub(super) fn take_gpu_present(&mut self) -> bool {
        let presented = self.gpu.take_present(&self.texture);
        self.have_frame |= presented;
        presented
    }

    pub(super) fn have_frame(&self) -> bool {
        self.have_frame
    }

    pub(super) fn viewport(&self, size: PhysicalSize<u32>) -> Option<Viewport> {
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let (scanout_width, scanout_height) = self.scanout_size();
        let (width, height) = (size.width as f32, size.height as f32);
        let scale = (width / scanout_width as f32).min(height / scanout_height as f32);
        let draw_width = (scanout_width as f32 * scale).round().clamp(1.0, width);
        let draw_height = (scanout_height as f32 * scale).round().clamp(1.0, height);
        Some(Viewport {
            x: ((width - draw_width) / 2.0).floor(),
            y: ((height - draw_height) / 2.0).floor(),
            width: draw_width,
            height: draw_height,
        })
    }

    pub(super) fn upload(&mut self, pixels: &[u8], damage: DisplayRegion) -> Result<(), String> {
        crate::gpu::upload_scanout(
            &self.queue,
            &self.texture,
            &mut self.have_frame,
            pixels,
            damage,
        )
    }

    pub(super) fn request_draw(&mut self, size: PhysicalSize<u32>) -> Result<(), String> {
        if self.awaiting_drawable || size.width == 0 || size.height == 0 {
            return Ok(());
        }
        if !self.configured || self.config.width != size.width || self.config.height != size.height
        {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
            self.configured = true;
        }
        let trace = crate::latency_trace::frame(size.width, size.height);
        self.acquisition.request(trace)?;
        self.awaiting_drawable = true;
        Ok(())
    }

    pub(super) fn draw(
        &mut self,
        size: PhysicalSize<u32>,
        visible: bool,
        drawable: super::drawable::Drawable,
    ) -> Result<Draw, String> {
        self.awaiting_drawable = false;
        if !visible || size.width == 0 || size.height == 0 {
            return Ok(Draw::Complete);
        }
        if size.width != self.config.width || size.height != self.config.height {
            // Release the old drawable before reconfiguring the surface.
            drop(drawable);
            self.request_draw(size)?;
            return Ok(Draw::Complete);
        }
        let viewport = self.viewport(size).expect("nonzero surface size");
        let trace = drawable.trace;
        let frame = match drawable.result {
            Ok(frame) => frame,
            Err(SurfaceError::Outdated | SurfaceError::Lost) => {
                crate::latency_trace::surface(trace, "surface_acquire_retry");
                self.configured = false;
                return Ok(Draw::Retry);
            }
            Err(SurfaceError::Timeout) => {
                crate::latency_trace::surface(trace, "surface_acquire_retry");
                return Ok(Draw::Retry);
            }
            Err(error) => {
                crate::latency_trace::surface(trace, "surface_acquire_error");
                return Err(error.to_string());
            }
        };
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor::default());
        if self.have_frame
            && self.config.usage.contains(TextureUsages::COPY_DST)
            && self.texture.size() == frame.texture.size()
        {
            encoder.copy_texture_to_texture(
                self.texture.as_image_copy(),
                frame.texture.as_image_copy(),
                self.texture.size(),
            );
        } else {
            let view = frame.texture.create_view(&TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color::BLACK),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if self.have_frame {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bindings, &[]);
                pass.set_viewport(
                    viewport.x,
                    viewport.y,
                    viewport.width,
                    viewport.height,
                    0.0,
                    1.0,
                );
                pass.draw(0..3, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        crate::latency_trace::surface(trace, "surface_present_begin");
        frame.present();
        crate::latency_trace::surface(trace, "surface_present_end");
        Ok(Draw::Complete)
    }
}

const SHADER: &str = r#"
@group(0) @binding(0) var scanout: texture_2d<f32>;
@group(0) @binding(1) var filtering: sampler;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let uv = array<vec2<f32>, 3>(vec2(0.0, 0.0), vec2(2.0, 0.0), vec2(0.0, 2.0))[index];
    return Vertex(vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0), uv);
}

@fragment
fn fragment(input: Vertex) -> @location(0) vec4<f32> {
    return vec4(textureSample(scanout, filtering, input.uv).rgb, 1.0);
}
"#;
