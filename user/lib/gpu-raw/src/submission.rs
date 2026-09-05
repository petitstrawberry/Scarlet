//! Additive async queue records; the existing synchronous records are unchanged.

use super::{GPU_ABI_VERSION, GPU_RESULT_SUCCESS};
use crate::{
    GPU_MAX_OPAQUE_COMMAND_SIZE, GPU_RESULT_BUSY, GpuCompletion, GpuQueue, HandleError,
    HandleResult, adopt_child_handle, commands, result_to_handle_error,
};

/// Fixed-width response for [`crate::commands::GPU_QUEUE_QUERY_ASYNC`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuQueueAsyncInfo {
    /// Requested ABI version, echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` query result.
    pub result: u32,
    /// Maximum retained submissions, or zero when async is not implemented.
    pub max_pending_submissions: u32,
    /// Maximum bytes in one async command stream; zero when unsupported.
    pub max_opaque_command_size: u32,
    /// Reserved for extension. Must be zero.
    pub reserved: u32,
    /// Reserved for extension. Must be zero.
    pub reserved2: u32,
}

impl GpuQueueAsyncInfo {
    /// Create a query with zeroed reserved fields.
    ///
    /// # Returns
    ///
    /// A request for the current ABI, not an observation until queried.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            max_pending_submissions: 0,
            max_opaque_command_size: 0,
            reserved: 0,
            reserved2: 0,
        }
    }
}

impl Default for GpuQueueAsyncInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request/response for [`crate::commands::GPU_QUEUE_SUBMIT_ASYNC`].
///
/// `accepted` is separate from `result`: a failing result with acceptance still
/// returns a receipt covering all possibly accepted work. A control/syscall
/// transport error is not a side-effect-free rejection. Commands and retained
/// resources remain kernel-owned even if copying this response fails.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuQueueSubmitAsync {
    /// Requested ABI version, echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` admission result, not GPU completion status.
    pub result: u32,
    /// No flags currently defined. Must be zero.
    pub flags: u32,
    /// Reserved for extension. Must be zero.
    pub reserved: u32,
    /// Userspace command bytes, copied before return; ignored for an empty stream.
    pub command_ptr: u64,
    /// Command byte length; zero requests an ordered queue checkpoint.
    pub command_size: u32,
    /// New read-only completion handle when accepted; zero otherwise.
    pub completion_handle: u32,
    /// One if a prefix may be accepted, zero for a side-effect-free rejection.
    pub accepted: u32,
    /// Reserved for extension. Must be zero.
    pub reserved2: u32,
}

impl GpuQueueSubmitAsync {
    /// Describe a command slice for an immediate async-submit control call.
    ///
    /// # Arguments
    ///
    /// * `commands` - Opaque bytes, or an empty queue checkpoint. Keep the slice
    ///   valid until the control call returns; the raw record does not borrow it.
    ///
    /// # Returns
    ///
    /// A fixed-width request, or an error when the global command bound is exceeded.
    pub fn new(commands: &[u8]) -> HandleResult<Self> {
        let command_size =
            u32::try_from(commands.len()).map_err(|_| HandleError::InvalidParameter)?;
        if command_size > GPU_MAX_OPAQUE_COMMAND_SIZE {
            return Err(HandleError::InvalidParameter);
        }
        Ok(Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            flags: 0,
            reserved: 0,
            command_ptr: if commands.is_empty() {
                0
            } else {
                commands.as_ptr() as u64
            },
            command_size,
            completion_handle: 0,
            accepted: 0,
            reserved2: 0,
        })
    }
}

/// Async submission failure, distinguishing rejection from possibly accepted work.
#[derive(Debug)]
#[non_exhaustive]
pub enum GpuSubmitError {
    /// Nothing was accepted; bounded admission capacity is currently unavailable.
    Busy,
    /// Nothing from this call was accepted. Earlier work is not rolled back.
    Rejected(HandleError),
    /// Work may have been accepted. Do not replay it as a side-effect-free rejection.
    Failed {
        /// Immediate admission or transport failure.
        error: HandleError,
        /// Receipt covering all possibly accepted work. `None` means the control
        /// transport or handle adoption failed, so observation is unavailable;
        /// it does not imply cancellation or authorize shared-buffer reuse.
        completion: Option<GpuCompletion>,
    },
}

enum Reply {
    Busy,
    Rejected(HandleError),
    Accepted {
        handle: u32,
        result: HandleResult<()>,
    },
    Invalid,
}

fn classify_reply(request: &GpuQueueSubmitAsync) -> Reply {
    if request.abi_version != GPU_ABI_VERSION || request.reserved != 0 || request.reserved2 != 0 {
        return Reply::Invalid;
    }
    match (request.accepted, request.result) {
        (0, GPU_RESULT_BUSY) => Reply::Busy,
        (0, GPU_RESULT_SUCCESS) => Reply::Invalid,
        (0, result) => match result_to_handle_error(result) {
            Err(error) => Reply::Rejected(error),
            Ok(()) => Reply::Invalid,
        },
        (1, result) => Reply::Accepted {
            handle: request.completion_handle,
            result: result_to_handle_error(result),
        },
        _ => Reply::Invalid,
    }
}

impl GpuQueue {
    /// Query whether this queue explicitly implements bounded async submission.
    ///
    /// # Returns
    ///
    /// Async limits; zero capacity means unsupported, not temporarily busy.
    /// Older kernels may reject this additive query operation outright.
    pub fn query_async(&self) -> HandleResult<GpuQueueAsyncInfo> {
        let mut info = GpuQueueAsyncInfo::new();
        self.handle.control(
            commands::GPU_QUEUE_QUERY_ASYNC,
            &mut info as *mut _ as usize,
        )?;
        result_to_handle_error(info.result)?;
        Ok(info)
    }

    /// Enqueue commands without waiting for GPU completion or admission capacity.
    ///
    /// # Arguments
    ///
    /// * `commands` - Opaque bytes; empty establishes an ordered queue checkpoint.
    ///   The kernel copies the slice before return. Resources must already be
    ///   attached to this queue's context and valid for the selected dialect.
    ///
    /// # Returns
    ///
    /// An owned read-only completion, or classified rejection/partial failure.
    /// The kernel independently retains accepted work when this receipt, queue,
    /// context, or submitting process is dropped. Completion is not presentation,
    /// CPU cache visibility, or permission to overwrite pending shared backing.
    /// This method never falls back to [`Self::submit`].
    pub fn submit_async(&self, commands: &[u8]) -> Result<GpuCompletion, GpuSubmitError> {
        let mut request = GpuQueueSubmitAsync::new(commands).map_err(GpuSubmitError::Rejected)?;
        if request.command_size > self.max_opaque_command_size {
            return Err(GpuSubmitError::Rejected(HandleError::InvalidParameter));
        }
        self.handle
            .control(
                commands::GPU_QUEUE_SUBMIT_ASYNC,
                &mut request as *mut _ as usize,
            )
            .map_err(|error| GpuSubmitError::Failed {
                error,
                completion: None,
            })?;
        match classify_reply(&request) {
            Reply::Busy => Err(GpuSubmitError::Busy),
            Reply::Rejected(error) => Err(GpuSubmitError::Rejected(error)),
            Reply::Invalid => Err(GpuSubmitError::Failed {
                error: HandleError::InvalidParameter,
                completion: None,
            }),
            Reply::Accepted { handle, result } => {
                // Handle zero is valid when accepted == 1. Acceptance, not a
                // sentinel handle value, controls whether this authority exists.
                let handle =
                    adopt_child_handle(handle).map_err(|error| GpuSubmitError::Failed {
                        error,
                        completion: None,
                    })?;
                let completion = GpuCompletion { handle };
                match result {
                    Ok(()) => Ok(completion),
                    Err(error) => Err(GpuSubmitError::Failed {
                        error,
                        completion: Some(completion),
                    }),
                }
            }
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<GpuQueueAsyncInfo>() == 24);
    assert!(core::mem::align_of::<GpuQueueAsyncInfo>() == 4);
    assert!(core::mem::size_of::<GpuQueueSubmitAsync>() == 40);
    assert!(core::mem::align_of::<GpuQueueSubmitAsync>() == 8);
    assert!(core::mem::offset_of!(GpuQueueSubmitAsync, command_ptr) == 16);
    assert!(core::mem::offset_of!(GpuQueueSubmitAsync, accepted) == 32);
};

#[cfg(test)]
mod tests {
    use super::{GpuQueueSubmitAsync, Reply, classify_reply};
    use crate::{GPU_RESULT_BUSY, GPU_RESULT_INVALID_ARGUMENT, GPU_RESULT_SUCCESS};

    #[test]
    fn reply_acceptance_is_separate_from_result_and_handle_zero() {
        let mut reply = GpuQueueSubmitAsync::new(&[]).expect("checkpoint");
        assert!(matches!(classify_reply(&reply), Reply::Invalid));
        reply.result = GPU_RESULT_BUSY;
        assert!(matches!(classify_reply(&reply), Reply::Busy));
        reply.result = GPU_RESULT_INVALID_ARGUMENT;
        assert!(matches!(classify_reply(&reply), Reply::Rejected(_)));
        reply.accepted = 1;
        assert!(matches!(
            classify_reply(&reply),
            Reply::Accepted {
                handle: 0,
                result: Err(_)
            }
        ));
        reply.result = GPU_RESULT_BUSY;
        assert!(matches!(
            classify_reply(&reply),
            Reply::Accepted { result: Err(_), .. }
        ));
        reply.result = GPU_RESULT_SUCCESS;
        assert!(matches!(
            classify_reply(&reply),
            Reply::Accepted {
                handle: 0,
                result: Ok(())
            }
        ));
        reply.accepted = 2;
        assert!(matches!(classify_reply(&reply), Reply::Invalid));
    }

    #[test]
    fn raw_async_request_preserves_bytes_and_zeroes_reserved_fields() {
        let bytes = [1u8, 2, 3, 4];
        let request = GpuQueueSubmitAsync::new(&bytes).expect("command request");
        assert_eq!(request.command_ptr, bytes.as_ptr() as u64);
        assert_eq!(request.command_size, 4);
        assert_eq!(
            (request.flags, request.reserved, request.reserved2),
            (0, 0, 0)
        );
        let checkpoint = GpuQueueSubmitAsync::new(&[]).expect("checkpoint");
        assert_eq!((checkpoint.command_ptr, checkpoint.command_size), (0, 0));
    }
}
