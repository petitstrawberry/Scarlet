//! Backend-neutral GPU information model.

use super::{GPU_BACKEND_ID_BYTES, GPU_BACKEND_INFO_BYTES};

/// The device is not usable for GPU control.
pub const GPU_DEVICE_STATE_UNAVAILABLE: u32 = 0;
/// The device is available for its advertised operations.
pub const GPU_DEVICE_STATE_READY: u32 = 1;
/// The device was lost after it became available.
pub const GPU_DEVICE_STATE_LOST: u32 = 2;

/// No generic execution support is available.
pub const GPU_EXECUTION_SUPPORT_NONE: u32 = 0;
/// Generic address-space operations are available.
pub const GPU_EXECUTION_SUPPORT_ADDRESS_SPACE: u32 = 1 << 0;
/// Generic memory operations are available.
pub const GPU_EXECUTION_SUPPORT_MEMORY: u32 = 1 << 1;
/// Generic queue operations are available.
pub const GPU_EXECUTION_SUPPORT_QUEUE: u32 = 1 << 2;
/// Generic timeline operations are available.
pub const GPU_EXECUTION_SUPPORT_TIMELINE: u32 = 1 << 3;
/// Generic presentation operations are available.
pub const GPU_EXECUTION_SUPPORT_PRESENTATION: u32 = 1 << 4;

/// Stable state of a GPU device.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuDeviceState {
    /// The backend is unavailable for GPU control.
    Unavailable = GPU_DEVICE_STATE_UNAVAILABLE,
    /// The backend is ready for its advertised operations.
    Ready = GPU_DEVICE_STATE_READY,
    /// The backend has been lost.
    Lost = GPU_DEVICE_STATE_LOST,
}

/// Backend-neutral stable GPU device information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuDeviceInfo {
    /// Current stable device state.
    pub state: GpuDeviceState,
    /// Truthful generic execution support bits.
    pub execution_support: u32,
    /// Maximum opaque command size for a generic command operation, or zero.
    pub max_opaque_command_size: u32,
}

impl GpuDeviceInfo {
    /// Build stable information for a GPU device.
    ///
    /// # Arguments
    ///
    /// * `state` - Current backend device state.
    /// * `execution_support` - Truthful generic execution support bits.
    /// * `max_opaque_command_size` - Maximum generic opaque command size, or zero.
    ///
    /// # Returns
    ///
    /// Stable GPU device information.
    pub const fn new(
        state: GpuDeviceState,
        execution_support: u32,
        max_opaque_command_size: u32,
    ) -> Self {
        Self {
            state,
            execution_support,
            max_opaque_command_size,
        }
    }
}

/// Backend-provided information exposed through a GPU connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackendInfo {
    /// Stable device information shared across all backend implementations.
    pub device: GpuDeviceInfo,
    /// Backend-defined negotiated feature bits.
    pub backend_feature_bits: u64,
    /// Opaque backend or dialect identifier bytes.
    pub backend_id: [u8; GPU_BACKEND_ID_BYTES],
    /// Length of meaningful bytes in `backend_id`.
    pub backend_id_len: u32,
    /// Opaque backend-defined bytes.
    pub opaque_info: [u8; GPU_BACKEND_INFO_BYTES],
    /// Length of meaningful bytes in `opaque_info`.
    pub opaque_info_len: u32,
}

impl GpuBackendInfo {
    /// Build backend information from bounded identifier and opaque data slices.
    ///
    /// # Arguments
    ///
    /// * `device` - Stable generic device information.
    /// * `backend_feature_bits` - Backend-defined negotiated feature bits.
    /// * `backend_id` - Opaque backend or dialect identifier bytes.
    /// * `opaque_info` - Opaque backend-defined information bytes.
    ///
    /// # Returns
    ///
    /// A fixed-capacity backend information record. Input slices are truncated
    /// to their ABI-defined fixed capacities.
    pub fn new(
        device: GpuDeviceInfo,
        backend_feature_bits: u64,
        backend_id: &[u8],
        opaque_info: &[u8],
    ) -> Self {
        let mut result = Self {
            device,
            backend_feature_bits,
            backend_id: [0; GPU_BACKEND_ID_BYTES],
            backend_id_len: 0,
            opaque_info: [0; GPU_BACKEND_INFO_BYTES],
            opaque_info_len: 0,
        };
        let backend_id_len = backend_id.len().min(GPU_BACKEND_ID_BYTES);
        result.backend_id[..backend_id_len].copy_from_slice(&backend_id[..backend_id_len]);
        result.backend_id_len = backend_id_len as u32;
        let opaque_info_len = opaque_info.len().min(GPU_BACKEND_INFO_BYTES);
        result.opaque_info[..opaque_info_len].copy_from_slice(&opaque_info[..opaque_info_len]);
        result.opaque_info_len = opaque_info_len as u32;
        result
    }
}

/// A backend that provides only stable GPU and opaque backend information.
pub trait GpuBackend: Send + Sync {
    /// Query the current backend-neutral GPU information.
    ///
    /// # Returns
    ///
    /// Stable device information plus backend-defined opaque identity and data.
    fn query_info(&self) -> GpuBackendInfo;
}
