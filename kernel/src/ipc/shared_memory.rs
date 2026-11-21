//! Shared memory implementation for inter-process communication
//!
//! This module provides shared memory objects for memory-based communication between processes.
//! Shared memory allows multiple processes to access the same physical memory region,
//! providing efficient data sharing without copying.

use crate::object::capability::memory_mapping::MemoryMappingOps;
use alloc::{string::String, sync::Arc};

#[cfg(not(feature = "hosted"))]
mod bare_metal;
#[cfg(not(feature = "hosted"))]
pub use bare_metal::SharedMemory;
#[cfg(feature = "hosted")]
mod hosted;
#[cfg(feature = "hosted")]
pub use hosted::SharedMemory;

pub(super) const LOG_SHARED_MEMORY_RESIZE: bool = false;

/// Kernel-only description of stable shared-memory backing.
///
/// This is returned only after a shared-memory range has been pinned. Its
/// physical address is never exposed through a userspace ABI.
#[derive(Debug, Clone, Copy)]
pub struct SharedMemoryBacking {
    paddr: u64,
    size: usize,
}

impl SharedMemoryBacking {
    pub(crate) const fn paddr(&self) -> u64 {
        self.paddr
    }

    pub(crate) const fn size(&self) -> usize {
        self.size
    }
}

/// Strong shared-memory pin retained by a kernel importer.
///
/// The owner Arc keeps the SharedMemory object alive until after the pin is
/// released, so a consumer never relies on a raw physical address alone.
pub(crate) struct SharedMemoryPin {
    owner: Arc<dyn SharedMemoryObject>,
    backing: SharedMemoryBacking,
}

impl SharedMemoryPin {
    pub(crate) fn new(
        owner: Arc<dyn SharedMemoryObject>,
        offset: usize,
        length: usize,
    ) -> Result<Self, &'static str> {
        let backing = owner.pin_range(offset, length)?;
        Ok(Self { owner, backing })
    }

    pub(crate) const fn backing(&self) -> SharedMemoryBacking {
        self.backing
    }
}

impl Drop for SharedMemoryPin {
    fn drop(&mut self) {
        self.owner.unpin_range();
    }
}

/// Shared memory operations
///
/// This trait extends the base functionality for shared memory objects.
pub trait SharedMemoryObject: MemoryMappingOps + Send + Sync {
    /// Get the size of the shared memory region in bytes
    fn size(&self) -> usize;

    /// Resize the shared memory region (within capacity)
    fn resize(&self, new_size: usize) -> Result<(), &'static str>;

    /// Get a unique identifier for this shared memory object
    fn id(&self) -> String;

    /// Check if the shared memory is still valid
    fn is_valid(&self) -> bool;

    /// Pin a live range so its backing cannot change while imported.
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset of the range to validate.
    /// * `length` - Non-zero byte length of the range to validate.
    ///
    /// # Returns
    ///
    /// Kernel-only metadata for the stable backing, or an error when the
    /// object is invalid or the range is outside its current size. Each
    /// successful call must be paired with [`SharedMemoryObject::unpin_range`].
    fn pin_range(&self, offset: usize, length: usize) -> Result<SharedMemoryBacking, &'static str>;

    /// Release one prior shared-memory backing pin.
    ///
    /// # Returns
    ///
    /// Nothing. Calls without a corresponding pin are ignored so importer drop
    /// paths remain safe during error cleanup.
    fn unpin_range(&self);
}
