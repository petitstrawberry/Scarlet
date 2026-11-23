//! Native clamp-to-border must sample transparent black rather than an edge texel.

use super::*;
use ir::*;

#[test]
fn transparent_border_sampling_and_disabled_feature_are_checked_on_gpu() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().unwrap();
    let instance = raw::Instance::new(&raw::InstanceDescriptor::default());
    let Some(adapter) = request_headless_adapter(&instance) else {
        #[cfg(target_os = "macos")]
        panic!("Metal adapter is required for the sampler contract test");
        #[cfg(not(target_os = "macos"))]
        return;
    };
    eprintln!("SGFX sampler contract adapter: {:?}", adapter.get_info());
    if !adapter
        .features()
        .contains(raw::Features::ADDRESS_MODE_CLAMP_TO_BORDER)
    {
        #[cfg(target_os = "macos")]
        panic!("Metal must support native transparent clamp-to-border");
        #[cfg(not(target_os = "macos"))]
        return;
    }
    let (disabled, queue) =
        pollster::block_on(adapter.request_device(&raw::DeviceDescriptor::default(), None))
            .unwrap();
    let disabled = Device::new(disabled, queue);
    let disabled_context = disabled.create_context();
    let table = Rc::new(ResourceTable::new());
    let sampler = table
        .define_sampler(SamplerDesc::new(
            FilterMode::Nearest,
            FilterMode::Nearest,
            AddressMode::ClampToBorder,
            AddressMode::ClampToBorder,
        ))
        .unwrap();
    let mut cache = disabled_context.create_resources(Rc::clone(&table));
    assert!(matches!(
        cache.sampler(sampler),
        Err(crate::Error::Unsupported(
            UnsupportedFeature::SamplerBorderColor
        ))
    ));
    drop(cache);

    let (device, queue) = pollster::block_on(adapter.request_device(
        &raw::DeviceDescriptor {
            required_features: raw::Features::ADDRESS_MODE_CLAMP_TO_BORDER,
            ..Default::default()
        },
        None,
    ))
    .unwrap();
    let device = Device::new(device, queue);
    let context = device.create_context();
    for filter in [FilterMode::Nearest, FilterMode::Linear] {
        let table = Rc::new(ResourceTable::new());
        let source = table
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Rgba8Unorm,
                    Extent2D::new(2, 2).unwrap(),
                    TextureUsage::SAMPLED | TextureUsage::COPY_DST,
                )
                .unwrap(),
            )
            .unwrap();
        let target = table
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Rgba8Unorm,
                    Extent2D::new(3, 3).unwrap(),
                    TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
                )
                .unwrap(),
            )
            .unwrap();
        let sampler = table
            .define_sampler(SamplerDesc::new(
                filter,
                filter,
                AddressMode::ClampToBorder,
                AddressMode::ClampToBorder,
            ))
            .unwrap();
        let module = table
            .define_shader_module(
                ShaderModuleDesc::wgsl(
                    r#"
                    @vertex fn vertex(@builtin(vertex_index) id: u32)
                        -> @builtin(position) vec4<f32> {
                        var positions = array<vec2<f32>, 3>(
                            vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
                        return vec4(positions[id], 0.0, 1.0);
                    }
                    @group(0) @binding(0) var source: texture_2d<f32>;
                    @group(0) @binding(1) var filtering: sampler;
                    @fragment fn fragment(@builtin(position) p: vec4<f32>)
                        -> @location(0) vec4<f32> {
                        // Pixel centers map to -0.5, 0.5, 1.5 on each axis.
                        return textureSampleLevel(source, filtering, p.xy - vec2(1.0), 0.0);
                    }
                    "#
                    .into(),
                )
                .unwrap(),
            )
            .unwrap();
        let layout = BindGroupLayoutDesc::new(vec![
            BindGroupLayoutEntry::new(0, ShaderStages::FRAGMENT, BindingType::SampledTexture),
            BindGroupLayoutEntry::new(1, ShaderStages::FRAGMENT, BindingType::Sampler),
        ])
        .unwrap();
        let group = table
            .define_bind_group(
                BindGroupDesc::new(
                    &table,
                    layout.clone(),
                    vec![
                        BindGroupEntry::new(0, BindingResource::Texture(source.id())),
                        BindGroupEntry::new(1, BindingResource::Sampler(sampler.id())),
                    ],
                )
                .unwrap(),
            )
            .unwrap();
        let pipeline = table
            .define_programmable_render_pipeline(
                ProgrammableRenderPipelineDesc::new(
                    ShaderEntryPoint::new(module, ShaderStage::Vertex, "vertex".into()).unwrap(),
                    ShaderEntryPoint::new(module, ShaderStage::Fragment, "fragment".into())
                        .unwrap(),
                    PipelineLayoutDesc::new(vec![layout]).unwrap(),
                    TextureFormat::Rgba8Unorm,
                    None,
                    PrimitiveTopology::TriangleList,
                    BlendState::REPLACE,
                    RasterState::new(CullMode::None, FrontFace::CounterClockwise),
                )
                .unwrap(),
            )
            .unwrap();
        let pixels = [255, 73, 17, 255].repeat(4);
        let mut encoder = CommandEncoder::new(&table);
        encoder
            .write_texture(
                source,
                TextureWrite::new(PixelRect::new(0, 0, 2, 2).unwrap(), 8, &pixels).unwrap(),
            )
            .unwrap();
        let mut pass = encoder
            .begin_render_pass(
                RenderPassDesc::new(
                    &table,
                    target,
                    PixelRect::new(0, 0, 3, 3).unwrap(),
                    LoadOp::Clear(Color::rgba(0.0, 1.0, 1.0, 1.0).unwrap()),
                    StoreOp::Store,
                )
                .unwrap(),
            )
            .unwrap();
        pass.set_programmable_pipeline(pipeline).unwrap();
        pass.set_bind_group(0, group).unwrap();
        pass.draw(3, 0).unwrap();
        pass.end().unwrap();
        let mut cache = context.create_resources(Rc::clone(&table));
        context
            .create_queue()
            .submit(&mut cache, &encoder.finish().unwrap())
            .unwrap();
        let result = cache.read_texture(target.id()).unwrap();
        for (index, pixel) in result.chunks_exact(4).enumerate() {
            assert_eq!(
                pixel,
                if index == 4 {
                    &[255, 73, 17, 255]
                } else {
                    &[0; 4]
                },
                "{filter:?} sample at {},{}",
                index % 3,
                index / 3,
            );
        }
    }
}
