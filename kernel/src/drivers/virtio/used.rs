#[repr(C)]
pub struct UsedRing {
    pub flags: u16,
    pub index: u16,
    pub ring: [UsedRingEntry; 0], /* Flexible array member */
    pub avail_event: u16,
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