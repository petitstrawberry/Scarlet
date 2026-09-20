//! Immutable, ready-to-read image capabilities shared by independent producers
//! and consumers. Keeping the capability alive leases all backing memory.

use super::GpuBackingSegment;
use crate::device::gpu::GpuObject;
use crate::object::capability::ControlOps;
use alloc::sync::Arc;
pub use scarlet_abi::shared_image::*;

/// Resident backing of a ready shared image.
///
/// # Safety
/// Descriptors and physical segments must stay unchanged and resident for the
/// lifetime of this object. All producer writes/cache maintenance must finish
/// before publication; no producer may write/recycle it while any owner exists.
pub unsafe trait SharedImageBacking: Send + Sync {
    fn descriptor(&self) -> SharedImageDescriptor;
    fn buffer_segments(&self, index: usize) -> Arc<[GpuBackingSegment]>;
}

#[derive(Clone)]
pub struct SharedImage {
    descriptor: SharedImageDescriptor,
    backing: Arc<dyn SharedImageBacking>,
}

impl core::fmt::Debug for SharedImage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SharedImage")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

impl SharedImage {
    pub fn new(backing: Arc<dyn SharedImageBacking>) -> Result<Self, &'static str> {
        let descriptor = backing.descriptor();
        descriptor.validate()?;
        for index in 0..descriptor.buffer_count as usize {
            let mut size = 0u64;
            for segment in backing.buffer_segments(index).iter() {
                if segment.physical_addr() % 4096 != 0
                    || segment.length() == 0
                    || segment.length() % 4096 != 0
                    || segment
                        .physical_addr()
                        .checked_add(segment.length() as u64)
                        .is_none()
                {
                    return Err("shared image backing is not page aligned");
                }
                size = size
                    .checked_add(segment.length() as u64)
                    .ok_or("shared image size overflow")?;
            }
            if size < descriptor.buffer_sizes[index] {
                return Err("shared image backing is truncated");
            }
        }
        Ok(Self {
            descriptor,
            backing,
        })
    }

    pub const fn descriptor(&self) -> SharedImageDescriptor {
        self.descriptor
    }
    pub fn backing(&self) -> Arc<dyn SharedImageBacking> {
        self.backing.clone()
    }
}

impl GpuObject for SharedImage {
    fn as_control_ops(&self) -> Option<&dyn ControlOps> {
        Some(self)
    }
    fn as_shared_image(&self) -> Option<&SharedImage> {
        Some(self)
    }
}

impl ControlOps for SharedImage {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        if command != SHARED_IMAGE_QUERY {
            return Err("unsupported shared image operation");
        }
        crate::device::gpu::connection::write_user_value(arg, &self.descriptor)?;
        Ok(0)
    }
    fn supported_control_commands(&self) -> alloc::vec::Vec<(u32, &'static str)> {
        alloc::vec![(SHARED_IMAGE_QUERY, "Query immutable shared image metadata")]
    }
}
