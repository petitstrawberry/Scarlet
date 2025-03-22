use alloc::vec::Vec;

pub struct UsedRing {
    pub flags: u16,
    pub index: u16,
    pub ring: Vec<UsedElement>,
}

pub struct UsedElement {
    pub id: u32,
    pub len: u32,
}