//! GPU device interface.
//!
//! This module defines the non-framebuffer GPU interface used by accelerated
//! backends such as virtio-gpu virgl. Display scanout remains in
//! `device::graphics`; this module is for GPU contexts, resources, command
//! submission, and fences.

use alloc::vec::Vec;

use super::Device;

/// Optional GPU backend features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuFeature {
    /// Virtio-gpu virgl command submission is available.
    Virgl,
    /// Host-visible blob resources are available.
    ResourceBlob,
    /// Explicit GPU fences are available.
    Fences,
    /// Context initialization parameters are available.
    ContextInit,
}

/// GPU backend capability summary.
#[derive(Debug, Clone)]
pub struct GpuCapabilities {
    /// Supported feature list.
    pub features: Vec<GpuFeature>,
    /// Number of capsets exposed by the backend.
    pub capset_count: u32,
}

impl GpuCapabilities {
    /// Create an empty capability set.
    ///
    /// # Returns
    ///
    /// A capability set with no optional GPU features.
    pub fn empty() -> Self {
        Self {
            features: Vec::new(),
            capset_count: 0,
        }
    }

    /// Check whether a feature is present.
    ///
    /// # Arguments
    ///
    /// * `feature` - Feature to check.
    ///
    /// # Returns
    ///
    /// `true` if the feature is present.
    pub fn contains(&self, feature: GpuFeature) -> bool {
        self.features.iter().any(|item| *item == feature)
    }
}

/// GPU capset metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuCapsetInfo {
    /// Backend-defined capset identifier.
    pub id: u32,
    /// Highest supported capset version.
    pub max_version: u32,
    /// Maximum byte size of this capset.
    pub max_size: u32,
}

/// GPU command submission descriptor.
pub struct GpuCommandSubmission<'a> {
    /// GPU context that owns the command stream.
    pub context_id: u32,
    /// Backend-specific command bytes.
    pub commands: &'a [u8],
    /// Optional fence identifier signaled when the submission completes.
    pub fence_id: Option<u64>,
}

/// Non-framebuffer GPU interface.
///
/// `GpuDevice` is intentionally separate from `GraphicsDevice`: a device may
/// provide scanout-only display, GPU acceleration, or both.
pub trait GpuDevice: Device {
    /// Get the GPU backend capabilities.
    ///
    /// # Returns
    ///
    /// Capability summary for this GPU backend.
    fn gpu_capabilities(&self) -> GpuCapabilities;

    /// Get metadata for a backend capset.
    ///
    /// # Arguments
    ///
    /// * `index` - Zero-based capset index.
    ///
    /// # Returns
    ///
    /// Capset metadata or an error describing why it is unavailable.
    fn get_capset_info(&self, _index: u32) -> Result<GpuCapsetInfo, &'static str> {
        Err("GPU capsets are not supported")
    }

    /// Read a backend capset into a caller-provided buffer.
    ///
    /// # Arguments
    ///
    /// * `id` - Backend-defined capset identifier.
    /// * `version` - Capset version to read.
    /// * `buffer` - Destination buffer for capset bytes.
    ///
    /// # Returns
    ///
    /// Number of bytes written or an error describing why the capset is unavailable.
    fn read_capset(
        &self,
        _id: u32,
        _version: u32,
        _buffer: &mut [u8],
    ) -> Result<usize, &'static str> {
        Err("GPU capsets are not supported")
    }

    /// Create a GPU execution context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Caller-selected context identifier.
    /// * `debug_name` - Optional human-readable context name.
    ///
    /// # Returns
    ///
    /// Success or an error describing why context creation failed.
    fn create_context(&self, _context_id: u32, _debug_name: &str) -> Result<(), &'static str> {
        Err("GPU contexts are not supported")
    }

    /// Destroy a GPU execution context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Context identifier to destroy.
    ///
    /// # Returns
    ///
    /// Success or an error describing why context destruction failed.
    fn destroy_context(&self, _context_id: u32) -> Result<(), &'static str> {
        Err("GPU contexts are not supported")
    }

    /// Submit a backend-specific GPU command stream.
    ///
    /// # Arguments
    ///
    /// * `submission` - Command stream, target context, and optional fence.
    ///
    /// # Returns
    ///
    /// Success or an error describing why submission failed.
    fn submit_commands(&self, _submission: GpuCommandSubmission<'_>) -> Result<(), &'static str> {
        Err("GPU command submission is not supported")
    }
}
