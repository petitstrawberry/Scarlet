use alloc::vec;
use alloc::vec::Vec;
#[repr(C)]
pub struct UsedRing {
    pub flags: u16,
    pub index: u16,
    pub ring: Vec<UsedRingEntry>,
    pub avail_event: u16,
}

impl UsedRing {
    pub fn new(size: usize) -> Self {
        Self {
            flags: 0,
            index: 0,
            ring: vec![UsedRingEntry::default(); size],
            avail_event: 0,
        }
    }

    pub fn init(&mut self) {
        self.flags = 0;
        self.index = 0;
        self.avail_event = 0;
    }
}

#[repr(C)]
#[derive(Clone)]
pub struct UsedRingEntry {
    pub id: u32,
    pub len: u32,
}

impl Default for UsedRingEntry {
    fn default() -> Self {
        Self {
            id: 0,
            len: 0,
        }
    }
}