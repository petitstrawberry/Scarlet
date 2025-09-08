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

use alloc::{boxed::Box, vec::Vec, collections::VecDeque};
use alloc::vec;
use spin::{Mutex, RwLock};

use core::{mem, ptr};

use crate::defer;
use crate::device::{Device, DeviceType};
use crate::drivers::virtio::features::{VIRTIO_F_ANY_LAYOUT, VIRTIO_RING_F_EVENT_IDX, VIRTIO_RING_F_INDIRECT_DESC};
use crate::object::capability::MemoryMappingOps;
use crate::{
    device::block::{request::{BlockIORequest, BlockIORequestType, BlockIOResult}, BlockDevice}, 
    drivers::virtio::{device::VirtioDevice, queue::{DescriptorFlag, VirtQueue}}, object::capability::ControlOps
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

pub struct VirtioBlockDevice {
    base_addr: usize,
    virtqueues: Mutex<[VirtQueue<'static>; 1]>, // Only one queue for request/response
    capacity: RwLock<u64>,
    sector_size: RwLock<u32>,
    features: RwLock<u32>,
    read_only: RwLock<bool>,
    request_queue: Mutex<VecDeque<Box<BlockIORequest>>>,
}

impl VirtioBlockDevice {
    pub fn new(base_addr: usize) -> Self {
        let mut device = Self {
            base_addr,
            // Minimal but sufficient queue size based on real usage:
            // - Average batch: 1.15 requests (85.2% are single requests)
            // - Observed max: <5 requests per batch typically
            // - Each request uses 3 descriptors (header + data + status)  
            // 32 descriptors = ~10 concurrent requests (5x typical usage)
            virtqueues: Mutex::new([VirtQueue::new(32)]),
            capacity: RwLock::new(0),
            sector_size: RwLock::new(512), // Default sector size
            features: RwLock::new(0),
            read_only: RwLock::new(false),
            request_queue: Mutex::new(VecDeque::new()),
        };
        
        // Initialize the device
        let negotiated_features = match device.init() {
            Ok(features) => features,
            Err(_) => panic!("Failed to initialize Virtio Block Device"),
        };

        // Read device configuration
        *device.capacity.write() = device.read_config::<u64>(0); // Capacity at offset 0

        // Store negotiated features
        *device.features.write() = negotiated_features;


        // Debug: Check actual negotiated features after init
        #[cfg(test)]
        {
            use crate::early_println;
            early_println!("[virtio-blk] Final negotiated features (after init): 0x{:x}", 
            negotiated_features);
        }
        
        // Check if block size feature is supported
        if negotiated_features & (1 << VIRTIO_BLK_F_BLK_SIZE) != 0 {
            *device.sector_size.write() = device.read_config::<u32>(20); // blk_size at offset 20
        }
        
        // Check if device is read-only
        *device.read_only.write() = negotiated_features & (1 << VIRTIO_BLK_F_RO) != 0;

        device
    }
    
    fn process_request(&self, req: &mut BlockIORequest) -> Result<(), &'static str> {
        crate::profile_scope!("virtio_blk::process_request");
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
        
        // Lock the virtqueues for processing
        let mut virtqueues = self.virtqueues.lock();
        
        // Allocate descriptors for the request
        let header_desc = virtqueues[0].alloc_desc().ok_or("Failed to allocate descriptor")?;
        let data_desc = match virtqueues[0].alloc_desc() {
            Some(desc) => desc,
            None => {
                virtqueues[0].free_desc(header_desc);
                return Err("Failed to allocate descriptor");
            }
        };
        let status_desc = match virtqueues[0].alloc_desc() {
            Some(desc) => desc,
            None => {
                virtqueues[0].free_desc(data_desc);
                virtqueues[0].free_desc(header_desc);
                return Err("Failed to allocate descriptor");
            }
        };
        
        // Set up header descriptor
        virtqueues[0].desc[header_desc].addr = (header_ptr as usize) as u64;
        virtqueues[0].desc[header_desc].len = mem::size_of::<VirtioBlkReqHeader>() as u32;
        virtqueues[0].desc[header_desc].flags = DescriptorFlag::Next as u16;
        virtqueues[0].desc[header_desc].next = data_desc as u16;
        
        // Set up data descriptor
        virtqueues[0].desc[data_desc].addr = (data_ptr as *mut u8 as usize) as u64;
        virtqueues[0].desc[data_desc].len = req.buffer.len() as u32;
        
        // Set flags based on request type
        match req.request_type {
            BlockIORequestType::Read => {
                DescriptorFlag::Next.set(&mut virtqueues[0].desc[data_desc].flags);
                DescriptorFlag::Write.set(&mut virtqueues[0].desc[data_desc].flags);
            },
            BlockIORequestType::Write => {
                DescriptorFlag::Next.set(&mut virtqueues[0].desc[data_desc].flags);
            }
        }
        
        virtqueues[0].desc[data_desc].next = status_desc as u16;
        
        // Set up status descriptor
        virtqueues[0].desc[status_desc].addr = (status_ptr as usize) as u64;
        virtqueues[0].desc[status_desc].len = 1;
        virtqueues[0].desc[status_desc].flags |= DescriptorFlag::Write as u16;
        
        // Submit the request to the queue
        if let Err(e) = virtqueues[0].push(header_desc) {
            // Free all descriptors if push fails
            virtqueues[0].free_desc(status_desc);
            virtqueues[0].free_desc(data_desc);
            virtqueues[0].free_desc(header_desc);
            return Err(e);
        }

        // Notify the device
        self.notify(0);
        
        // Wait for the response (polling)
        while virtqueues[0].is_busy() {}

        // Process completed request
        let desc_idx = match virtqueues[0].pop() {
            Some(idx) => idx,
            None => {
                // Free descriptors even if pop fails
                virtqueues[0].free_desc(status_desc);
                virtqueues[0].free_desc(data_desc);
                virtqueues[0].free_desc(header_desc);
                return Err("No response from device");
            }
        };
        
        if desc_idx != header_desc {
            // Free descriptors before returning error
            virtqueues[0].free_desc(status_desc);
            virtqueues[0].free_desc(data_desc);
            virtqueues[0].free_desc(header_desc);
            return Err("Invalid descriptor index");
        }
        
        // Check status
        let status_val = unsafe { *status_ptr };
        let result = match status_val {
            VIRTIO_BLK_S_OK => {
                // For read requests, copy data to the buffer
                if let BlockIORequestType::Read = req.request_type {
                    unsafe {
                        req.buffer.clear();
                        req.buffer.extend_from_slice(core::slice::from_raw_parts(
                            data_ptr as *const u8,
                            virtqueues[0].desc[data_desc].len as usize
                        ));
                    }
                }
                Ok(())
            },
            VIRTIO_BLK_S_IOERR => Err("I/O error"),
            VIRTIO_BLK_S_UNSUPP => Err("Unsupported request"),
            _ => Err("Unknown error"),
        };
        
        // Free descriptors after processing (responsibility of driver)
        virtqueues[0].free_desc(status_desc);
        virtqueues[0].free_desc(data_desc);
        virtqueues[0].free_desc(header_desc);
        
        result
    }

    /// Process multiple requests in a true batch manner
    /// All requests are submitted first, then we wait for all completions
    fn process_requests_batch(&self, requests: &mut [Box<BlockIORequest>]) -> Vec<Result<(), &'static str>> {
        crate::profile_scope!("virtio_blk::process_requests_batch");
        
        if requests.is_empty() {
            return Vec::new();
        }

        // Safety check: Limit batch size to prevent virtqueue overflow
        // Based on real usage: avg 1.15 requests/batch, 85.2% single requests
        // Each request uses 3 descriptors, queue has 32 descriptors
        // Conservative limit: 10 requests = 30 descriptors (2 descriptors reserved)
        const MAX_BATCH_SIZE: usize = 10;
        
        if requests.len() > MAX_BATCH_SIZE {
            crate::early_println!("[virtio_blk] WARNING: Batch size {} exceeds safe limit {}, processing in chunks", 
                requests.len(), MAX_BATCH_SIZE);
                
            // Process in chunks
            let mut all_results = Vec::with_capacity(requests.len());
            let chunks = requests.chunks_mut(MAX_BATCH_SIZE);
            
            for chunk in chunks {
                let mut chunk_results = self.process_requests_batch(chunk);
                all_results.append(&mut chunk_results);
            }
            
            return all_results;
        }

        // Debug: Log batch size with read/write breakdown
        let read_count = requests.iter().filter(|r| matches!(r.request_type, BlockIORequestType::Read)).count();
        let write_count = requests.iter().filter(|r| matches!(r.request_type, BlockIORequestType::Write)).count();
        
   
        #[cfg(test)]
        {
            // Add batch size tracking for debugging
            static BATCH_SIZES: spin::Mutex<alloc::vec::Vec<usize>> = spin::Mutex::new(alloc::vec::Vec::new());
            static CALL_COUNT: spin::Mutex<usize> = spin::Mutex::new(0);
            let mut sizes = BATCH_SIZES.lock();
            let mut count = CALL_COUNT.lock();
            sizes.push(requests.len());
            *count += 1;
            
            // Print statistics every 100 calls
            if *count % 100 == 0 {
                let total_requests: usize = sizes.iter().sum();
                let avg_batch_size = total_requests as f64 / sizes.len() as f64;
                let single_requests = sizes.iter().filter(|&&size| size == 1).count();
                crate::early_println!("[virtio_blk] Batch stats: {} calls, avg_batch={:.2}, single_req={}/{} ({:.1}%)", 
                    sizes.len(), avg_batch_size, single_requests, sizes.len(),
                    (single_requests as f64 / sizes.len() as f64) * 100.0);
            }
        }
        
        let batch_size = requests.len();
        let mut results = vec![Err("Not processed"); batch_size];
        let mut request_data = Vec::new();
        
        // Lock the virtqueues for the entire batch
        let mut virtqueues = self.virtqueues.lock();
        
        // First pass: Submit all requests
        for (idx, req) in requests.iter_mut().enumerate() {
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
            
            let header_ptr = Box::into_raw(header);
            let data_ptr = Box::into_raw(data) as *mut [u8];
            let status_ptr = Box::into_raw(status);
            
            // Copy data for write requests
            if let BlockIORequestType::Write = req.request_type {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        req.buffer.as_ptr(),
                        data_ptr as *mut u8,
                        req.buffer.len()
                    );
                }
            }
            
            // Try to allocate descriptors
            if let (Some(header_desc), Some(data_desc), Some(status_desc)) = (
                virtqueues[0].alloc_desc(),
                virtqueues[0].alloc_desc(),
                virtqueues[0].alloc_desc(),
            ) {
                // Set up descriptors
                virtqueues[0].desc[header_desc].addr = (header_ptr as usize) as u64;
                virtqueues[0].desc[header_desc].len = mem::size_of::<VirtioBlkReqHeader>() as u32;
                virtqueues[0].desc[header_desc].flags = DescriptorFlag::Next as u16;
                virtqueues[0].desc[header_desc].next = data_desc as u16;
                
                virtqueues[0].desc[data_desc].addr = (data_ptr as *mut u8 as usize) as u64;
                virtqueues[0].desc[data_desc].len = req.buffer.len() as u32;
                
                match req.request_type {
                    BlockIORequestType::Read => {
                        DescriptorFlag::Next.set(&mut virtqueues[0].desc[data_desc].flags);
                        DescriptorFlag::Write.set(&mut virtqueues[0].desc[data_desc].flags);
                    },
                    BlockIORequestType::Write => {
                        DescriptorFlag::Next.set(&mut virtqueues[0].desc[data_desc].flags);
                    }
                }
                
                virtqueues[0].desc[data_desc].next = status_desc as u16;
                
                virtqueues[0].desc[status_desc].addr = (status_ptr as usize) as u64;
                virtqueues[0].desc[status_desc].len = 1;
                virtqueues[0].desc[status_desc].flags |= DescriptorFlag::Write as u16;
                
                // Submit the request
                if virtqueues[0].push(header_desc).is_ok() {
                    request_data.push((idx, header_desc, data_desc, status_desc, header_ptr, data_ptr, status_ptr));
                } else {
                    // Clean up on push failure
                    virtqueues[0].free_desc(status_desc);
                    virtqueues[0].free_desc(data_desc);
                    virtqueues[0].free_desc(header_desc);
                    unsafe {
                        drop(Box::from_raw(header_ptr));
                        drop(Box::from_raw(data_ptr));
                        drop(Box::from_raw(status_ptr));
                    }
                    results[idx] = Err("Failed to submit request");
                }
            } else {
                // Descriptor allocation failure - should be very rare with 256 queue size
                crate::early_println!("[virtio_blk] ERROR: Failed to allocate descriptors for request {} (batch size: {})", 
                    idx, batch_size);
                    
                // Clean up on descriptor allocation failure
                unsafe {
                    drop(Box::from_raw(header_ptr));
                    drop(Box::from_raw(data_ptr));
                    drop(Box::from_raw(status_ptr));
                }
                results[idx] = Err("Virtqueue descriptor allocation failed - queue may be full");
            }
        }
        
        // Notify the device once for all requests
        if !request_data.is_empty() {
            self.notify(0);
        }
        
        // Second pass: Wait for all completions (true batch processing)
        use alloc::collections::BTreeMap;
        let mut pending_requests: BTreeMap<usize, (usize, usize, usize, *mut VirtioBlkReqHeader, *mut [u8], *mut u8)> = BTreeMap::new();
        
        // Map descriptor IDs to request data
        for (req_idx, header_desc, data_desc, status_desc, header_ptr, data_ptr, status_ptr) in request_data {
            pending_requests.insert(header_desc, (req_idx, data_desc, status_desc, header_ptr, data_ptr, status_ptr));
        }
        
        // Process all completions until everything is done
        while !pending_requests.is_empty() {
            // Wait for something to complete
            while virtqueues[0].is_busy() {}
            
            // Process all completed requests in this round
            while let Some(desc_idx) = virtqueues[0].pop() {
                if let Some((req_idx, data_desc, status_desc, header_ptr, data_ptr, status_ptr)) = pending_requests.remove(&desc_idx) {
                    // Check status
                    let status_val = unsafe { *status_ptr };
                    results[req_idx] = match status_val {
                        VIRTIO_BLK_S_OK => {
                            // For read requests, copy data back to the buffer
                            if let BlockIORequestType::Read = requests[req_idx].request_type {
                                unsafe {
                                    requests[req_idx].buffer.clear();
                                    requests[req_idx].buffer.extend_from_slice(core::slice::from_raw_parts(
                                        data_ptr as *const u8,
                                        virtqueues[0].desc[data_desc].len as usize
                                    ));
                                }
                            }
                            Ok(())
                        },
                        VIRTIO_BLK_S_IOERR => Err("I/O error"),
                        VIRTIO_BLK_S_UNSUPP => Err("Unsupported request"),
                        _ => Err("Unknown error"),
                    };
                    
                    // Clean up descriptors and memory for this completed request
                    virtqueues[0].free_desc(status_desc);
                    virtqueues[0].free_desc(data_desc);
                    virtqueues[0].free_desc(desc_idx); // header_desc
                    unsafe {
                        drop(Box::from_raw(header_ptr));
                        drop(Box::from_raw(data_ptr));
                        drop(Box::from_raw(status_ptr));
                    }
                } else {
                    // Unexpected descriptor - this shouldn't happen but handle gracefully
                    crate::early_println!("[virtio-blk] Warning: Unexpected descriptor completion: {}", desc_idx);
                }
            }
        }
        
        results
    }
}

impl MemoryMappingOps for VirtioBlockDevice {
    fn get_mapping_info(&self, _offset: usize, _length: usize) 
                       -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by VirtIO block device")
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // VirtIO block devices don't support memory mapping
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // VirtIO block devices don't support memory mapping
    }
    
    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Device for VirtioBlockDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }
    
    fn name(&self) -> &'static str {
        "virtio-blk"
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
    
    fn into_block_device(self: alloc::sync::Arc<Self>) -> Option<alloc::sync::Arc<dyn crate::device::block::BlockDevice>> {
        Some(self)
    }
}

impl VirtioDevice for VirtioBlockDevice {
    fn get_base_addr(&self) -> usize {
        self.base_addr
    }
    
    fn get_virtqueue_count(&self) -> usize {
        1 // We have one virtqueue
    }

    fn get_virtqueue_size(&self, queue_idx: usize) -> usize {
        if queue_idx >= 1 {
            panic!("Invalid queue index for VirtIO block device: {}", queue_idx);
        }
        
        let virtqueues = self.virtqueues.lock();
        virtqueues[queue_idx].get_queue_size()
    }
    
    fn get_supported_features(&self, device_features: u32) -> u32 {
        // Accept most features but we might want to be selective
        device_features & !(1 << VIRTIO_BLK_F_RO |
            1 << VIRTIO_BLK_F_SCSI |
            1 << VIRTIO_BLK_F_CONFIG_WCE |
            1 << VIRTIO_BLK_F_MQ |
            1 << VIRTIO_F_ANY_LAYOUT |
            1 << VIRTIO_RING_F_EVENT_IDX |
            1 << VIRTIO_RING_F_INDIRECT_DESC)
    }
    
    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= 1 {
            return None;
        }
        
        let virtqueues = self.virtqueues.lock();
        Some(virtqueues[queue_idx].get_raw_ptr() as u64)
    }
    
    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= 1 {
            return None;
        }
        
        let virtqueues = self.virtqueues.lock();
        Some(virtqueues[queue_idx].avail.flags as *const _ as u64)
    }
    
    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= 1 {
            return None;
        }
        
        let virtqueues = self.virtqueues.lock();
        Some(virtqueues[queue_idx].used.flags as *const _ as u64)
    }
}

impl BlockDevice for VirtioBlockDevice {
    fn get_disk_name(&self) -> &'static str {
        "virtio-blk"
    }
    
    fn get_disk_size(&self) -> usize {
        let capacity = *self.capacity.read();
        let sector_size = *self.sector_size.read();
        (capacity * sector_size as u64) as usize
    }
    
    fn enqueue_request(&self, request: Box<BlockIORequest>) {
        // Enqueue the request
        self.request_queue.lock().push_back(request);
    }
    
    fn process_requests(&self) -> Vec<BlockIOResult> {
        crate::profile_scope!("virtio_blk::process_requests");
        let mut queue = self.request_queue.lock();
        
        // Collect all requests first
        let mut requests = Vec::new();
        while let Some(request) = queue.pop_front() {
            requests.push(request);
        }
        drop(queue); // Release the lock early
        
        if requests.is_empty() {
            return Vec::new();
        }
        
        // Process all requests in true batch
        let batch_results = self.process_requests_batch(&mut requests);
        
        // Convert results back to the expected format
        requests.into_iter()
            .zip(batch_results.into_iter())
            .map(|(request, result)| BlockIOResult { request, result })
            .collect()
    }
}

impl ControlOps for VirtioBlockDevice {
    // VirtIO block devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
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
        
        assert_eq!(device.get_disk_name(), "virtio-blk");
        assert_eq!(device.get_disk_size(), (*device.capacity.read() * *device.sector_size.read() as u64) as usize);
    }
    
    #[test_case]
    fn test_virtio_block_device() {
        let base_addr = 0x10001000; // Example base address
        let device = VirtioBlockDevice::new(base_addr);
        
        assert_eq!(device.get_disk_name(), "virtio-blk");
        assert_eq!(device.get_disk_size(), (*device.capacity.read() * *device.sector_size.read() as u64) as usize);
        
        // Test enqueue and process requests
        let sector_size = *device.sector_size.read();
        let request = BlockIORequest {
            request_type: BlockIORequestType::Read,
            sector: 0,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0; sector_size as usize],
        };
        device.enqueue_request(Box::new(request));
        
        let results = device.process_requests();
        assert_eq!(results.len(), 1);

        let result = &results[0];
        assert!(result.result.is_ok());

        // Test that we can read data from the device (without assuming specific content)
        let buffer = &result.request.buffer;
        assert_eq!(buffer.len(), sector_size as usize);
        
        // For FAT32 filesystem, we should at least check the boot sector signature
        if buffer.len() >= 512 {
            // Check FAT32 boot sector signature at bytes 510-511
            assert_eq!(buffer[510], 0x55);
            assert_eq!(buffer[511], 0xAA);
        }
    }
}