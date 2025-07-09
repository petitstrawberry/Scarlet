use alloc::{boxed::Box, vec::Vec};

#[derive(Debug)]
pub struct BlockIORequest {
    pub request_type: BlockIORequestType,
    pub sector: usize,
    pub sector_count: usize,
    pub head: usize,
    pub cylinder: usize,
    pub buffer: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockIORequestType {
    Read,
    Write,
}

pub struct BlockIOResult {
    pub request: Box<BlockIORequest>,
    pub result: Result<(), &'static str>,
}
