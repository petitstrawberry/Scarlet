use alloc::vec::Vec;
use request::BlockIORequest;

pub mod request;
pub mod manager;

extern crate alloc;

pub struct BlockDevice {
    id: usize,
    disk_name: &'static str,
    disk_size: usize,
    request_fn: fn(&mut BlockIORequest) -> Result<(), &'static str>,
    request_queue: Vec<request::BlockIORequest>,
}

impl BlockDevice {
    pub fn new(id: usize, disk_name: &'static str, disk_size: usize, request_fn: fn(&mut BlockIORequest) -> Result<(), &'static str>) -> Self {
        Self { id, disk_name, disk_size, request_fn, request_queue: Vec::new() }
    }

    pub fn get_id(&self) -> usize {
        self.id
    }

    pub fn get_disk_name(&self) -> &'static str {
        self.disk_name
    }

    pub fn get_disk_size(&self) -> usize {
        self.disk_size
    }

    pub fn add_request(&mut self, request: request::BlockIORequest) {
        self.request_queue.push(request);
    }


    pub fn process_requests(&mut self) -> Result<(), &'static str> {
        for request in &mut self.request_queue {
            // Check if the request is valid
            if request.sector >= self.disk_size {
                return Err("Invalid sector");
            }
            (self.request_fn)(request)?;
        }
        self.request_queue.clear();
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::device::block::request::BlockIORequest;

    fn dummy_request_fn(_request: &mut BlockIORequest) -> Result<(), &'static str> {
        Ok(())
    }

    fn failure_request_fn(_request: &mut BlockIORequest) -> Result<(), &'static str> {
        Err("Request failed")
    }

    #[test_case]
    fn test_block_device_creation() {
        let device = BlockDevice::new(1, "test_disk", 1024, dummy_request_fn);
        assert_eq!(device.get_id(), 1);
        assert_eq!(device.get_disk_name(), "test_disk");
        assert_eq!(device.get_disk_size(), 1024);
        assert_eq!(device.request_queue.len(), 0);
    }

    #[test_case]
    fn test_block_device_add_request() {
        let mut device = BlockDevice::new(1, "test_disk", 1024, dummy_request_fn);
        let request = BlockIORequest {
            request_type: request::BlockIORequestType::Read,
            sector: 0,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0; 512],
        };
        device.add_request(request);
        assert_eq!(device.request_queue.len(), 1);
    }

    #[test_case]
    fn test_block_device_process_requests() {
        let mut device = BlockDevice::new(1, "test_disk", 1024, dummy_request_fn);
        let request = BlockIORequest {
            request_type: request::BlockIORequestType::Read,
            sector: 0,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0; 512],
        };
        device.add_request(request);
        assert_eq!(device.process_requests(), Ok(()));
        assert_eq!(device.request_queue.len(), 0);
    }

    #[test_case]
    fn test_invalid_sector() {
        let mut device = BlockDevice::new(1, "test_disk", 1024, dummy_request_fn);
        let request = BlockIORequest {
            request_type: request::BlockIORequestType::Read,
            sector: 1024, // Invalid sector
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0; 512],
        };
        device.add_request(request);
        assert_eq!(device.process_requests(), Err("Invalid sector"));
    }
}