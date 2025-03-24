#[repr(C, align(2))]

pub struct RawAvailableRing {
    pub flags: u16,
    pub index: u16,
    pub ring: [u16; 0], /* Flexible array member */
    pub used_event: u16, /* Locate after ring */
}

#[repr(C)]
pub struct AvailableRing<'a> {
    size: usize,
    pub flags: &'a mut u16,
    pub index: &'a mut u16,
    pub ring: &'a mut [u16],
    pub used_event: &'a mut u16,
}

impl<'a> AvailableRing<'a> {
    /// Create a new `AvailableRing` instance
    /// 
    /// This function creates a new `AvailableRing` instance from a raw pointer to a `RawAvailableRing`.
    /// 
    /// # Safety
    /// 
    /// This function is unsafe because it dereferences raw pointers and assumes that the memory layout is correct.
    /// The caller must ensure that the pointer is valid and points to a properly initialized `RawAvailableRing`.
    ///
    /// # Arguments
    /// 
    /// * `size` - The size of the ring.
    /// * `ptr` - A raw pointer to a `RawAvailableRing`.
    /// 
    /// # Returns
    /// 
    /// `AvailableRing` - A new `AvailableRing` instance.
    pub unsafe fn new(size: usize, ptr: *mut RawAvailableRing) -> Self {
        let flags = unsafe { &mut (*ptr).flags };
        let index = unsafe { &mut (*ptr).index };
        let ring = unsafe { core::slice::from_raw_parts_mut((*ptr).ring.as_mut_ptr(), size) };
        let used_event = unsafe { &mut *((*ptr).ring.as_mut_ptr().add(size) as *mut u16) };

        Self {
            size,
            flags,
            index,
            ring,
            used_event,
        }
    }
}
