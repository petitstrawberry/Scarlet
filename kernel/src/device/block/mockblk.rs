use core::any::Any;

use crate::sync::IrqSpinLock;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use super::request::BlockIORequestType;
use super::*;
use crate::device::block::request::BlockIOResult;
use crate::device::{Device, DeviceType};
use crate::object::capability::selectable::Selectable;
use crate::object::capability::{ControlOps, MemoryMappingOps};

// Mock block device
pub struct MockBlockDevice {
    disk_name: &'static str,
    disk_size: usize,
    data: IrqSpinLock<Vec<Vec<u8>>>,
    request_queue: IrqSpinLock<Vec<Box<BlockIORequest>>>,
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
            data: IrqSpinLock::new(data),
            request_queue: IrqSpinLock::new(Vec::new()),
        }
    }

    fn process_request(&self, request: &mut BlockIORequest) -> Result<(), &'static str> {
        if request.sector_count == 0 {
            if request.request_type == BlockIORequestType::Read {
                request.buffer.clear();
            }
            return Ok(());
        }

        let end = request
            .sector
            .checked_add(request.sector_count)
            .ok_or("Invalid sector range")?;
        let mut data = self.data.lock();
        let sectors = data
            .get_mut(request.sector..end)
            .ok_or("Invalid sector range")?;

        match request.request_type {
            BlockIORequestType::Read => {
                request.buffer.clear();
                for sector in sectors {
                    request.buffer.extend_from_slice(sector);
                }
            }
            BlockIORequestType::Write => {
                // Validate the entire transfer before modifying any sectors.
                let byte_count: usize = sectors.iter().map(Vec::len).sum();
                if request.buffer.len() < byte_count {
                    return Err("Buffer too small");
                }

                let mut buffer = request.buffer.as_slice();
                for sector in sectors {
                    let (bytes, rest) = buffer.split_at(sector.len());
                    sector.copy_from_slice(bytes);
                    buffer = rest;
                }
            }
        }

        Ok(())
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

    fn into_block_device(self: Arc<Self>) -> Option<Arc<dyn BlockDevice>> {
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
        // Extract all requests at once to minimize lock time
        let requests = {
            let mut queue = self.request_queue.lock();
            core::mem::replace(&mut *queue, Vec::new())
        }; // request_queue lock is automatically released here

        self.submit_requests(requests)
    }

    fn submit_requests(&self, requests: Vec<Box<BlockIORequest>>) -> Vec<BlockIOResult> {
        let mut results = Vec::with_capacity(requests.len());
        // Process all requests without holding the request_queue lock
        for mut request in requests {
            let result = self.process_request(&mut request);

            results.push(BlockIOResult { request, result });
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

impl MemoryMappingOps for MockBlockDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Mock implementation - no operation
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Mock implementation - no operation
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for MockBlockDevice {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn request(
        request_type: BlockIORequestType,
        sector: usize,
        sector_count: usize,
        buffer: Vec<u8>,
    ) -> Box<BlockIORequest> {
        Box::new(BlockIORequest {
            request_type,
            sector,
            sector_count,
            head: 0,
            cylinder: 0,
            buffer,
        })
    }

    #[test_case]
    fn test_multi_sector_read() {
        let device = MockBlockDevice::new("mock_read", 512, 4);
        for sector in 0..4 {
            device.enqueue_request(request(
                BlockIORequestType::Write,
                sector,
                1,
                vec![sector as u8; 512],
            ));
        }
        let writes = device.process_requests();
        assert_eq!(writes.len(), 4);
        assert!(writes.iter().all(|result| result.result.is_ok()));

        // Include a read ending exactly at the device boundary.
        for start in [1, 2] {
            let reads = device.submit_requests(vec![request(
                BlockIORequestType::Read,
                start,
                2,
                vec![0; 1024],
            )]);
            assert_eq!(reads.len(), 1);
            assert_eq!(reads[0].result, Ok(()));
            assert_eq!(reads[0].request.buffer.len(), 1024);
            assert_eq!(&reads[0].request.buffer[..512], &vec![start as u8; 512]);
            assert_eq!(
                &reads[0].request.buffer[512..],
                &vec![(start + 1) as u8; 512]
            );
        }
    }

    #[test_case]
    fn test_multi_sector_write() {
        let device = MockBlockDevice::new("mock_write", 512, 4);
        let mut buffer = vec![0xab; 512];
        buffer.extend_from_slice(&[0xcd; 512]);
        device.enqueue_request(request(BlockIORequestType::Write, 1, 2, buffer));
        let writes = device.process_requests();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].result, Ok(()));

        // Read each sector separately so a broken read cannot mask a broken write.
        for (sector, byte) in [0, 0xab, 0xcd, 0].into_iter().enumerate() {
            let reads = device.submit_requests(vec![request(
                BlockIORequestType::Read,
                sector,
                1,
                vec![0; 512],
            )]);
            assert_eq!(reads[0].result, Ok(()));
            assert_eq!(reads[0].request.buffer, vec![byte; 512]);
        }
    }

    #[test_case]
    fn test_invalid_requests_leave_data_unchanged() {
        let device = MockBlockDevice::new("mock_bounds", 512, 4);
        for request_type in [BlockIORequestType::Read, BlockIORequestType::Write] {
            for (sector, count) in [(4, 1), (3, 2), (usize::MAX, 2), (1, usize::MAX)] {
                let results = device.submit_requests(vec![request(
                    request_type,
                    sector,
                    count,
                    vec![0xff; 1024],
                )]);
                assert!(results[0].result.is_err());
            }
        }

        for len in [0, 512, 1023] {
            let results = device.submit_requests(vec![request(
                BlockIORequestType::Write,
                1,
                2,
                vec![0xff; len],
            )]);
            assert!(results[0].result.is_err());
        }

        let reads =
            device.submit_requests(vec![request(BlockIORequestType::Read, 0, 4, vec![0; 2048])]);
        assert_eq!(reads[0].result, Ok(()));
        assert_eq!(reads[0].request.buffer, vec![0; 2048]);
    }

    #[test_case]
    fn test_zero_sector_requests_are_noops() {
        let device = MockBlockDevice::new("mock_empty", 512, 0);
        for request_type in [BlockIORequestType::Read, BlockIORequestType::Write] {
            let results =
                device.submit_requests(vec![request(request_type, usize::MAX, 0, vec![0xff; 512])]);
            assert_eq!(results[0].result, Ok(()));
            if request_type == BlockIORequestType::Read {
                assert!(results[0].request.buffer.is_empty());
            }
        }
    }
}
