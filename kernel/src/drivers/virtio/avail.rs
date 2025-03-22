use alloc::vec::Vec;

#[repr(C)]
pub struct AvailableRing {
    pub flags: u16,
    pub idx: u16,
    pub ring: Vec<u16>,
    pub used_event: u16,
}