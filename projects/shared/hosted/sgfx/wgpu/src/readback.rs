//! Explicit, blocking CPU readback of logical resources.

use super::{BufferId, Error, Resources, Result, TextureId, UnsupportedFeature};

impl Resources {
    /// Read a nonempty byte range from a logical buffer after preceding queue work.
    ///
    /// The buffer must allow `COPY_SRC`; `offset` and `size` must be multiples
    /// of four and the range must fit the buffer. This explicitly waits for
    /// the readback copy on native targets. Browsers return
    /// [`UnsupportedFeature::BlockingWait`].
    pub fn read_buffer(&mut self, id: BufferId, offset: u64, size: u64) -> Result<Vec<u8>> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (id, offset, size);
            Err(Error::Unsupported(UnsupportedFeature::BlockingWait))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use super::{Arc, BufferUsage, Rc, raw};

            let validation = Arc::clone(&self.context.device.validation);
            let _scope_guard = validation.lock().map_err(|_| Error::InvalidState)?;
            let resources = Rc::clone(&self.resources);
            let reference = resources.buffer_ref(id)?;
            let descriptor = resources.buffer(reference)?;
            if !descriptor.usage().contains(BufferUsage::COPY_SRC)
                || size == 0
                || offset
                    .checked_add(size)
                    .is_none_or(|end| end > descriptor.size())
            {
                return Err(Error::InvalidState);
            }
            if !offset.is_multiple_of(raw::COPY_BUFFER_ALIGNMENT)
                || !size.is_multiple_of(raw::COPY_BUFFER_ALIGNMENT)
            {
                return Err(Error::Validation(
                    "buffer readback offset and size must be multiples of four".into(),
                ));
            }
            let device = self.context.device.clone();
            validate_readback_size(&device, size)?;
            let (staging, command) = scoped(&device, || {
                let source = self.buffer(reference)?;
                let staging = staging_buffer(&device, size);
                let mut encoder =
                    device
                        .raw_device()
                        .create_command_encoder(&raw::CommandEncoderDescriptor {
                            label: Some("sgfx WGPU buffer readback"),
                        });
                encoder.copy_buffer_to_buffer(&source.buffer, offset, &staging, 0, size);
                Ok((staging, encoder.finish()))
            })?;
            let index = scoped(&device, || Ok(device.raw_queue().submit([command])))?;
            map_readback(&device, &staging, index)
        }
    }

    /// Read all pixels from a logical color texture after preceding queue work.
    ///
    /// The texture must allow `COPY_SRC`. Returned rows are tightly packed
    /// in the texture's logical channel order, without WGPU copy padding.
    /// This explicitly waits on native targets. Depth textures are unsupported;
    /// browsers return [`UnsupportedFeature::BlockingWait`].
    pub fn read_texture(&mut self, id: TextureId) -> Result<Vec<u8>> {
        self.read_texture_mip(id, 0)
    }

    /// Read the tightly packed pixels of one checked color mip level.
    pub fn read_texture_mip(&mut self, id: TextureId, mip_level: u32) -> Result<Vec<u8>> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (id, mip_level);
            Err(Error::Unsupported(UnsupportedFeature::BlockingWait))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use super::{Arc, Rc, TextureFormat, TextureUsage, raw};

            let validation = Arc::clone(&self.context.device.validation);
            let _scope_guard = validation.lock().map_err(|_| Error::InvalidState)?;
            let resources = Rc::clone(&self.resources);
            let reference = resources.texture_ref(id)?;
            let descriptor = resources.texture(reference)?;
            if descriptor.format() == TextureFormat::Depth32Float {
                return Err(Error::Unsupported(UnsupportedFeature::TextureFormat));
            }
            if !descriptor.usage().contains(TextureUsage::COPY_SRC) {
                return Err(Error::InvalidState);
            }
            let extent = descriptor.mip_extent(mip_level)?;
            let width = extent.width();
            let height = extent.height();
            let row_size = width
                .checked_mul(
                    descriptor
                        .format()
                        .bytes_per_pixel()
                        .ok_or(Error::Unsupported(UnsupportedFeature::TextureFormat))?,
                )
                .ok_or(Error::Unsupported(UnsupportedFeature::ResourceSize))?;
            let alignment = raw::COPY_BYTES_PER_ROW_ALIGNMENT;
            let row_stride = row_size
                .checked_add(alignment - 1)
                .map(|size| size / alignment * alignment)
                .ok_or(Error::Unsupported(UnsupportedFeature::ResourceSize))?;
            let size = u64::from(row_stride) * u64::from(height);
            let device = self.context.device.clone();
            validate_readback_size(&device, size)?;
            let (staging, command) = scoped(&device, || {
                let source = self.texture(reference)?;
                let staging = staging_buffer(&device, size);
                let mut encoder =
                    device
                        .raw_device()
                        .create_command_encoder(&raw::CommandEncoderDescriptor {
                            label: Some("sgfx WGPU texture readback"),
                        });
                encoder.copy_texture_to_buffer(
                    raw::TexelCopyTextureInfo {
                        texture: &source.texture,
                        mip_level,
                        origin: raw::Origin3d::ZERO,
                        aspect: raw::TextureAspect::All,
                    },
                    raw::TexelCopyBufferInfo {
                        buffer: &staging,
                        layout: raw::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(row_stride),
                            rows_per_image: Some(height),
                        },
                    },
                    raw::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
                Ok((staging, encoder.finish()))
            })?;
            let index = scoped(&device, || Ok(device.raw_queue().submit([command])))?;
            let mut bytes = map_readback(&device, &staging, index)?;
            let row_size = row_size as usize;
            let row_stride = row_stride as usize;
            // Compact in place so even a one-byte texture with heavily padded
            // copy rows needs only one CPU allocation.
            for row in 1..height as usize {
                bytes.copy_within(
                    row * row_stride..row * row_stride + row_size,
                    row * row_size,
                );
            }
            bytes.truncate(row_size * height as usize);
            Ok(bytes)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
use super::{Device, raw};
#[cfg(not(target_arch = "wasm32"))]
use core::sync::atomic::Ordering;

#[cfg(not(target_arch = "wasm32"))]
fn validate_readback_size(device: &Device, size: u64) -> Result<()> {
    if device.tracker.lost.load(Ordering::Acquire) {
        return Err(Error::DeviceLost);
    }
    if size > device.raw_device().limits().max_buffer_size || usize::try_from(size).is_err() {
        return Err(Error::Unsupported(UnsupportedFeature::ResourceSize));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn staging_buffer(device: &Device, size: u64) -> raw::Buffer {
    device.raw_device().create_buffer(&raw::BufferDescriptor {
        label: Some("sgfx WGPU readback staging"),
        size,
        usage: raw::BufferUsages::COPY_DST | raw::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

// WGPU 24 native error scopes resolve immediately, independently of queue
// completion. Keep scopes balanced even when a logical lookup fails.
#[cfg(not(target_arch = "wasm32"))]
fn scoped<T>(device: &Device, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    if device.tracker.lost.load(Ordering::Acquire) {
        return Err(Error::DeviceLost);
    }
    for filter in [
        raw::ErrorFilter::OutOfMemory,
        raw::ErrorFilter::Internal,
        raw::ErrorFilter::Validation,
    ] {
        device.raw_device().push_error_scope(filter);
    }
    let result = operation();
    let mut failure = None;
    for _ in 0..3 {
        let error = pollster::block_on(device.raw_device().pop_error_scope());
        if failure.is_none() {
            failure = error;
        }
    }
    if device.tracker.lost.load(Ordering::Acquire) {
        Err(Error::DeviceLost)
    } else if let Some(error) = failure {
        Err(Error::Validation(error.to_string()))
    } else {
        result
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn map_readback(
    device: &Device,
    staging: &raw::Buffer,
    index: raw::SubmissionIndex,
) -> Result<Vec<u8>> {
    let slice = staging.slice(..);
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    scoped(device, || {
        slice.map_async(raw::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        Ok(())
    })?;
    let _ = device
        .raw_device()
        .poll(raw::Maintain::WaitForSubmissionIndex(index));
    if device.tracker.lost.load(Ordering::Acquire) {
        return Err(Error::DeviceLost);
    }
    receiver
        .try_recv()
        .map_err(|_| Error::CompletionObservation)?
        .map_err(|_| Error::CompletionObservation)?;
    let bytes = slice.get_mapped_range();
    let mut result = Vec::new();
    result
        .try_reserve_exact(bytes.len())
        .map_err(|_| Error::Unsupported(UnsupportedFeature::ResourceSize))?;
    result.extend_from_slice(&bytes);
    drop(bytes);
    staging.unmap();
    Ok(result)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::{Rc, ResourceTable};
    use sgfx_core::{
        backend::CommandExecutor,
        ir::{
            BufferDesc, BufferUsage, CommandEncoder, Extent2D, PixelRect, TextureDesc,
            TextureFormat, TextureUsage, TextureWrite,
        },
    };

    #[test]
    fn logical_readback_preserves_buffer_ranges_and_unpadded_texture_rows() {
        let _guard = crate::tests::HEADLESS_WGPU_TEST_LOCK
            .lock()
            .expect("lock WGPU tests");
        let instance = raw::Instance::new(&raw::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&raw::RequestAdapterOptions::default()));
        let Some(adapter) = adapter else {
            #[cfg(target_os = "macos")]
            panic!("Metal adapter must be available for SGFX readback tests");
            #[cfg(not(target_os = "macos"))]
            return;
        };
        let (raw_device, raw_queue) =
            pollster::block_on(adapter.request_device(&raw::DeviceDescriptor::default(), None))
                .expect("headless readback device");
        let device = Device::new(raw_device, raw_queue);
        let context = device.create_context();
        let queue = context.create_queue();
        let table = Rc::new(ResourceTable::new());
        let mut cache = context.create_resources(Rc::clone(&table));
        let buffer = table
            .define_buffer(
                BufferDesc::new(16, BufferUsage::COPY_SRC | BufferUsage::COPY_DST).unwrap(),
            )
            .unwrap();
        let bytes: Vec<u8> = (0..16).collect();
        let mut encoder = CommandEncoder::new(&table);
        encoder.write_buffer(buffer, 0, &bytes).unwrap();
        queue
            .executor(&mut cache)
            .execute(&encoder.finish().unwrap())
            .unwrap();
        assert_eq!(cache.read_buffer(buffer.id(), 4, 8).unwrap(), bytes[4..12]);
        assert!(cache.read_buffer(buffer.id(), 1, 4).is_err());
        assert!(cache.read_buffer(buffer.id(), 12, 8).is_err());
        assert!(cache.read_buffer(buffer.id(), u64::MAX - 3, 8).is_err());
        assert!(cache.read_buffer(buffer.id(), 0, 0).is_err());
        let no_copy = table
            .define_buffer(BufferDesc::new(4, BufferUsage::VERTEX).unwrap())
            .unwrap();
        assert!(cache.read_buffer(no_copy.id(), 0, 4).is_err());
        let other_table = ResourceTable::new();
        let foreign = other_table
            .define_buffer(BufferDesc::new(4, BufferUsage::COPY_SRC).unwrap())
            .unwrap();
        assert!(cache.read_buffer(foreign.id(), 0, 4).is_err());

        for format in [
            TextureFormat::R8Unorm,
            TextureFormat::Rgba8Unorm,
            TextureFormat::Bgra8Unorm,
        ] {
            let extent = Extent2D::new(3, 2).unwrap();
            let texture = table
                .define_texture(
                    TextureDesc::new(
                        format,
                        extent,
                        TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
                    )
                    .unwrap(),
                )
                .unwrap();
            let row_size = 3 * format.bytes_per_pixel().unwrap();
            let pixels: Vec<u8> = (0..row_size * 2)
                .map(|value| (value * 7 + 3) as u8)
                .collect();
            let mut encoder = CommandEncoder::new(&table);
            encoder
                .write_texture(
                    texture,
                    TextureWrite::new(PixelRect::new(0, 0, 3, 2).unwrap(), row_size, &pixels)
                        .unwrap(),
                )
                .unwrap();
            queue
                .executor(&mut cache)
                .execute(&encoder.finish().unwrap())
                .unwrap();
            assert_eq!(
                cache.read_texture(texture.id()).unwrap(),
                pixels,
                "format {format:?}"
            );
        }
        let depth = table
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Depth32Float,
                    Extent2D::new(1, 1).unwrap(),
                    TextureUsage::RENDER_ATTACHMENT,
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            cache.read_texture(depth.id()),
            Err(Error::Unsupported(UnsupportedFeature::TextureFormat))
        ));
        let no_copy_texture = table
            .define_texture(
                TextureDesc::new(
                    TextureFormat::R8Unorm,
                    Extent2D::new(1, 1).unwrap(),
                    TextureUsage::SAMPLED,
                )
                .unwrap(),
            )
            .unwrap();
        assert!(cache.read_texture(no_copy_texture.id()).is_err());
        let foreign_texture = other_table
            .define_texture(
                TextureDesc::new(
                    TextureFormat::R8Unorm,
                    Extent2D::new(1, 1).unwrap(),
                    TextureUsage::COPY_SRC,
                )
                .unwrap(),
            )
            .unwrap();
        assert!(cache.read_texture(foreign_texture.id()).is_err());
        // Failed requests leave the cache and error-scope stack usable.
        assert_eq!(cache.read_buffer(buffer.id(), 0, 16).unwrap(), bytes);
        device.raw_device().destroy();
        let _ = device.raw_device().poll(raw::Maintain::Poll);
        assert_eq!(cache.read_buffer(buffer.id(), 0, 4), Err(Error::DeviceLost));
    }
}
