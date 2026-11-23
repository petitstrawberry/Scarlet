//! Real-device programmable rendering, compute, bindings and error containment.

use super::execution::headless_device;
use super::*;
use crate::Error;
use ir::*;
use sgfx_core::backend::{Completion, CompletionStatus};

#[test]
fn triangle_fans_are_rejected_during_preflight_without_gpu_access() {
    let table = ResourceTable::new();
    let module = shader(
        &table,
        &format!(
            "{FULLSCREEN_VERTEX}\n@fragment fn fragment() -> @location(0) vec4<f32> {{ return vec4<f32>(1.0); }}"
        ),
    );
    let pipeline = table
        .define_programmable_render_pipeline(
            ProgrammableRenderPipelineDesc::new(
                entry(module, ShaderStage::Vertex, "vertex"),
                entry(module, ShaderStage::Fragment, "fragment"),
                PipelineLayoutDesc::new(vec![]).unwrap(),
                TextureFormat::Rgba8Unorm,
                None,
                PrimitiveTopology::TriangleFan,
                BlendState::REPLACE,
                RasterState::new(CullMode::None, FrontFace::CounterClockwise),
            )
            .unwrap(),
        )
        .unwrap();
    let target = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Rgba8Unorm,
                Extent2D::new(4, 4).unwrap(),
                TextureUsage::RENDER_ATTACHMENT,
            )
            .unwrap(),
        )
        .unwrap();
    let mut encoder = CommandEncoder::new(&table);
    let mut pass = encoder
        .begin_render_pass(
            RenderPassDesc::new(
                &table,
                target,
                PixelRect::new(0, 0, 4, 4).unwrap(),
                LoadOp::Load,
                StoreOp::Store,
            )
            .unwrap(),
        )
        .unwrap();
    pass.set_programmable_pipeline(pipeline).unwrap();
    pass.draw(5, 0).unwrap();
    pass.end().unwrap();
    let commands = encoder.finish().unwrap();
    assert!(matches!(
        crate::validate_indexed_topologies(&commands),
        Err(Error::Unsupported(crate::UnsupportedFeature::TriangleFan))
    ));
}

#[test]
fn multiple_outputs_and_read_only_depth_feed_a_lighting_pass_on_gpu() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().unwrap();
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let extent = Extent2D::new(8, 8).unwrap();
    let make_texture = |format| {
        table
            .define_texture(
                TextureDesc::new(
                    format,
                    extent,
                    TextureUsage::RENDER_ATTACHMENT
                        | TextureUsage::SAMPLED
                        | TextureUsage::COPY_SRC,
                )
                .unwrap(),
            )
            .unwrap()
    };
    let color = make_texture(TextureFormat::Rgba8Unorm);
    let normal = make_texture(TextureFormat::Bgra8Unorm);
    let output = make_texture(TextureFormat::Rgba8Unorm);
    let depth = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Depth32Float,
                extent,
                TextureUsage::RENDER_ATTACHMENT | TextureUsage::SAMPLED,
            )
            .unwrap(),
        )
        .unwrap();
    let module = shader(
        &table,
        r#"
        @vertex fn vertex(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
            var p = array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
            return vec4(p[i],0.25,1.0);
        }
        struct GBuffer { @location(0) color:vec4<f32>, @location(1) normal:vec4<f32> }
        @fragment fn geometry() -> GBuffer { return GBuffer(vec4(0.25,0.0,0.0,1.0),vec4(0.0,0.5,0.9,0.2)); }
        @group(0) @binding(0) var color:texture_2d<f32>;
        @group(0) @binding(1) var normal:texture_2d<f32>;
        @group(0) @binding(2) var depth:texture_depth_2d;
        @fragment fn lighting(@builtin(position) p:vec4<f32>) -> @location(0) vec4<f32> {
            let xy = vec2<i32>(p.xy);
            return vec4(textureLoad(color,xy,0).r,textureLoad(normal,xy,0).g,textureLoad(depth,xy,0),1.0);
        }
    "#,
    );
    let pipeline = |fragment, layout| {
        ProgrammableRenderPipelineDesc::new(
            entry(module, ShaderStage::Vertex, "vertex"),
            entry(module, ShaderStage::Fragment, fragment),
            layout,
            TextureFormat::Rgba8Unorm,
            None,
            PrimitiveTopology::TriangleList,
            BlendState::REPLACE,
            RasterState::new(CullMode::None, FrontFace::CounterClockwise),
        )
        .unwrap()
    };
    let geometry = table
        .define_programmable_render_pipeline(
            pipeline("geometry", PipelineLayoutDesc::new(vec![]).unwrap())
                .with_color_targets(vec![
                    ColorTargetState::new(
                        TextureFormat::Rgba8Unorm,
                        BlendState::REPLACE,
                        ColorWriteMask::ALL,
                    )
                    .unwrap(),
                    ColorTargetState::new(
                        TextureFormat::Bgra8Unorm,
                        BlendState::REPLACE,
                        ColorWriteMask::from_bits(3).unwrap(),
                    )
                    .unwrap(),
                ])
                .unwrap()
                .with_depth_state(DepthState::new(
                    TextureFormat::Depth32Float,
                    CompareFunction::Always,
                    true,
                ))
                .unwrap(),
        )
        .unwrap();
    let layout = BindGroupLayoutDesc::new(
        (0..3)
            .map(|binding| {
                BindGroupLayoutEntry::new(
                    binding,
                    ShaderStages::FRAGMENT,
                    BindingType::SampledTextureView {
                        dimension: TextureViewDimension::D2,
                        depth: binding == 2,
                    },
                )
            })
            .collect(),
    )
    .unwrap();
    let lighting = table
        .define_programmable_render_pipeline(
            pipeline(
                "lighting",
                PipelineLayoutDesc::new(vec![layout.clone()]).unwrap(),
            )
            .with_depth_state(DepthState::new(
                TextureFormat::Depth32Float,
                CompareFunction::LessEqual,
                false,
            ))
            .unwrap(),
        )
        .unwrap();
    let group = table
        .define_bind_group(
            BindGroupDesc::new(
                &table,
                layout,
                [color, normal, depth]
                    .into_iter()
                    .enumerate()
                    .map(|(slot, t)| {
                        let desc = table.texture(t).unwrap();
                        BindGroupEntry::new(
                            slot as u32,
                            BindingResource::TextureView {
                                texture: t.id(),
                                view: TextureViewDesc::new(
                                    desc,
                                    desc.format(),
                                    TextureViewDimension::D2,
                                    0,
                                    1,
                                    0,
                                    1,
                                )
                                .unwrap(),
                            },
                        )
                    })
                    .collect(),
            )
            .unwrap(),
        )
        .unwrap();
    let area = PixelRect::new(0, 0, 8, 8).unwrap();
    let mut encoder = CommandEncoder::new(&table);
    let mut pass = encoder
        .begin_render_pass(
            RenderPassDesc::new(&table, color, area, LoadOp::DontCare, StoreOp::Store)
                .unwrap()
                .with_color_attachment(
                    &table,
                    normal,
                    LoadOp::Clear(Color::rgba(0.0, 0.0, 0.2, 1.0).unwrap()),
                    StoreOp::Store,
                )
                .unwrap()
                .with_depth_attachment(&table, depth, DepthLoadOp::Clear(1.0), StoreOp::Store)
                .unwrap(),
        )
        .unwrap();
    pass.set_programmable_pipeline(geometry).unwrap();
    pass.draw(3, 0).unwrap();
    pass.end().unwrap();
    let mut pass = encoder
        .begin_render_pass(
            RenderPassDesc::new(&table, output, area, LoadOp::DontCare, StoreOp::Store)
                .unwrap()
                .with_depth_attachment(&table, depth, DepthLoadOp::Load, StoreOp::Store)
                .unwrap()
                .with_read_only_depth()
                .unwrap(),
        )
        .unwrap();
    pass.set_programmable_pipeline(lighting).unwrap();
    pass.set_bind_group(0, group).unwrap();
    pass.draw(3, 0).unwrap();
    pass.end().unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    context
        .create_queue()
        .submit(&mut cache, &encoder.finish().unwrap())
        .unwrap();
    assert_eq!(
        cache.read_texture(output.id()).unwrap(),
        [64, 128, 64, 255].repeat(64)
    );
    // Readback retains the image's native BGRA byte order.
    assert_eq!(
        cache.read_texture(normal.id()).unwrap(),
        [51, 128, 0, 255].repeat(64)
    );
}

#[test]
fn instanced_and_indexed_draws_preserve_first_instance_on_gpu() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().unwrap();
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let module = shader(
        &table,
        r#"
        struct Vertex { @builtin(position) position:vec4<f32>, @location(0) @interpolate(flat) shade:f32 }
        @vertex fn vertex(@builtin(vertex_index) index:u32, @builtin(instance_index) instance:u32) -> Vertex {
            var points = array<vec2<f32>,6>(vec2(0.0,0.0),vec2(1.0,0.0),vec2(1.0,1.0),vec2(0.0,0.0),vec2(1.0,1.0),vec2(0.0,1.0));
            let cell = instance - 5u;
            var result:Vertex;
            result.position = vec4(points[index] + vec2(f32(cell % 2u),f32(cell / 2u)) - vec2(1.0),0.0,1.0);
            result.shade = f32(instance + 1u) / 16.0;
            return result;
        }
        @fragment fn fragment(v:Vertex) -> @location(0) vec4<f32> { return vec4(v.shade,0.0,0.0,1.0); }
    "#,
    );
    let pipeline = table
        .define_programmable_render_pipeline(
            ProgrammableRenderPipelineDesc::new(
                entry(module, ShaderStage::Vertex, "vertex"),
                entry(module, ShaderStage::Fragment, "fragment"),
                PipelineLayoutDesc::new(vec![]).unwrap(),
                TextureFormat::Rgba8Unorm,
                None,
                PrimitiveTopology::TriangleList,
                BlendState::REPLACE,
                RasterState::new(CullMode::None, FrontFace::CounterClockwise),
            )
            .unwrap(),
        )
        .unwrap();
    let target = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Rgba8Unorm,
                Extent2D::new(8, 8).unwrap(),
                TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
            )
            .unwrap(),
        )
        .unwrap();
    let indices = table
        .define_buffer(BufferDesc::new(12, BufferUsage::INDEX | BufferUsage::COPY_DST).unwrap())
        .unwrap();
    let mut encoder = CommandEncoder::new(&table);
    encoder
        .write_buffer(indices, 0, &[0, 0, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0])
        .unwrap();
    let mut pass = encoder
        .begin_render_pass(
            RenderPassDesc::new(
                &table,
                target,
                PixelRect::new(0, 0, 8, 8).unwrap(),
                LoadOp::Clear(Color::rgba(0.0, 0.0, 0.0, 1.0).unwrap()),
                StoreOp::Store,
            )
            .unwrap(),
        )
        .unwrap();
    pass.set_programmable_pipeline(pipeline).unwrap();
    pass.draw_instanced(6, 0, 2, 5).unwrap();
    pass.set_index_buffer(indices, 0, IndexFormat::Uint16)
        .unwrap();
    pass.draw_indexed_instanced(6, 0, 0, 2, 7).unwrap();
    pass.end().unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    context
        .create_queue()
        .submit(&mut cache, &encoder.finish().unwrap())
        .unwrap();
    let pixels = cache.read_texture(target.id()).unwrap();
    for (x, y, red) in [(2, 6, 96), (6, 6, 112), (2, 2, 128), (6, 2, 143)] {
        let offset = (y * 8 + x) * 4;
        assert_eq!(
            &pixels[offset..offset + 4],
            &[red, 0, 0, 255],
            "instance at {x},{y}"
        );
    }
}

fn shader<'a>(table: &'a ResourceTable, source: &str) -> ShaderModuleRef<'a> {
    table
        .define_shader_module(ShaderModuleDesc::wgsl(source.into()).expect("WGSL descriptor"))
        .expect("shader")
}
fn entry(module: ShaderModuleRef<'_>, stage: ShaderStage, name: &str) -> ShaderEntryPoint {
    ShaderEntryPoint::new(module, stage, name.into()).expect("entry point")
}
fn buffer_group<'a>(
    table: &'a ResourceTable,
    layout: &BindGroupLayoutDesc,
    buffers: &[(BufferRef<'a>, u64)],
) -> BindGroupRef<'a> {
    let entries = buffers
        .iter()
        .enumerate()
        .map(|(binding, (buffer, size))| {
            BindGroupEntry::new(
                binding as u32,
                BindingResource::Buffer {
                    buffer: buffer.id(),
                    offset: 0,
                    size: *size,
                },
            )
        })
        .collect();
    table
        .define_bind_group(
            BindGroupDesc::new(table, layout.clone(), entries).expect("group descriptor"),
        )
        .expect("group")
}

#[test]
fn indexed_strips_are_rejected_before_gpu_acceptance() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    for (topology, expected) in [
        (
            PrimitiveTopology::TriangleStrip,
            crate::UnsupportedFeature::IndexedTriangleStrip,
        ),
        (
            PrimitiveTopology::LineStrip,
            crate::UnsupportedFeature::IndexedLineStrip,
        ),
    ] {
        let context = device.create_context();
        let table = Rc::new(ResourceTable::new());
        let module = shader(
            &table,
            &format!(
                "{FULLSCREEN_VERTEX}\n@fragment fn fragment() -> @location(0) vec4<f32> {{ return vec4<f32>(0.0, 1.0, 0.0, 1.0); }}"
            ),
        );
        let pipeline = table
            .define_programmable_render_pipeline(
                ProgrammableRenderPipelineDesc::new(
                    entry(module, ShaderStage::Vertex, "vertex"),
                    entry(module, ShaderStage::Fragment, "fragment"),
                    PipelineLayoutDesc::new(vec![]).unwrap(),
                    TextureFormat::Rgba8Unorm,
                    None,
                    topology,
                    BlendState::REPLACE,
                    RasterState::new(CullMode::None, FrontFace::CounterClockwise),
                )
                .unwrap(),
            )
            .unwrap();
        let indices = table
            .define_buffer(BufferDesc::new(8, BufferUsage::INDEX | BufferUsage::COPY_DST).unwrap())
            .unwrap();
        let target = table
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Rgba8Unorm,
                    Extent2D::new(4, 4).unwrap(),
                    TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
                )
                .unwrap(),
            )
            .unwrap();
        let mut encoder = CommandEncoder::new(&table);
        encoder.write_buffer(indices, 0, &[0; 8]).unwrap();
        let mut pass = encoder
            .begin_render_pass(
                RenderPassDesc::new(
                    &table,
                    target,
                    PixelRect::new(0, 0, 4, 4).unwrap(),
                    LoadOp::Clear(Color::rgba(1.0, 0.0, 0.0, 1.0).unwrap()),
                    StoreOp::Store,
                )
                .unwrap(),
            )
            .unwrap();
        pass.set_programmable_pipeline(pipeline).unwrap();
        pass.set_index_buffer(indices, 0, IndexFormat::Uint16)
            .unwrap();
        pass.draw_indexed(4, 0, 0).unwrap();
        pass.end().unwrap();
        let commands = encoder.finish().unwrap();
        let mut cache = context.create_resources(Rc::clone(&table));
        let result = context.create_queue().submit_tracked(&mut cache, &commands);
        assert!(matches!(
            result,
            Err(sgfx_core::backend::SubmitError::Rejected(
                Error::Unsupported(error)
            )) if error == expected
        ));
        assert_eq!(cache.read_texture(target.id()).unwrap(), vec![0; 4 * 4 * 4]);
    }
}

#[test]
fn cached_bind_groups_observe_updates_and_replace_remapped_images() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let source = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Rgba8Unorm,
                Extent2D::new(1, 1).unwrap(),
                TextureUsage::PRESENT
                    | TextureUsage::RENDER_ATTACHMENT
                    | TextureUsage::COPY_DST
                    | TextureUsage::SAMPLED,
            )
            .unwrap(),
        )
        .unwrap();
    let output = table
        .define_buffer(BufferDesc::new(16, BufferUsage::STORAGE | BufferUsage::COPY_SRC).unwrap())
        .unwrap();
    let module = shader(
        &table,
        r#"
        @group(0) @binding(0) var image: texture_2d<f32>;
        @group(0) @binding(1) var<storage, read_write> output: vec4<u32>;
        @compute @workgroup_size(1) fn main() {
            output = vec4<u32>(round(textureLoad(image, vec2<i32>(0), 0) * 255.0));
        }
    "#,
    );
    let layout = BindGroupLayoutDesc::new(vec![
        BindGroupLayoutEntry::new(0, ShaderStages::COMPUTE, BindingType::SampledTexture),
        BindGroupLayoutEntry::new(
            1,
            ShaderStages::COMPUTE,
            BindingType::StorageBuffer { read_only: false },
        ),
    ])
    .unwrap();
    let pipeline = table
        .define_compute_pipeline(
            ComputePipelineDesc::new(
                entry(module, ShaderStage::Compute, "main"),
                PipelineLayoutDesc::new(vec![layout.clone()]).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let group = table
        .define_bind_group(
            BindGroupDesc::new(
                &table,
                layout,
                vec![
                    BindGroupEntry::new(0, BindingResource::Texture(source.id())),
                    BindGroupEntry::new(
                        1,
                        BindingResource::Buffer {
                            buffer: output.id(),
                            offset: 0,
                            size: 16,
                        },
                    ),
                ],
            )
            .unwrap(),
        )
        .unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    let first = context
        .create_image(1, 1, TextureFormat::Rgba8Unorm)
        .unwrap();
    cache.map_image(source.id(), first).unwrap();
    let queue = context.create_queue();
    for (iteration, pixel) in [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]
        .into_iter()
        .enumerate()
    {
        if iteration == 2 {
            cache.unmap_image(source.id());
            let replacement = context
                .create_image(1, 1, TextureFormat::Rgba8Unorm)
                .unwrap();
            cache.map_image(source.id(), replacement).unwrap();
        }
        let mut encoder = CommandEncoder::new(&table);
        encoder
            .write_texture(
                source,
                TextureWrite::new(PixelRect::new(0, 0, 1, 1).unwrap(), 4, &pixel).unwrap(),
            )
            .unwrap();
        let mut pass = encoder.begin_compute_pass().unwrap();
        pass.set_pipeline(pipeline).unwrap();
        pass.set_bind_group(0, group).unwrap();
        pass.dispatch(1, 1, 1).unwrap();
        pass.end().unwrap();
        let commands = encoder.finish().unwrap();
        let receipt = queue.submit_tracked(&mut cache, &commands).unwrap();
        assert_eq!(receipt.wait(None), Ok(CompletionStatus::Complete));
        let expected = pixel.map(u32::from);
        assert_eq!(
            cache.read_buffer(output.id(), 0, 16).unwrap(),
            bytemuck::cast_slice(&expected)
        );
    }
}

#[test]
fn compute_storage_uniform_bindings_barriers_and_ordered_uploads_produce_readback() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let shader = shader(
        &table,
        r#"
        @group(0) @binding(0) var<uniform> input: vec4<u32>;
        @group(0) @binding(1) var<storage, read_write> output: array<u32>;
        @compute @workgroup_size(4)
        fn main(@builtin(global_invocation_id) id: vec3<u32>) { output[id.x] = input.x + id.x; }
    "#,
    );
    let layout = BindGroupLayoutDesc::new(vec![
        BindGroupLayoutEntry::new(0, ShaderStages::COMPUTE, BindingType::UniformBuffer),
        BindGroupLayoutEntry::new(
            1,
            ShaderStages::COMPUTE,
            BindingType::StorageBuffer { read_only: false },
        ),
    ])
    .unwrap();
    let pipeline = table
        .define_compute_pipeline(
            ComputePipelineDesc::new(
                entry(shader, ShaderStage::Compute, "main"),
                PipelineLayoutDesc::new(vec![layout.clone()]).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let input = table
        .define_buffer(BufferDesc::new(16, BufferUsage::UNIFORM | BufferUsage::COPY_DST).unwrap())
        .unwrap();
    let output = table
        .define_buffer(BufferDesc::new(32, BufferUsage::STORAGE | BufferUsage::COPY_SRC).unwrap())
        .unwrap();
    let snapshot = table
        .define_buffer(BufferDesc::new(32, BufferUsage::COPY_DST | BufferUsage::COPY_SRC).unwrap())
        .unwrap();
    let group = buffer_group(&table, &layout, &[(input, 16), (output, 32)]);
    let first = [5_u32, 0, 0, 0];
    let second = [91_u32, 0, 0, 0];
    let mut encoder = CommandEncoder::new(&table);
    encoder
        .write_buffer(input, 0, bytemuck::cast_slice(&first))
        .unwrap();
    encoder
        .resource_barrier(ResourceBarrier::Buffer {
            buffer: input,
            before: BufferAccess::CopyDestination,
            after: BufferAccess::Uniform,
        })
        .unwrap();
    let mut pass = encoder.begin_compute_pass().unwrap();
    pass.set_pipeline(pipeline).unwrap();
    pass.set_bind_group(0, group).unwrap();
    pass.dispatch(2, 1, 1).unwrap();
    pass.end().unwrap();
    encoder
        .resource_barrier(ResourceBarrier::Buffer {
            buffer: output,
            before: BufferAccess::StorageReadWrite,
            after: BufferAccess::CopySource,
        })
        .unwrap();
    encoder
        .copy_buffer_to_buffer(output, 0, snapshot, 0, 32)
        .unwrap();
    encoder
        .write_buffer(input, 0, bytemuck::cast_slice(&second))
        .unwrap();
    let mut pass = encoder.begin_compute_pass().unwrap();
    pass.set_pipeline(pipeline).unwrap();
    pass.set_bind_group(0, group).unwrap();
    pass.dispatch(2, 1, 1).unwrap();
    pass.end().unwrap();
    let commands = encoder.finish().unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    let receipt = context
        .create_queue()
        .submit_tracked(&mut cache, &commands)
        .unwrap();
    drop(commands);
    assert_eq!(receipt.wait(None), Ok(CompletionStatus::Complete));
    assert_eq!(
        cache.read_buffer(snapshot.id(), 0, 32).unwrap(),
        bytemuck::cast_slice(&(5_u32..13).collect::<Vec<_>>())
    );
    assert_eq!(
        cache.read_buffer(output.id(), 0, 32).unwrap(),
        bytemuck::cast_slice(&(91_u32..99).collect::<Vec<_>>())
    );
}

const FULLSCREEN_VERTEX: &str = r#"
    @vertex fn vertex(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
        var positions = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
        return vec4(positions[id], 0.0, 1.0);
    }
"#;

#[test]
fn compute_storage_texture_then_programmable_draw_samples_the_result() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let texture = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Rgba8Unorm,
                Extent2D::new(4, 4).unwrap(),
                TextureUsage::STORAGE | TextureUsage::SAMPLED,
            )
            .unwrap(),
        )
        .unwrap();
    let target = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Rgba8Unorm,
                Extent2D::new(4, 4).unwrap(),
                TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
            )
            .unwrap(),
        )
        .unwrap();
    let compute_shader = shader(
        &table,
        r#"
        @group(0) @binding(0) var destination: texture_storage_2d<rgba8unorm, write>;
        @compute @workgroup_size(1) fn compute(@builtin(global_invocation_id) id: vec3<u32>) {
            textureStore(destination, vec2<i32>(id.xy), vec4(1.0, 0.0, 0.0, 1.0));
        }
    "#,
    );
    let compute_layout = BindGroupLayoutDesc::new(vec![BindGroupLayoutEntry::new(
        0,
        ShaderStages::COMPUTE,
        BindingType::StorageTexture {
            format: TextureFormat::Rgba8Unorm,
            access: StorageTextureAccess::WriteOnly,
        },
    )])
    .unwrap();
    let compute_group = table
        .define_bind_group(
            BindGroupDesc::new(
                &table,
                compute_layout.clone(),
                vec![BindGroupEntry::new(
                    0,
                    BindingResource::Texture(texture.id()),
                )],
            )
            .unwrap(),
        )
        .unwrap();
    let compute_pipeline = table
        .define_compute_pipeline(
            ComputePipelineDesc::new(
                entry(compute_shader, ShaderStage::Compute, "compute"),
                PipelineLayoutDesc::new(vec![compute_layout]).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let render_shader = shader(
        &table,
        &format!(
            "{FULLSCREEN_VERTEX}\n{}",
            r#"
        @group(0) @binding(0) var source: texture_2d<f32>;
        @group(0) @binding(1) var filtering: sampler;
        @fragment fn fragment() -> @location(0) vec4<f32> { return textureSample(source, filtering, vec2(0.5, 0.5)); }
    "#
        ),
    );
    let sampler = table
        .define_sampler(SamplerDesc::new(
            FilterMode::Nearest,
            FilterMode::Nearest,
            AddressMode::ClampToEdge,
            AddressMode::ClampToEdge,
        ))
        .unwrap();
    let render_layout = BindGroupLayoutDesc::new(vec![
        BindGroupLayoutEntry::new(0, ShaderStages::FRAGMENT, BindingType::SampledTexture),
        BindGroupLayoutEntry::new(1, ShaderStages::FRAGMENT, BindingType::Sampler),
    ])
    .unwrap();
    let render_group = table
        .define_bind_group(
            BindGroupDesc::new(
                &table,
                render_layout.clone(),
                vec![
                    BindGroupEntry::new(0, BindingResource::Texture(texture.id())),
                    BindGroupEntry::new(1, BindingResource::Sampler(sampler.id())),
                ],
            )
            .unwrap(),
        )
        .unwrap();
    let pipeline = table
        .define_programmable_render_pipeline(
            ProgrammableRenderPipelineDesc::new(
                entry(render_shader, ShaderStage::Vertex, "vertex"),
                entry(render_shader, ShaderStage::Fragment, "fragment"),
                PipelineLayoutDesc::new(vec![render_layout]).unwrap(),
                TextureFormat::Rgba8Unorm,
                None,
                PrimitiveTopology::TriangleList,
                BlendState::SOURCE_OVER_STRAIGHT_ALPHA,
                RasterState::new(CullMode::None, FrontFace::CounterClockwise),
            )
            .unwrap(),
        )
        .unwrap();
    let mut encoder = CommandEncoder::new(&table);
    let mut pass = encoder.begin_compute_pass().unwrap();
    pass.set_pipeline(compute_pipeline).unwrap();
    pass.set_bind_group(0, compute_group).unwrap();
    pass.dispatch(4, 4, 1).unwrap();
    pass.end().unwrap();
    encoder
        .resource_barrier(ResourceBarrier::Texture {
            texture,
            before: TextureAccess::StorageWrite,
            after: TextureAccess::Sampled,
        })
        .unwrap();
    let desc = RenderPassDesc::new(
        &table,
        target,
        PixelRect::new(0, 0, 4, 4).unwrap(),
        LoadOp::Clear(Color::rgba(0.0, 0.0, 1.0, 1.0).unwrap()),
        StoreOp::Store,
    )
    .unwrap();
    let mut pass = encoder.begin_render_pass(desc).unwrap();
    pass.set_programmable_pipeline(pipeline).unwrap();
    pass.set_bind_group(0, render_group).unwrap();
    pass.draw(3, 0).unwrap();
    pass.end().unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    context
        .create_queue()
        .submit(&mut cache, &encoder.finish().unwrap())
        .unwrap();
    assert_eq!(
        cache.read_texture(target.id()).unwrap(),
        [255, 0, 0, 255].repeat(16)
    );
}

#[test]
fn shader_errors_and_pipeline_layout_mismatches_return_errors_without_uncaptured_failures() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let mut cache = context.create_resources(Rc::clone(&table));
    for source in [
        "@compute @workgroup_size(1) fn main() { invalid wgsl; }",
        "@group(0) @binding(0) var<storage, read_write> output: array<u32>; @compute @workgroup_size(1) fn main() { output[0] = 1u; }",
    ] {
        let module = shader(&table, source);
        let pipeline = table
            .define_compute_pipeline(
                ComputePipelineDesc::new(
                    entry(module, ShaderStage::Compute, "main"),
                    PipelineLayoutDesc::new(vec![]).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        let mut encoder = CommandEncoder::new(&table);
        let mut pass = encoder.begin_compute_pass().unwrap();
        pass.set_pipeline(pipeline).unwrap();
        pass.dispatch(1, 1, 1).unwrap();
        pass.end().unwrap();
        device
            .raw_device()
            .push_error_scope(raw::ErrorFilter::Validation);
        let result = context
            .create_queue()
            .submit(&mut cache, &encoder.finish().unwrap());
        assert!(matches!(result, Err(Error::Validation(_))), "{result:?}");
        assert!(pollster::block_on(device.raw_device().pop_error_scope()).is_none());
    }
}

#[test]
fn shader_minimum_binding_size_is_validated_before_submission() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let module = shader(
        &table,
        "@group(0) @binding(0) var<storage, read_write> output: vec4<u32>; @compute @workgroup_size(1) fn main() { output = vec4(1u); }",
    );
    let layout = BindGroupLayoutDesc::new(vec![BindGroupLayoutEntry::new(
        0,
        ShaderStages::COMPUTE,
        BindingType::StorageBuffer { read_only: false },
    )])
    .unwrap();
    let pipeline = table
        .define_compute_pipeline(
            ComputePipelineDesc::new(
                entry(module, ShaderStage::Compute, "main"),
                PipelineLayoutDesc::new(vec![layout.clone()]).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let small = table
        .define_buffer(BufferDesc::new(4, BufferUsage::STORAGE).unwrap())
        .unwrap();
    let group = buffer_group(&table, &layout, &[(small, 4)]);
    let mut encoder = CommandEncoder::new(&table);
    let mut pass = encoder.begin_compute_pass().unwrap();
    pass.set_pipeline(pipeline).unwrap();
    pass.set_bind_group(0, group).unwrap();
    pass.dispatch(1, 1, 1).unwrap();
    pass.end().unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    let result = context
        .create_queue()
        .submit(&mut cache, &encoder.finish().unwrap());
    assert!(matches!(result, Err(Error::Validation(_))), "{result:?}");
}

#[test]
fn spirv_compute_modules_are_validated_translated_and_executed() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let source = "@group(1) @binding(0) var<storage, read_write> output: array<u32>; @compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3<u32>) { output[id.x] = 37u + id.x; }";
    let module = naga::front::wgsl::parse_str(source).unwrap();
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
    let words = naga::back::spv::write_vec(
        &module,
        &info,
        &naga::back::spv::Options::default(),
        Some(&naga::back::spv::PipelineOptions {
            shader_stage: naga::ShaderStage::Compute,
            entry_point: "main".into(),
        }),
    )
    .unwrap();
    let module = table
        .define_shader_module(ShaderModuleDesc::spirv(words).unwrap())
        .unwrap();
    let layout = BindGroupLayoutDesc::new(vec![BindGroupLayoutEntry::new(
        0,
        ShaderStages::COMPUTE,
        BindingType::StorageBuffer { read_only: false },
    )])
    .unwrap();
    let pipeline = table
        .define_compute_pipeline(
            ComputePipelineDesc::new(
                entry(module, ShaderStage::Compute, "main"),
                PipelineLayoutDesc::new(vec![
                    BindGroupLayoutDesc::new(vec![]).unwrap(),
                    layout.clone(),
                ])
                .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let output = table
        .define_buffer(BufferDesc::new(16, BufferUsage::STORAGE | BufferUsage::COPY_SRC).unwrap())
        .unwrap();
    let group = buffer_group(&table, &layout, &[(output, 16)]);
    let mut encoder = CommandEncoder::new(&table);
    let mut pass = encoder.begin_compute_pass().unwrap();
    pass.set_pipeline(pipeline).unwrap();
    pass.set_bind_group(1, group).unwrap();
    pass.dispatch(4, 1, 1).unwrap();
    pass.end().unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    context
        .create_queue()
        .submit(&mut cache, &encoder.finish().unwrap())
        .unwrap();
    assert_eq!(
        cache.read_buffer(output.id(), 0, 16).unwrap(),
        bytemuck::cast_slice(&[37_u32, 38, 39, 40])
    );
}

#[test]
fn indexed_draw_ignores_unused_groups_and_allows_depth_disabled_with_depth_attachment() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let module = shader(
        &table,
        r#"
        @vertex fn vertex(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> { return vec4(position, 0.0, 1.0); }
        @fragment fn fragment() -> @location(0) vec4<f32> { return vec4(0.0, 1.0, 0.0, 1.0); }
    "#,
    );
    let extent = Extent2D::new(4, 4).unwrap();
    let target = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Rgba8Unorm,
                extent,
                TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC | TextureUsage::SAMPLED,
            )
            .unwrap(),
        )
        .unwrap();
    let depth = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Depth32Float,
                extent,
                TextureUsage::RENDER_ATTACHMENT,
            )
            .unwrap(),
        )
        .unwrap();
    let unused_layout = BindGroupLayoutDesc::new(vec![BindGroupLayoutEntry::new(
        0,
        ShaderStages::FRAGMENT,
        BindingType::SampledTexture,
    )])
    .unwrap();
    let unused_group = table
        .define_bind_group(
            BindGroupDesc::new(
                &table,
                unused_layout,
                vec![BindGroupEntry::new(
                    0,
                    BindingResource::Texture(target.id()),
                )],
            )
            .unwrap(),
        )
        .unwrap();
    let vertices = [[-1_f32, -1.0], [3.0, -1.0], [-1.0, 3.0]];
    let indices = [0_u16, 1, 2, 0];
    let vertex = table
        .define_buffer(BufferDesc::new(24, BufferUsage::VERTEX | BufferUsage::COPY_DST).unwrap())
        .unwrap();
    let index = table
        .define_buffer(BufferDesc::new(8, BufferUsage::INDEX | BufferUsage::COPY_DST).unwrap())
        .unwrap();
    let pipeline = table
        .define_programmable_render_pipeline(
            ProgrammableRenderPipelineDesc::new(
                entry(module, ShaderStage::Vertex, "vertex"),
                entry(module, ShaderStage::Fragment, "fragment"),
                PipelineLayoutDesc::new(vec![BindGroupLayoutDesc::new(vec![]).unwrap()]).unwrap(),
                TextureFormat::Rgba8Unorm,
                Some(
                    VertexBufferLayout::new(
                        8,
                        vec![VertexAttribute::new(0, VertexFormat::Float32x2, 0)],
                    )
                    .unwrap(),
                ),
                PrimitiveTopology::TriangleList,
                BlendState::SOURCE_OVER_STRAIGHT_ALPHA,
                RasterState::new(CullMode::None, FrontFace::CounterClockwise),
            )
            .unwrap(),
        )
        .unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    cache.validate_shader_module(module.id()).unwrap();
    cache
        .validate_programmable_render_pipeline(pipeline.id())
        .unwrap();
    let mut encoder = CommandEncoder::new(&table);
    encoder
        .write_buffer(vertex, 0, bytemuck::cast_slice(&vertices))
        .unwrap();
    encoder
        .write_buffer(index, 0, bytemuck::cast_slice(&indices))
        .unwrap();
    let desc = RenderPassDesc::new(
        &table,
        target,
        PixelRect::new(0, 0, 4, 4).unwrap(),
        LoadOp::Clear(Color::rgba(0.0, 0.0, 0.0, 1.0).unwrap()),
        StoreOp::Store,
    )
    .unwrap()
    .with_depth_attachment(&table, depth, DepthLoadOp::Clear(0.0), StoreOp::Store)
    .unwrap();
    let mut pass = encoder.begin_render_pass(desc).unwrap();
    pass.set_programmable_pipeline(pipeline).unwrap();
    pass.set_bind_group(3, unused_group).unwrap();
    pass.set_vertex_buffer(vertex, 0).unwrap();
    pass.set_index_buffer(index, 0, IndexFormat::Uint16)
        .unwrap();
    pass.draw_indexed(3, 0, 0).unwrap();
    pass.end().unwrap();
    context
        .create_queue()
        .submit(&mut cache, &encoder.finish().unwrap())
        .unwrap();
    assert_eq!(
        cache.read_texture(target.id()).unwrap(),
        [0, 255, 0, 255].repeat(16)
    );
}

#[test]
fn cube_faces_and_srgb_views_sample_the_same_allocation_on_gpu() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().unwrap();
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let desc = TextureDesc::new(
        TextureFormat::Rgba8Unorm,
        Extent2D::new(1, 1).unwrap(),
        TextureUsage::SAMPLED | TextureUsage::COPY_DST,
    )
    .unwrap()
    .with_array_layer_count(6)
    .unwrap();
    let texture = table.define_texture(desc).unwrap();
    let output = table
        .define_buffer(BufferDesc::new(192, BufferUsage::STORAGE | BufferUsage::COPY_SRC).unwrap())
        .unwrap();
    let sampler = table
        .define_sampler(SamplerDesc::new(
            FilterMode::Nearest,
            FilterMode::Nearest,
            AddressMode::ClampToEdge,
            AddressMode::ClampToEdge,
        ))
        .unwrap();
    let module = shader(
        &table,
        r#"
        @group(0) @binding(0) var linear_image: texture_cube<f32>;
        @group(0) @binding(1) var srgb_image: texture_cube<f32>;
        @group(0) @binding(2) var image_sampler: sampler;
        @group(0) @binding(3) var<storage, read_write> output: array<vec4<u32>, 12>;
        @compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3<u32>) {
            let directions = array<vec3<f32>, 6>(vec3<f32>(1,0,0), vec3<f32>(-1,0,0),
                vec3<f32>(0,1,0), vec3<f32>(0,-1,0), vec3<f32>(0,0,1), vec3<f32>(0,0,-1));
            let direction = directions[id.x];
            output[id.x] = vec4<u32>(round(textureSampleLevel(linear_image, image_sampler, direction, 0.0) * 255.0));
            output[id.x + 6u] = vec4<u32>(round(textureSampleLevel(srgb_image, image_sampler, direction, 0.0) * 255.0));
        }
    "#,
    );
    let texture_type = BindingType::SampledTextureView {
        dimension: TextureViewDimension::Cube,
        depth: false,
    };
    let layout = BindGroupLayoutDesc::new(vec![
        BindGroupLayoutEntry::new(0, ShaderStages::COMPUTE, texture_type),
        BindGroupLayoutEntry::new(1, ShaderStages::COMPUTE, texture_type),
        BindGroupLayoutEntry::new(2, ShaderStages::COMPUTE, BindingType::Sampler),
        BindGroupLayoutEntry::new(
            3,
            ShaderStages::COMPUTE,
            BindingType::StorageBuffer { read_only: false },
        ),
    ])
    .unwrap();
    let view = |format| BindingResource::TextureView {
        texture: texture.id(),
        view: TextureViewDesc::new(desc, format, TextureViewDimension::Cube, 0, 1, 0, 6).unwrap(),
    };
    let group = table
        .define_bind_group(
            BindGroupDesc::new(
                &table,
                layout.clone(),
                vec![
                    BindGroupEntry::new(0, view(TextureFormat::Rgba8Unorm)),
                    BindGroupEntry::new(1, view(TextureFormat::Rgba8UnormSrgb)),
                    BindGroupEntry::new(2, BindingResource::Sampler(sampler.id())),
                    BindGroupEntry::new(
                        3,
                        BindingResource::Buffer {
                            buffer: output.id(),
                            offset: 0,
                            size: 192,
                        },
                    ),
                ],
            )
            .unwrap(),
        )
        .unwrap();
    let pipeline = table
        .define_compute_pipeline(
            ComputePipelineDesc::new(
                entry(module, ShaderStage::Compute, "main"),
                PipelineLayoutDesc::new(vec![layout]).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let colors = [
        [128u8, 64, 32, 255],
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 0, 255],
        [0, 255, 255, 255],
    ];
    let mut encoder = CommandEncoder::new(&table);
    for (layer, color) in colors.iter().enumerate() {
        encoder
            .write_texture(
                texture,
                TextureWrite::new(PixelRect::new(0, 0, 1, 1).unwrap(), 4, color)
                    .unwrap()
                    .with_array_layer(layer as u32),
            )
            .unwrap();
    }
    let mut pass = encoder.begin_compute_pass().unwrap();
    pass.set_pipeline(pipeline).unwrap();
    pass.set_bind_group(0, group).unwrap();
    pass.dispatch(6, 1, 1).unwrap();
    pass.end().unwrap();
    let commands = encoder.finish().unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    let receipt = context
        .create_queue()
        .submit_tracked(&mut cache, &commands)
        .unwrap();
    assert_eq!(receipt.wait(None), Ok(CompletionStatus::Complete));
    let bytes = cache.read_buffer(output.id(), 0, 192).unwrap();
    let actual = bytes
        .chunks_exact(4)
        .map(|word| u32::from_ne_bytes(word.try_into().unwrap()))
        .collect::<Vec<_>>();
    let expected_linear = colors
        .into_iter()
        .flatten()
        .map(u32::from)
        .collect::<Vec<_>>();
    assert_eq!(&actual[..24], expected_linear);
    assert_eq!(&actual[24..28], &[55, 13, 4, 255]);
    assert_eq!(&actual[28..], &expected_linear[4..]);
}

#[test]
fn packed_normals_integer_and_half_attributes_use_multiple_gpu_vertex_streams() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().unwrap();
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let module = shader(
        &table,
        r#"
        struct Input {
            @location(0) position: vec2<f32>,
            @location(1) normal: vec4<f32>,
            @location(2) material: i32,
            @location(3) uv: vec2<f32>,
            @location(4) glyph: vec2<u32>,
        };
        struct Output { @builtin(position) position: vec4<f32>, @location(0) color: vec4<f32> };
        @vertex fn vertex(input: Input) -> Output {
            var output: Output;
            output.position = vec4<f32>(input.position, 0, 1);
            output.color = vec4<f32>(input.normal.xyz * 0.5 + 0.5, input.normal.w);
            if input.material != -17 || any(input.uv != vec2<f32>(0.5, 1.0))
                || any(input.glyph != vec2<u32>(65535, 32768)) {
                output.color = vec4<f32>(1,0,0,0);
            }
            return output;
        }
        @fragment fn fragment(input: Output) -> @location(0) vec4<f32> { return input.color; }
    "#,
    );
    let target = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Rgba8Unorm,
                Extent2D::new(4, 4).unwrap(),
                TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
            )
            .unwrap(),
        )
        .unwrap();
    let positions = table
        .define_buffer(BufferDesc::new(24, BufferUsage::VERTEX | BufferUsage::COPY_DST).unwrap())
        .unwrap();
    let attributes = table
        .define_buffer(BufferDesc::new(24, BufferUsage::VERTEX | BufferUsage::COPY_DST).unwrap())
        .unwrap();
    let uvs = table
        .define_buffer(BufferDesc::new(12, BufferUsage::VERTEX | BufferUsage::COPY_DST).unwrap())
        .unwrap();
    let glyphs = table
        .define_buffer(BufferDesc::new(12, BufferUsage::VERTEX | BufferUsage::COPY_DST).unwrap())
        .unwrap();
    let pipeline = table
        .define_programmable_render_pipeline(
            ProgrammableRenderPipelineDesc::new(
                entry(module, ShaderStage::Vertex, "vertex"),
                entry(module, ShaderStage::Fragment, "fragment"),
                PipelineLayoutDesc::new(vec![]).unwrap(),
                TextureFormat::Rgba8Unorm,
                None,
                PrimitiveTopology::TriangleList,
                BlendState::REPLACE,
                RasterState::new(CullMode::None, FrontFace::CounterClockwise),
            )
            .unwrap()
            .with_vertex_buffers(vec![
                VertexBufferLayout::new(
                    8,
                    vec![VertexAttribute::new(0, VertexFormat::Float32x2, 0)],
                )
                .unwrap(),
                VertexBufferLayout::new(
                    8,
                    vec![
                        VertexAttribute::new(1, VertexFormat::Snorm10_10_10_2, 0),
                        VertexAttribute::new(2, VertexFormat::Sint32, 4),
                    ],
                )
                .unwrap(),
                VertexBufferLayout::new(
                    4,
                    vec![VertexAttribute::new(3, VertexFormat::Float16x2, 0)],
                )
                .unwrap(),
                VertexBufferLayout::new(
                    4,
                    vec![VertexAttribute::new(4, VertexFormat::Uint16x2, 0)],
                )
                .unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
    // R=-512 clamps to -1; G=511 becomes 1; B=-256; A=1.
    let packed = 512u32 | (511 << 10) | (768 << 20) | (1 << 30);
    let data = [packed, (-17i32) as u32].repeat(3);
    let half = [0x3800u16, 0x3c00].repeat(3);
    let glyph = [65535u16, 32768].repeat(3);
    let pos = [-1.0f32, -1.0, 3.0, -1.0, -1.0, 3.0];
    let mut encoder = CommandEncoder::new(&table);
    encoder
        .write_buffer(positions, 0, bytemuck::cast_slice(&pos))
        .unwrap();
    encoder
        .write_buffer(attributes, 0, bytemuck::cast_slice(&data))
        .unwrap();
    encoder
        .write_buffer(uvs, 0, bytemuck::cast_slice(&half))
        .unwrap();
    encoder
        .write_buffer(glyphs, 0, bytemuck::cast_slice(&glyph))
        .unwrap();
    let mut pass = encoder
        .begin_render_pass(
            RenderPassDesc::new(
                &table,
                target,
                PixelRect::new(0, 0, 4, 4).unwrap(),
                LoadOp::DontCare,
                StoreOp::Store,
            )
            .unwrap(),
        )
        .unwrap();
    pass.set_programmable_pipeline(pipeline).unwrap();
    pass.set_vertex_buffer(positions, 0).unwrap();
    pass.set_vertex_buffer_slot(1, attributes, 0).unwrap();
    pass.set_vertex_buffer_slot(2, uvs, 0).unwrap();
    pass.set_vertex_buffer_slot(3, glyphs, 0).unwrap();
    pass.draw(3, 0).unwrap();
    pass.end().unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    context
        .create_queue()
        .submit(&mut cache, &encoder.finish().unwrap())
        .unwrap();
    assert_eq!(
        cache.read_texture(target.id()).unwrap(),
        [0, 255, 64, 255].repeat(16)
    );
}

#[test]
fn comparison_sampler_reads_depth_produced_by_a_render_pass_on_gpu() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().unwrap();
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let extent = Extent2D::new(2, 2).unwrap();
    let target = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Rgba8Unorm,
                extent,
                TextureUsage::RENDER_ATTACHMENT,
            )
            .unwrap(),
        )
        .unwrap();
    let depth_desc = TextureDesc::new(
        TextureFormat::Depth32Float,
        extent,
        TextureUsage::RENDER_ATTACHMENT | TextureUsage::SAMPLED,
    )
    .unwrap();
    let depth = table.define_texture(depth_desc).unwrap();
    let output = table
        .define_buffer(BufferDesc::new(8, BufferUsage::STORAGE | BufferUsage::COPY_SRC).unwrap())
        .unwrap();
    let sampler = table
        .define_sampler(
            SamplerDesc::new(
                FilterMode::Nearest,
                FilterMode::Nearest,
                AddressMode::ClampToEdge,
                AddressMode::ClampToEdge,
            )
            .with_compare(Some(CompareFunction::LessEqual)),
        )
        .unwrap();
    let module = shader(
        &table,
        r#"
        @group(0) @binding(0) var depth: texture_depth_2d;
        @group(0) @binding(1) var comparison: sampler_comparison;
        @group(0) @binding(2) var<storage, read_write> results: array<f32,2>;
        @compute @workgroup_size(1) fn main() {
            results[0] = textureSampleCompareLevel(depth, comparison, vec2<f32>(0.5), 0.25);
            results[1] = textureSampleCompareLevel(depth, comparison, vec2<f32>(0.5), 0.75);
        }
    "#,
    );
    let layout = BindGroupLayoutDesc::new(vec![
        BindGroupLayoutEntry::new(
            0,
            ShaderStages::COMPUTE,
            BindingType::SampledTextureView {
                dimension: TextureViewDimension::D2,
                depth: true,
            },
        ),
        BindGroupLayoutEntry::new(1, ShaderStages::COMPUTE, BindingType::ComparisonSampler),
        BindGroupLayoutEntry::new(
            2,
            ShaderStages::COMPUTE,
            BindingType::StorageBuffer { read_only: false },
        ),
    ])
    .unwrap();
    let group = table
        .define_bind_group(
            BindGroupDesc::new(
                &table,
                layout.clone(),
                vec![
                    BindGroupEntry::new(
                        0,
                        BindingResource::TextureView {
                            texture: depth.id(),
                            view: TextureViewDesc::new(
                                depth_desc,
                                TextureFormat::Depth32Float,
                                TextureViewDimension::D2,
                                0,
                                1,
                                0,
                                1,
                            )
                            .unwrap(),
                        },
                    ),
                    BindGroupEntry::new(1, BindingResource::Sampler(sampler.id())),
                    BindGroupEntry::new(
                        2,
                        BindingResource::Buffer {
                            buffer: output.id(),
                            offset: 0,
                            size: 8,
                        },
                    ),
                ],
            )
            .unwrap(),
        )
        .unwrap();
    let pipeline = table
        .define_compute_pipeline(
            ComputePipelineDesc::new(
                entry(module, ShaderStage::Compute, "main"),
                PipelineLayoutDesc::new(vec![layout]).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let mut encoder = CommandEncoder::new(&table);
    encoder
        .begin_render_pass(
            RenderPassDesc::new(
                &table,
                target,
                PixelRect::new(0, 0, 2, 2).unwrap(),
                LoadOp::DontCare,
                StoreOp::Store,
            )
            .unwrap()
            .with_depth_attachment(&table, depth, DepthLoadOp::Clear(0.5), StoreOp::Store)
            .unwrap(),
        )
        .unwrap()
        .end()
        .unwrap();
    let mut pass = encoder.begin_compute_pass().unwrap();
    pass.set_pipeline(pipeline).unwrap();
    pass.set_bind_group(0, group).unwrap();
    pass.dispatch(1, 1, 1).unwrap();
    pass.end().unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    context
        .create_queue()
        .submit(&mut cache, &encoder.finish().unwrap())
        .unwrap();
    assert_eq!(
        cache.read_buffer(output.id(), 0, 8).unwrap(),
        bytemuck::cast_slice::<f32, u8>(&[1.0, 0.0])
    );
}

#[test]
fn generational_program_cache_churn_preserves_already_submitted_pixels() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().unwrap();
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let mut cache = context.create_resources(Rc::clone(&table));
    let queue = context.create_queue();
    let mut targets = Vec::new();
    for _ in 0..2 {
        targets.push(
            table
                .define_texture(
                    TextureDesc::new(
                        TextureFormat::Rgba8Unorm,
                        Extent2D::new(4, 4).unwrap(),
                        TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
                    )
                    .unwrap(),
                )
                .unwrap()
                .id(),
        );
    }
    let mut previous: Option<(crate::Submission, TextureId, u8)> = None;
    let mut retired = None;
    for iteration in 0..300u32 {
        let red = if iteration % 2 == 0 { 1.0 } else { 0.0 };
        let module = shader(
            &table,
            &format!(
                "{FULLSCREEN_VERTEX}\n@fragment fn fragment() -> @location(0) vec4<f32> {{ return vec4<f32>({red}, 0.0, 0.0, 1.0); }}\n@compute @workgroup_size(1) fn compute() {{}}"
            ),
        );
        let graphics = table
            .define_programmable_render_pipeline(
                ProgrammableRenderPipelineDesc::new(
                    entry(module, ShaderStage::Vertex, "vertex"),
                    entry(module, ShaderStage::Fragment, "fragment"),
                    PipelineLayoutDesc::new(vec![]).unwrap(),
                    TextureFormat::Rgba8Unorm,
                    None,
                    PrimitiveTopology::TriangleList,
                    BlendState::REPLACE,
                    RasterState::new(CullMode::None, FrontFace::CounterClockwise),
                )
                .unwrap(),
            )
            .unwrap();
        let compute = table
            .define_compute_pipeline(
                ComputePipelineDesc::new(
                    entry(module, ShaderStage::Compute, "compute"),
                    PipelineLayoutDesc::new(vec![]).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        cache.validate_shader_module(module.id()).unwrap();
        cache.validate_compute_pipeline(compute.id()).unwrap();
        cache
            .validate_programmable_render_pipeline(graphics.id())
            .unwrap();
        if let Some((shader, graphics, compute)) = retired {
            assert!(cache.validate_shader_module(shader).is_err());
            assert!(
                cache
                    .validate_programmable_render_pipeline(graphics)
                    .is_err()
            );
            assert!(cache.validate_compute_pipeline(compute).is_err());
        }
        let mut encoder = CommandEncoder::new(&table);
        let mut pass = encoder.begin_compute_pass().unwrap();
        pass.set_pipeline(compute).unwrap();
        pass.dispatch(1, 1, 1).unwrap();
        pass.end().unwrap();
        let target = table.texture_ref(targets[iteration as usize % 2]).unwrap();
        let mut pass = encoder
            .begin_render_pass(
                RenderPassDesc::new(
                    &table,
                    target,
                    PixelRect::new(0, 0, 4, 4).unwrap(),
                    LoadOp::Load,
                    StoreOp::Store,
                )
                .unwrap(),
            )
            .unwrap();
        pass.set_programmable_pipeline(graphics).unwrap();
        pass.draw(3, 0).unwrap();
        pass.end().unwrap();
        let commands = encoder.finish().unwrap();
        let receipt = queue.submit_tracked(&mut cache, &commands).unwrap();
        // WGPU submissions own GPU references independently of this cache. Drop
        // its old generation immediately and recreate the same slots next time.
        cache
            .release_programmable_render_pipeline(graphics.id())
            .unwrap();
        table
            .release_programmable_render_pipeline(graphics.id())
            .unwrap();
        cache.release_compute_pipeline(compute.id()).unwrap();
        table.release_compute_pipeline(compute.id()).unwrap();
        cache.release_shader_module(module.id()).unwrap();
        table.release_shader_module(module.id()).unwrap();
        assert_eq!(cache.programmable.cached_program_counts(), (0, 0, 0, 0));
        if let Some((old_receipt, old_target, red)) = previous.take() {
            assert_eq!(old_receipt.wait(None), Ok(CompletionStatus::Complete));
            assert_eq!(
                cache.read_texture(old_target).unwrap(),
                [red, 0, 0, 255].repeat(16)
            );
        }
        previous = Some((
            receipt,
            target.id(),
            if iteration % 2 == 0 { 255 } else { 0 },
        ));
        retired = Some((module.id(), graphics.id(), compute.id()));
    }
    let (receipt, target, red) = previous.unwrap();
    assert_eq!(receipt.wait(None), Ok(CompletionStatus::Complete));
    assert_eq!(
        cache.read_texture(target).unwrap(),
        [red, 0, 0, 255].repeat(16)
    );
}
