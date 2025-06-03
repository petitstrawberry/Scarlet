use core::any::Any;

use alloc::{boxed::Box, vec::Vec};
use request::{BlockIORequest, BlockIOResult};

use super::Device;

pub mod request;

extern crate alloc;

/// Block device interface
/// 
/// This trait defines the interface for block devices.
/// It provides methods for querying device information and handling I/O requests.
pub trait BlockDevice: Device {
    /// Get the device identifier
    fn get_id(&self) -> usize;
    
    /// Get the disk name
    fn get_disk_name(&self) -> &'static str;
    
    /// Get the disk size in bytes
    fn get_disk_size(&self) -> usize;
    
    /// Enqueue a block I/O request
    fn enqueue_request(&mut self, request: Box<BlockIORequest>);
    
    /// Process all queued requests
    /// 
    /// # Returns
    /// 
    /// A vector of results for all processed requests
    fn process_requests(&mut self) -> Vec<BlockIOResult>;
}

/// A generic implementation of a block device
pub struct GenericBlockDevice {
    id: usize,
    disk_name: &'static str,
    disk_size: usize,
    request_fn: fn(&mut BlockIORequest) -> Result<(), &'static str>,
    request_queue: Vec<Box<BlockIORequest>>,
}

impl GenericBlockDevice {
    pub fn new(id: usize, disk_name: &'static str, disk_size: usize, request_fn: fn(&mut BlockIORequest) -> Result<(), &'static str>) -> Self {
        Self { id, disk_name, disk_size, request_fn, request_queue: Vec::new() }
    }
}

impl Device for GenericBlockDevice {
    fn device_type(&self) -> super::DeviceType {
        super::DeviceType::Block
    }

    fn name(&self) -> &'static str {
        self.disk_name
    }

    fn id(&self) -> usize {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    
    fn as_block_device(&mut self) -> Option<&mut dyn BlockDevice> {
        Some(self)
    }
}

impl BlockDevice for GenericBlockDevice {
    fn get_id(&self) -> usize {
        self.id
    }

    fn get_disk_name(&self) -> &'static str {
        self.disk_name
    }

    fn get_disk_size(&self) -> usize {
        self.disk_size
    }

    fn enqueue_request(&mut self, request: Box<BlockIORequest>) {
        self.request_queue.push(request);
    }

    fn process_requests(&mut self) -> Vec<BlockIOResult> {
        let mut results = Vec::new();
    
        while let Some(mut request) = self.request_queue.pop() {
            let result = (self.request_fn)(&mut *request);
            results.push(BlockIOResult { request, result });
        }
    
        results
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub mod mockblk;