//! Scarlet GPU capabilities and kernel-owned attachment/backing leases.
//! The SGFX host owns its native graphics resources.
mod host;
mod scale;
mod session;
mod transport;
pub use host::{Host, run_offscreen};
use scarlet::device::{gpu::*, graphics::GpuDisplayResource};
use scarlet_hosted_sgfx_protocol as wire;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak, atomic::Ordering},
};
pub use transport::Endpoint;
use transport::{Command, Rect};

impl Endpoint {
    fn image(&self, id: u64) -> Result<Arc<Image>, &'static str> {
        self.0
            .images
            .lock()
            .unwrap()
            .get(&id)
            .and_then(Weak::upgrade)
            .ok_or("Unknown SGFX image")
    }
}
impl GpuBackend for Endpoint {
    fn query_info(&self) -> GpuBackendInfo {
        let mut features = GPU_EXECUTION_SUPPORT_QUEUE
            | GPU_EXECUTION_SUPPORT_MEMORY
            | GPU_EXECUTION_SUPPORT_IMAGE_READBACK
            | GPU_EXECUTION_SUPPORT_IMAGE_UPLOAD;
        if self.0.presentation {
            features |= GPU_EXECUTION_SUPPORT_PRESENTATION;
        }
        GpuBackendInfo::new(
            GpuDeviceInfo::new(GpuDeviceState::Ready, features, GPU_MAX_OPAQUE_COMMAND_SIZE),
            0,
            wire::BACKEND,
            &[],
        )
    }
    fn query_dialect(&self, index: u32) -> Result<GpuBackendDialectInfo, &'static str> {
        if index != 0 {
            return Err("Unknown SGFX dialect");
        }
        Ok(GpuBackendDialectInfo::new(
            index,
            wire::DIALECT,
            wire::BACKEND,
        ))
    }
    fn create_context(
        &self,
        d: GpuBackendDialectDescriptor,
    ) -> Result<Arc<dyn GpuBackendContext>, &'static str> {
        if d.index != 0 || d.token != wire::DIALECT {
            return Err("Invalid SGFX dialect");
        }
        Ok(Arc::new(Context {
            endpoint: self.clone(),
            images: Arc::default(),
        }))
    }
    fn create_image(
        &self,
        c: GpuImageCreateInfo,
        b: GpuImageBackingInfo,
    ) -> Result<Arc<dyn GpuBackendImage>, &'static str> {
        if c.format != GPU_IMAGE_FORMAT_BGRA8_UNORM || !b.is_physically_contiguous() {
            return Err("Unsupported SGFX image layout");
        }
        let id = self.id()?;
        u32::try_from(id).map_err(|_| "Display IDs exhausted")?;
        self.call(Command::CreateImage(id, c.width, c.height), false)?;
        let image = Arc::new(Image {
            info: GpuBackendImageInfo::new(c, id, b.allocation_size),
            endpoint: self.clone(),
            backing: b,
        });
        self.0
            .images
            .lock()
            .unwrap()
            .insert(id, Arc::downgrade(&image));
        Ok(image)
    }
}
struct Image {
    info: GpuBackendImageInfo,
    endpoint: Endpoint,
    backing: GpuImageBackingInfo,
}
impl Drop for Image {
    fn drop(&mut self) {
        let id = self.info.command_resource_token;
        self.endpoint.0.images.lock().unwrap().remove(&id);
        self.endpoint.release(Command::DropImage(id));
    }
}
impl GpuBackendImage for Image {
    fn query_info(&self) -> GpuBackendImageInfo {
        self.info
    }
    fn backend_cookie(&self) -> u64 {
        self.endpoint.cookie()
    }
    fn display_resource(&self) -> Option<GpuDisplayResource> {
        (self.endpoint.0.presentation && self.info.usage & GPU_IMAGE_USAGE_PRESENTABLE != 0).then(
            || {
                GpuDisplayResource::new(
                    self.info.command_resource_token as u32,
                    self.info.width,
                    self.info.height,
                    self.backend_cookie(),
                )
            },
        )
    }
}
#[derive(Clone)]
struct Context {
    endpoint: Endpoint,
    images: Arc<Mutex<Attachments>>,
}
#[derive(Default)]
struct Attachments {
    pinned: BTreeMap<u64, (Arc<Image>, GpuBackingPin)>,
    // Accepted commands keep the permission snapshot from submission time.
    permissions: Arc<BTreeMap<u64, u32>>,
}

impl Context {
    fn attached(
        &self,
        image: &dyn GpuBackendImage,
    ) -> Result<(Arc<Image>, GpuBackingPin), &'static str> {
        if image.backend_cookie() != self.endpoint.cookie() {
            return Err("Foreign SGFX image");
        }
        self.images
            .lock()
            .unwrap()
            .pinned
            .get(&image.query_info().command_resource_token)
            .cloned()
            .ok_or("Detached SGFX image")
    }
    fn transfer(
        &self,
        image: &dyn GpuBackendImage,
        t: GpuImageUploadInfo,
        readback: bool,
    ) -> Result<(), &'static str> {
        let (image, pin) = self.attached(image)?;
        if readback && !pin.allows_gpu_write() {
            return Err("Read-only image backing");
        }
        let row = t.width as usize * 4;
        let stride = t.backing_stride as usize;
        let offset = t.backing_offset as usize;
        let height = t.height as usize;
        let size = height
            .checked_mul(row)
            .ok_or("SGFX transfer size overflow")?;
        let end = height
            .checked_sub(1)
            .and_then(|n| n.checked_mul(stride))
            .and_then(|n| n.checked_add(row))
            .and_then(|n| n.checked_add(offset))
            .ok_or("Invalid SGFX transfer")?;
        if row == 0 || row > stride || end > image.backing.allocation_size as usize {
            return Err("Invalid SGFX transfer range");
        }
        let id = image.info.command_resource_token;
        let rect = Rect {
            x: t.dst_x,
            y: t.dst_y,
            width: t.width,
            height: t.height,
        };
        let base = scarlet::vm::phys_to_virt(image.backing.paddr) as *mut u8;
        // The attached pin and the device's CPU-access lease span the complete
        // copy. No kernel alias or backing descriptor is sent to the SGFX host.
        if readback {
            let bytes = self.endpoint.call(Command::Readback(id, rect), true)?;
            if bytes.len() != size {
                return Err("Invalid SGFX readback length");
            }
            for (y, src) in bytes.chunks_exact(row).enumerate() {
                unsafe {
                    std::ptr::copy_nonoverlapping(src.as_ptr(), base.add(offset + y * stride), row);
                }
            }
        } else {
            let mut bytes = Vec::<u8>::with_capacity(size);
            // Copy directly from pinned backing into the owned transport buffer.
            unsafe {
                let src = base.add(offset);
                let dst = bytes.as_mut_ptr();
                if stride == row {
                    std::ptr::copy_nonoverlapping(src, dst, size);
                } else {
                    for y in 0..height {
                        std::ptr::copy_nonoverlapping(src.add(y * stride), dst.add(y * row), row);
                    }
                }
                // Both layouts initialize exactly size bytes, excluding guest padding.
                bytes.set_len(size);
            }
            self.endpoint.call(Command::Upload(id, rect, bytes), true)?;
        }
        Ok(())
    }
}
impl GpuBackendContext for Context {
    fn query_info(&self) -> GpuBackendContextInfo {
        GpuBackendContextInfo::new(0, wire::DIALECT)
    }
    fn create_queue(&self) -> Result<Arc<dyn GpuBackendQueue>, &'static str> {
        let id = self.endpoint.id()?;
        self.endpoint.call(Command::CreateQueue(id), false)?;
        Ok(Arc::new(Queue {
            id,
            ctx: self.clone(),
        }))
    }
    fn attach_image_pinned(
        &self,
        image: &dyn GpuBackendImage,
        pin: GpuBackingPin,
    ) -> Result<u64, &'static str> {
        if image.backend_cookie() != self.endpoint.cookie() {
            return Err("Foreign SGFX image");
        }
        let id = image.query_info().command_resource_token;
        let image = self.endpoint.image(id)?;
        let mut usage = image.info.usage;
        if !pin.allows_gpu_write() {
            usage &= !(GPU_IMAGE_USAGE_RENDER_TARGET | GPU_IMAGE_USAGE_TRANSFER_DST);
        }
        let mut images = self.images.lock().unwrap();
        images.pinned.insert(id, (image, pin));
        Arc::make_mut(&mut images.permissions).insert(id, usage);
        Ok(id)
    }
    fn detach_image(&self, image: &dyn GpuBackendImage) -> Result<(), &'static str> {
        if image.backend_cookie() != self.endpoint.cookie() {
            return Err("Foreign SGFX image");
        }
        let id = image.query_info().command_resource_token;
        let mut images = self.images.lock().unwrap();
        images.pinned.remove(&id).ok_or("Detached SGFX image")?;
        Arc::make_mut(&mut images.permissions).remove(&id);
        Ok(())
    }
    fn begin_image_cpu_access_inline(
        &self,
        image: &dyn GpuBackendImage,
    ) -> Result<Option<u64>, &'static str> {
        self.attached(image)?;
        self.endpoint.reserve_cpu()?;
        Ok(Some(1))
    }
    fn end_image_cpu_access_inline(&self, _: &dyn GpuBackendImage, _: u64) {
        self.endpoint.release_cpu();
    }
    fn readback_image_bgra(
        &self,
        i: &dyn GpuBackendImage,
        t: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        self.transfer(i, t, true)
    }
    fn upload_image_bgra(
        &self,
        i: &dyn GpuBackendImage,
        t: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        self.transfer(i, t, false)
    }
    fn transfer_imported_image_bgra(
        &self,
        i: &dyn GpuBackendImage,
        t: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        self.transfer(i, t, false)
    }
}
struct Queue {
    id: u64,
    ctx: Context,
}
impl Drop for Queue {
    fn drop(&mut self) {
        self.ctx.endpoint.release(Command::DropQueue(self.id));
    }
}
impl Queue {
    fn command(&self, bytes: &[u8]) -> Result<Command, GpuBackendSubmitError> {
        if bytes.len() % 8 != 0 {
            return Err(GpuBackendSubmitError::Rejected("Misaligned SGFX packet"));
        }
        let words: Vec<_> = bytes
            .chunks_exact(8)
            .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
            .collect();
        let images = self.ctx.images.lock().unwrap();
        if let [wire::READ_BUFFER | wire::READ_TEXTURE, _, _, _, destination] = words.as_slice() {
            let writable = images
                .permissions
                .get(destination)
                .is_some_and(|usage| usage & GPU_IMAGE_USAGE_TRANSFER_DST != 0);
            if !writable {
                return Err(GpuBackendSubmitError::Rejected(
                    "Invalid readback destination",
                ));
            }
        }
        Ok(Command::Execute {
            queue: self.id,
            words,
            attachments: images.permissions.clone(),
        })
    }
}
impl GpuBackendQueue for Queue {
    fn query_info(&self) -> GpuBackendQueueInfo {
        GpuBackendQueueInfo::new(GPU_MAX_OPAQUE_COMMAND_SIZE)
    }
    fn submit(&self, b: &[u8]) -> Result<(), GpuBackendSubmitError> {
        self.ctx
            .endpoint
            .call(self.command(b)?, false)
            .map(drop)
            .map_err(GpuBackendSubmitError::Rejected)
    }
    fn async_capacity(&self) -> u32 {
        8
    }
    fn enqueue(&self, s: GpuSubmission) -> Result<(), GpuBackendEnqueueError> {
        let command = match self.command(s.commands()) {
            Ok(c) => c,
            Err(e) => return Err(GpuBackendEnqueueError::Rejected(e, s)),
        };
        self.ctx.endpoint.enqueue(command, s)
    }
}
