//! Async queue admission, attachment snapshots, and response publication.

use alloc::{sync::Arc, vec::Vec};

use super::GpuQueue;
use crate::device::gpu::connection::{read_user_value, write_user_value};
use crate::device::gpu::submission::{
    MAX_PENDING_SUBMISSIONS, SubmissionPermit, SubmissionResources,
};
use crate::device::gpu::{
    GPU_ABI_VERSION, GPU_MAX_OPAQUE_COMMAND_SIZE, GPU_RESULT_BUSY, GPU_RESULT_DEVICE_LOST,
    GPU_RESULT_INVALID_ABI, GPU_RESULT_INVALID_ARGUMENT, GPU_RESULT_INVALID_STATE,
    GPU_RESULT_OUT_OF_RESOURCES, GPU_RESULT_SUCCESS, GPU_RESULT_UNSUPPORTED,
    GpuBackendEnqueueError, GpuBackendSubmitError, GpuCompletion, GpuCompletionSignal,
    GpuQueueAsyncInfo, GpuQueueSubmitAsync, GpuSubmission,
};
use crate::object::KernelObject;
use crate::object::handle::{AccessMode, HandleTable};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnqueueOutcome {
    Rejected(u32),
    Accepted(u32),
}

fn backend_result(error: GpuBackendSubmitError) -> u32 {
    match error {
        GpuBackendSubmitError::Rejected(_) => GPU_RESULT_INVALID_ARGUMENT,
        GpuBackendSubmitError::Unavailable(_) => GPU_RESULT_INVALID_STATE,
        GpuBackendSubmitError::DeviceLost(_) => GPU_RESULT_DEVICE_LOST,
    }
}

fn snapshot<T: Clone>(attached: &[T]) -> Result<Vec<T>, ()> {
    let mut owned = Vec::new();
    owned.try_reserve_exact(attached.len()).map_err(|_| ())?;
    owned.extend_from_slice(attached);
    Ok(owned)
}

impl GpuQueue {
    fn async_capacity(&self) -> u32 {
        self.backend_queue
            .async_capacity()
            .min(MAX_PENDING_SUBMISSIONS)
    }

    pub(super) fn fill_async_info(&self, info: &mut GpuQueueAsyncInfo) {
        let abi_version = info.abi_version;
        let invalid_reserved = info.reserved != 0 || info.reserved2 != 0;
        *info = GpuQueueAsyncInfo::new();
        info.abi_version = abi_version;
        if abi_version != GPU_ABI_VERSION {
            info.result = GPU_RESULT_INVALID_ABI;
        } else if invalid_reserved {
            info.result = GPU_RESULT_INVALID_ARGUMENT;
        } else {
            info.max_pending_submissions = self.async_capacity();
            if info.max_pending_submissions != 0 {
                info.max_opaque_command_size = self.async_command_limit;
            }
        }
    }

    pub(super) fn handle_query_async(&self, arg: usize) -> Result<i32, &'static str> {
        let mut info: GpuQueueAsyncInfo = read_user_value(arg)?;
        self.fill_async_info(&mut info);
        write_user_value(arg, &info)?;
        Ok(0)
    }

    pub(super) fn handle_submit_async(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuQueueSubmitAsync = read_user_value(arg)?;
        let task = crate::task::mytask().ok_or("No current task for asynchronous GPU submit")?;
        self.submit_async_request(
            request,
            &task.handle_table,
            super::copy_command_bytes,
            |reply| write_user_value(arg, reply),
        )
    }

    pub(super) fn submit_async_request(
        &self,
        mut request: GpuQueueSubmitAsync,
        handles: &HandleTable,
        copy_commands: impl FnOnce(u64, u32) -> Result<Vec<u8>, &'static str>,
        publish: impl FnOnce(&GpuQueueSubmitAsync) -> Result<(), &'static str>,
    ) -> Result<i32, &'static str> {
        let invalid_flags = request.flags != 0 || request.reserved != 0 || request.reserved2 != 0;
        request.result = GPU_RESULT_SUCCESS;
        request.accepted = 0;
        request.completion_handle = 0;
        request.reserved = 0;
        request.reserved2 = 0;
        let rejection = if request.abi_version != GPU_ABI_VERSION {
            Some(GPU_RESULT_INVALID_ABI)
        } else if invalid_flags || request.command_size > GPU_MAX_OPAQUE_COMMAND_SIZE {
            Some(GPU_RESULT_INVALID_ARGUMENT)
        } else if self.async_capacity() == 0 {
            Some(GPU_RESULT_UNSUPPORTED)
        } else if request.command_size > self.async_command_limit {
            Some(GPU_RESULT_INVALID_ARGUMENT)
        } else {
            None
        };
        if let Some(result) = rejection {
            request.result = result;
            publish(&request)?;
            return Ok(0);
        }
        let Some(permit) = self.submission_slots.reserve(self.async_capacity()) else {
            request.result = GPU_RESULT_BUSY;
            publish(&request)?;
            return Ok(0);
        };
        let commands = if request.command_size == 0 {
            Vec::new()
        } else {
            match copy_commands(request.command_ptr, request.command_size) {
                Ok(commands) => commands,
                Err("GPU command allocation failed") => {
                    request.result = GPU_RESULT_OUT_OF_RESOURCES;
                    publish(&request)?;
                    return Ok(0);
                }
                Err(error) => return Err(error),
            }
        };
        let (observer, signal) = GpuCompletion::pair();
        // Reserve the response handle before any work can be accepted. Closing
        // this handle later affects observation only, not the driver's ownership.
        let handle = match handles.insert_with_metadata(
            KernelObject::Gpu(Arc::new(observer)),
            crate::device::gpu::child_handle_metadata(AccessMode::ReadOnly),
        ) {
            Ok(handle) => handle,
            Err(_) => {
                request.result = GPU_RESULT_OUT_OF_RESOURCES;
                publish(&request)?;
                return Ok(0);
            }
        };
        match self.enqueue_owned(commands, signal, permit) {
            EnqueueOutcome::Rejected(result) => {
                handles.remove(handle);
                request.result = result;
            }
            EnqueueOutcome::Accepted(result) => {
                request.result = result;
                request.accepted = 1;
                request.completion_handle = handle;
            }
        }
        if let Err(error) = publish(&request) {
            if request.accepted != 0 {
                // The driver retains the request independently even when no
                // observer handle can be delivered to the submitting process.
                handles.remove(handle);
            }
            return Err(error);
        }
        Ok(0)
    }

    fn enqueue_owned(
        &self,
        commands: Vec<u8>,
        signal: GpuCompletionSignal,
        permit: SubmissionPermit,
    ) -> EnqueueOutcome {
        // Do not wait for a synchronous attach/detach to finish GPU work. Keep
        // both locks through backend validation/enqueue to prevent snapshots
        // from retaining different authority than the backend actually checks.
        let Some(images) = self._attached_images.try_lock() else {
            return EnqueueOutcome::Rejected(GPU_RESULT_BUSY);
        };
        let Some(buffers) = self._attached_buffers.try_lock() else {
            return EnqueueOutcome::Rejected(GPU_RESULT_BUSY);
        };
        let (Ok(owned_images), Ok(owned_buffers)) = (snapshot(&images), snapshot(&buffers)) else {
            return EnqueueOutcome::Rejected(GPU_RESULT_OUT_OF_RESOURCES);
        };
        let submission = GpuSubmission::new(
            commands,
            SubmissionResources {
                _images: owned_images,
                _buffers: owned_buffers,
                _context: Arc::clone(&self._backend_context),
                _queue: Arc::clone(&self.backend_queue),
            },
            permit,
            signal,
        );
        let result = self.backend_queue.enqueue(submission);
        drop(buffers);
        drop(images);
        // A rejected request may own the last backend reference. Releasing it
        // outside attachment locks avoids re-entering a locked context on Drop.
        match result {
            Ok(()) => EnqueueOutcome::Accepted(GPU_RESULT_SUCCESS),
            Err(GpuBackendEnqueueError::Busy(submission)) => {
                submission.reject();
                EnqueueOutcome::Rejected(GPU_RESULT_BUSY)
            }
            Err(GpuBackendEnqueueError::Rejected(error, submission)) => {
                submission.reject();
                EnqueueOutcome::Rejected(backend_result(error))
            }
            Err(GpuBackendEnqueueError::Failed(error)) => {
                EnqueueOutcome::Accepted(backend_result(error))
            }
        }
    }
}
