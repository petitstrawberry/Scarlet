//! Virtio Queue module.
//! 
//! This module provides the implementation of the Virtio Queue.
//! It includes the data structures and methods to manage the Virtio Queue.
//! 

use core::{alloc::Layout, mem};
use alloc::alloc::alloc_zeroed;

// struct RawVirtQueue {
//     pub desc: [Descriptor; 0], /* Flexible array member */
//     pub avail: RawAvailableRing,
//     pub padding: [u8; 0], /* Padding to align the used ring */
//     pub used: RawUsedRing,
// }

/// VirtQueue structure
/// 
/// This structure represents the wrapper of the virtqueue.
/// It contains the descriptor table, available ring, and used ring.
///
/// # Fields
/// 
/// * `desc`: A mutable slice of descriptors.
/// * `avail`: The available ring.
/// * `used`: The used ring.
pub struct VirtQueue<'a> {
    pub desc: &'a mut [Descriptor],
    pub avail: AvailableRing<'a>,
    pub used: UsedRing<'a>,
}

impl<'a> VirtQueue<'a> {
    pub fn new(queue_size: usize) -> Self {
        /* Calculate the size of each ring */
        let desc_size = queue_size * mem::size_of::<Descriptor>();
        let avail_size = mem::size_of::<RawAvailableRing>() + queue_size * mem::size_of::<u16>();
        let used_size = mem::size_of::<RawUsedRing>() + queue_size * mem::size_of::<RawUsedRingEntry>();

        /* Floor the sum of desc_size, avail_size to the nearest multiple of 4 */
        let floor_size = (desc_size + avail_size + 3) & !3;
        /* Align the size to the nearest multiple of 4 */
        let align_size = (floor_size + 3) & !3;
        /* Calculate the size of the padding for the used ring */
        let padding_size = align_size - (desc_size + avail_size);

        /* Make layout for the virtqueue */
        /* The size is the sum of the sizes of the descriptor table, available ring, and used ring */
        let layout = Layout::from_size_align(
            desc_size + avail_size + padding_size + used_size,
            mem::align_of::<Descriptor>(),
        )
        .unwrap();

        /* Allocate memory for the virtqueue */
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            panic!("Memory allocation failed");
        }

        /* Create the descriptor table */
        let desc_ptr = ptr as *mut Descriptor;
        let desc = unsafe { core::slice::from_raw_parts_mut(desc_ptr, queue_size) };

        /* Create the available ring */
        let avail_ptr = unsafe { desc_ptr.add(queue_size) as *mut RawAvailableRing };
        let avail = unsafe { AvailableRing::new(queue_size, avail_ptr) };

        /* Create the used ring */
        let used_ptr = unsafe {
            (avail_ptr as *mut u8).add(mem::size_of::<RawAvailableRing>() + queue_size * mem::size_of::<u16>() + padding_size) as *mut RawUsedRing
        };
        let used = unsafe { UsedRing::new(queue_size, used_ptr) };

        Self { desc, avail, used }
    }

    /// Initialize the virtqueue
    /// 
    /// This function initializes the descriptor table, available ring, and used ring.
    /// It sets the next pointer of each descriptor to point to the next descriptor in the table.
    /// 
    pub fn init(&mut self) {
        // DescriptorTable の初期化
        for i in 0..self.desc.len() {
            self.desc[i].next = (i as u16 + 1) % self.desc.len() as u16;
        }

        *(self.avail.flags) = 0;
        *(self.avail.idx) = 0;
        *(self.used.flags) = 0;
        *(self.used.idx) = 0;
    }

    /// Get the raw pointer to the virtqueue
    /// 
    /// This function returns a raw pointer to the start of the virtqueue memory.
    /// It can be used to access the memory directly.
    /// 
    /// # Returns
    /// 
    /// *const u8: A raw pointer to the start of the virtqueue memory.
    pub fn get_raw_ptr(&self) -> *const u8 {
        self.desc.as_ptr() as *const u8
    }

    /// Get the size of the raw virtqueue
    /// 
    /// This function returns the size of the virtqueue in bytes.
    /// It is calculated as the sum of the sizes of the descriptor table, available ring, and used ring.
    ///
    /// # Returns
    /// 
    /// usize: The size of the virtqueue in bytes.
    pub fn get_raw_size(&self) -> usize {
        let desc_size = self.desc.len() * mem::size_of::<Descriptor>();
        let avail_size = mem::size_of::<RawAvailableRing>() + self.desc.len() * mem::size_of::<u16>();
        let used_size = mem::size_of::<RawUsedRing>() + self.desc.len() * mem::size_of::<RawUsedRingEntry>();
        let floor_size = (desc_size + avail_size + 3) & !3;
        let align_size = (floor_size + 3) & !3;
        let padding_size = align_size - (desc_size + avail_size);
        desc_size + avail_size + used_size + padding_size
    }
}

/// Descriptor structure
///
/// This structure represents a descriptor in the descriptor table.
/// It contains the address, length, flags, and next pointer.
/// This structure is located in the physical memory directly.
#[repr(C)]
pub struct Descriptor {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

/// Raw available ring structure
/// 
/// This structure represents the raw available ring.
/// It contains the flags, index, ring buffer, and used event. 
/// This structure is located in the physical memory directly.
#[repr(C, align(2))]
pub struct RawAvailableRing {
    flags: u16,
    idx: u16,
    ring: [u16; 0], /* Flexible array member */
    used_event: u16, /* Locate after ring */
}

/// Available ring structure
/// 
/// This structure is wrapped around the `RawAvailableRing` structure.
/// It provides a safe interface to access the available ring entries.
#[repr(C)]
pub struct AvailableRing<'a> {
    size: usize,
    pub flags: &'a mut u16,
    pub idx: &'a mut u16,
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
        let idx = unsafe { &mut (*ptr).idx };
        let ring = unsafe { core::slice::from_raw_parts_mut((*ptr).ring.as_mut_ptr(), size) };
        let used_event = unsafe { &mut *((*ptr).ring.as_mut_ptr().add(size) as *mut u16) };

        Self {
            size,
            flags,
            idx,
            ring,
            used_event,
        }
    }
}

/// Raw used ring structure
/// 
/// This structure represents the raw used ring.
/// It contains the flags, index, ring buffer, and available event.
/// This structure is located in the physical memory directly.
#[repr(C, align(4))]
pub struct RawUsedRing {
    flags: u16,
    idx: u16,
    ring: [RawUsedRingEntry; 0], /* Flexible array member */
    avail_event: u16,
}

/// Raw used ring entry structure
/// 
/// This structure represents a single entry in the used ring.
/// It contains the ID and length of the used buffer.
/// 
/// This structure is located in the physical memory directly.
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
    pub flags: &'a mut u16,
    pub idx: &'a mut u16,
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
        let idx = unsafe { &mut (*ptr).idx };
        let ring_ptr = unsafe { (*ptr).ring.as_mut_ptr() };
        let ring = unsafe { core::slice::from_raw_parts_mut(ring_ptr, size) };
        let avail_event = unsafe { &mut *((*ptr).ring.as_mut_ptr().add(size) as *mut u16) };

        Self {
            flags,
            idx,
            ring,
            avail_event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_initialize_virtqueue() {
        let queue_size = 2;
        let mut virtqueue = VirtQueue::new(queue_size);
        virtqueue.init();

        let total = 68;

        assert_eq!(virtqueue.desc.len(), queue_size);
        assert_eq!(*virtqueue.avail.idx, 0);
        assert_eq!(*virtqueue.used.idx, 0);

        // Check the size of the allocated memory
        let allocated_size = virtqueue.get_raw_size();
        assert_eq!(allocated_size, total);

        // Check the next index of each descriptor
        for i in 0..queue_size {
            assert_eq!(virtqueue.desc[i].next, (i as u16 + 1) % queue_size as u16);
            assert_eq!(virtqueue.avail.ring[i], 0);
            assert_eq!(virtqueue.used.ring[i].len, 0);
        }
    }
}

