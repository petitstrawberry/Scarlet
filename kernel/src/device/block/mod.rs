use alloc::vec::Vec;

pub mod request;

extern crate alloc;

pub struct BlockDevice {
    id: usize,
    disk_name: &'static str,
    disk_size: usize,
    request_fn: fn(&mut request::BlockIORequest) -> Result<(), &'static str>,
    request_queue: Vec<request::BlockIORequest>,
}

impl BlockDevice {
    pub fn new(id: usize, disk_name: &'static str, disk_size: usize, request_fn: fn(&mut request::BlockIORequest) -> Result<(), &'static str>) -> Self {
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
            (self.request_fn)(request)?;
        }
        self.request_queue.clear();
        Ok(())
    }
}
