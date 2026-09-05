//! Read-only observation of accepted GPU work, separate from writable timelines.

use crate::{GPU_ABI_VERSION, GPU_RESULT_SUCCESS, Handle, HandleResult, result_to_handle_error};

/// Covered GPU work has not yet been observed to retire.
pub const GPU_COMPLETION_PENDING: u32 = 0;
/// All covered GPU accesses have retired; not a presentation or cache barrier.
pub const GPU_COMPLETION_COMPLETE: u32 = 1;
/// Completion failed. GPU quiescence is not implied.
pub const GPU_COMPLETION_FAILED: u32 = 2;
/// No failure has been reported.
pub const GPU_COMPLETION_FAILURE_NONE: u32 = 0;
/// A hardware fault, timeout, or reset made the device unusable.
pub const GPU_COMPLETION_FAILURE_DEVICE_LOST: u32 = 1;
/// The kernel producer was dropped without reporting a terminal result.
pub const GPU_COMPLETION_FAILURE_ABANDONED: u32 = 2;
/// Another failure occurred while executing accepted work.
pub const GPU_COMPLETION_FAILURE_EXECUTION: u32 = 3;

/// Fixed-width query and response for [`crate::commands::GPU_COMPLETION_QUERY`].
///
/// Both successful and failed completions become readable/selectable. A query
/// must still inspect `state` and `failure`; readiness alone is not success.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuCompletionInfo {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` query result, separate from execution failure.
    pub result: u32,
    /// One of `GPU_COMPLETION_PENDING`, `GPU_COMPLETION_COMPLETE`, or `GPU_COMPLETION_FAILED`.
    pub state: u32,
    /// `GPU_COMPLETION_FAILURE_*` reason, zero unless `state` is failed.
    pub failure: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved2: u32,
}

impl GpuCompletionInfo {
    /// Create a completion query for the current ABI version.
    ///
    /// # Returns
    ///
    /// A request with reserved fields zeroed; output fields are not observations
    /// until a successful completion-query control call fills them.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            state: GPU_COMPLETION_PENDING,
            failure: GPU_COMPLETION_FAILURE_NONE,
            reserved: 0,
            reserved2: 0,
        }
    }
}

impl Default for GpuCompletionInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Owning, read-only completion handle; closing it does not cancel GPU work.
///
/// This handle cannot be signalled or failed by userspace. A successful query
/// certifies only that the observation was read: inspect its state separately.
#[derive(Debug)]
pub struct GpuCompletion {
    pub(crate) handle: Handle,
}

impl GpuCompletion {
    /// Observe completion without waiting for GPU retirement.
    ///
    /// # Returns
    ///
    /// A pending, complete, or failed observation, or a query/handle error.
    /// Failure does not authorize reuse of externally shared GPU backing.
    pub fn query(&self) -> HandleResult<GpuCompletionInfo> {
        let mut info = GpuCompletionInfo::new();
        self.handle.control(
            crate::commands::GPU_COMPLETION_QUERY,
            &mut info as *mut _ as usize,
        )?;
        result_to_handle_error(info.result)?;
        Ok(info)
    }

    /// Borrow the selectable completion handle for poll/select integration.
    ///
    /// # Returns
    ///
    /// A handle which becomes readable on success or failure and is never
    /// writable. A readiness notification must be followed by [`Self::query`].
    pub fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Consume this wrapper and return its owning handle.
    ///
    /// # Returns
    ///
    /// The same read-only observation authority, without cancelling GPU work.
    pub fn into_handle(self) -> Handle {
        self.handle
    }
}

const _: () = {
    assert!(core::mem::size_of::<GpuCompletionInfo>() == 24);
    assert!(core::mem::align_of::<GpuCompletionInfo>() == 4);
    assert!(core::mem::offset_of!(GpuCompletionInfo, failure) == 12);
};
