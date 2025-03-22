use alloc::vec::Vec;

#[repr(C)]
pub struct UsedRing {
    pub flags: u16,
    pub index: u16,
    pub ring: Vec<UsedRingEntry>,
    pub avail_event: u16,
}

#[repr(C)]
pub struct UsedRingEntry {
    pub id: u32,
    pub len: u32,
}