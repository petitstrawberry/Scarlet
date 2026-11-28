//! Resize an SGFX image into the retained scanout, preserving undamaged pixels.
use super::Rect;
use wgpu::*;

pub(super) struct ScanoutScaler(RenderPipeline);

impl ScanoutScaler {
    pub fn new(device: &Device) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("SGFX scaled damage"),
            source: ShaderSource::Wgsl(SHADER.into()),
        });
        Self(device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("SGFX scaled damage"),
            layout: None,
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                targets: &[Some(TextureFormat::Bgra8Unorm.into())],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        }))
    }

    pub fn encode(
        &self,
        device: &Device,
        encoder: &mut CommandEncoder,
        src: &Texture,
        dst: &Texture,
        damage: Rect,
    ) {
        let source = src.create_view(&Default::default());
        let target = dst.create_view(&TextureViewDescriptor {
            format: Some(TextureFormat::Bgra8Unorm),
            ..Default::default()
        });
        let bindings = device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &self.0.get_bind_group_layout(0),
            entries: &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(&source),
            }],
        });
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("SGFX scaled damage"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &target,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Load,
                    store: StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.0);
        pass.set_bind_group(0, &bindings, &[]);
        pass.set_scissor_rect(damage.x, damage.y, damage.width, damage.height);
        pass.draw(0..3, 0..1);
    }
}

const SHADER: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
@vertex fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return Vertex(vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0), uv);
}
@fragment fn fragment(input: Vertex) -> @location(0) vec4<f32> {
    return textureLoad(src, vec2<i32>(input.uv * vec2<f32>(textureDimensions(src))), 0);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a Metal adapter"]
    fn scaled_damage_preserves_bgra_bytes_and_surrounding_pixels() {
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::METAL,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default(), None)).unwrap();
        let texture = |width, height, format, usage, view_formats: &[TextureFormat]| {
            device.create_texture(&TextureDescriptor {
                label: None,
                size: Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format,
                usage,
                view_formats,
            })
        };
        let write = |t: &Texture, bytes: &[u8]| {
            queue.write_texture(
                t.as_image_copy(),
                bytes,
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(t.width() * 4),
                    rows_per_image: None,
                },
                t.size(),
            )
        };
        let src = texture(
            2,
            2,
            TextureFormat::Bgra8Unorm,
            TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            &[],
        );
        let color = [33, 81, 123, 157];
        let mut src_pixels = [0; 16];
        src_pixels[8..12].copy_from_slice(&color);
        write(&src, &src_pixels);
        let scaler = ScanoutScaler::new(&device);
        for format in [TextureFormat::Bgra8Unorm, TextureFormat::Bgra8UnormSrgb] {
            let formats: &[TextureFormat] = if format.is_srgb() {
                &[TextureFormat::Bgra8Unorm]
            } else {
                &[]
            };
            let dst = texture(
                6,
                4,
                format,
                TextureUsages::RENDER_ATTACHMENT
                    | TextureUsages::COPY_DST
                    | TextureUsages::COPY_SRC,
                formats,
            );
            let background = [6, 16, 26, 36];
            write(&dst, &background.repeat(24));
            let mut encoder = device.create_command_encoder(&Default::default());
            let damage = Rect {
                x: 0,
                y: 1,
                width: 1,
                height: 1,
            }
            .scaled(src.size(), dst.size());
            scaler.encode(&device, &mut encoder, &src, &dst, damage);
            let pitch = COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = device.create_buffer(&BufferDescriptor {
                label: None,
                size: u64::from(pitch) * 4,
                usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                dst.as_image_copy(),
                TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(pitch),
                        rows_per_image: None,
                    },
                },
                dst.size(),
            );
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            buffer
                .slice(..)
                .map_async(MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(Maintain::Wait);
            rx.recv().unwrap().unwrap();
            let bytes = buffer.slice(..).get_mapped_range();
            for y in 0..4 {
                for x in 0..6 {
                    let start = y * pitch as usize + x * 4;
                    let expected = if y >= 2 && x < 3 { color } else { background };
                    assert_eq!(bytes[start..start + 4], expected, "{format:?} ({x}, {y})");
                }
            }
        }
    }
}
