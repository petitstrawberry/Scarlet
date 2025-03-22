use alloc::{boxed::Box, vec::Vec};
use request::{BlockIORequest, BlockIOResult};

pub mod request;
pub mod manager;

extern crate alloc;

pub struct BlockDevice {
    id: usize,
    disk_name: &'static str,
    disk_size: usize,
    request_fn: fn(&mut BlockIORequest) -> Result<(), &'static str>,
    request_queue: Vec<Box<BlockIORequest>>,
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

    pub fn enqueue_request(&mut self, request: Box<BlockIORequest>) {
        self.request_queue.push(request);
    }

    pub fn process_requests(&mut self) -> Vec<BlockIOResult> {
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