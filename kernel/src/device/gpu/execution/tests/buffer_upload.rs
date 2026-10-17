//! Context authority and response clearing for real owned buffers.
use super::*;
use crate::device::gpu::{
    GPU_ABI_VERSION, GPU_RESULT_INVALID_ABI, GPU_RESULT_INVALID_ARGUMENT, GPU_RESULT_INVALID_STATE,
    GPU_RESULT_SUCCESS, GpuContextUploadBuffer,
};
use crate::object::KernelObject;
use crate::sync::IrqSpinLock;

#[test_case]
fn buffer_upload_abi_has_fixed_48_byte_layout() {
    use core::mem::{offset_of, size_of};
    assert_eq!(size_of::<GpuContextUploadBuffer>(), 48);
    assert_eq!(offset_of!(GpuContextUploadBuffer, buffer_handle), 8);
    assert_eq!(offset_of!(GpuContextUploadBuffer, buffer_offset), 16);
    assert_eq!(offset_of!(GpuContextUploadBuffer, source_ptr), 24);
    assert_eq!(offset_of!(GpuContextUploadBuffer, size_bytes), 32);
    assert_eq!(offset_of!(GpuContextUploadBuffer, uploaded_bytes), 40);
}

#[test_case]
fn buffer_upload_context_rejects_invalid_requests_and_unattached_capability() {
    let drops = Arc::new(IrqSpinLock::new(0));
    let backend = Arc::new(TestBufferBackend {
        drops: drops.clone(),
    });
    let buffer = GpuBuffer::new(backend, 4096, 0).unwrap();
    let context = GpuContext::new(Arc::new(TestContext {
        drops,
        buffer_detaches: Arc::new(IrqSpinLock::new(0)),
    }));
    let task = crate::task::new_user_task("buffer-upload-authority".into(), 0);
    let mut request = GpuContextUploadBuffer::new(1, 0, 0x1000, 1);
    request.uploaded_bytes = u64::MAX;
    context.upload_buffer_for_task(&task, &buffer, &mut request);
    assert_eq!(request.result, GPU_RESULT_INVALID_STATE);
    assert_eq!(request.uploaded_bytes, 0);
    context.attach_buffer(&buffer).unwrap();
    request.reserved = 1;
    request.uploaded_bytes = u64::MAX;
    context.upload_buffer_for_task(&task, &buffer, &mut request);
    assert_eq!(request.result, GPU_RESULT_INVALID_ARGUMENT);
    assert_eq!(request.uploaded_bytes, 0);
    request.reserved = 0;
    request.buffer_offset = buffer.allocation_size() as u64;
    context.upload_buffer_for_task(&task, &buffer, &mut request);
    assert_eq!(request.result, GPU_RESULT_INVALID_ARGUMENT);
    request.abi_version = 0;
    context.upload_buffer_for_task(&task, &buffer, &mut request);
    assert_eq!(request.result, GPU_RESULT_INVALID_ABI);
    request.abi_version = GPU_ABI_VERSION;
    request.buffer_offset = 0;
    request.source_ptr = 0;
    context.upload_buffer_for_task(&task, &buffer, &mut request);
    assert_eq!(request.result, GPU_RESULT_INVALID_ARGUMENT);
    assert_ne!(request.result, GPU_RESULT_SUCCESS);
    assert_eq!(request.uploaded_bytes, 0);
}

struct UploadContext {
    calls: Arc<IrqSpinLock<usize>>,
    fail: bool,
}
impl GpuBackendContext for UploadContext {
    fn query_info(&self) -> GpuBackendContextInfo {
        GpuBackendContextInfo::new(0, 1)
    }
    fn create_queue(&self) -> Result<Arc<dyn GpuBackendQueue>, &'static str> {
        Err("no test queue")
    }
    fn attach_buffer(&self, _: &dyn GpuBackendBuffer) -> Result<u64, &'static str> {
        Ok(1)
    }
    fn upload_buffer(
        &self,
        _: &dyn GpuBackendBuffer,
        upload: crate::device::gpu::GpuBufferUploadInfo,
    ) -> Result<(), &'static str> {
        assert_eq!(upload.offset, 4095);
        assert_eq!(upload.size_bytes, 1);
        *self.calls.lock() += 1;
        if self.fail {
            Err("test transfer failed")
        } else {
            Ok(())
        }
    }
}

#[test_case]
fn buffer_upload_context_reports_only_complete_success_bytes() {
    for fail in [false, true] {
        let buffer = GpuBuffer::new(
            Arc::new(TestBufferBackend {
                drops: Arc::new(IrqSpinLock::new(0)),
            }),
            4096,
            0,
        )
        .unwrap();
        let calls = Arc::new(IrqSpinLock::new(0));
        let context = GpuContext::new(Arc::new(UploadContext {
            calls: calls.clone(),
            fail,
        }));
        context.attach_buffer(&buffer).unwrap();
        let task = crate::task::new_user_task("buffer-upload-response".into(), 0);
        let address = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        task.allocate_stack_pages(address, 1).unwrap();
        crate::library::std::usercopy::copy_to_user(&task, address, &[0x52]).unwrap();
        let mut request = GpuContextUploadBuffer::new(1, 4095, address as u64, 1);
        request.uploaded_bytes = u64::MAX;
        context.upload_buffer_for_task(&task, &buffer, &mut request);
        assert_eq!(
            request.result,
            if fail {
                GPU_RESULT_INVALID_STATE
            } else {
                GPU_RESULT_SUCCESS
            }
        );
        assert_eq!(request.uploaded_bytes, if fail { 0 } else { 1 });
        assert_eq!(*calls.lock(), 1);
        // Reject the entire out-of-bounds range before invoking the backend.
        request.size_bytes = 2;
        context.upload_buffer_for_task(&task, &buffer, &mut request);
        assert_eq!(request.result, GPU_RESULT_INVALID_ARGUMENT);
        assert_eq!(request.uploaded_bytes, 0);
        assert_eq!(*calls.lock(), 1);
    }
}

#[test_case]
fn buffer_upload_handle_zero_uses_real_capability_authority() {
    let buffer = Arc::new(
        GpuBuffer::new(
            Arc::new(TestBufferBackend {
                drops: Arc::new(IrqSpinLock::new(0)),
            }),
            4096,
            0,
        )
        .unwrap(),
    );
    let calls = Arc::new(IrqSpinLock::new(0));
    let context = GpuContext::new(Arc::new(UploadContext {
        calls: calls.clone(),
        fail: false,
    }));
    context.attach_buffer(buffer.as_ref()).unwrap();
    let mut task = crate::task::new_user_task("buffer-handle-zero".into(), 0);
    task.handle_table = crate::object::handle::HandleTable::new();
    let handle = task
        .handle_table
        .insert(KernelObject::Gpu(buffer.clone()))
        .unwrap();
    assert_eq!(handle, 0);
    let address = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
    task.allocate_stack_pages(address, 1).unwrap();
    crate::library::std::usercopy::copy_to_user(&task, address, &[0x52]).unwrap();
    let mut request = GpuContextUploadBuffer::new(handle, 4095, address as u64, 1);
    context.upload_buffer_request_for_task(&task, &mut request);
    assert_eq!(request.result, GPU_RESULT_SUCCESS);
    assert_eq!(request.uploaded_bytes, 1);
    assert_eq!(*calls.lock(), 1);
    // A removed capability is invalid, even while an attachment retains backing.
    drop(task.handle_table.remove(handle).unwrap());
    context.upload_buffer_request_for_task(&task, &mut request);
    assert_eq!(request.result, GPU_RESULT_INVALID_ARGUMENT);
    assert_eq!(request.uploaded_bytes, 0);
    assert_eq!(*calls.lock(), 1);
}
