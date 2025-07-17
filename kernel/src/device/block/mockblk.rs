use core::any::Any;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use super::request::BlockIORequestType;
use super::*;
use crate::device::block::request::BlockIOResult;
use crate::device::{Device, DeviceType};
use crate::object::capability::ControlOps;

// Mock block device
pub struct MockBlockDevice {
    disk_name: &'static str,
    disk_size: usize,
    data: Mutex<Vec<Vec<u8>>>,
    request_queue: Mutex<Vec<Box<BlockIORequest>>>,
}

impl MockBlockDevice {
    pub fn new(disk_name: &'static str, sector_size: usize, sector_count: usize) -> Self {
        let mut data = Vec::with_capacity(sector_count);
        for _ in 0..sector_count {
            data.push(vec![0; sector_size]);
        }
        
        Self {
            disk_name,
            disk_size: sector_size * sector_count,
            data: Mutex::new(data),
            request_queue: Mutex::new(Vec::new()),
        }
    }
}

impl Device for MockBlockDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn name(&self) -> &'static str {
        "MockBlockDevice"
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
}


impl BlockDevice for MockBlockDevice {
    fn get_disk_name(&self) -> &'static str {
        self.disk_name
    }
    
    fn get_disk_size(&self) -> usize {
        self.disk_size
    }
    
    fn enqueue_request(&self, request: Box<BlockIORequest>) {
        self.request_queue.lock().push(request);
    }
    
    /// Process all queued block I/O requests
    /// 
    /// This method processes all pending requests using a deadlock-safe approach:
    /// 
    /// 1. Extracts all requests at once using mem::replace
    /// 2. Processes requests without holding the request_queue lock
    /// 3. Acquires data lock only when needed for each request
    /// 
    /// This prevents deadlocks by:
    /// - Never holding multiple locks simultaneously
    /// - Minimizing lock hold time
    /// - Using a consistent lock ordering
    /// 
    /// # Returns
    /// Vector of `BlockIOResult` containing completed requests and their results
    fn process_requests(&self) -> Vec<BlockIOResult> {
        let mut results = Vec::new();
        
        // Extract all requests at once to minimize lock time
        let requests = {
            let mut queue = self.request_queue.lock();
            core::mem::replace(&mut *queue, Vec::new())
        }; // request_queue lock is automatically released here
        
        // Process all requests without holding the request_queue lock
        for mut request in requests {
            let result = match request.request_type {
                BlockIORequestType::Read => {
                    let sector = request.sector;
                    // Acquire data lock only for this operation
                    let data = self.data.lock();
                    if sector < data.len() {
                        request.buffer = data[sector].clone();
                        Ok(())
                    } else {
                        Err("Invalid sector")
                    }
                    // data lock is automatically released here
                },
                BlockIORequestType::Write => {
                    let sector = request.sector;
                    // Acquire data lock only for this operation
                    let mut data = self.data.lock();
                    if sector < data.len() {
                        let buffer_len = request.buffer.len();
                        let sector_len = data[sector].len();
                        let len = buffer_len.min(sector_len);
                        
                        data[sector][..len].copy_from_slice(&request.buffer[..len]);
                        Ok(())
                    } else {
                        Err("Invalid sector")
                    }
                    // data lock is automatically released here
                }
            };
            
            results.push(BlockIOResult {
                request,
                result,
            });
        }
        
        results
    }
}

impl ControlOps for MockBlockDevice {
    // Mock block devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}
