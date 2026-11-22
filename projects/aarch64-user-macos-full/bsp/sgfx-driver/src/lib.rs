//! Scarlet dynamic SGFX driver. The existing GPU handles own sessions and
//! shareable images. Only bounded data and attachment tokens cross the broker.
mod driver;
use gpu_raw::{Gpu, GpuContext, GpuImage, GpuQueue};
use scarlet_hosted_sgfx_protocol as wire;
use scarlet_os::handle::Handle;
use sgfx_backend_abi::{self as abi, Object, Span};
use sgfx_core::ir::ResourceTable;
use std::{collections::BTreeMap, ptr, rc::Rc, sync::Arc};

type Status<T> = Result<T, i32>;
fn status(r: Status<()>) -> i32 {
    r.err().unwrap_or(abi::OK)
}
struct Context {
    gpu: Rc<Gpu>,
    ctx: Rc<GpuContext>,
}
struct Session {
    ctx: Rc<GpuContext>,
    queue: GpuQueue,
    limit: usize,
    images: BTreeMap<u32, GpuImage>,
}
impl Session {
    fn new(ctx: &Rc<GpuContext>, metadata: &[u64]) -> Status<Self> {
        let queue = ctx.create_queue().map_err(|_| abi::INITIALIZATION_FAILED)?;
        let limit = queue
            .query()
            .map_err(|_| abi::INVALID)?
            .max_opaque_command_size as usize
            / 8;
        if limit < 3 {
            return Err(abi::UNSUPPORTED);
        }
        let session = Self {
            ctx: ctx.clone(),
            queue,
            limit,
            images: BTreeMap::new(),
        };
        snapshot(&session, metadata)?;
        Ok(session)
    }
    fn send(&self, words: &[u64]) -> Status<()> {
        let submit = |w: &[u64]| {
            self.queue
                .submit(wire::as_bytes(w))
                .map(|_| ())
                .map_err(|_| abi::INVALID)
        };
        if words.len() <= self.limit {
            return submit(words);
        }
        if words.len() > wire::MAX_MESSAGE / 8 {
            return Err(abi::OUT_OF_MEMORY);
        }
        submit(&[wire::START, words.len() as u64])?;
        let mut packet = Vec::with_capacity(self.limit);
        for chunk in words.chunks(self.limit - 1) {
            packet.clear();
            packet.push(wire::APPEND);
            packet.extend_from_slice(chunk);
            submit(&packet)?;
        }
        submit(&[wire::FINISH])
    }
    fn map(&mut self, slot: u32, image: GpuImage) -> Status<()> {
        let token = self.ctx.attach_image(&image).map_err(|_| abi::INVALID)?;
        if let Err(e) = self.send(&[wire::MAP, u64::from(slot), token]) {
            let _ = self.ctx.detach_image(&image);
            return Err(e);
        }
        if let Some(previous) = self.images.insert(slot, image) {
            self.ctx
                .detach_image(&previous)
                .map_err(|_| abi::DEVICE_LOST)?;
        }
        Ok(())
    }
    fn forget_image(&mut self, slot: u32) -> Status<()> {
        self.send(&[wire::UNMAP, slot.into()])?;
        let image = self.images.remove(&slot).ok_or(abi::INVALID)?;
        self.ctx.detach_image(&image).map_err(|_| abi::INVALID)?;
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        for (_, image) in std::mem::take(&mut self.images) {
            let _ = self.ctx.detach_image(&image);
        }
    }
}

// The SGFX loader validates the function table and serializes calls to owned
// objects. All these pointers are local to the guest dynamic library.
unsafe fn borrow<'a, T>(p: Object) -> Status<&'a mut T> {
    unsafe { p.cast::<T>().as_mut() }.ok_or(abi::INVALID)
}
unsafe extern "C" fn destroy<T>(p: Object) {
    if !p.is_null() {
        drop(unsafe { Box::from_raw(p.cast::<T>()) });
    }
}
fn output<T>(out: *mut Object, build: impl FnOnce() -> Status<T>) -> i32 {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return abi::INVALID;
    };
    *out = ptr::null_mut();
    status(build().map(|object| *out = Box::into_raw(Box::new(object)).cast()))
}
unsafe extern "C" fn open(path: Span<u8>, out: *mut Object, flags: *mut u64) -> i32 {
    let Some(flags) = (unsafe { flags.as_mut() }) else {
        return abi::INVALID;
    };
    *flags = 0;
    output(out, || {
        let path = std::str::from_utf8(unsafe { path.as_slice() }).map_err(|_| abi::INVALID)?;
        let gpu = Gpu::open(path).map_err(|_| abi::INITIALIZATION_FAILED)?;
        let info = gpu.query_info().map_err(|_| abi::INITIALIZATION_FAILED)?;
        if info.backend_id_bytes() != wire::BACKEND {
            return Err(abi::UNSUPPORTED);
        }
        *flags = abi::RENDERING
            | abi::IMAGE_UPLOAD
            | abi::IMAGE_READBACK
            | abi::DEPTH
            | abi::PROGRAMMABLE_GRAPHICS
            | abi::RGBA8_COLOR_ATTACHMENT;
        if info.execution_support & gpu_raw::GPU_EXECUTION_SUPPORT_PRESENTATION != 0 {
            *flags |= abi::PRESENTATION;
        }
        Ok(Rc::new(gpu))
    })
}
unsafe extern "C" fn create_context(p: Object, out: *mut Object) -> i32 {
    output(out, || {
        let gpu = unsafe { borrow::<Rc<Gpu>>(p) }?.clone();
        let dialect = gpu.query_dialect(0).map_err(|_| abi::UNSUPPORTED)?;
        let ctx = Rc::new(
            gpu.create_context(&dialect)
                .map_err(|_| abi::INITIALIZATION_FAILED)?,
        );
        Ok(Context { gpu, ctx })
    })
}
fn snapshot(s: &Session, words: &[u64]) -> Status<()> {
    if words.len() >= wire::MAX_MESSAGE / 8 {
        return Err(abi::OUT_OF_MEMORY);
    }
    let mut msg = Vec::with_capacity(1 + words.len());
    msg.push(wire::SNAPSHOT);
    msg.extend_from_slice(words);
    s.send(&msg)
}
fn shared_image(gpu: &Gpu, width: u32, height: u32) -> Status<GpuImage> {
    gpu.create_image_with_usage(
        width,
        height,
        gpu_raw::GPU_IMAGE_USAGE_PRESENTABLE
            | gpu_raw::GPU_IMAGE_USAGE_RENDER_TARGET
            | gpu_raw::GPU_IMAGE_USAGE_TRANSFER_SRC
            | gpu_raw::GPU_IMAGE_USAGE_TRANSFER_DST
            | gpu_raw::GPU_IMAGE_USAGE_SAMPLED,
    )
    .map_err(|_| abi::OUT_OF_MEMORY)
}
unsafe extern "C" fn create_session(
    p: Object,
    metadata: Span<u64>,
    targets: Span<u32>,
    out: *mut Object,
    flags: *mut u64,
) -> i32 {
    let Some(flags) = (unsafe { flags.as_mut() }) else {
        return abi::INVALID;
    };
    *flags = 0;
    output(out, || {
        let Context { gpu, ctx } = unsafe { borrow::<Context>(p) }?;
        let metadata = unsafe { metadata.as_slice() };
        let table = ResourceTable::new();
        table
            .sync_abi_snapshot(metadata)
            .map_err(|_| abi::INVALID)?;
        let mut s = Session::new(ctx, metadata)?;
        for &slot in unsafe { targets.as_slice() } {
            let texture = table
                .abi_texture(slot)
                .and_then(|r| table.texture(r))
                .map_err(|_| abi::INVALID)?;
            let extent = texture.extent();
            let image = shared_image(gpu, extent.width(), extent.height())?;
            s.map(slot, image)?;
        }
        if s.queue
            .query_async()
            .is_ok_and(|info| info.max_pending_submissions > 0)
        {
            *flags |= abi::ASYNC;
        }
        Ok(s)
    })
}
unsafe extern "C" fn sync_resources(p: Object, metadata: Span<u64>) -> i32 {
    status((|| {
        snapshot(unsafe { borrow::<Session>(p) }?, unsafe {
            metadata.as_slice()
        })
    })())
}
unsafe extern "C" fn image(p: Object, slot: u32, out: *mut abi::ImageInfo) -> i32 {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return abi::INVALID;
    };
    *out = abi::ImageInfo {
        handle: -1,
        ..Default::default()
    };
    status((|| {
        let image = unsafe { borrow::<Session>(p) }?
            .images
            .get(&slot)
            .ok_or(abi::INVALID)?;
        let info = image.query().map_err(|_| abi::INVALID)?;
        let handle = image
            .as_handle()
            .duplicate()
            .map_err(|_| abi::OUT_OF_MEMORY)?;
        *out = abi::ImageInfo {
            width: info.width,
            height: info.height,
            handle: handle.as_raw(),
            reserved: 0,
        };
        std::mem::forget(handle);
        Ok(())
    })())
}
unsafe extern "C" fn readback(
    p: Object,
    slot: u32,
    out: *mut u8,
    len: usize,
    stride: u32,
    r: abi::Rect,
) -> i32 {
    if out.is_null() || len > isize::MAX as usize {
        return abi::INVALID;
    }
    status((|| {
        let s = unsafe { borrow::<Session>(p) }?;
        let image = s.images.get(&slot).ok_or(abi::INVALID)?;
        s.ctx
            .readback_image_bgra(
                image,
                unsafe { std::slice::from_raw_parts_mut(out, len) },
                stride,
                gpu_raw::GpuImageBgraRect::new(r.x, r.y, r.width, r.height),
            )
            .map_err(|_| abi::INVALID)
    })())
}
unsafe extern "C" fn import_bgra(p: Object, slot: u32, raw: i32) -> i32 {
    status((|| {
        let handle = unsafe { Handle::from_raw(raw) }.map_err(|_| abi::INVALID)?;
        let image = GpuImage::from_handle(handle).map_err(|_| abi::INVALID)?;
        unsafe { borrow::<Session>(p) }?.map(slot, image)
    })())
}
unsafe extern "C" fn release_import(p: Object, slot: u32) -> i32 {
    status((|| unsafe { borrow::<Session>(p) }?.forget_image(slot))())
}
// Snapshot every guest upload before handing the recording to a kernel queue.
unsafe fn recording(batch: *const abi::Batch) -> Status<Vec<u64>> {
    let b = unsafe { batch.as_ref() }.ok_or(abi::INVALID)?;
    if b.words.len > wire::MAX_MESSAGE / 8 {
        return Err(abi::INVALID);
    }
    unsafe { wire::pack_recording(b.table, b.count, b.words.as_slice()) }.map_err(|_| abi::INVALID)
}
unsafe extern "C" fn execute(p: Object, batch: *const abi::Batch) -> i32 {
    status((|| {
        let session = unsafe { borrow::<Session>(p) }?;
        session.send(&unsafe { recording(batch) }?)
    })())
}
// Failed transport may leave acceptance unknown. Such a receipt can report
// failure but can never certify that it is safe to reuse the source.
enum Receipt {
    Tracked(gpu_raw::GpuCompletion),
    Complete,
    Lost,
}
unsafe extern "C" fn drop_receipt(p: Object) {
    if !p.is_null() {
        unsafe { Arc::decrement_strong_count(p.cast::<Receipt>()) };
    }
}

unsafe extern "C" fn submit(p: Object, batch: *const abi::Batch, out: *mut abi::SubmitResult) {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return;
    };
    let submit = || {
        let session = unsafe { borrow::<Session>(p) }?;
        let words = unsafe { recording(batch) }?;
        if words.len() > session.limit {
            // Fragment uploads synchronously. FINISH executes the complete recording
            // and the host fences Metal before acknowledging it.
            let result = session.send(&words);
            let (disposition, error, receipt) = match result {
                Ok(()) => (abi::ACCEPTED, abi::OK, Receipt::Complete),
                Err(_) => (abi::PARTIAL, abi::DEVICE_LOST, Receipt::Lost),
            };
            return Ok(abi::SubmitResult {
                disposition,
                error,
                receipt: Arc::into_raw(Arc::new(receipt)).cast_mut().cast(),
            });
        }
        let (disposition, error, completion) =
            match session.queue.submit_async(wire::as_bytes(&words)) {
                Ok(receipt) => (abi::ACCEPTED, abi::OK, Some(receipt)),
                Err(gpu_raw::GpuSubmitError::Busy) => return Err(abi::BUSY),
                Err(gpu_raw::GpuSubmitError::Failed { completion, .. }) => {
                    (abi::PARTIAL, abi::DEVICE_LOST, completion)
                }
                Err(_) => return Err(abi::INVALID),
            };
        Ok(abi::SubmitResult {
            disposition,
            error,
            receipt: Arc::into_raw(Arc::new(completion.map_or(Receipt::Lost, Receipt::Tracked)))
                .cast_mut()
                .cast(),
        })
    };
    *out = submit().unwrap_or_else(|error| abi::SubmitResult {
        error,
        ..Default::default()
    });
}
unsafe extern "C" fn wait(p: Object, timeout: u64, out: *mut u32) -> i32 {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return abi::INVALID;
    };
    *out = abi::PENDING;
    status((|| {
        let receipt = unsafe { p.cast::<Receipt>().as_ref() }.ok_or(abi::INVALID)?;
        let completion = match receipt {
            Receipt::Tracked(c) => c,
            Receipt::Lost => return Err(abi::DEVICE_LOST),
            Receipt::Complete => {
                *out = abi::COMPLETE;
                return Ok(());
            }
        };
        if timeout != 0 {
            use scarlet_os::poll::{POLLIN, PollHandle, poll};
            let mut watch = [PollHandle::new(
                completion.as_handle().as_raw() as u32,
                POLLIN,
            )];
            let ns = if timeout == u64::MAX {
                -1
            } else {
                timeout.min(i64::MAX as u64) as i64
            };
            poll(&mut watch, ns).map_err(|_| abi::INVALID)?;
        }
        match completion.query().map_err(|_| abi::INVALID)?.state {
            gpu_raw::GPU_COMPLETION_COMPLETE => {
                *out = abi::COMPLETE;
                Ok(())
            }
            gpu_raw::GPU_COMPLETION_PENDING => Ok(()),
            _ => Err(abi::DEVICE_LOST),
        }
    })())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sgfx_backend_get_api_v2(
    version: u32,
    size: usize,
    host: *const abi::HostInfo,
    out: *mut abi::BackendApi,
) -> i32 {
    let Some(host) = (unsafe { host.as_ref() }) else {
        return abi::INVALID;
    };
    if version != abi::VERSION
        || size < size_of::<abi::BackendApi>()
        || host.size as usize != size_of::<abi::HostInfo>()
        || host.reserved != 0
    {
        return abi::ABI_MISMATCH;
    }
    if out.is_null() {
        return abi::INVALID;
    }
    #[cfg(target_arch = "aarch64")]
    if host.cpu_features & abi::CPU_AARCH64_LSE != 0 {
        unsafe extern "C" {
            fn __rust_enable_lse();
        }
        unsafe { __rust_enable_lse() };
    }
    let mut name = [0; 64];
    name[..wire::BACKEND.len()].copy_from_slice(wire::BACKEND);
    let mut gpu_backend = [0; 32];
    gpu_backend[..wire::BACKEND.len()].copy_from_slice(wire::BACKEND);
    unsafe {
        out.write(abi::BackendApi {
            version,
            size: size_of::<abi::BackendApi>() as u32,
            name,
            gpu_backend,
            open,
            drop_device: destroy::<Rc<Gpu>>,
            create_context,
            drop_context: destroy::<Context>,
            create_session,
            drop_session: destroy::<Session>,
            sync_resources,
            image,
            readback,
            import_bgra,
            release_import,
            execute,
            submit,
            wait,
            drop_receipt,
        })
    };
    abi::OK
}
