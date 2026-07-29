//! Kernel-owned GPU execution context and queue capabilities.

use alloc::{sync::Arc, vec::Vec};

use super::connection::{read_user_value, write_user_value};
use super::{
    GPU_ABI_VERSION, GPU_CONTEXT_ATTACH_IMAGE, GPU_CONTEXT_QUERY, GPU_CONTEXT_UPLOAD_IMAGE_BGRA,
    GPU_CREATE_QUEUE, GPU_MAX_OPAQUE_COMMAND_SIZE, GPU_QUEUE_QUERY, GPU_QUEUE_SUBMIT,
    GPU_QUEUE_SUBMIT_FLAG_SIGNAL_TIMELINE, GPU_QUEUE_SUBMIT_FLAGS_VALID, GPU_RESULT_INVALID_ABI,
    GPU_RESULT_INVALID_ARGUMENT, GPU_RESULT_INVALID_STATE, GPU_RESULT_OUT_OF_RESOURCES,
    GpuBackendBuffer, GpuBackendContext, GpuBackendContextInfo, GpuBackendImage, GpuBackendQueue,
    GpuBackendQueueInfo, GpuBuffer, GpuContextAttachBuffer, GpuContextAttachImage, GpuContextInfo,
    GpuContextUploadImageBgra, GpuCreateQueue, GpuImage, GpuObject, GpuQueueInfo, GpuQueueSubmit,
};
use crate::library::std::usercopy::copy_from_user;
use crate::object::KernelObject;
use crate::object::capability::ControlOps;
use crate::object::handle::AccessMode;

/// Kernel-owned GPU execution context child capability.
///
/// The context owns the backend context lifetime. Queues retain a strong
/// reference to this backend context so closing a context handle cannot destroy
/// a real backend context while one of its child queues remains usable.
pub struct GpuContext {
    backend_context: Arc<dyn GpuBackendContext>,
    attached_images: Arc<crate::sync::IrqSpinLock<Vec<GpuAttachedImage>>>,
    attached_buffers: Arc<crate::sync::IrqSpinLock<Vec<GpuAttachedBuffer>>>,
}

/// Backend buffer and page backing retained by a context and its child queues.
struct GpuAttachedBuffer {
    _backend_buffer: Arc<dyn GpuBackendBuffer>,
    _backing: Arc<super::resource::GpuBufferBacking>,
}

/// Backend image and generic backing retained by a context and its queues.
struct GpuAttachedImage {
    backend_image: Arc<dyn GpuBackendImage>,
    _backing: Arc<super::resource::GpuImageBacking>,
}

impl GpuContext {
    /// Create a GPU execution context capability.
    ///
    /// # Arguments
    ///
    /// * `backend_context` - Real backend context retained by this capability.
    ///
    /// # Returns
    ///
    /// A context capability that owns the supplied backend context.
    pub fn new(backend_context: Arc<dyn GpuBackendContext>) -> Self {
        Self {
            backend_context,
            attached_images: Arc::new(crate::sync::IrqSpinLock::new(Vec::new())),
            attached_buffers: Arc::new(crate::sync::IrqSpinLock::new(Vec::new())),
        }
    }

    /// Query backend-neutral information for this context.
    ///
    /// # Returns
    ///
    /// Effective backend context information.
    pub fn query_info(&self) -> GpuBackendContextInfo {
        self.backend_context.query_info()
    }

    /// Create a child queue capability for this context.
    ///
    /// # Returns
    ///
    /// A queue retaining this context and its real backend queue, or an error
    /// if the backend cannot create a queue.
    pub fn create_queue(&self) -> Result<GpuQueue, &'static str> {
        let backend_queue = self.backend_context.create_queue()?;
        if bounded_command_limit(backend_queue.query_info()) == 0 {
            return Err("GPU backend queue has no usable command limit");
        }
        Ok(GpuQueue {
            _backend_context: Arc::clone(&self.backend_context),
            _attached_images: Arc::clone(&self.attached_images),
            _attached_buffers: Arc::clone(&self.attached_buffers),
            backend_queue,
        })
    }

    /// Attach an image to this context and retain it for all derived queues.
    ///
    /// # Arguments
    ///
    /// * `image` - Image capability authorized by the caller's handle table.
    ///
    /// # Returns
    ///
    /// The opaque command resource token authorized for this context.
    pub fn attach_image(&self, image: &GpuImage) -> Result<u64, &'static str> {
        let backend_image = image.backend_image();
        let mut attached_images = self.attached_images.lock();
        attached_images
            .try_reserve(1)
            .map_err(|_| "Failed to retain GPU image for context lifetime")?;
        let token = self.backend_context.attach_image(backend_image.as_ref())?;
        if token == 0 || token != backend_image.query_info().command_resource_token {
            let _ = self.backend_context.detach_image(backend_image.as_ref());
            return Err("GPU backend returned an invalid image attachment token");
        }
        attached_images.push(GpuAttachedImage {
            backend_image,
            _backing: image.backing(),
        });
        Ok(token)
    }

    /// Attach a buffer to this context and retain its backing for all derived queues.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Buffer capability authorized by the caller's handle table.
    ///
    /// # Returns
    ///
    /// The opaque command resource token authorized for this context.
    pub fn attach_buffer(&self, buffer: &GpuBuffer) -> Result<u64, &'static str> {
        let backend_buffer = buffer.backend_buffer();
        let backing = buffer.backing();
        let backend_info = backend_buffer.query_info();
        let mut attached_buffers = self.attached_buffers.lock();
        attached_buffers
            .try_reserve(1)
            .map_err(|_| "Failed to retain GPU buffer for context lifetime")?;
        let token = self
            .backend_context
            .attach_buffer(backend_buffer.as_ref())?;
        if token == 0 || token != backend_info.command_resource_token {
            let _ = self.backend_context.detach_buffer(backend_buffer.as_ref());
            return Err("GPU backend returned an invalid buffer attachment token");
        }
        attached_buffers.push(GpuAttachedBuffer {
            _backend_buffer: backend_buffer,
            _backing: backing,
        });
        Ok(token)
    }

    fn fill_query_info(&self, info: &mut GpuContextInfo) {
        let reserved = info.reserved;
        let reserved2 = info.reserved2;
        info.clear_response();
        if info.abi_version != GPU_ABI_VERSION {
            info.result = GPU_RESULT_INVALID_ABI;
            return;
        }
        if reserved != 0 || reserved2 != 0 {
            info.result = GPU_RESULT_INVALID_ARGUMENT;
            return;
        }
        let backend_info = self.query_info();
        info.effective_dialect_index = backend_info.dialect_index;
        info.effective_dialect_token = backend_info.dialect_token;
    }

    fn handle_query_info(&self, arg: usize) -> Result<i32, &'static str> {
        let mut info: GpuContextInfo = read_user_value(arg)?;
        self.fill_query_info(&mut info);
        write_user_value(arg, &info)?;
        Ok(0)
    }

    fn handle_create_queue(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuCreateQueue = read_user_value(arg)?;
        let reserved2 = request.reserved2;
        request.clear_response();
        if request.abi_version != GPU_ABI_VERSION {
            request.result = GPU_RESULT_INVALID_ABI;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        if request.flags != 0 || request.reserved != 0 || reserved2 != 0 {
            request.result = GPU_RESULT_INVALID_ARGUMENT;
            write_user_value(arg, &request)?;
            return Ok(0);
        }

        let queue = match self.create_queue() {
            Ok(queue) => queue,
            Err(_) => {
                request.result = GPU_RESULT_INVALID_STATE;
                write_user_value(arg, &request)?;
                return Ok(0);
            }
        };
        let max_opaque_command_size = queue.max_opaque_command_size();
        let task = crate::task::mytask().ok_or("No current task for GPU queue creation")?;
        let handle = task.handle_table.insert_with_metadata(
            KernelObject::Gpu(Arc::new(queue)),
            super::child_handle_metadata(AccessMode::ReadWrite),
        );
        match handle {
            Ok(handle) => {
                request.queue_handle = handle;
                request.max_opaque_command_size = max_opaque_command_size;
                if let Err(error) = write_user_value(arg, &request) {
                    task.handle_table.remove(handle);
                    return Err(error);
                }
                Ok(0)
            }
            Err(_) => {
                request.result = GPU_RESULT_OUT_OF_RESOURCES;
                write_user_value(arg, &request)?;
                Ok(0)
            }
        }
    }

    fn handle_attach_image(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuContextAttachImage = read_user_value(arg)?;
        let reserved = request.reserved;
        request.clear_response();
        if request.abi_version != GPU_ABI_VERSION {
            request.result = GPU_RESULT_INVALID_ABI;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        if request.flags != 0 || reserved != 0 || request.image_handle == 0 {
            request.result = GPU_RESULT_INVALID_ARGUMENT;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        let task = crate::task::mytask().ok_or("No current task for GPU image attachment")?;
        let image_owner = match task.handle_table.get_arc_clone(request.image_handle) {
            Some(object) if object.as_gpu().and_then(GpuObject::as_image).is_some() => object,
            _ => {
                request.result = GPU_RESULT_INVALID_ARGUMENT;
                write_user_value(arg, &request)?;
                return Ok(0);
            }
        };
        let image = image_owner
            .as_gpu()
            .and_then(GpuObject::as_image)
            .ok_or("GPU image handle changed while attaching")?;
        match self.attach_image(image) {
            Ok(token) => request.command_resource_token = token,
            Err(_) => request.result = GPU_RESULT_INVALID_STATE,
        }
        write_user_value(arg, &request)?;
        Ok(0)
    }

    fn handle_attach_buffer(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuContextAttachBuffer = read_user_value(arg)?;
        let reserved = request.reserved;
        request.clear_response();
        if request.abi_version != GPU_ABI_VERSION {
            request.result = GPU_RESULT_INVALID_ABI;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        if request.flags != 0 || reserved != 0 || request.buffer_handle == 0 {
            request.result = GPU_RESULT_INVALID_ARGUMENT;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        let task = crate::task::mytask().ok_or("No current task for GPU buffer attachment")?;
        let buffer_owner = match task.handle_table.get_arc_clone(request.buffer_handle) {
            Some(object) if object.as_gpu().and_then(GpuObject::as_buffer).is_some() => object,
            _ => {
                request.result = GPU_RESULT_INVALID_ARGUMENT;
                write_user_value(arg, &request)?;
                return Ok(0);
            }
        };
        let buffer = buffer_owner
            .as_gpu()
            .and_then(GpuObject::as_buffer)
            .ok_or("GPU buffer handle changed while attaching")?;
        match self.attach_buffer(buffer) {
            Ok(token) => request.command_resource_token = token,
            Err(_) => request.result = GPU_RESULT_INVALID_STATE,
        }
        write_user_value(arg, &request)?;
        Ok(0)
    }

    fn image_is_attached(&self, image: &GpuImage) -> bool {
        let backend_image = image.backend_image();
        self.attached_images
            .lock()
            .iter()
            .any(|attached| Arc::ptr_eq(&attached.backend_image, &backend_image))
    }

    fn handle_upload_image_bgra(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuContextUploadImageBgra = read_user_value(arg)?;
        let reserved = request.reserved;
        let reserved2 = request.reserved2;
        request.clear_response();
        if request.abi_version != GPU_ABI_VERSION {
            request.result = GPU_RESULT_INVALID_ABI;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        if reserved != 0 || reserved2 != 0 || request.image_handle == 0 {
            request.result = GPU_RESULT_INVALID_ARGUMENT;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        let task = crate::task::mytask().ok_or("No current task for GPU image upload")?;
        let image_owner = match task.handle_table.get_arc_clone(request.image_handle) {
            Some(object) if object.as_gpu().and_then(GpuObject::as_image).is_some() => object,
            _ => {
                request.result = GPU_RESULT_INVALID_ARGUMENT;
                write_user_value(arg, &request)?;
                return Ok(0);
            }
        };
        let image = image_owner
            .as_gpu()
            .and_then(GpuObject::as_image)
            .ok_or("GPU image handle changed while uploading")?;
        let layout = match super::resource::image_upload_layout(&request, image.query_info()) {
            Ok(layout) => layout,
            Err(_) => {
                request.result = GPU_RESULT_INVALID_ARGUMENT;
                write_user_value(arg, &request)?;
                return Ok(0);
            }
        };
        let source_ptr = match usize::try_from(request.source_ptr) {
            Ok(pointer) => pointer,
            Err(_) => {
                request.result = GPU_RESULT_INVALID_ARGUMENT;
                write_user_value(arg, &request)?;
                return Ok(0);
            }
        };
        if !self.image_is_attached(image) {
            request.result = GPU_RESULT_INVALID_STATE;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        if image
            .upload_bgra_from_user(source_ptr, layout, |backend_image, upload| {
                self.backend_context
                    .upload_image_bgra(backend_image, upload)
            })
            .is_err()
        {
            request.result = GPU_RESULT_INVALID_STATE;
        }
        write_user_value(arg, &request)?;
        Ok(0)
    }
}

impl ControlOps for GpuContext {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            GPU_CONTEXT_QUERY => self.handle_query_info(arg),
            GPU_CREATE_QUEUE => self.handle_create_queue(arg),
            GPU_CONTEXT_ATTACH_IMAGE => self.handle_attach_image(arg),
            super::GPU_CONTEXT_ATTACH_BUFFER => self.handle_attach_buffer(arg),
            GPU_CONTEXT_UPLOAD_IMAGE_BGRA => self.handle_upload_image_bgra(arg),
            _ => Err("Unsupported GPU context control command"),
        }
    }
}

impl GpuObject for GpuContext {
    fn as_control_ops(&self) -> Option<&dyn ControlOps> {
        Some(self)
    }

    fn as_context(&self) -> Option<&GpuContext> {
        Some(self)
    }
}

/// Kernel-owned GPU execution queue child capability.
pub struct GpuQueue {
    _backend_context: Arc<dyn GpuBackendContext>,
    _attached_images: Arc<crate::sync::IrqSpinLock<Vec<GpuAttachedImage>>>,
    _attached_buffers: Arc<crate::sync::IrqSpinLock<Vec<GpuAttachedBuffer>>>,
    backend_queue: Arc<dyn GpuBackendQueue>,
}

impl GpuQueue {
    /// Query the bounded opaque command limit for this queue.
    ///
    /// # Returns
    ///
    /// The command limit enforced by the generic GPU ABI.
    pub fn max_opaque_command_size(&self) -> u32 {
        bounded_command_limit(self.backend_queue.query_info())
    }

    fn fill_query_info(&self, info: &mut GpuQueueInfo) {
        let reserved = info.reserved;
        let reserved2 = info.reserved2;
        info.clear_response();
        if info.abi_version != GPU_ABI_VERSION {
            info.result = GPU_RESULT_INVALID_ABI;
            return;
        }
        if reserved != 0 || reserved2 != 0 {
            info.result = GPU_RESULT_INVALID_ARGUMENT;
            return;
        }
        info.max_opaque_command_size = self.max_opaque_command_size();
    }

    fn handle_query_info(&self, arg: usize) -> Result<i32, &'static str> {
        let mut info: GpuQueueInfo = read_user_value(arg)?;
        self.fill_query_info(&mut info);
        write_user_value(arg, &info)?;
        Ok(0)
    }

    fn handle_submit(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuQueueSubmit = read_user_value(arg)?;
        let reserved = request.reserved;
        let reserved2 = request.reserved2;
        request.clear_response();
        if request.abi_version != GPU_ABI_VERSION {
            request.result = GPU_RESULT_INVALID_ABI;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        if reserved != 0
            || reserved2 != 0
            || request.flags & !GPU_QUEUE_SUBMIT_FLAGS_VALID != 0
            || !command_size_is_valid(request.command_size, self.max_opaque_command_size())
        {
            request.result = GPU_RESULT_INVALID_ARGUMENT;
            write_user_value(arg, &request)?;
            return Ok(0);
        }

        let signals_timeline = request.flags & GPU_QUEUE_SUBMIT_FLAG_SIGNAL_TIMELINE != 0;
        if !signals_timeline && (request.signal_timeline_handle != 0 || request.signal_value != 0) {
            request.result = GPU_RESULT_INVALID_ARGUMENT;
            write_user_value(arg, &request)?;
            return Ok(0);
        }

        let commands = match copy_command_bytes(request.command_ptr, request.command_size) {
            Ok(commands) => commands,
            Err("GPU command allocation failed") => {
                request.result = GPU_RESULT_OUT_OF_RESOURCES;
                write_user_value(arg, &request)?;
                return Ok(0);
            }
            Err(error) => return Err(error),
        };

        let task = crate::task::mytask().ok_or("No current task for GPU queue submission")?;
        let timeline_owner = if signals_timeline {
            match task
                .handle_table
                .get_arc_clone(request.signal_timeline_handle)
            {
                Some(object) if object.as_gpu().and_then(GpuObject::as_timeline).is_some() => {
                    Some(object)
                }
                _ => {
                    request.result = GPU_RESULT_INVALID_ARGUMENT;
                    write_user_value(arg, &request)?;
                    return Ok(0);
                }
            }
        } else {
            None
        };
        let timeline = timeline_owner
            .as_ref()
            .and_then(KernelObject::as_gpu)
            .and_then(GpuObject::as_timeline);

        if self.backend_queue.submit(&commands).is_err() {
            if let Some(timeline) = timeline {
                timeline.fail();
                let (value, failed) = timeline.state();
                request.completed_value = value;
                request.timeline_failed = u32::from(failed);
            }
            request.result = GPU_RESULT_INVALID_STATE;
            write_user_value(arg, &request)?;
            return Ok(0);
        }

        if let Some(timeline) = timeline {
            if timeline.signal(request.signal_value).is_err() {
                request.result = GPU_RESULT_INVALID_STATE;
            }
            let (value, failed) = timeline.state();
            request.completed_value = value;
            request.timeline_failed = u32::from(failed);
        }
        write_user_value(arg, &request)?;
        Ok(0)
    }
}

impl ControlOps for GpuQueue {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            GPU_QUEUE_QUERY => self.handle_query_info(arg),
            GPU_QUEUE_SUBMIT => self.handle_submit(arg),
            _ => Err("Unsupported GPU queue control command"),
        }
    }
}

impl GpuObject for GpuQueue {
    fn as_control_ops(&self) -> Option<&dyn ControlOps> {
        Some(self)
    }
}

/// Return a generic queue limit clamped to the ABI maximum.
///
/// # Arguments
///
/// * `info` - Backend-provided queue limits.
///
/// # Returns
///
/// The usable command limit, never greater than the generic ABI maximum.
pub(crate) const fn bounded_command_limit(info: GpuBackendQueueInfo) -> u32 {
    if info.max_opaque_command_size > GPU_MAX_OPAQUE_COMMAND_SIZE {
        GPU_MAX_OPAQUE_COMMAND_SIZE
    } else {
        info.max_opaque_command_size
    }
}

/// Validate one opaque command stream length against a queue limit.
///
/// # Arguments
///
/// * `command_size` - Requested opaque command byte length.
/// * `queue_limit` - Queue-specific bounded command limit.
///
/// # Returns
///
/// `true` when the command stream is non-empty and fits both limits.
pub(crate) const fn command_size_is_valid(command_size: u32, queue_limit: u32) -> bool {
    command_size != 0 && command_size <= GPU_MAX_OPAQUE_COMMAND_SIZE && command_size <= queue_limit
}

fn copy_command_bytes(command_ptr: u64, command_size: u32) -> Result<Vec<u8>, &'static str> {
    let command_ptr = usize::try_from(command_ptr)
        .map_err(|_| "GPU command pointer does not fit the kernel address size")?;
    if command_ptr == 0 {
        return Err("GPU command pointer is null");
    }
    let command_size = usize::try_from(command_size)
        .map_err(|_| "GPU command size does not fit the kernel address size")?;
    let mut commands = Vec::new();
    commands
        .try_reserve_exact(command_size)
        .map_err(|_| "GPU command allocation failed")?;
    commands.resize(command_size, 0);
    let task = crate::task::mytask().ok_or("No current task for GPU queue submission")?;
    copy_from_user(&task, command_ptr, &mut commands)
        .map_err(|_| "Failed to copy GPU commands from user")?;
    Ok(commands)
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use super::{GpuContext, bounded_command_limit, command_size_is_valid};
    use crate::device::gpu::{
        GPU_EXECUTION_SUPPORT_NONE, GPU_MAX_OPAQUE_COMMAND_SIZE, GpuBackend, GpuBackendContext,
        GpuBackendContextInfo, GpuBackendDialectDescriptor, GpuBackendImage, GpuBackendImageInfo,
        GpuBackendInfo, GpuBackendQueue, GpuBackendQueueInfo, GpuDeviceInfo, GpuDeviceState,
        GpuImage, GpuImageCreateInfo, GpuObject,
    };
    use crate::device::graphics::GpuDisplayResource;
    use crate::sync::IrqSpinLock;

    struct InfoOnlyBackend;

    impl GpuBackend for InfoOnlyBackend {
        fn query_info(&self) -> GpuBackendInfo {
            GpuBackendInfo::new(
                GpuDeviceInfo::new(GpuDeviceState::Ready, GPU_EXECUTION_SUPPORT_NONE, 0),
                0,
                b"test",
                &[],
            )
        }
    }

    struct TestContext {
        drops: Arc<IrqSpinLock<u32>>,
    }

    impl Drop for TestContext {
        fn drop(&mut self) {
            *self.drops.lock() += 1;
        }
    }

    impl GpuBackendContext for TestContext {
        fn query_info(&self) -> GpuBackendContextInfo {
            GpuBackendContextInfo::new(7, 0x1234)
        }

        fn create_queue(&self) -> Result<Arc<dyn GpuBackendQueue>, &'static str> {
            Ok(Arc::new(TestQueue))
        }

        fn attach_image(&self, image: &dyn GpuBackendImage) -> Result<u64, &'static str> {
            Ok(image.query_info().command_resource_token)
        }
    }

    struct TestQueue;

    struct TestImage {
        drops: Arc<IrqSpinLock<u32>>,
        create: GpuImageCreateInfo,
        allocation_size: u64,
    }

    struct TestImageBackend {
        drops: Arc<IrqSpinLock<u32>>,
    }

    impl GpuBackend for TestImageBackend {
        fn query_info(&self) -> GpuBackendInfo {
            GpuBackendInfo::new(
                GpuDeviceInfo::new(GpuDeviceState::Ready, GPU_EXECUTION_SUPPORT_NONE, 0),
                0,
                b"test-image",
                &[],
            )
        }

        fn create_image(
            &self,
            create: GpuImageCreateInfo,
            backing: crate::device::gpu::GpuImageBackingInfo,
        ) -> Result<Arc<dyn GpuBackendImage>, &'static str> {
            Ok(Arc::new(TestImage {
                drops: Arc::clone(&self.drops),
                create,
                allocation_size: backing.allocation_size,
            }))
        }
    }

    impl Drop for TestImage {
        fn drop(&mut self) {
            *self.drops.lock() += 1;
        }
    }

    impl GpuBackendImage for TestImage {
        fn query_info(&self) -> GpuBackendImageInfo {
            GpuBackendImageInfo::new(self.create, 9, self.allocation_size)
        }

        fn backend_cookie(&self) -> u64 {
            1
        }

        fn display_resource(&self) -> Option<GpuDisplayResource> {
            Some(GpuDisplayResource::new(9, 8, 8, 1))
        }
    }

    impl GpuBackendQueue for TestQueue {
        fn query_info(&self) -> GpuBackendQueueInfo {
            GpuBackendQueueInfo::new(GPU_MAX_OPAQUE_COMMAND_SIZE)
        }

        fn submit(&self, commands: &[u8]) -> Result<(), &'static str> {
            if commands.is_empty() {
                return Err("empty test commands");
            }
            Ok(())
        }
    }

    #[test_case]
    fn execution_backend_defaults_are_unsupported() {
        let backend = InfoOnlyBackend;
        assert!(backend.query_dialect(0).is_err());
        assert!(
            backend
                .create_context(GpuBackendDialectDescriptor::new(0, 0))
                .is_err()
        );
    }

    #[test_case]
    fn queue_command_sizes_are_bounded_and_non_empty() {
        assert!(command_size_is_valid(1, GPU_MAX_OPAQUE_COMMAND_SIZE));
        assert!(command_size_is_valid(
            GPU_MAX_OPAQUE_COMMAND_SIZE,
            GPU_MAX_OPAQUE_COMMAND_SIZE
        ));
        assert!(!command_size_is_valid(0, GPU_MAX_OPAQUE_COMMAND_SIZE));
        assert!(!command_size_is_valid(
            GPU_MAX_OPAQUE_COMMAND_SIZE + 1,
            GPU_MAX_OPAQUE_COMMAND_SIZE + 1
        ));
        assert!(!command_size_is_valid(8, 4));
        assert_eq!(
            bounded_command_limit(GpuBackendQueueInfo::new(GPU_MAX_OPAQUE_COMMAND_SIZE + 1)),
            GPU_MAX_OPAQUE_COMMAND_SIZE
        );
    }

    #[test_case]
    fn queue_keeps_backend_context_alive_until_queue_drop() {
        let drops = Arc::new(IrqSpinLock::new(0));
        let backend_context: Arc<dyn GpuBackendContext> = Arc::new(TestContext {
            drops: Arc::clone(&drops),
        });
        let context = GpuContext::new(backend_context);
        let queue = context
            .create_queue()
            .expect("test context should create a queue");

        assert!(GpuObject::as_context(&context).is_some());
        assert!(GpuObject::as_control_ops(&queue).is_some());
        drop(context);
        assert_eq!(*drops.lock(), 0);
        drop(queue);
        assert_eq!(*drops.lock(), 1);
    }

    #[test_case]
    fn attached_image_survives_context_until_derived_queue_drops() {
        let drops = Arc::new(IrqSpinLock::new(0));
        let backend: Arc<dyn GpuBackend> = Arc::new(TestImageBackend {
            drops: Arc::clone(&drops),
        });
        let image = GpuImage::new(backend, GpuImageCreateInfo::new(1, 3, 8, 8))
            .expect("test image metadata should be valid");
        let backend_context: Arc<dyn GpuBackendContext> = Arc::new(TestContext {
            drops: Arc::new(IrqSpinLock::new(0)),
        });
        let context = GpuContext::new(backend_context);
        let queue = context
            .create_queue()
            .expect("test context should create a queue");
        assert_eq!(context.attach_image(&image), Ok(9));

        drop(image);
        drop(context);
        assert_eq!(*drops.lock(), 0);
        drop(queue);
        assert_eq!(*drops.lock(), 1);
    }
}
