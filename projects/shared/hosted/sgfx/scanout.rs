//! Retained BGRA scanout shared by CPU uploads and SGFX presentation.
use scarlet::device::graphics::output::DisplayRegion;
use wgpu::*;

/// The caller owns drawable presentation. Dimensions must be nonzero and
/// within device limits. Use this texture with Worker::start/resize.
pub fn create_scanout(device: &Device, format: TextureFormat, width: u32, height: u32) -> Texture {
    device.create_texture(&TextureDescriptor {
        label: Some("Scarlet scanout"),
        size: Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: if format.is_srgb() {
            TextureFormat::Bgra8UnormSrgb
        } else {
            TextureFormat::Bgra8Unorm
        },
        usage: TextureUsages::COPY_SRC
            | TextureUsages::COPY_DST
            | TextureUsages::TEXTURE_BINDING
            | TextureUsages::RENDER_ATTACHMENT,
        // SGFX presents write the original BGRA bytes. Display sampling
        // still uses the default sRGB view when the surface is sRGB.
        view_formats: if format.is_srgb() {
            &[TextureFormat::Bgra8Unorm]
        } else {
            &[]
        },
    })
}

/// Upload a complete CPU framebuffer's damage; the first frame uploads all
/// pixels. The caller submits the queue before displaying the texture.
pub fn upload_scanout(
    queue: &Queue,
    texture: &Texture,
    have_frame: &mut bool,
    pixels: &[u8],
    damage: DisplayRegion,
) -> Result<(), String> {
    let (width, height) = (texture.width(), texture.height());
    let stride = width * 4;
    if pixels.len() != (stride * height) as usize
        || damage
            .x
            .checked_add(damage.width)
            .is_none_or(|end| end > width)
        || damage
            .y
            .checked_add(damage.height)
            .is_none_or(|end| end > height)
    {
        return Err("Invalid scanout damage".into());
    }
    let damage = if *have_frame {
        damage
    } else {
        DisplayRegion::new(0, 0, width, height)
    };
    if damage.width == 0 || damage.height == 0 {
        return Ok(());
    }
    let start = (damage.y * stride + damage.x * 4) as usize;
    let len = ((damage.height - 1) * stride + damage.width * 4) as usize;
    queue.write_texture(
        TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: Origin3d {
                x: damage.x,
                y: damage.y,
                z: 0,
            },
            aspect: TextureAspect::All,
        },
        &pixels[start..start + len],
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: None,
        },
        Extent3d {
            width: damage.width,
            height: damage.height,
            depth_or_array_layers: 1,
        },
    );
    *have_frame = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a Metal adapter"]
    fn cpu_upload_initializes_then_preserves_undamaged_pixels() {
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::METAL,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default(), None)).unwrap();
        for format in [TextureFormat::Bgra8Unorm, TextureFormat::Bgra8UnormSrgb] {
            let texture = create_scanout(&device, format, 2, 2);
            let mut have_frame = false;
            let background = [17, 51, 93, 255];
            let changed = [201, 103, 7, 255];
            let damage = DisplayRegion::new(1, 1, 1, 1);
            // Even with partial damage the first upload initializes all pixels.
            upload_scanout(
                &queue,
                &texture,
                &mut have_frame,
                &background.repeat(4),
                damage,
            )
            .unwrap();
            assert!(have_frame);
            // The second upload must not copy the supplied pixels outside damage.
            upload_scanout(
                &queue,
                &texture,
                &mut have_frame,
                &changed.repeat(4),
                damage,
            )
            .unwrap();
            assert!(
                upload_scanout(
                    &queue,
                    &texture,
                    &mut have_frame,
                    &[0; 16],
                    DisplayRegion::new(2, 0, 1, 1)
                )
                .is_err()
            );
            assert!(upload_scanout(&queue, &texture, &mut have_frame, &[0; 15], damage).is_err());
            let pitch = COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = device.create_buffer(&BufferDescriptor {
                label: None,
                size: u64::from(pitch) * 2,
                usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(pitch),
                        rows_per_image: None,
                    },
                },
                texture.size(),
            );
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            buffer
                .slice(..)
                .map_async(MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(Maintain::Wait);
            rx.recv().unwrap().unwrap();
            let bytes = buffer.slice(..).get_mapped_range();
            for y in 0..2 {
                for x in 0..2 {
                    let offset = y * pitch as usize + x * 4;
                    let expected = if x == 1 && y == 1 {
                        changed
                    } else {
                        background
                    };
                    assert_eq!(bytes[offset..offset + 4], expected, "{format:?} ({x}, {y})");
                }
            }
        }
    }
}
