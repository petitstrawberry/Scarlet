mod disk;

use alloc::vec;

use super::*;
use crate::{device::block::request::BlockIORequest, println, print};

fn dummy_request_fn(_request: &mut BlockIORequest) -> Result<(), &'static str> {
    Ok(())
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
    let request = Box::new(BlockIORequest {
        request_type: request::BlockIORequestType::Read,
        sector: 0,
        sector_count: 1,
        head: 0,
        cylinder: 0,
        buffer: vec![0; 512],
    });
    device.enqueue_request(request);
    assert_eq!(device.request_queue.len(), 1);
}

#[test_case]
fn test_block_device_process_requests() {
    let mut device = BlockDevice::new(1, "test_disk", 1024, dummy_request_fn);
    let request = Box::new(BlockIORequest {
        request_type: request::BlockIORequestType::Read,
        sector: 0,
        sector_count: 1,
        head: 0,
        cylinder: 0,
        buffer: vec![0; 512],
    });
    device.enqueue_request(request);
    let results = device.process_requests();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].result, Ok(()));
}

#[test_case]
fn test_read_write() {
    let mut device = disk::TestDisk::get_device();
    let request = Box::new(BlockIORequest {
        request_type: request::BlockIORequestType::Write,
        sector: 0,
        sector_count: 1,
        head: 0,
        cylinder: 0,
        buffer: vec![0xff; 512],
    });
    device.enqueue_request(request);
    let results = device.process_requests();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].result, Ok(()));


    let read_request = Box::new(BlockIORequest {
        request_type: request::BlockIORequestType::Read,
        sector: 0,
        sector_count: 1,
        head: 0,
        cylinder: 0,
        buffer: vec![0; 512],
    });
    device.enqueue_request(read_request);
    let results = device.process_requests();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].result, Ok(()));

    let read_request = &results[0].request;

    for i in 0..512 {
        assert_eq!(read_request.buffer[i], 0xff);
    }
    
}