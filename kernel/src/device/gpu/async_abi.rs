//! Additive async queue records; the existing synchronous records are unchanged.

use super::{GPU_ABI_VERSION, GPU_RESULT_SUCCESS};

/// Fixed-width response for [`super::GPU_QUEUE_QUERY_ASYNC`].
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

/// Fixed-width request/response for [`super::GPU_QUEUE_SUBMIT_ASYNC`].
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

const _: () = {
    assert!(core::mem::size_of::<GpuQueueAsyncInfo>() == 24);
    assert!(core::mem::align_of::<GpuQueueAsyncInfo>() == 4);
    assert!(core::mem::size_of::<GpuQueueSubmitAsync>() == 40);
    assert!(core::mem::align_of::<GpuQueueSubmitAsync>() == 8);
    assert!(core::mem::offset_of!(GpuQueueSubmitAsync, command_ptr) == 16);
    assert!(core::mem::offset_of!(GpuQueueSubmitAsync, accepted) == 32);
};
