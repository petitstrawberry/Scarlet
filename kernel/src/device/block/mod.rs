use core::any::Any;

use crate::sync::IrqSpinLock;
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use request::{BlockIORequest, BlockIOResult};

use super::Device;
use crate::object::capability::selectable::Selectable;
use crate::object::capability::{ControlOps, MemoryMappingOps};

pub mod partition;
pub mod request;

extern crate alloc;

/// Block device interface
///
/// This trait defines the interface for block devices.
/// It provides methods for querying device information and handling I/O requests.
pub trait BlockDevice: Device {
    /// Get the disk name
    ///
    /// # Returns
    ///
    /// Static disk name used for diagnostics.
    fn get_disk_name(&self) -> &'static str;

    /// Get the disk size in bytes
    ///
    /// # Returns
    ///
    /// Total device size in bytes.
    fn get_disk_size(&self) -> usize;

    /// Enqueue a block I/O request
    ///
    /// # Arguments
    ///
    /// * `request` - Request to enqueue for later processing.
    fn enqueue_request(&self, request: Box<BlockIORequest>);

    /// Get the logical sector size in bytes.
    ///
    /// # Returns
    ///
    /// Logical sector size in bytes. Devices that do not report a size default
    /// to 512-byte sectors.
    fn get_sector_size(&self) -> usize {
        512
    }

    /// Process all queued requests
    ///
    /// # Returns
    ///
    /// A vector of results for all processed requests
    fn process_requests(&self) -> Vec<BlockIOResult>;
}

/// A generic implementation of a block device
#[allow(clippy::vec_box)]
pub struct GenericBlockDevice {
    disk_name: &'static str,
    disk_size: usize,
    request_fn: fn(&mut BlockIORequest) -> Result<(), &'static str>,
    request_queue: IrqSpinLock<Vec<Box<BlockIORequest>>>,
}

impl GenericBlockDevice {
    pub fn new(
        disk_name: &'static str,
        disk_size: usize,
        request_fn: fn(&mut BlockIORequest) -> Result<(), &'static str>,
    ) -> Self {
        Self {
            disk_name,
            disk_size,
            request_fn,
            request_queue: IrqSpinLock::new(Vec::new()),
        }
    }
}

impl Device for GenericBlockDevice {
    fn device_type(&self) -> super::DeviceType {
        super::DeviceType::Block
    }

    fn name(&self) -> &'static str {
        self.disk_name
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_block_device(&self) -> Option<&dyn BlockDevice> {
        Some(self)
    }

    fn into_block_device(self: Arc<Self>) -> Option<Arc<dyn BlockDevice>> {
        Some(self)
    }
}

impl ControlOps for GenericBlockDevice {
    // Generic block devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for GenericBlockDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported by this block device")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Generic block devices don't support memory mapping
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Generic block devices don't support memory mapping
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for GenericBlockDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

impl BlockDevice for GenericBlockDevice {
    fn get_disk_name(&self) -> &'static str {
        self.disk_name
    }

    fn get_disk_size(&self) -> usize {
        self.disk_size
    }

    fn enqueue_request(&self, request: Box<BlockIORequest>) {
        // Use an IRQ spin lock for internal mutability.
        self.request_queue.lock().push(request);
    }

    /// Process all queued block I/O requests
    ///
    /// This method processes all pending requests using a lock-efficient approach:
    ///
    /// 1. Acquires the request_queue lock once
    /// 2. Extracts all requests at once using mem::replace
    /// 3. Releases the lock immediately
    /// 4. Processes all requests without holding any locks
    ///
    /// This approach minimizes lock contention and prevents deadlocks by:
    /// - Never holding the lock during request processing
    /// - Allowing other threads to enqueue requests while processing
    /// - Avoiding any circular lock dependencies
    ///
    /// # Returns
    /// Vector of `BlockIOResult` containing completed requests and their results
    fn process_requests(&self) -> Vec<BlockIOResult> {
        let mut results = Vec::new();

        // Extract all requests at once to minimize lock time
        let requests = {
            let mut queue = self.request_queue.lock();
            core::mem::take(&mut *queue)
        }; // Lock is automatically released here

        // Process all requests without holding any locks
        for mut request in requests {
            // Process the request using the function pointer
            let result = (self.request_fn)(&mut request);

            // Add the result to the results vector
            results.push(BlockIOResult { request, result });
        }

        results
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub mod mockblk;
