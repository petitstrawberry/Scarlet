//! SGFX's low-level resource and queue ABI, used by the native Vulkan ICD.
use super::*;

struct Resources {
    gpu: Rc<Gpu>,
    session: Session,
    table: ResourceTable,
}

unsafe extern "C" fn create_resources(p: Object, metadata: Span<u64>, out: *mut Object) -> i32 {
    output(out, || {
        let context = unsafe { borrow::<Context>(p) }?;
        let metadata = unsafe { metadata.as_slice() };
        let table = ResourceTable::new();
        table
            .sync_abi_snapshot(metadata)
            .map_err(|_| abi::INVALID)?;
        Ok(Resources {
            gpu: context.gpu.clone(),
            session: Session::new(&context.ctx, metadata)?,
            table,
        })
    })
}

unsafe extern "C" fn sync_resources(p: Object, metadata: Span<u64>) -> i32 {
    status((|| {
        let r = unsafe { borrow::<Resources>(p) }?;
        let metadata = unsafe { metadata.as_slice() };
        let (textures, _) = r
            .table
            .abi_retired_resources(metadata)
            .map_err(|_| abi::INVALID)?;
        let slots = textures
            .into_iter()
            .map(|id| r.table.texture_ref(id).map(|t| t.slot() as u32))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| abi::INVALID)?;
        snapshot(&r.session, metadata)?;
        r.table
            .sync_abi_snapshot(metadata)
            .map_err(|_| abi::INVALID)?;
        for slot in slots {
            if let Some(image) = r.session.images.remove(&slot) {
                r.session
                    .ctx
                    .detach_image(&image)
                    .map_err(|_| abi::DEVICE_LOST)?;
            }
        }
        Ok(())
    })())
}
unsafe extern "C" fn validate(p: Object, kind: u32, slot: u32) -> i32 {
    status((|| {
        unsafe { borrow::<Resources>(p) }?
            .session
            .send(&[wire::VALIDATE, kind.into(), slot.into()])
    })())
}
unsafe extern "C" fn release_buffer(p: Object, slot: u32) -> i32 {
    status((|| {
        unsafe { borrow::<Resources>(p) }?
            .session
            .send(&[wire::RELEASE_BUFFER, slot.into()])
    })())
}
unsafe extern "C" fn release_group(p: Object, slot: u32) -> i32 {
    status((|| {
        unsafe { borrow::<Resources>(p) }?
            .session
            .send(&[wire::RELEASE_GROUP, slot.into()])
    })())
}
unsafe extern "C" fn release_texture(p: Object, slot: u32) -> i32 {
    status((|| {
        let r = unsafe { borrow::<Resources>(p) }?;
        r.session.send(&[wire::RELEASE_TEXTURE, slot.into()])?;
        if let Some(image) = r.session.images.remove(&slot) {
            r.session
                .ctx
                .detach_image(&image)
                .map_err(|_| abi::DEVICE_LOST)?;
        }
        Ok(())
    })())
}

unsafe extern "C" fn create_image(
    p: Object,
    width: u32,
    height: u32,
    out: *mut Object,
    info: *mut abi::ImageInfo,
) -> i32 {
    let Some(info) = (unsafe { info.as_mut() }) else {
        return abi::INVALID;
    };
    *info = abi::ImageInfo {
        handle: -1,
        ..Default::default()
    };
    output(out, || {
        let ctx = unsafe { borrow::<Context>(p) }?;
        let image = shared_image(&ctx.gpu, width, height)?;
        let handle = image
            .as_handle()
            .duplicate()
            .map_err(|_| abi::OUT_OF_MEMORY)?;
        *info = abi::ImageInfo {
            width,
            height,
            handle: handle.as_raw(),
            reserved: 0,
        };
        std::mem::forget(handle);
        Ok(image)
    })
}
unsafe extern "C" fn map_image(p: Object, slot: u32, image: Object) -> i32 {
    status((|| {
        let r = unsafe { borrow::<Resources>(p) }?;
        let image = unsafe { borrow::<GpuImage>(image) }?;
        let copy = image
            .as_handle()
            .duplicate()
            .map_err(|_| abi::OUT_OF_MEMORY)?;
        r.session
            .map(slot, GpuImage::from_handle(copy).map_err(|_| abi::INVALID)?)
    })())
}
unsafe extern "C" fn unmap_image(p: Object, slot: u32) -> i32 {
    status((|| {
        unsafe { borrow::<Resources>(p) }?
            .session
            .forget_image(slot)
    })())
}

// Logical queues on the same context submit to one ordered Metal queue. The
// Scarlet transport queue belonging to the resource table retains its native cache.
unsafe extern "C" fn create_queue(p: Object, out: *mut Object) -> i32 {
    output(out, || Ok(unsafe { borrow::<Context>(p) }?.ctx.clone()))
}
unsafe extern "C" fn submit(
    p: Object,
    resources: Object,
    batch: *const abi::Batch,
    out: *mut abi::SubmitResult,
) {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return;
    };
    let result = (|| {
        let ctx = unsafe { borrow::<Rc<GpuContext>>(p) }?;
        let r = unsafe { borrow::<Resources>(resources) }?;
        if !Rc::ptr_eq(ctx, &r.session.ctx) {
            return Err(abi::INVALID);
        }
        unsafe { super::submit((&raw mut r.session).cast(), batch, out) };
        Ok(())
    })();
    if let Err(error) = result {
        *out = abi::SubmitResult {
            error,
            ..Default::default()
        };
    }
}

impl Resources {
    fn read(&self, kind: u64, slot: u32, offset: u64, out: &mut [u8]) -> Status<()> {
        if out.is_empty() {
            return Ok(());
        }
        if out.len() > wire::MAX_READBACK {
            return Err(abi::OUT_OF_MEMORY);
        }
        let pixels = out.len().div_ceil(4);
        let width = pixels.min(4096) as u32;
        let height = pixels.div_ceil(width as usize) as u32;
        // Match a mapped texture's shape so the host can copy it to staging
        // on Metal. Logical buffers and other formats keep the byte layout.
        let (width, height) =
            if kind == wire::READ_TEXTURE && self.session.images.contains_key(&slot) {
                let texture = self.table.abi_texture(slot).map_err(|_| abi::INVALID)?;
                let desc = self.table.texture(texture).map_err(|_| abi::INVALID)?;
                let extent = desc.extent();
                if (u64::from(extent.width()) * u64::from(extent.height())).checked_mul(4)
                    != Some(out.len() as u64)
                {
                    return Err(abi::INVALID);
                }
                (extent.width(), extent.height())
            } else {
                (width, height)
            };
        let staging = self
            .gpu
            .create_image_with_usage(
                width,
                height,
                gpu_raw::GPU_IMAGE_USAGE_TRANSFER_SRC | gpu_raw::GPU_IMAGE_USAGE_TRANSFER_DST,
            )
            .map_err(|_| abi::OUT_OF_MEMORY)?;
        let token = self
            .session
            .ctx
            .attach_image(&staging)
            .map_err(|_| abi::INVALID)?;
        let result = (|| {
            self.session
                .send(&[kind, slot.into(), offset, out.len() as u64, token])?;
            let rect = gpu_raw::GpuImageBgraRect::new(0, 0, width, height);
            let total = width as usize * height as usize * 4;
            let mut padded = Vec::new();
            let destination = if total == out.len() {
                &mut *out
            } else {
                padded.resize(total, 0);
                &mut padded
            };
            self.session
                .ctx
                .readback_image_bgra(&staging, destination, width * 4, rect)
                .map_err(|_| abi::INVALID)?;
            if !padded.is_empty() {
                out.copy_from_slice(&padded[..out.len()]);
            }
            Ok(())
        })();
        let detached = self
            .session
            .ctx
            .detach_image(&staging)
            .map(|_| ())
            .map_err(|_| abi::DEVICE_LOST);
        result.and(detached)
    }
}

unsafe fn destination<'a>(out: *mut u8, len: usize) -> Status<&'a mut [u8]> {
    if len == 0 {
        return Ok(&mut []);
    }
    if out.is_null() || len > wire::MAX_READBACK {
        return Err(abi::INVALID);
    }
    Ok(unsafe { std::slice::from_raw_parts_mut(out, len) })
}
unsafe extern "C" fn read_buffer(
    p: Object,
    slot: u32,
    offset: u64,
    out: *mut u8,
    len: usize,
) -> i32 {
    status((|| {
        unsafe { borrow::<Resources>(p) }?.read(wire::READ_BUFFER, slot, offset, unsafe {
            destination(out, len)
        }?)
    })())
}
unsafe extern "C" fn read_texture(
    p: Object,
    resources: Object,
    slot: u32,
    out: *mut u8,
    len: usize,
) -> i32 {
    status((|| {
        let r = unsafe { borrow::<Resources>(resources) }?;
        if !Rc::ptr_eq(&r.session.ctx, &unsafe { borrow::<Context>(p) }?.ctx) {
            return Err(abi::INVALID);
        }
        r.read(wire::READ_TEXTURE, slot, 0, unsafe {
            destination(out, len)
        }?)
    })())
}
unsafe extern "C" fn clone_receipt(p: Object) {
    if !p.is_null() {
        unsafe { Arc::increment_strong_count(p.cast::<Receipt>()) };
    }
}

/// # Safety
/// The loader supplies writable storage of the negotiated size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sgfx_backend_get_driver_api_v2(
    version: u32,
    size: usize,
    out: *mut abi::DriverApi,
) -> i32 {
    if version != abi::VERSION || size < size_of::<abi::DriverApi>() {
        return abi::ABI_MISMATCH;
    }
    if out.is_null() {
        return abi::INVALID;
    }
    let api = abi::DriverApi {
        version,
        size: size_of::<abi::DriverApi>() as u32,
        create_resources,
        drop_resources: destroy::<Resources>,
        sync_resources,
        release_buffer,
        validate,
        read_buffer,
        create_image,
        drop_image: destroy::<GpuImage>,
        map_image,
        unmap_image,
        create_queue,
        drop_queue: destroy::<Rc<GpuContext>>,
        submit,
        read_texture,
        clone_receipt,
        release_texture,
        release_bind_group: release_group,
    };
    unsafe { out.write(api) };
    abi::OK
}

unsafe fn release_program(p: Object, kind: u64, slot: u32, generation: u64) -> i32 {
    status((|| {
        unsafe { borrow::<Resources>(p) }?
            .session
            .send(&[kind, slot.into(), generation])
    })())
}
unsafe extern "C" fn release_shader(p: Object, slot: u32, generation: u64) -> i32 {
    unsafe { release_program(p, wire::RELEASE_SHADER, slot, generation) }
}
unsafe extern "C" fn release_compute(p: Object, slot: u32, generation: u64) -> i32 {
    unsafe { release_program(p, wire::RELEASE_COMPUTE_PIPELINE, slot, generation) }
}
unsafe extern "C" fn release_render(p: Object, slot: u32, generation: u64) -> i32 {
    unsafe { release_program(p, wire::RELEASE_RENDER_PIPELINE, slot, generation) }
}
/// # Safety
/// The loader supplies writable storage of the negotiated size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sgfx_backend_get_program_release_api_v2(
    version: u32,
    size: usize,
    out: *mut abi::ProgramReleaseApi,
) -> i32 {
    if version != abi::VERSION || size < size_of::<abi::ProgramReleaseApi>() {
        return abi::ABI_MISMATCH;
    }
    if out.is_null() {
        return abi::INVALID;
    }
    unsafe {
        out.write(abi::ProgramReleaseApi {
            version,
            size: size_of::<abi::ProgramReleaseApi>() as u32,
            release_shader_module: release_shader,
            release_compute_pipeline: release_compute,
            release_programmable_render_pipeline: release_render,
        })
    };
    abi::OK
}
