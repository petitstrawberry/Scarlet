use alloc::vec::Vec;

pub struct AvailableRing {
    pub flags: u16,
    pub idx: u16,
    pub ring: Vec<u16>,
}