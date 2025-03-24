#[repr(C, align(4))]
pub struct RawUsedRing {
    pub flags: u16,
    pub index: u16,
    pub ring: [RawUsedRingEntry; 0], /* Flexible array member */
    pub avail_event: u16,
}

#[derive(Clone)]
#[repr(C)]
pub struct RawUsedRingEntry {
    pub id: u32,
    pub len: u32,
}

impl Default for RawUsedRingEntry {
    fn default() -> Self {
        Self {
            id: 0,
            len: 0,
        }
    }
}

/// Used ring structure
/// 
/// This structure is wrapped around the `RawUsedRing` structure.
/// It provides a safe interface to access the used ring entries.
pub struct UsedRing<'a> {
    size: usize,
    pub flags: &'a mut u16,
    pub index: &'a mut u16,
    pub ring: &'a mut [RawUsedRingEntry],
    pub avail_event: &'a mut u16,
}

impl<'a> UsedRing<'a> {

    /// Create a new `UsedRing` instance
    /// 
    /// This function creates a new `UsedRing` instance from a raw pointer to a `RawUsedRing`.
    /// 
    /// # Safety
    /// 
    /// This function is unsafe because it dereferences raw pointers and assumes that the memory layout is correct.
    /// The caller must ensure that the pointer is valid and points to a properly initialized `RawUsedRing`.
    /// 
    /// # Arguments
    /// 
    /// * `size` - The size of the ring.
    /// * `ptr` - A raw pointer to a `RawUsedRing`.
    /// 
    /// # Returns
    /// 
    /// `UsedRing` - A new `UsedRing` instance.
    pub unsafe fn new(size: usize, ptr: *mut RawUsedRing) -> Self {
        let flags = unsafe { &mut (*ptr).flags };
        let index = unsafe { &mut (*ptr).index };
        let ring_ptr = unsafe { (*ptr).ring.as_mut_ptr() };
        let ring = unsafe { core::slice::from_raw_parts_mut(ring_ptr, size) };
        let avail_event = unsafe { &mut *((*ptr).ring.as_mut_ptr().add(size) as *mut u16) };

        Self {
            size,
            flags,
            index,
            ring,
            avail_event,
        }
    }
}