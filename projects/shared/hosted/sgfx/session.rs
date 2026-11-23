//! SGFX command decoder and resource table owned by the SGFX execution thread.
use super::{host::Images, wire};
use scarlet::device::gpu::{
    GPU_IMAGE_USAGE_PRESENTABLE, GPU_IMAGE_USAGE_RENDER_TARGET, GPU_IMAGE_USAGE_SAMPLED,
    GPU_IMAGE_USAGE_TRANSFER_DST, GPU_IMAGE_USAGE_TRANSFER_SRC,
};
use sgfx_backend_wgpu as sgfx;
use sgfx_core::ir::{CommandBuffer, ResourceTable, TextureUsage};
use std::{collections::BTreeMap, rc::Rc};
pub(super) enum ResourceRead<'a> {
    Bytes(Vec<u8>),
    Texture(&'a wgpu::Texture),
}

pub(super) struct Session {
    table: Rc<ResourceTable>,
    resources: sgfx::Resources,
    queue: sgfx::Queue,
    bindings: BTreeMap<u64, (u64, u32)>,
    source: Option<(u64, u64)>,
    packet: Vec<u64>,
    expected: usize,
}
impl Session {
    pub fn new(gpu: &sgfx::Context) -> Self {
        let table = Rc::new(ResourceTable::new());
        Self {
            resources: gpu.create_resources(table.clone()),
            table,
            queue: gpu.create_queue(),
            bindings: BTreeMap::new(),
            source: None,
            packet: Vec::new(),
            expected: 0,
        }
    }
}
impl Session {
    fn authorized(allowed: &BTreeMap<u64, u32>, &(id, required): &(u64, u32)) -> bool {
        allowed
            .get(&id)
            .is_some_and(|usage| usage & required == required)
    }
    pub fn receive(
        &mut self,
        images: &Images,
        allowed: &BTreeMap<u64, u32>,
        mut words: Vec<u64>,
    ) -> Result<(), &'static str> {
        match words.as_slice() {
            [wire::START, len] if *len > 0 && *len <= wire::MAX_MESSAGE as u64 / 8 => {
                self.packet.clear();
                self.expected = *len as usize;
                Ok(())
            }
            [wire::APPEND, chunk @ ..]
                if !chunk.is_empty()
                    && self.expected != 0
                    && chunk.len() <= self.expected - self.packet.len() =>
            {
                let received = self.packet.len() + chunk.len();
                if received > self.packet.capacity() {
                    // Bound growth by both the actual input and the final length.
                    // A START alone must never allocate its advertised byte count.
                    let capacity = self.expected.min(received.saturating_mul(2));
                    self.packet.reserve_exact(capacity - self.packet.len());
                }
                self.packet.extend_from_slice(chunk);
                Ok(())
            }
            [wire::FINISH] if self.expected != 0 && self.packet.len() == self.expected => {
                self.expected = 0;
                let mut packet = std::mem::take(&mut self.packet);
                self.apply(images, allowed, &mut packet)
            }
            _ => {
                self.expected = 0;
                self.packet.clear();
                self.apply(images, allowed, &mut words)
            }
        }
    }
    pub fn read_resource<'a>(
        &mut self,
        kind: u64,
        slot: u32,
        offset: u64,
        len: usize,
        allowed: &BTreeMap<u64, u32>,
        images: &'a Images,
        target: &wgpu::Texture,
    ) -> Result<ResourceRead<'a>, &'static str> {
        match kind {
            wire::READ_BUFFER => {
                let buffer = self.table.abi_buffer(slot).map_err(|_| "Unknown buffer")?;
                // wgpu copies four-byte units; expose the requested byte range.
                let start = offset & !3;
                let end = offset
                    .checked_add(len as u64)
                    .and_then(|v| v.checked_next_multiple_of(4))
                    .ok_or("Invalid read range")?;
                let mut bytes = self
                    .resources
                    .read_buffer(buffer.id(), start, end - start)
                    .map_err(|_| "SGFX buffer readback failed")?;
                let skip = (offset - start) as usize;
                if skip != 0 {
                    bytes.copy_within(skip..skip + len, 0);
                }
                bytes.truncate(len);
                Ok(ResourceRead::Bytes(bytes))
            }
            wire::READ_TEXTURE if offset == 0 => {
                if self
                    .bindings
                    .get(&u64::from(slot))
                    .is_some_and(|binding| !Self::authorized(allowed, binding))
                {
                    return Err("Detached SGFX texture");
                }
                let texture = self
                    .table
                    .abi_texture(slot)
                    .map_err(|_| "Unknown texture")?;
                let desc = self.table.texture(texture).map_err(|_| "Unknown texture")?;
                let extent = desc.extent();
                let size = u64::from(extent.width()) * u64::from(extent.height());
                if size.checked_mul(u64::from(
                    desc.format()
                        .bytes_per_pixel()
                        .ok_or("Unknown texel size")?,
                )) != Some(len as u64)
                {
                    return Err("Invalid texture readback size");
                }
                // A mapped BGRA source with the destination layout can stay on
                // the GPU until the client's final readback of the staging image.
                if desc.usage().contains(TextureUsage::COPY_SRC)
                    && let Some((id, _)) = self.bindings.get(&u64::from(slot))
                    && let Some(image) = images.get(id)
                {
                    let src = image.raw_texture();
                    if src.size() == target.size() && !std::ptr::eq(src, target) {
                        return Ok(ResourceRead::Texture(src));
                    }
                }
                self.resources
                    .read_texture(texture.id())
                    .map(ResourceRead::Bytes)
                    .map_err(|_| "SGFX texture readback failed")
            }
            _ => Err("Invalid SGFX readback"),
        }
    }
    fn apply(
        &mut self,
        images: &Images,
        allowed: &BTreeMap<u64, u32>,
        words: &mut [u64],
    ) -> Result<(), &'static str> {
        match &*words {
            [wire::SNAPSHOT, metadata @ ..] => {
                if metadata.len() < 3 {
                    return Err("Short SGFX resource snapshot");
                }
                if let Some((id, revision)) = self.source {
                    if metadata[1] != id || metadata[2] < revision {
                        return Err("Stale SGFX snapshot");
                    }
                    if metadata[2] == revision {
                        return Ok(());
                    }
                    let (textures, buffers) = self
                        .table
                        .abi_retired_resources(metadata)
                        .map_err(|_| "Invalid SGFX resource retirement")?;
                    for id in textures {
                        let slot = self
                            .table
                            .texture_ref(id)
                            .map_err(|_| "Unknown retired texture")?
                            .slot();
                        self.bindings.remove(&(slot as u64));
                        self.resources
                            .release_texture(id)
                            .map_err(|_| "SGFX texture retirement failed")?;
                    }
                    for id in buffers {
                        self.resources
                            .release_buffer(id)
                            .map_err(|_| "SGFX buffer retirement failed")?;
                    }
                    let (shaders, compute, render) = self
                        .table
                        .abi_retired_programs(metadata)
                        .map_err(|_| "Invalid SGFX program retirement")?;
                    for id in render {
                        self.resources
                            .release_programmable_render_pipeline(id)
                            .map_err(|_| "SGFX pipeline retirement failed")?;
                    }
                    for id in compute {
                        self.resources
                            .release_compute_pipeline(id)
                            .map_err(|_| "SGFX pipeline retirement failed")?;
                    }
                    for id in shaders {
                        self.resources
                            .release_shader_module(id)
                            .map_err(|_| "SGFX shader retirement failed")?;
                    }
                }
                self.source = Some(
                    self.table
                        .sync_abi_snapshot(metadata)
                        .map_err(|_| "Invalid SGFX snapshot")?,
                );
                Ok(())
            }
            [
                kind @ (wire::RELEASE_SHADER
                | wire::RELEASE_COMPUTE_PIPELINE
                | wire::RELEASE_RENDER_PIPELINE),
                slot,
                generation,
            ] => {
                let slot = u32::try_from(*slot).map_err(|_| "Invalid SGFX program slot")?;
                match *kind {
                    wire::RELEASE_SHADER => {
                        let shader = self
                            .table
                            .abi_shader_module(slot)
                            .map_err(|_| "Unknown shader")?;
                        if shader.generation() != *generation {
                            return Err("Stale SGFX shader");
                        }
                        self.resources.release_shader_module(shader.id())
                    }
                    wire::RELEASE_COMPUTE_PIPELINE => {
                        let pipeline = self
                            .table
                            .abi_compute_pipeline(slot)
                            .map_err(|_| "Unknown compute pipeline")?;
                        if pipeline.generation() != *generation {
                            return Err("Stale SGFX compute pipeline");
                        }
                        self.resources.release_compute_pipeline(pipeline.id())
                    }
                    _ => {
                        let pipeline = self
                            .table
                            .abi_programmable_render_pipeline(slot)
                            .map_err(|_| "Unknown render pipeline")?;
                        if pipeline.generation() != *generation {
                            return Err("Stale SGFX render pipeline");
                        }
                        self.resources
                            .release_programmable_render_pipeline(pipeline.id())
                    }
                }
                .map_err(|_| "SGFX program retirement failed")
            }
            [wire::VALIDATE, kind, slot] => {
                let slot = u32::try_from(*slot).map_err(|_| "Invalid SGFX slot")?;
                match *kind as u32 {
                    sgfx_backend_abi::VALIDATE_SHADER => self.resources.validate_shader_module(
                        self.table
                            .abi_shader_module(slot)
                            .map_err(|_| "Unknown shader")?
                            .id(),
                    ),
                    sgfx_backend_abi::VALIDATE_RENDER_PIPELINE => {
                        self.resources.validate_programmable_render_pipeline(
                            self.table
                                .abi_programmable_render_pipeline(slot)
                                .map_err(|_| "Unknown render pipeline")?
                                .id(),
                        )
                    }
                    sgfx_backend_abi::VALIDATE_COMPUTE_PIPELINE => {
                        self.resources.validate_compute_pipeline(
                            self.table
                                .abi_compute_pipeline(slot)
                                .map_err(|_| "Unknown compute pipeline")?
                                .id(),
                        )
                    }
                    _ => return Err("Invalid SGFX validation kind"),
                }
                .map_err(|_| "SGFX validation failed")
            }
            [
                kind @ (wire::RELEASE_BUFFER | wire::RELEASE_TEXTURE | wire::RELEASE_GROUP),
                slot,
            ] => {
                let index = u32::try_from(*slot).map_err(|_| "Invalid SGFX slot")?;
                match *kind {
                    wire::RELEASE_BUFFER => self.resources.release_buffer(
                        self.table
                            .abi_buffer(index)
                            .map_err(|_| "Unknown buffer")?
                            .id(),
                    ),
                    wire::RELEASE_TEXTURE => {
                        let id = self
                            .table
                            .abi_texture(index)
                            .map_err(|_| "Unknown texture")?
                            .id();
                        self.resources
                            .release_texture(id)
                            .map_err(|_| "SGFX texture release failed")?;
                        self.bindings.remove(slot);
                        return Ok(());
                    }
                    _ => self.resources.release_bind_group(
                        self.table
                            .abi_bind_group(index)
                            .map_err(|_| "Unknown bind group")?
                            .id(),
                    ),
                }
                .map_err(|_| "SGFX resource release failed")
            }
            [wire::MAP, slot, token] => {
                let image = images.get(token).ok_or("Unknown SGFX image")?;
                let texture = self
                    .table
                    .abi_texture((*slot).try_into().map_err(|_| "Invalid SGFX slot")?)
                    .map_err(|_| "Unknown SGFX texture")?;
                let usage = self
                    .table
                    .texture(texture)
                    .map_err(|_| "Unknown SGFX texture")?
                    .usage();
                let mut required = 0;
                for (logical, physical) in [
                    (TextureUsage::SAMPLED, GPU_IMAGE_USAGE_SAMPLED),
                    (TextureUsage::COPY_SRC, GPU_IMAGE_USAGE_TRANSFER_SRC),
                    (TextureUsage::COPY_DST, GPU_IMAGE_USAGE_TRANSFER_DST),
                    (
                        TextureUsage::RENDER_ATTACHMENT,
                        GPU_IMAGE_USAGE_RENDER_TARGET,
                    ),
                    (TextureUsage::PRESENT, GPU_IMAGE_USAGE_PRESENTABLE),
                ] {
                    if usage.contains(logical) {
                        required |= physical;
                    }
                }
                let binding = (*token, required);
                if !Self::authorized(allowed, &binding) {
                    return Err("Unauthorized SGFX image usage");
                }
                let image = image.clone();
                if usage.contains(TextureUsage::PRESENT) {
                    self.resources.map_image(texture.id(), image)
                } else if usage.contains(TextureUsage::RENDER_ATTACHMENT) {
                    self.resources.map_color_target(texture.id(), image)
                } else {
                    self.resources.import_sampled_image(texture.id(), image)
                }
                .map_err(|_| "SGFX image layout mismatch")?;
                self.bindings.insert(*slot, binding);
                Ok(())
            }
            [wire::UNMAP, slot] => {
                let id = self
                    .table
                    .abi_texture((*slot).try_into().map_err(|_| "Invalid SGFX slot")?)
                    .map_err(|_| "Unknown SGFX texture")?
                    .id();
                self.resources.unmap_image(id);
                self.bindings.remove(slot);
                Ok(())
            }
            [wire::EXECUTE, table, count, len, payload @ ..] => {
                if self
                    .bindings
                    .values()
                    .any(|binding| !Self::authorized(allowed, binding))
                {
                    return Err("Detached SGFX texture binding");
                }
                let (source, _) = self.source.ok_or("Missing SGFX resource table")?;
                if *table != source || *len > payload.len() as u64 {
                    return Err("Invalid SGFX recording");
                }
                let (count, len) = (*count as usize, *len as usize);
                let (record, upload) = words[4..].split_at_mut(len);
                let upload = wire::as_bytes(upload);
                wire::uploads(record, count, |offset, len| {
                    let start = usize::try_from(*offset).map_err(|_| ())?;
                    let bytes = upload
                        .get(start..start.checked_add(len).ok_or(())?)
                        .ok_or(())?;
                    *offset = bytes.as_ptr() as u64;
                    Ok(())
                })
                .map_err(|_| "Invalid SGFX upload range")?;
                let batch = sgfx_backend_abi::Batch {
                    table: source,
                    words: sgfx_backend_abi::Span::from_slice(record),
                    count,
                };
                // Every embedded span now points into this immutable packet.
                let commands = unsafe { CommandBuffer::from_abi(&self.table, source, batch) }
                    .map_err(|_| "Invalid SGFX commands")?;
                self.queue
                    .submit(&mut self.resources, &commands)
                    .map_err(|_| "SGFX wgpu submission failed")?;
                Ok(())
            }
            _ => Err("Invalid SGFX transport message"),
        }
    }
}
