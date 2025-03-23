#[repr(C)]
pub struct AvailableRing {
    pub flags: u16,
    pub index: u16,
    pub ring: [u16; 0], /* Flexible array member */
    pub used_event: u16,
}