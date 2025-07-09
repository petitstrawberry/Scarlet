use core::any::Any;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use super::request::BlockIORequestType;
use super::*;
use crate::device::block::request::BlockIOResult;
use crate::device::{Device, DeviceType};

// Mock block device
pub struct MockBlockDevice {
    id: usize,
    disk_name: &'static str,
    disk_size: usize,
    data: Mutex<Vec<Vec<u8>>>,
    request_queue: Mutex<Vec<Box<BlockIORequest>>>,
}

impl MockBlockDevice {
    pub fn new(id: usize, disk_name: &'static str, sector_size: usize, sector_count: usize) -> Self {
        let mut data = Vec::with_capacity(sector_count);
        for _ in 0..sector_count {
            data.push(vec![0; sector_size]);
        }
        
        Self {
            id,
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

    fn id(&self) -> usize {
        self.id
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
    fn get_id(&self) -> usize {
        self.id
    }
    
    fn get_disk_name(&self) -> &'static str {
        self.disk_name
    }
    
    fn get_disk_size(&self) -> usize {
        self.disk_size
    }
    
    fn enqueue_request(&self, request: Box<BlockIORequest>) {
        self.request_queue.lock().push(request);
    }
    
    fn process_requests(&self) -> Vec<BlockIOResult> {
        let mut results = Vec::new();
        let requests = {
            let mut queue = self.request_queue.lock();
            core::mem::replace(&mut *queue, Vec::new())
        };
        
        for mut request in requests {
            let result = match request.request_type {
                BlockIORequestType::Read => {
                    let sector = request.sector;
                    let data = self.data.lock();
                    if sector < data.len() {
                        request.buffer = data[sector].clone();
                        Ok(())
                    } else {
                        Err("Invalid sector")
                    }
                },
                BlockIORequestType::Write => {
                    let sector = request.sector;
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
