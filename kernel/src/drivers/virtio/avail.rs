use alloc::vec;
use alloc::vec::Vec;
#[repr(C)]
pub struct AvailableRing {
    pub flags: u16,
    pub idx: u16,
    pub ring: Vec<u16>,
    pub used_event: u16,
}

impl AvailableRing {
    pub fn new(size: usize) -> Self {
        Self {
            flags: 0,
            idx: 0,
            ring: vec![0; size],
            used_event: 0,
        }
    }

    pub fn init(&mut self) {
        self.flags = 0;
        self.idx = 0;
        self.used_event = 0;
    }
}