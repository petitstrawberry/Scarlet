use alloc::vec::Vec;

pub struct BlockIORequest {
    pub request_type: BlockIORequestType,
    pub sector: usize,
    pub sector_count: usize,
    pub head: usize,
    pub cylinder: usize,
    pub buffer: Vec<u8>,
}

pub enum BlockIORequestType {
    Read,
    Write,
}