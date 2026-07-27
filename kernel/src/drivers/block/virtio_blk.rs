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

use crate::sync::{IrqRwSpinLock, IrqSpinLock};
use alloc::vec;
use alloc::{boxed::Box, collections::VecDeque, vec::Vec};

use core::{mem, ptr};

use crate::defer;
use crate::device::{Device, DeviceType};
use crate::drivers::virtio::features::{
    VIRTIO_F_ANY_LAYOUT, VIRTIO_RING_F_EVENT_IDX, VIRTIO_RING_F_INDIRECT_DESC,
};
use crate::environment::PAGE_SIZE;
use crate::mem::page::ContiguousPages;
use crate::object::capability::{MemoryMappingOps, Selectable};
use crate::vm::addr::virt_to_phys;
use crate::{
    device::block::{
        BlockDevice,
        request::{BlockIORequest, BlockIORequestType, BlockIOResult},
    },
    drivers::virtio::{
        device::VirtioDevice,
        pci::VirtioPciTransport,
        queue::{DescriptorFlag, VirtQueue},
    },
    object::capability::ControlOps,
};

// VirtIO Block Request Type
const VIRTIO_BLK_T_IN: u32 = 0; // Read
const VIRTIO_BLK_T_OUT: u32 = 1; // Write
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
    pci_transport: Option<VirtioPciTransport>,
    virtqueues: IrqSpinLock<[VirtQueue<'static>; 1]>, // Only one queue for request/response
    capacity: IrqRwSpinLock<u64>,
    sector_size: IrqRwSpinLock<u32>,
    features: IrqRwSpinLock<u64>,
    read_only: IrqRwSpinLock<bool>,
    request_queue: IrqSpinLock<VecDeque<Box<BlockIORequest>>>,
}

impl VirtioBlockDevice {
    pub fn new(base_addr: usize) -> Self {
        Self::new_with_transport(base_addr, None)
    }

    /// Create a VirtIO block device backed by the PCI transport.
    ///
    /// # Arguments
    ///
    /// * `transport` - Mapped VirtIO PCI configuration regions
    ///
    /// # Returns
    ///
    /// A new initialized block device.
    pub fn new_pci(transport: VirtioPciTransport) -> Self {
        Self::new_with_transport(transport.common_cfg, Some(transport))
    }

    fn new_with_transport(base_addr: usize, pci_transport: Option<VirtioPciTransport>) -> Self {
        let mut device = Self {
            base_addr,
            pci_transport,
            // Minimal but sufficient queue size based on real usage:
            // - Average batch: 1.15 requests (85.2% are single requests)
            // - Observed max: <5 requests per batch typically
            // - Each request uses 3 descriptors (header + data + status)
            // 32 descriptors = ~10 concurrent requests (5x typical usage)
            virtqueues: IrqSpinLock::new([VirtQueue::new(32)]),
            capacity: IrqRwSpinLock::new(0),
            sector_size: IrqRwSpinLock::new(512), // Default sector size
            features: IrqRwSpinLock::new(0),
            read_only: IrqRwSpinLock::new(false),
            request_queue: IrqSpinLock::new(VecDeque::new()),
        };

        // Initialize the device
        let negotiated_features = match device.init() {
            Ok(features) => features,
            Err(e) => panic!("Failed to initialize Virtio Block Device: {}", e),
        };

        // Read device configuration
        *device.capacity.write() = device.read_config::<u64>(0); // Capacity at offset 0

        // Store negotiated features
        *device.features.write() = negotiated_features;

        // Debug: Check actual negotiated features after init
        #[cfg(test)]
        {
            use crate::early_println;
            early_println!(
                "[virtio-blk] Final negotiated features (after init): 0x{:x}",
                negotiated_features
            );
        }

        // Check if block size feature is supported
        if negotiated_features & (1u64 << VIRTIO_BLK_F_BLK_SIZE) != 0 {
            *device.sector_size.write() = device.read_config::<u32>(20); // blk_size at offset 20
        }

        // Check if device is read-only
        *device.read_only.write() = negotiated_features & (1u64 << VIRTIO_BLK_F_RO) != 0;

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

        // Allocate data buffer from PMM for DMA
        let data_pages = (req.buffer.len() + PAGE_SIZE - 1) / PAGE_SIZE;
        let data_alloc =
            ContiguousPages::new(data_pages).ok_or("Failed to allocate data buffer")?;

        let status = Box::new(0u8);

        let header_ptr = Box::into_raw(header);
        let data_ptr = data_alloc.as_ptr() as *mut u8;
        let status_ptr = Box::into_raw(status);

        defer! {
            unsafe {
                drop(Box::from_raw(header_ptr));
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
                    req.buffer.len(),
                );
            }
        }

        // Lock the virtqueues for processing
        let mut virtqueues = self.virtqueues.lock();

        // Allocate descriptors for the request
        let header_desc = virtqueues[0]
            .alloc_desc()
            .ok_or("Failed to allocate descriptor")?;
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
        let header_phys = crate::vm::get_kernel_vm_manager()
            .translate_to_phys(header_ptr as usize)
            .ok_or("Failed to translate header vaddr")?;
        virtqueues[0].desc[header_desc].addr = header_phys as u64;
        virtqueues[0].desc[header_desc].len = mem::size_of::<VirtioBlkReqHeader>() as u32;
        virtqueues[0].desc[header_desc].flags = DescriptorFlag::Next as u16;
        virtqueues[0].desc[header_desc].next = data_desc as u16;

        // Set up data descriptor
        let data_phys = data_alloc.as_paddr();
        virtqueues[0].desc[data_desc].addr = data_phys as u64;
        virtqueues[0].desc[data_desc].len = req.buffer.len() as u32;

        // Set flags based on request type
        match req.request_type {
            BlockIORequestType::Read => {
                DescriptorFlag::Next.set(&mut virtqueues[0].desc[data_desc].flags);
                DescriptorFlag::Write.set(&mut virtqueues[0].desc[data_desc].flags);
            }
            BlockIORequestType::Write => {
                DescriptorFlag::Next.set(&mut virtqueues[0].desc[data_desc].flags);
            }
        }

        virtqueues[0].desc[data_desc].next = status_desc as u16;

        // Set up status descriptor
        let status_phys = crate::vm::get_kernel_vm_manager()
            .translate_to_phys(status_ptr as usize)
            .ok_or("Failed to translate status vaddr")?;
        virtqueues[0].desc[status_desc].addr = status_phys as u64;
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
        let status_val = unsafe { core::ptr::read_volatile(status_ptr) };
        let result = match status_val {
            VIRTIO_BLK_S_OK => {
                // For read requests, copy data to the buffer
                if let BlockIORequestType::Read = req.request_type {
                    unsafe {
                        req.buffer.clear();
                        req.buffer.extend_from_slice(core::slice::from_raw_parts(
                            data_ptr as *const u8,
                            virtqueues[0].desc[data_desc].len as usize,
                        ));
                    }
                }
                Ok(())
            }
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
    fn process_requests_batch(
        &self,
        requests: &mut [Box<BlockIORequest>],
    ) -> Vec<Result<(), &'static str>> {
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
            crate::println!(
                "[virtio_blk] WARNING: Batch size {} exceeds safe limit {}, processing in chunks",
                requests.len(),
                MAX_BATCH_SIZE
            );

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
        let read_count = requests
            .iter()
            .filter(|r| matches!(r.request_type, BlockIORequestType::Read))
            .count();
        let write_count = requests
            .iter()
            .filter(|r| matches!(r.request_type, BlockIORequestType::Write))
            .count();

        #[cfg(test)]
        {
            // Add batch size tracking for debugging
            static BATCH_SIZES: IrqSpinLock<alloc::vec::Vec<usize>> =
                IrqSpinLock::new(alloc::vec::Vec::new());
            static CALL_COUNT: IrqSpinLock<usize> = IrqSpinLock::new(0);
            let mut sizes = BATCH_SIZES.lock();
            let mut count = CALL_COUNT.lock();
            sizes.push(requests.len());
            *count += 1;

            // Print statistics every 100 calls
            if *count % 100 == 0 {
                let total_requests: usize = sizes.iter().sum();
                let avg_batch_size = total_requests as f64 / sizes.len() as f64;
                let single_requests = sizes.iter().filter(|&&size| size == 1).count();
                crate::println!(
                    "[virtio_blk] Batch stats: {} calls, avg_batch={:.2}, single_req={}/{} ({:.1}%)",
                    sizes.len(),
                    avg_batch_size,
                    single_requests,
                    sizes.len(),
                    (single_requests as f64 / sizes.len() as f64) * 100.0
                );
            }
        }

        let batch_size = requests.len();
        let mut results = vec![Err("Not processed"); batch_size];
        let mut request_data: Vec<(
            usize,
            usize,
            usize,
            usize,
            *mut VirtioBlkReqHeader,
            ContiguousPages,
            ContiguousPages,
        )> = Vec::new();

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

            // Allocate data buffer from PMM for DMA
            let data_pages = (req.buffer.len() + PAGE_SIZE - 1) / PAGE_SIZE;
            let data_alloc = match ContiguousPages::new(data_pages) {
                Some(alloc) => alloc,
                None => {
                    results[idx] = Err("Failed to allocate data buffer");
                    continue;
                }
            };

            // Allocate status buffer from PMM (1 page is plenty for a single byte)
            let status_alloc = match ContiguousPages::new(1) {
                Some(alloc) => alloc,
                None => {
                    results[idx] = Err("Failed to allocate status buffer");
                    continue;
                }
            };

            let header_ptr = Box::into_raw(header);
            let data_ptr = data_alloc.as_ptr() as *mut u8;
            let status_ptr = status_alloc.as_ptr() as *mut u8;

            // Copy data for write requests
            if let BlockIORequestType::Write = req.request_type {
                unsafe {
                    core::ptr::copy_nonoverlapping(req.buffer.as_ptr(), data_ptr, req.buffer.len());
                }
            }

            // Try to allocate descriptors
            if let (Some(header_desc), Some(data_desc), Some(status_desc)) = (
                virtqueues[0].alloc_desc(),
                virtqueues[0].alloc_desc(),
                virtqueues[0].alloc_desc(),
            ) {
                // Set up descriptors
                let header_phys = match crate::vm::get_kernel_vm_manager()
                    .translate_to_phys(header_ptr as usize)
                {
                    Some(phys) => phys,
                    None => {
                        virtqueues[0].free_desc(status_desc);
                        virtqueues[0].free_desc(data_desc);
                        virtqueues[0].free_desc(header_desc);
                        results[idx] = Err("Failed to translate header vaddr");
                        continue;
                    }
                };
                virtqueues[0].desc[header_desc].addr = header_phys as u64;
                virtqueues[0].desc[header_desc].len = mem::size_of::<VirtioBlkReqHeader>() as u32;
                virtqueues[0].desc[header_desc].flags = DescriptorFlag::Next as u16;
                virtqueues[0].desc[header_desc].next = data_desc as u16;

                // Use physical address directly from ContiguousPages for DMA
                let data_phys = data_alloc.as_paddr();
                virtqueues[0].desc[data_desc].addr = data_phys as u64;
                virtqueues[0].desc[data_desc].len = req.buffer.len() as u32;

                match req.request_type {
                    BlockIORequestType::Read => {
                        DescriptorFlag::Next.set(&mut virtqueues[0].desc[data_desc].flags);
                        DescriptorFlag::Write.set(&mut virtqueues[0].desc[data_desc].flags);
                    }
                    BlockIORequestType::Write => {
                        DescriptorFlag::Next.set(&mut virtqueues[0].desc[data_desc].flags);
                    }
                }

                virtqueues[0].desc[data_desc].next = status_desc as u16;

                // Use physical address directly from ContiguousPages for DMA
                let status_phys = status_alloc.as_paddr();
                virtqueues[0].desc[status_desc].addr = status_phys as u64;
                virtqueues[0].desc[status_desc].len = 1;
                virtqueues[0].desc[status_desc].flags |= DescriptorFlag::Write as u16;

                // Submit the request
                if virtqueues[0].push(header_desc).is_ok() {
                    // Store ContiguousPagess to keep them alive until completion
                    // The allocations will be dropped when removed from request_data
                    request_data.push((
                        idx,
                        header_desc,
                        data_desc,
                        status_desc,
                        header_ptr,
                        data_alloc,
                        status_alloc,
                    ));
                } else {
                    // Clean up on push failure - descriptors freed, ContiguousPagess dropped automatically
                    virtqueues[0].free_desc(status_desc);
                    virtqueues[0].free_desc(data_desc);
                    virtqueues[0].free_desc(header_desc);
                    unsafe {
                        drop(Box::from_raw(header_ptr));
                    }
                    results[idx] = Err("Failed to submit request");
                }
            } else {
                // Descriptor allocation failure - should be very rare with 256 queue size
                crate::println!(
                    "[virtio_blk] ERROR: Failed to allocate descriptors for request {} (batch size: {})",
                    idx,
                    batch_size
                );

                // Clean up on descriptor allocation failure - ContiguousPagess dropped automatically
                unsafe {
                    drop(Box::from_raw(header_ptr));
                }
                results[idx] = Err("Virtqueue descriptor allocation failed - queue may be full");
            }
        }

        // Notify the device once for all requests
        if !request_data.is_empty() {
            // crate::println!("[virtio-blk] Notifying queue 0 for {} requests", request_data.len());
            self.notify(0);
        }

        // Second pass: Wait for all completions (true batch processing)
        // Build a map from header_desc to index in request_data for O(1) lookup
        use alloc::collections::BTreeMap;
        let mut pending_requests: BTreeMap<usize, usize> = BTreeMap::new();
        for (index, request) in request_data.iter().enumerate() {
            let header_desc = request.1;
            pending_requests.insert(header_desc, index);
        }

        // Track which indices have been processed for cleanup
        let mut processed_indices = Vec::new();

        // Process all completions until everything is done
        while !pending_requests.is_empty() {
            // Read status before polling to check if device entered a FAILED state
            let status = self.read32_register(crate::drivers::virtio::device::Register::Status);

            // Wait for something to complete, but also check for device failure.
            while virtqueues[0].is_busy() {
                let status = self.read32_register(crate::drivers::virtio::device::Register::Status);
                if crate::drivers::virtio::device::DeviceStatus::DeviceNeedReset.is_set(status) {
                    crate::println!(
                        "[virtio-blk] ERROR: Device entered NEEDS_RESET state during poll. Aborting. Status=0x{:x}",
                        status
                    );
                    break;
                }
                if crate::drivers::virtio::device::DeviceStatus::Failed.is_set(status) {
                    crate::println!(
                        "[virtio-blk] ERROR: Device entered FAILED state during poll. Aborting. Status=0x{:x}",
                        status
                    );
                    break;
                }
            }

            // Process all completed requests in this round
            while let Some(desc_idx) = virtqueues[0].pop() {
                if let Some(data_index) = pending_requests.remove(&desc_idx) {
                    let (
                        req_idx,
                        _header_desc,
                        data_desc,
                        status_desc,
                        header_ptr,
                        ref data_alloc,
                        ref status_alloc,
                    ): (
                        usize,
                        usize,
                        usize,
                        usize,
                        *mut VirtioBlkReqHeader,
                        ContiguousPages,
                        ContiguousPages,
                    ) = request_data[data_index];
                    let status_ptr = status_alloc.as_ptr() as *mut u8;
                    let data_ptr = data_alloc.as_ptr() as *mut u8;

                    // Check status
                    let status_val = unsafe { core::ptr::read_volatile(status_ptr) };
                    results[req_idx] = match status_val {
                        VIRTIO_BLK_S_OK => {
                            // For read requests, copy data back to the buffer
                            if let BlockIORequestType::Read = requests[req_idx].request_type {
                                unsafe {
                                    requests[req_idx].buffer.clear();
                                    requests[req_idx].buffer.extend_from_slice(
                                        core::slice::from_raw_parts(
                                            data_ptr as *const u8,
                                            virtqueues[0].desc[data_desc].len as usize,
                                        ),
                                    );
                                }
                            }
                            Ok(())
                        }
                        VIRTIO_BLK_S_IOERR => Err("I/O error"),
                        VIRTIO_BLK_S_UNSUPP => Err("Unsupported request"),
                        _ => Err("Unknown error"),
                    };

                    // Clean up descriptors for this completed request
                    virtqueues[0].free_desc(status_desc);
                    virtqueues[0].free_desc(data_desc);
                    virtqueues[0].free_desc(desc_idx); // header_desc
                    unsafe {
                        drop(Box::from_raw(header_ptr));
                    }
                    // ContiguousPagess will be dropped when we remove from request_data
                    processed_indices.push(data_index);
                } else {
                    // Unexpected descriptor - this shouldn't happen but handle gracefully
                    crate::println!(
                        "[virtio-blk] Warning: Unexpected descriptor completion: {}",
                        desc_idx
                    );
                }
            }
        }

        // Clean up request_data - remove processed entries to drop ContiguousPagess
        // Sort in reverse order so we can remove without affecting other indices
        processed_indices.sort_unstable_by(|a: &usize, b: &usize| b.cmp(a));
        for index in processed_indices {
            request_data.remove(index);
        }

        results
    }
}

impl MemoryMappingOps for VirtioBlockDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
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

    fn into_block_device(
        self: alloc::sync::Arc<Self>,
    ) -> Option<alloc::sync::Arc<dyn crate::device::block::BlockDevice>> {
        Some(self)
    }
}

impl Selectable for VirtioBlockDevice {
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

impl VirtioDevice for VirtioBlockDevice {
    fn pci_transport(&self) -> Option<VirtioPciTransport> {
        self.pci_transport
    }

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

    fn get_supported_features(&self, device_features: u64) -> u64 {
        // Accept most features but we might want to be selective
        let mut result = (device_features & u64::from(u32::MAX))
            & !(1u64 << VIRTIO_BLK_F_RO
                | 1u64 << VIRTIO_BLK_F_SCSI
                | 1u64 << VIRTIO_BLK_F_CONFIG_WCE
                | 1u64 << VIRTIO_BLK_F_MQ
                | 1u64 << VIRTIO_F_ANY_LAYOUT);

        if !self.allow_ring_features() {
            result &= !(1u64 << VIRTIO_RING_F_EVENT_IDX | 1u64 << VIRTIO_RING_F_INDIRECT_DESC);
        }

        if self.pci_transport().is_some() {
            result |=
                device_features & (1u64 << crate::drivers::virtio::features::VIRTIO_F_VERSION_1);
        }

        result
    }

    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= 1 {
            return None;
        }

        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].get_raw_ptr() as usize) as u64)
    }

    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= 1 {
            return None;
        }

        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].avail.flags as *const _ as usize) as u64)
    }

    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= 1 {
            return None;
        }

        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].used.flags as *const _ as usize) as u64)
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

    fn get_sector_size(&self) -> usize {
        *self.sector_size.read() as usize
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

        self.submit_requests(requests)
    }

    fn submit_requests(&self, mut requests: Vec<Box<BlockIORequest>>) -> Vec<BlockIOResult> {
        crate::profile_scope!("virtio_blk::submit_requests");
        if requests.is_empty() {
            return Vec::new();
        }

        // Process all requests in true batch
        let batch_results = self.process_requests_batch(&mut requests);

        // Convert results back to the expected format
        requests
            .into_iter()
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

#[cfg(all(test, target_arch = "riscv64"))]
pub mod tests {
    use super::*;
    use alloc::vec;

    /// Physical address of the VirtIO Block device on QEMU RISC-V virt.
    const VIRTIO_BLK_PADDR: usize = 0x10001000;

    /// Map the VirtIO Block MMIO region for use in tests.
    fn map_blk() -> usize {
        crate::vm::ioremap(VIRTIO_BLK_PADDR, crate::environment::PAGE_SIZE)
            .expect("ioremap should succeed for VirtIO Block test device")
    }

    #[test_case]
    fn test_virtio_block_device_init() {
        let vaddr = map_blk();
        let device = VirtioBlockDevice::new(vaddr);

        assert_eq!(device.get_disk_name(), "virtio-blk");
        assert_eq!(
            device.get_disk_size(),
            (*device.capacity.read() * *device.sector_size.read() as u64) as usize
        );
        crate::vm::iounmap(vaddr);
    }

    #[test_case]
    fn test_virtio_block_device() {
        let vaddr = map_blk();
        let device = VirtioBlockDevice::new(vaddr);

        assert_eq!(device.get_disk_name(), "virtio-blk");
        assert_eq!(
            device.get_disk_size(),
            (*device.capacity.read() * *device.sector_size.read() as u64) as usize
        );

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
        crate::vm::iounmap(vaddr);
    }
}
