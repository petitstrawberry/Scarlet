use alloc::vec::Vec;

#[repr(C)]
pub struct UsedRing {
    pub flags: u16,
    pub index: u16,
    pub ring: Vec<UsedElement>,
}

#[repr(C)]
pub struct UsedElement {
    pub id: u32,
    pub len: u32,
}