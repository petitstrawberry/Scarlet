//! # VirtIO Block Device Driver
//! 
//! This module provides a driver for VirtIO block devices, implementing the
//! BlockDevice trait for integration with the kernel's block device subsystem.
//!
//! The driver supports basic block operations (read/write) and handles the VirtIO
//! queue management for block device requests.
//!
//! ## Features Support
//! 
//! The driver checks for and handles the following VirtIO block device features:
//! - `VIRTIO_BLK_F_BLK_SIZE`: Custom sector size
//! - `VIRTIO_BLK_F_RO`: Read-only device detection
//!
//! ## Implementation Details
//!
//! The driver uses a single virtqueue for processing block I/O requests. Each request
//! consists of three parts:
//! 1. Request header (specifying operation type and sector)
//! 2. Data buffer (for read/write content)
//! 3. Status byte (for operation result)
//!
//! Requests are processed through the VirtIO descriptor chain mechanism, with proper
//! memory management using Box allocations to ensure data remains valid during transfers.

use alloc::{boxed::Box, vec::Vec, sync::Arc};
use alloc::vec;
use spin::{RwLock, Mutex};

use core::{mem, ptr};

use crate::defer;
use crate::device::{Device, DeviceType};
use crate::{
    device::block::{request::{BlockIORequest, BlockIORequestType, BlockIOResult}, BlockDevice}, drivers::virtio::{device::{Register, VirtioDevice}, queue::{DescriptorFlag, VirtQueue}}
};

// VirtIO Block Request Type
const VIRTIO_BLK_T_IN: u32 = 0;     // Read
const VIRTIO_BLK_T_OUT: u32 = 1;    // Write
// const VIRTIO_BLK_T_FLUSH: u32 = 4;  // Flush

// VirtIO Block Status Codes
const VIRTIO_BLK_S_OK: u8 = 0;
const VIRTIO_BLK_S_IOERR: u8 = 1;
const VIRTIO_BLK_S_UNSUPP: u8 = 2;

// Device Feature bits
// const VIRTIO_BLK_F_SIZE_MAX: u32 = 1;
// const VIRTIO_BLK_F_SEG_MAX: u32 = 2;
// const VIRTIO_BLK_F_GEOMETRY: u32 = 4;
const VIRTIO_BLK_F_RO: u32 = 5;
const VIRTIO_BLK_F_BLK_SIZE: u32 = 6;
const VIRTIO_BLK_F_SCSI: u32 = 7;
// const VIRTIO_BLK_F_FLUSH: u32 = 9;
const VIRTIO_BLK_F_CONFIG_WCE: u32 = 11;
const VIRTIO_BLK_F_MQ: u32 = 12;
const VIRTIO_F_ANY_LAYOUT: u32 = 27;
const VIRTIO_RING_F_INDIRECT_DESC: u32 = 28;
const VIRTIO_RING_F_EVENT_IDX: u32 = 29;

// #define VIRTIO_BLK_F_RO              5	/* Disk is read-only */
// #define VIRTIO_BLK_F_SCSI            7	/* Supports scsi command passthru */
// #define VIRTIO_BLK_F_CONFIG_WCE     11	/* Writeback mode available in config */
// #define VIRTIO_BLK_F_MQ             12	/* support more than one vq */
// #define VIRTIO_F_ANY_LAYOUT         27
// #define VIRTIO_RING_F_INDIRECT_DESC 28
// #define VIRTIO_RING_F_EVENT_IDX     29

#[repr(C)]
pub struct VirtioBlkConfig {
    pub capacity: u64,
    pub size_max: u32,
    pub seg_max: u32,
    pub geometry: VirtioBlkGeometry,
    pub blk_size: u32,
    pub topology: VirtioBlkTopology,
    pub writeback: u8,
}

#[repr(C)]
pub struct VirtioBlkGeometry {
    pub cylinders: u16,
    pub heads: u8,
    pub sectors: u8,
}

#[repr(C)]
pub struct VirtioBlkTopology {
    pub physical_block_exp: u8,
    pub alignment_offset: u8,
    pub min_io_size: u16,
    pub opt_io_size: u32,
}

#[repr(C)]
pub struct VirtioBlkReqHeader {
    pub type_: u32,
    pub reserved: u32,
    pub sector: u64,
}

pub struct VirtioBlockDeviceInner {
    base_addr: usize,
    virtqueues: [VirtQueue<'static>; 1], // Only one queue for request/response
    capacity: u64,
    sector_size: u32,
    features: u32,
    read_only: bool,
}

pub struct VirtioBlockDevice {
    inner: Arc<Mutex<VirtioBlockDeviceInner>>,
    request_queue: Arc<Mutex<Vec<Box<BlockIORequest>>>>,
}

impl VirtioBlockDevice {
    pub fn new(base_addr: usize) -> Self {
        let inner = VirtioBlockDeviceInner {
            base_addr,
            virtqueues: [VirtQueue::new(8)],
            capacity: 0,
            sector_size: 512, // Default sector size
            features: 0,
            read_only: false,
        };
        
        let mut device = Self {
            inner: Arc::new(Mutex::new(inner)),
            request_queue: Arc::new(Mutex::new(Vec::new())),
        };
        
        // Initialize the device
        if device.init().is_err() {
            panic!("Failed to initialize Virtio Block Device");
        }

        // Read device configuration - avoid deadlock by getting base_addr first
        let base_addr = device.get_base_addr();
        {
            let mut inner = device.inner.lock();
            
            // Read capacity directly to avoid deadlock
            let capacity_addr = base_addr + Register::DeviceConfig.offset() + 0;
            inner.capacity = unsafe { core::ptr::read_volatile(capacity_addr as *const u64) };

            // Read device features directly to avoid deadlock
            let features_addr = base_addr + Register::DeviceFeatures.offset();
            inner.features = unsafe { core::ptr::read_volatile(features_addr as *const u32) };
            inner.sector_size = 512; // Default sector size
        
            // Check if block size feature is supported
            if inner.features & (1 << VIRTIO_BLK_F_BLK_SIZE) != 0 {
                // Read blk_size directly to avoid deadlock
                let blk_size_addr = base_addr + Register::DeviceConfig.offset() + 20;
                inner.sector_size = unsafe { core::ptr::read_volatile(blk_size_addr as *const u32) };
            }
        
            // Check if device is read-only
            inner.read_only = inner.features & (1 << VIRTIO_BLK_F_RO) != 0;
        }

        device
    }
    
    fn process_request_internal(&self, req: &mut BlockIORequest) -> Result<(), &'static str> {
        // We need to ensure that we don't lock self.inner again if it's already locked
        // To avoid this, we'll implement the logic directly without using with_virtqueue_mut
        self.process_request_with_inner_lock(req)
    }

    fn process_request_with_inner_lock(&self, req: &mut BlockIORequest) -> Result<(), &'static str> {
        // Allocate memory for request header, data, and status
        let header = Box::new(VirtioBlkReqHeader {
            type_: match req.request_type {
                BlockIORequestType::Read => VIRTIO_BLK_T_IN,
                BlockIORequestType::Write => VIRTIO_BLK_T_OUT,
            },
            reserved: 0,
            sector: req.sector as u64,
        });
        let data = vec![0u8; req.buffer.len()].into_boxed_slice();
        let status = Box::new(0u8);        
        // Cast pages to appropriate types
        let header_ptr = Box::into_raw(header);
        let data_ptr = Box::into_raw(data) as *mut [u8];
        let status_ptr = Box::into_raw(status);

        defer! {
            // Deallocate memory after use
            unsafe {
                drop(Box::from_raw(header_ptr));
                drop(Box::from_raw(data_ptr));
                drop(Box::from_raw(status_ptr));
            }
        }

        // Set up request header
        unsafe {
            // Copy data for write requests
            if let BlockIORequestType::Write = req.request_type {
                ptr::copy_nonoverlapping(
                    req.buffer.as_ptr(),
                    data_ptr as *mut u8,
                    req.buffer.len()
                );
            }
        }
        
        // Process the request with a single lock acquisition
        let mut inner = self.inner.lock();
        if inner.virtqueues.is_empty() {
            return Err("No virtqueues available");
        }
        
        let virtqueue = &mut inner.virtqueues[0];
        
        // Allocate descriptors for the request
        let header_desc = virtqueue.alloc_desc().ok_or("Failed to allocate descriptor")?;
        let data_desc = virtqueue.alloc_desc().ok_or("Failed to allocate descriptor")?;
        let status_desc = virtqueue.alloc_desc().ok_or("Failed to allocate descriptor")?;
        
        // Set up header descriptor
        virtqueue.desc[header_desc].addr = (header_ptr as usize) as u64;
        virtqueue.desc[header_desc].len = mem::size_of::<VirtioBlkReqHeader>() as u32;
        virtqueue.desc[header_desc].flags = DescriptorFlag::Next as u16;
        virtqueue.desc[header_desc].next = data_desc as u16;
        
        // Set up data descriptor
        virtqueue.desc[data_desc].addr = (data_ptr as *mut u8 as usize) as u64;
        virtqueue.desc[data_desc].len = req.buffer.len() as u32;
        
        // Set flags based on request type
        match req.request_type {
            BlockIORequestType::Read => {
                DescriptorFlag::Next.set(&mut virtqueue.desc[data_desc].flags);
                DescriptorFlag::Write.set(&mut virtqueue.desc[data_desc].flags);
            },
            BlockIORequestType::Write => {
                DescriptorFlag::Next.set(&mut virtqueue.desc[data_desc].flags);
            }
        }
        
        virtqueue.desc[data_desc].next = status_desc as u16;
        
        // Set up status descriptor
        virtqueue.desc[status_desc].addr = (status_ptr as usize) as u64;
        virtqueue.desc[status_desc].len = 1;
        virtqueue.desc[status_desc].flags |= DescriptorFlag::Write as u16;
        
        // Submit the request to the queue
        virtqueue.push(header_desc)?;

        // Notify the device - need to call parent's notify method
        // self.notify(0);
        
        // Wait for the response (polling)
        while virtqueue.is_busy() {}
        while *virtqueue.used.idx as usize == virtqueue.last_used_idx {}

        // Process completed request
        let desc_idx = virtqueue.pop().ok_or("No response from device")?;
        if desc_idx != header_desc {
            return Err("Invalid descriptor index");
        }
        
        // Check status
        let status_val = unsafe { *status_ptr };
        match status_val {
            VIRTIO_BLK_S_OK => {
                // For read requests, copy data to the buffer
                if let BlockIORequestType::Read = req.request_type {
                    unsafe {
                        req.buffer.clear();
                        req.buffer.extend_from_slice(core::slice::from_raw_parts(
                            data_ptr as *const u8,
                            virtqueue.desc[data_desc].len as usize
                        ));
                    }
                }
                Ok(())
            },
            VIRTIO_BLK_S_IOERR => Err("I/O error"),
            VIRTIO_BLK_S_UNSUPP => Err("Unsupported request"),
            _ => Err("Unknown error"),
        }
    }
}

impl Device for VirtioBlockDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }
    
    fn name(&self) -> &'static str {
        "virtio-blk"
    }
    
    fn id(&self) -> usize {
        self.inner.lock().base_addr
    }
    
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
    
    fn as_block_device(&self) -> Option<&dyn crate::device::block::BlockDevice> {
        Some(self)
    }
}

impl VirtioDevice for VirtioBlockDevice {
    fn get_base_addr(&self) -> usize {
        self.inner.lock().base_addr
    }
    
    fn get_virtqueue_count(&self) -> usize {
        self.inner.lock().virtqueues.len()
    }

    fn with_virtqueue<R>(&self, queue_idx: usize, f: impl FnOnce(&VirtQueue) -> R) -> Option<R> {
        let inner = self.inner.lock();
        if queue_idx < inner.virtqueues.len() {
            Some(f(&inner.virtqueues[queue_idx]))
        } else {
            None
        }
    }
    
    fn with_virtqueue_mut<R>(&self, queue_idx: usize, f: impl FnOnce(&mut VirtQueue) -> R) -> Option<R> {
        let mut inner = self.inner.lock();
        if queue_idx < inner.virtqueues.len() {
            Some(f(&mut inner.virtqueues[queue_idx]))
        } else {
            None
        }
    }
    
    fn get_supported_features(&self, device_features: u32) -> u32 {
        // Accept most features but we might want to be selective
        // device_features
        // NOT Negotiated
        // - VIRTIO_BLK_F_RO
        // - VIRTIO_BLK_F_SCSI
        // - VIRTIO_BLK_F_CONFIG_WCE
        // - VIRTIO_BLK_F_MQ
        // - VIRTIO_F_ANY_LAYOUT
        // - VIRTIO_RING_F_EVENT_IDX
        // - VIRTIO_RING_F_INDIRECT_DESC

        device_features & !(1 << VIRTIO_BLK_F_RO |
            1 << VIRTIO_BLK_F_SCSI |
            1 << VIRTIO_BLK_F_CONFIG_WCE |
            1 << VIRTIO_BLK_F_MQ |
            1 << VIRTIO_F_ANY_LAYOUT |
            1 << VIRTIO_RING_F_EVENT_IDX |
            1 << VIRTIO_RING_F_INDIRECT_DESC)
    }
}

impl BlockDevice for VirtioBlockDevice {
    fn get_id(&self) -> usize {
        self.inner.lock().base_addr // Use base address as ID
    }
    
    fn get_disk_name(&self) -> &'static str {
        "virtio-blk"
    }
    
    fn get_disk_size(&self) -> usize {
        let inner = self.inner.lock();
        (inner.capacity * inner.sector_size as u64) as usize
    }
    
    fn enqueue_request(&self, request: Box<BlockIORequest>) {
        // Enqueue the request
        self.request_queue.lock().push(request);
    }
    
    fn process_requests(&self) -> Vec<BlockIOResult> {
        let mut results = Vec::new();
        let mut queue = self.request_queue.lock();
        while let Some(mut request) = queue.pop() {
            drop(queue); // Release the lock before processing
            let result = self.process_request_internal(&mut *request);
            results.push(BlockIOResult { request, result });
            queue = self.request_queue.lock(); // Reacquire the lock
        }
        
        results
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use alloc::vec;

    #[test_case]
    fn test_virtio_block_device_init() {
        let base_addr = 0x10001000; // Example base address
        let device = VirtioBlockDevice::new(base_addr);
        
        assert_eq!(device.get_id(), base_addr);
        assert_eq!(device.get_disk_name(), "virtio-blk");
        
        // Fix: Use single lock to avoid potential deadlock
        let inner = device.inner.lock();
        let expected_size = (inner.capacity * inner.sector_size as u64) as usize;
        drop(inner);
        assert_eq!(device.get_disk_size(), expected_size);
    }
    
    #[test_case]
    fn test_virtio_block_device() {
        let base_addr = 0x10001000; // Example base address
        let device = VirtioBlockDevice::new(base_addr);
        
        assert_eq!(device.get_id(), base_addr);
        assert_eq!(device.get_disk_name(), "virtio-blk");
        
        // Fix: Use single lock to avoid potential deadlock
        let inner = device.inner.lock();
        let expected_size = (inner.capacity * inner.sector_size as u64) as usize;
        let sector_size = inner.sector_size;
        drop(inner);
        assert_eq!(device.get_disk_size(), expected_size);

        // Test enqueue and process requests
        let request = BlockIORequest {
            request_type: BlockIORequestType::Read,
            sector: 0,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0; sector_size as usize], // Use previously obtained sector_size
        };
        device.enqueue_request(Box::new(request));
        
        let results = device.process_requests();
        assert_eq!(results.len(), 1);

        let result = &results[0];
        assert!(result.result.is_ok());

        // str from buffer (trim \0)
        let buffer = &result.request.buffer;
        let buffer_str = core::str::from_utf8(buffer).unwrap_or("Invalid UTF-8").trim_matches(char::from(0));
        assert_eq!(buffer_str, "Hello, world!");
    }
}