//! Virtio Queue module.
//! 
//! This module provides the implementation of the Virtio Queue.
//! It includes the data structures and methods to manage the Virtio Queue.
//! 

use core::{alloc::Layout, mem};
use alloc::{alloc::alloc_zeroed, vec::Vec};

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
/// * `index`: The ID of the virtqueue.
/// * `desc`: A mutable slice of descriptors.
/// * `avail`: The available ring.
/// * `used`: The used ring.
/// * `free_head`: The index of the next free descriptor.
/// * `last_used_idx`: The index of the last used descriptor.
pub struct VirtQueue<'a> {
    pub index: usize,
    pub desc: &'a mut [Descriptor],
    pub avail: AvailableRing<'a>,
    pub used: UsedRing<'a>,
    pub free_descriptors: Vec<usize>,
    pub last_used_idx: usize,
}

impl<'a> VirtQueue<'a> {
    pub fn new(index: usize, queue_size: usize) -> Self {
        /* Calculate the size of each ring */
        let desc_size = queue_size * mem::size_of::<Descriptor>();
        let avail_size = mem::size_of::<RawAvailableRing>() + queue_size * mem::size_of::<u16>();
        let used_size = mem::size_of::<RawUsedRing>() + queue_size * mem::size_of::<RawUsedRingEntry>();

        /* Floor the sum of desc_size, avail_size to the nearest multiple of 4 */
        let align_size = (desc_size + avail_size + 3) & !3;
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

        /* Create the virtqueue */
        let mut free_descriptors = Vec::new();
        for i in 0..queue_size {
            free_descriptors.push(i);
        }
        let last_used_idx = 0;
        Self { index, desc, avail, used, free_descriptors, last_used_idx }
    }

    /// Initialize the virtqueue
    /// 
    /// This function initializes the descriptor table, available ring, and used ring.
    /// It sets the next pointer of each descriptor to point to the next descriptor in the table.
    /// 
    pub fn init(&mut self) {
        // Initialize the descriptor table
        for i in 0..self.desc.len() {
            self.desc[i].addr = 0;
            self.desc[i].len = 0;
            self.desc[i].flags = 0;
            self.desc[i].next = (i as u16 + 1) % self.desc.len() as u16;
        }

        *(self.avail.flags) = 0;
        *(self.avail.idx) = 0;
        *(self.avail.used_event) = 0;
        *(self.used.flags) = 0;
        *(self.used.idx) = 0;
        *(self.used.avail_event) = 0;
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
        let align_size = (desc_size + avail_size + 3) & !3;
        let padding_size = align_size - (desc_size + avail_size);
        desc_size + avail_size + used_size + padding_size
    }

    /// Allocate a descriptor
    ///
    /// This function allocates a descriptor from the free list.
    /// 
    /// # Returns
    /// 
    /// Option<usize>: The index of the allocated descriptor, or None if no descriptors are available.
    /// 
    pub fn alloc_desc(&mut self) -> Option<usize> {
        let desc = self.free_descriptors.pop();
        if let Some(desc_idx) = desc {
            self.desc[desc_idx].next = 0;
            self.desc[desc_idx].addr = 0;
            self.desc[desc_idx].len = 0;
            self.desc[desc_idx].flags = 0;
            Some(desc_idx)
        } else {
            None
        }
    }

    /// Free a descriptor
    /// 
    /// This function frees a descriptor and adds it back to the free list.
    /// 
    /// # Arguments
    /// 
    /// * `desc_idx` - The index of the descriptor to free.
    /// 
    pub fn free_desc(&mut self, desc_idx: usize) {
        if desc_idx < self.desc.len() {
            self.desc[desc_idx].next = 0;
            self.free_descriptors.push(desc_idx);
        } else {
            panic!("Invalid descriptor index");
        }
    }

    /// Allocate a chain of descriptors
    /// 
    /// This function allocates a chain of descriptors of the specified length.
    /// 
    /// # Arguments
    /// 
    /// * `length` - The length of the chain to allocate.
    /// 
    /// # Returns
    /// 
    /// Option<usize>: The index of the first descriptor in the chain, or None if no descriptors are available.
    /// 
    pub fn alloc_desc_chain(&mut self, length: usize) -> Option<usize> {
        let desc_idx = self.alloc_desc();
        if desc_idx.is_none() {
            return None;
        }
        let desc_idx = desc_idx.unwrap();
        let mut prev_idx = desc_idx;

        for _ in 1..length {
            let next_idx = self.alloc_desc();
            if next_idx.is_none() {
                self.free_desc_chain(desc_idx);
                return None;
            }
            let next_idx = next_idx.unwrap();
            self.desc[prev_idx].next = next_idx as u16;
            self.desc[prev_idx].flags = DescriptorFlag::Next as u16;
            prev_idx = next_idx;
        }

        self.desc[prev_idx].next = 0;
        Some(desc_idx)
    }

    /// Free a chain of descriptors
    /// 
    /// This function frees a chain of descriptors starting from the given index.
    /// 
    /// # Arguments
    /// 
    /// * `desc_idx` - The index of the first descriptor in the chain.
    /// 
    pub fn free_desc_chain(&mut self, desc_idx: usize) {
        let mut idx = desc_idx;
        loop {
            if idx >= self.desc.len() {
                break;
            }
            let next = self.desc[idx].next;
            self.free_desc(idx);

            if !DescriptorFlag::Next.is_set(self.desc[idx].flags) {
                break;
            }
            idx = next as usize;
        }
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

/// Descriptor flags
/// 
/// This enum represents the flags that can be set for a descriptor.
/// It includes flags for indicating the next descriptor, write operation, and indirect descriptor.
#[derive(Clone, Copy)]
pub enum DescriptorFlag {
    Next = 0x1,
    Write = 0x2,
    Indirect = 0x4,
}

impl DescriptorFlag {
    /// Check if the flag is set
    /// 
    /// This method checks if the specified flag is set in the given flags.
    /// 
    /// # Arguments
    /// 
    /// * `flags` - The flags to check.
    /// 
    /// # Returns
    /// 
    /// Returns true if the flag is set, false otherwise.
    ///
    pub fn is_set(&self, flags: u16) -> bool {
        (flags & *self as u16) != 0
    }

    /// Set the flag
    /// 
    /// This method sets the specified flag in the given flags.
    /// 
    /// # Arguments
    /// 
    /// * `flags` - A mutable reference to the flags to modify.
    /// 
    pub fn set(&self, flags: &mut u16) {
        (*flags) |= *self as u16;
    }

    /// Clear the flag
    /// 
    /// This method clears the specified flag in the given flags.
    /// 
    /// # Arguments
    /// 
    /// * `flags` - A mutable reference to the flags to modify.
    /// 
    pub fn clear(&self, flags: &mut u16) {
        (*flags) &= !(*self as u16);
    }

    /// Toggle the flag
    /// 
    /// This method toggles the specified flag in the given flags.
    /// 
    /// # Arguments
    /// 
    /// * `flags` - A mutable reference to the flags to modify.
    /// 
    pub fn toggle(&self, flags: &mut u16) {
        (*flags) ^= *self as u16;
    }
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
    use crate::println;
    use crate::print;

    use super::*;

    #[test_case]
    fn test_initialize_virtqueue() {
        let queue_size = 2;
        let mut virtqueue = VirtQueue::new(0, queue_size);
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

    #[test_case]
    fn test_alloc_free_desc() {
        let queue_size = 1;
        let mut virtqueue = VirtQueue::new(0, queue_size);
        virtqueue.init();

        // Allocate a descriptor
        let desc_idx = virtqueue.alloc_desc().unwrap();
        assert_eq!(desc_idx, 0);

        // Free the descriptor
        virtqueue.free_desc(desc_idx);
        assert_eq!(virtqueue.free_descriptors.len(), 1);
    }

    #[test_case]
    fn test_free_desc_chain() {
        let queue_size = 2;
        let mut virtqueue = VirtQueue::new(0, queue_size);
        virtqueue.init();

        // Allocate two descriptors
        let desc_idx1 = virtqueue.alloc_desc().unwrap();
        let desc_idx2 = virtqueue.alloc_desc().unwrap();

        // // Set the next pointer of the first descriptor to point to the second descriptor
        virtqueue.desc[desc_idx1].next = desc_idx2 as u16;
        // Set the flags of the first descriptor to indicate that it is the last descriptor in the chain
        DescriptorFlag::Next.set(&mut virtqueue.desc[desc_idx1].flags);

        // Free the chain starting from the first descriptor
        virtqueue.free_desc_chain(desc_idx1);

        println!("Free descriptors: {:?}", virtqueue.free_descriptors);

        // // Check that both descriptors are free
        assert_eq!(virtqueue.free_descriptors.len(), 2);
    }
}

