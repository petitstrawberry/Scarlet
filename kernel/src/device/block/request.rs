pub struct BlockIORequest {
    pub sector: usize,
    pub sector_count: usize,
    pub head: usize,
    pub cylinder: usize,
    pub buffer: *mut u8,
    pub request_type: BlockIORequestType,
}

pub enum BlockIORequestType {
    Read,
    Write,
}