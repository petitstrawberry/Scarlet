//! Real-device checks for the portable command execution contract.

use sgfx_core::backend::CommandExecutor;

use super::*;

pub(super) fn headless_device() -> Option<Device> {
    let instance = raw::Instance::new(&raw::InstanceDescriptor::default());
    let Some(adapter) = request_headless_adapter(&instance) else {
        #[cfg(target_os = "macos")]
        panic!("Metal adapter must be available for SGFX execution contract tests");
        #[cfg(not(target_os = "macos"))]
        {
            eprintln!("skipping SGFX execution contract test: no adapter available");
            return None;
        }
    };
    eprintln!("SGFX execution contract adapter: {:?}", adapter.get_info());
    let (device, queue) =
        pollster::block_on(adapter.request_device(&raw::DeviceDescriptor::default(), None))
            .expect("headless WGPU device");
    Some(Device::new(device, queue))
}

pub(super) fn readback_pixels(device: &Device, image: &Image) -> Vec<[u8; 4]> {
    let stride = (image.width() * 4).div_ceil(256) * 256;
    let readback = device.raw_device().create_buffer(&raw::BufferDescriptor {
        label: Some("SGFX execution contract readback"),
        size: u64::from(stride) * u64::from(image.height()),
        usage: raw::BufferUsages::COPY_DST | raw::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device
        .raw_device()
        .create_command_encoder(&raw::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        raw::TexelCopyTextureInfo {
            texture: image.raw_texture(),
            mip_level: 0,
            origin: raw::Origin3d::ZERO,
            aspect: raw::TextureAspect::All,
        },
        raw::TexelCopyBufferInfo {
            buffer: &readback,
            layout: raw::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(image.height()),
            },
        },
        raw::Extent3d {
            width: image.width(),
            height: image.height(),
            depth_or_array_layers: 1,
        },
    );
    device.raw_queue().submit([encoder.finish()]);
    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(raw::MapMode::Read, move |result| {
        sender.send(result).expect("send readback result");
    });
    let _ = device.raw_device().poll(raw::Maintain::Wait);
    receiver
        .recv()
        .expect("receive readback result")
        .expect("map readback");
    let bytes = slice.get_mapped_range();
    bytes
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..image.width() as usize * 4].chunks_exact(4))
        .map(|pixel| pixel.try_into().expect("BGRA pixel"))
        .collect()
}

#[test]
fn blits_r8_mips_on_the_gpu() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let texture = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::R8Unorm,
                Extent2D::new(4, 4).unwrap(),
                TextureUsage::SAMPLED | TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
            )
            .unwrap()
            .with_mip_level_count(2)
            .unwrap(),
        )
        .unwrap();
    let pixels = [173_u8; 16];
    let mut encoder = CommandEncoder::new(table.as_ref());
    encoder
        .write_texture(
            texture,
            TextureWrite::new(PixelRect::new(0, 0, 4, 4).unwrap(), 4, &pixels).unwrap(),
        )
        .unwrap();
    encoder
        .blit_texture(texture, 0, texture, 1, FilterMode::Nearest)
        .unwrap();
    let commands = encoder.finish().unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    context
        .create_queue()
        .executor(&mut cache)
        .execute(&commands)
        .unwrap();
    assert_eq!(cache.read_texture_mip(texture.id(), 1).unwrap(), [173; 4]);
}

#[test]
fn uploads_outlive_caller_storage_and_keep_submission_order() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let queue = context.create_queue();
    let table = Rc::new(ResourceTable::new());
    let extent = Extent2D::new(4, 4).expect("extent");
    let area = PixelRect::new(0, 0, 4, 4).expect("area");
    let source = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Bgra8Unorm,
                extent,
                TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
            )
            .expect("source descriptor"),
        )
        .expect("source");
    let mut cache = context.create_resources(Rc::clone(&table));
    let colors = [[17, 31, 211, 255], [197, 83, 29, 255]];
    let mut images = Vec::new();
    for color in colors {
        let target = table
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    extent,
                    TextureUsage::RENDER_ATTACHMENT
                        | TextureUsage::PRESENT
                        | TextureUsage::COPY_DST,
                )
                .expect("target descriptor"),
            )
            .expect("target");
        let image = context
            .create_image(4, 4, TextureFormat::Bgra8Unorm)
            .expect("image");
        cache
            .map_image(target.id(), Arc::clone(&image))
            .expect("map image");
        let mut pixels = color.repeat(16);
        let mut encoder = CommandEncoder::new(table.as_ref());
        encoder
            .write_texture(
                source,
                TextureWrite::new(area, 16, &pixels).expect("upload"),
            )
            .expect("record upload");
        encoder
            .copy_texture_to_texture(source, area, target, area)
            .expect("record copy");
        let commands = encoder.finish().expect("finish commands");
        queue
            .executor(&mut cache)
            .execute(&commands)
            .expect("execute");
        drop(commands);
        // No wait: the backend must not retain a borrow of this allocation.
        pixels.fill(0);
        drop(pixels);
        images.push(image);
    }
    // The source texture is owned only by the cache and pending WGPU work.
    // Drop the cache before waiting; each queued copy must still see its own
    // upload, not the later upload or the caller's zeroed allocation.
    drop(cache);
    drop(queue);
    drop(context);
    drop(table);
    for (image, color) in images.iter().zip(colors) {
        assert_eq!(readback_pixels(&device, image), vec![color; 16]);
    }
}

#[test]
fn executor_rejects_a_different_resource_table_even_when_empty() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let queue = context.create_queue();
    let table = Rc::new(ResourceTable::new());
    let other_table = ResourceTable::new();
    let mut cache = context.create_resources(Rc::clone(&table));
    let empty = CommandEncoder::new(&table).finish().expect("empty stream");
    queue
        .executor(&mut cache)
        .execute(&empty)
        .expect("empty no-op");
    let foreign = CommandEncoder::new(&other_table)
        .finish()
        .expect("foreign stream");
    assert!(matches!(
        queue.executor(&mut cache).execute(&foreign),
        Err(Error::ResourceTableMismatch)
    ));
}

#[test]
fn executor_keeps_interleaved_uploads_and_copies_in_command_order() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let descriptor = TextureDesc::new(
        TextureFormat::Bgra8Unorm,
        Extent2D::new(1, 1).expect("extent"),
        TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
    )
    .expect("descriptor");
    let source = table.define_texture(descriptor).expect("source");
    let first_target = table.define_texture(descriptor).expect("first target");
    let second_target = table.define_texture(descriptor).expect("second target");
    let area = PixelRect::new(0, 0, 1, 1).expect("area");
    let first_color = [19, 43, 131, 255];
    let second_color = [207, 71, 23, 255];
    let mut encoder = CommandEncoder::new(table.as_ref());
    encoder
        .write_texture(
            source,
            TextureWrite::new(area, 4, &first_color).expect("first upload"),
        )
        .expect("record first upload");
    encoder
        .copy_texture_to_texture(source, area, first_target, area)
        .expect("copy first color");
    encoder
        .write_texture(
            source,
            TextureWrite::new(area, 4, &second_color).expect("second upload"),
        )
        .expect("record upload after copy");
    encoder
        .copy_texture_to_texture(source, area, second_target, area)
        .expect("copy second color");
    let commands = encoder.finish().expect("finish stream");
    let mut cache = context.create_resources(Rc::clone(&table));
    context
        .create_queue()
        .executor(&mut cache)
        .execute(&commands)
        .expect("execute interleaved uploads and copies");
    assert_eq!(cache.read_texture(first_target.id()).unwrap(), first_color);
    assert_eq!(
        cache.read_texture(second_target.id()).unwrap(),
        second_color
    );
    assert_eq!(cache.read_texture(source.id()).unwrap(), second_color);
}

#[test]
fn executor_rejects_unaligned_buffer_uploads_before_wgpu_validation() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let queue = context.create_queue();
    let table = Rc::new(ResourceTable::new());
    let buffer = table
        .define_buffer(ir::BufferDesc::new(8, BufferUsage::COPY_DST).expect("buffer descriptor"))
        .expect("buffer");
    let mut cache = context.create_resources(Rc::clone(&table));
    for (offset, data) in [
        (1, &[1, 2, 3, 4][..]),
        (0, &[1, 2, 3][..]),
        (1, &[1, 2, 3][..]),
    ] {
        let mut encoder = CommandEncoder::new(&table);
        encoder
            .write_buffer(buffer, offset, data)
            .expect("byte-granular IR upload");
        let commands = encoder.finish().expect("finish stream");
        device
            .raw_device()
            .push_error_scope(raw::ErrorFilter::Validation);
        let result = queue.executor(&mut cache).execute(&commands);
        let raw_error = pollster::block_on(device.raw_device().pop_error_scope());
        assert!(
            matches!(
                result,
                Err(Error::Unsupported(UnsupportedFeature::BufferWriteAlignment))
            ),
            "result: {result:?}; raw error: {raw_error:?}"
        );
        assert!(
            raw_error.is_none(),
            "SGFX must reject before raw WGPU validation: {raw_error:?}"
        );
        assert!(
            cache.buffers.is_empty(),
            "rejected upload must not materialize its buffer"
        );
    }
    let mut encoder = CommandEncoder::new(&table);
    encoder
        .write_buffer(buffer, 4, &[1, 2, 3, 4])
        .expect("aligned upload");
    device
        .raw_device()
        .push_error_scope(raw::ErrorFilter::Validation);
    queue
        .executor(&mut cache)
        .execute(&encoder.finish().expect("finish stream"))
        .expect("aligned upload after rejected writes");
    assert!(pollster::block_on(device.raw_device().pop_error_scope()).is_none());
}

#[test]
fn executor_rejects_buffers_exceeding_device_limits_before_allocation() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let size = device
        .raw_device()
        .limits()
        .max_buffer_size
        .checked_add(4)
        .expect("size beyond device limit");
    let buffer = table
        .define_buffer(
            ir::BufferDesc::new(size, BufferUsage::COPY_DST).expect("logical descriptor"),
        )
        .expect("logical buffer");
    let mut encoder = CommandEncoder::new(&table);
    encoder
        .write_buffer(buffer, 0, &[0; 4])
        .expect("small in-bounds upload");
    let mut cache = context.create_resources(Rc::clone(&table));
    device
        .raw_device()
        .push_error_scope(raw::ErrorFilter::Validation);
    let result = context
        .create_queue()
        .executor(&mut cache)
        .execute(&encoder.finish().expect("finish stream"));
    let raw_error = pollster::block_on(device.raw_device().pop_error_scope());
    assert!(
        matches!(
            result,
            Err(Error::Unsupported(UnsupportedFeature::ResourceSize))
        ),
        "result: {result:?}; raw error: {raw_error:?}"
    );
    assert!(
        raw_error.is_none(),
        "SGFX must reject before raw WGPU validation: {raw_error:?}"
    );
    assert!(cache.buffers.is_empty());
}

#[test]
fn image_creation_and_ir_textures_reject_excessive_dimensions() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let dimension = device
        .raw_device()
        .limits()
        .max_texture_dimension_2d
        .checked_add(1)
        .expect("dimension beyond device limit");
    for (width, height) in [(dimension, 1), (1, dimension)] {
        device
            .raw_device()
            .push_error_scope(raw::ErrorFilter::Validation);
        let result = context.create_image(width, height, TextureFormat::Bgra8Unorm);
        let raw_error = pollster::block_on(device.raw_device().pop_error_scope());
        assert!(
            matches!(
                result,
                Err(Error::Unsupported(UnsupportedFeature::ResourceSize))
            ),
            "image creation must reject excessive dimensions; raw error: {raw_error:?}"
        );
        assert!(
            raw_error.is_none(),
            "SGFX must reject before raw WGPU validation: {raw_error:?}"
        );

        let table = Rc::new(ResourceTable::new());
        let texture = table
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    Extent2D::new(width, height).expect("nonzero extent"),
                    TextureUsage::COPY_DST,
                )
                .expect("logical descriptor"),
            )
            .expect("logical texture");
        let mut encoder = CommandEncoder::new(&table);
        encoder
            .write_texture(
                texture,
                TextureWrite::new(PixelRect::new(0, 0, 1, 1).expect("one pixel"), 4, &[0; 4])
                    .expect("upload layout"),
            )
            .expect("small in-bounds upload");
        let mut cache = context.create_resources(Rc::clone(&table));
        device
            .raw_device()
            .push_error_scope(raw::ErrorFilter::Validation);
        let result = context
            .create_queue()
            .executor(&mut cache)
            .execute(&encoder.finish().expect("finish stream"));
        let raw_error = pollster::block_on(device.raw_device().pop_error_scope());
        assert!(
            matches!(
                result,
                Err(Error::Unsupported(UnsupportedFeature::ResourceSize))
            ),
            "result: {result:?}; raw error: {raw_error:?}"
        );
        assert!(
            raw_error.is_none(),
            "SGFX must reject before raw WGPU validation: {raw_error:?}"
        );
        assert!(cache.textures.is_empty());
    }
    context
        .create_image(1, 1, TextureFormat::Bgra8Unorm)
        .expect("valid image after rejected dimensions");
}

#[test]
fn reduced_state_keeps_draw_colors_clips_and_fresh_pass_bindings_on_gpu() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().unwrap();
    let Some(device) = headless_device() else {
        return;
    };
    for abi in [false, true] {
        let context = device.create_context();
        let table = Rc::new(ResourceTable::new());
        if abi {
            table.enable_abi_commands();
        }
        let target = table
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    Extent2D::new(4, 4).unwrap(),
                    TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
                )
                .unwrap(),
            )
            .unwrap();
        let buffer = table
            .define_buffer(
                BufferDesc::new(24, BufferUsage::VERTEX | BufferUsage::COPY_DST).unwrap(),
            )
            .unwrap();
        let pipeline = table
            .define_render_pipeline(
                RenderPipelineDesc::new(
                    TextureFormat::Bgra8Unorm,
                    PrimitiveTopology::TriangleList,
                    VertexBufferLayout::new(
                        8,
                        vec![ir::VertexAttribute::new(0, ir::VertexFormat::Float32x2, 0)],
                    )
                    .unwrap(),
                    FragmentProgram::Solid,
                    BlendState::REPLACE,
                    RasterState::new(ir::CullMode::None, ir::FrontFace::CounterClockwise),
                )
                .unwrap(),
            )
            .unwrap();
        let vertices: Vec<u8> = [-1_f32, -1., 3., -1., -1., 3.]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let mut encoder = CommandEncoder::new(&table);
        encoder.write_buffer(buffer, 0, &vertices).unwrap();
        let red = Color::rgba(1., 0., 0., 1.).unwrap();
        let green = Color::rgba(0., 1., 0., 1.).unwrap();
        for (load, draws) in [
            (
                LoadOp::Clear(Color::rgba(0., 0., 0., 1.).unwrap()),
                &[(red, 0), (red, 0), (green, 2), (green, 2)][..],
            ),
            (LoadOp::Load, &[(red, 0)][..]),
        ] {
            let mut pass = encoder
                .begin_render_pass(
                    RenderPassDesc::new(
                        &table,
                        target,
                        PixelRect::new(0, 0, 4, 4).unwrap(),
                        load,
                        StoreOp::Store,
                    )
                    .unwrap(),
                )
                .unwrap();
            for &(color, x) in draws {
                pass.set_pipeline(pipeline).unwrap();
                pass.set_vertex_buffer(buffer, 0).unwrap();
                pass.set_uniforms(DrawUniforms::new(Transform::identity(), color))
                    .unwrap();
                pass.set_scissor(Some(PixelRect::new(x, 0, 2, 4).unwrap()))
                    .unwrap();
                pass.draw(3, 0).unwrap();
            }
            pass.end().unwrap();
        }
        let commands = encoder.finish().unwrap();
        let mut cache = context.create_resources(Rc::clone(&table));
        context
            .create_queue()
            .executor(&mut cache)
            .execute(&commands)
            .unwrap();
        let pixels = cache.read_texture(target.id()).unwrap();
        for (index, pixel) in pixels.chunks_exact(4).enumerate() {
            assert_eq!(
                pixel,
                if index % 4 < 2 {
                    &[0, 0, 255, 255]
                } else {
                    &[0, 255, 0, 255]
                }
            );
        }
    }
}

#[test]
fn shared_color_target_and_sampled_import_retain_one_allocation() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let extent = Extent2D::new(2, 1).unwrap();
    let texture = |usage| {
        table
            .define_texture(TextureDesc::new(TextureFormat::Bgra8Unorm, extent, usage).unwrap())
            .unwrap()
    };
    let target = texture(TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_DST);
    let sampled = texture(TextureUsage::SAMPLED | TextureUsage::COPY_SRC);
    let writable = texture(TextureUsage::SAMPLED | TextureUsage::COPY_DST);
    let image = context
        .create_image(2, 1, TextureFormat::Bgra8Unorm)
        .unwrap();
    let mut producer = context.create_resources(table.clone());
    let mut consumer = context.create_resources(table.clone());
    assert_eq!(
        producer.map_image(target.id(), image.clone()),
        Err(Error::InvalidState)
    );
    producer
        .map_color_target(target.id(), image.clone())
        .unwrap();
    assert_eq!(
        consumer.import_sampled_image(writable.id(), image.clone()),
        Err(Error::InvalidState)
    );
    consumer.import_sampled_image(sampled.id(), image).unwrap();
    let bytes = [5, 15, 25, 255, 7, 17, 27, 255];
    let area = PixelRect::new(0, 0, 2, 1).unwrap();
    let mut encoder = CommandEncoder::new(&table);
    encoder
        .write_texture(target, TextureWrite::new(area, 8, &bytes).unwrap())
        .unwrap();
    context
        .create_queue()
        .executor(&mut producer)
        .execute(&encoder.finish().unwrap())
        .unwrap();
    drop(producer);
    assert_eq!(consumer.read_texture(sampled.id()).unwrap(), bytes);
}
