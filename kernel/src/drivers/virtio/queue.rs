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
    pub desc: &'a mut [Descriptor],
    pub avail: AvailableRing<'a>,
    pub used: UsedRing<'a>,
    pub free_descriptors: Vec<usize>,
    pub last_used_idx: usize,
}

impl<'a> VirtQueue<'a> {
    pub fn new(queue_size: usize) -> Self {
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
        
        Self { desc, avail, used, free_descriptors, last_used_idx }
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

    /// Check if the virtqueue is busy
    /// 
    /// This function checks if the virtqueue is busy by comparing the last used index with the current index.
    /// 
    /// # Returns
    /// 
    /// bool: True if the virtqueue is busy, false otherwise.
    pub fn is_busy(&self) -> bool {
        self.last_used_idx == *self.used.idx as usize
    }

    /// Push a descriptor index to the available ring
    /// 
    /// This function pushes a descriptor index to the available ring.
    /// 
    /// # Arguments
    /// 
    /// * `desc_idx` - The index of the descriptor to push. 
    /// If you want to push a chain of descriptors, you should pass the first descriptor index.
    /// 
    /// # Returns
    /// 
    /// Result<(), &'static str>: Ok if the push was successful, or an error message if it failed.
    pub fn push(&mut self, desc_idx: usize) -> Result<(), &'static str> {
        if desc_idx >= self.desc.len() {
            return Err("Invalid descriptor index");
        }
        
        self.avail.ring[*self.avail.idx as usize] = desc_idx as u16;
        *self.avail.idx = (*self.avail.idx + 1) % self.avail.size as u16;
        Ok(())
    }

    /// Pop a buffer from the used ring
    /// 
    /// This function retrieves a buffer from the used ring when the device has finished processing it.
    /// The caller is responsible for freeing the descriptor when it's done with the buffer.
    /// 
    /// # Returns
    /// 
    /// Option<usize>: The index of the descriptor that was used, or None if no descriptors are available.
    ///
    pub fn pop(&mut self) -> Option<usize> {
        // Check if there are any used buffers available
        if self.last_used_idx == *self.used.idx as usize {
            return None;
        }
        
        // Calculate the index in the used ring
        let used_idx = self.last_used_idx % self.desc.len();
        
        // Retrieve the descriptor index from the used ring
        let desc_idx = self.used.ring[used_idx].id as usize;
        // Update the last used index
        self.last_used_idx = (self.last_used_idx + 1) % self.used.ring.len();

        Some(desc_idx)
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
    use super::*;

    #[test_case]
    fn test_used_ring_flags_update() {
        let mut raw = RawUsedRing {
            flags: 0,
            idx: 0,
            ring: [RawUsedRingEntry { id: 0, len: 0 }; 0],
            avail_event: 0,
        };
    
        let used_ring = unsafe { UsedRing::new(0, &mut raw) };
    
        // Verify initial values
        assert_eq!(raw.flags, 0);
        assert_eq!(*used_ring.flags, 0);
    
        // Modify flags
        *used_ring.flags = 42;
    
        // Verify the modification is reflected
        assert_eq!(raw.flags, 42);
        assert_eq!(*used_ring.flags, 42);
    }

    #[test_case]
    fn test_raw_used_ring_direct_access() {
        let queue_size = 2;
        let mut virtqueue = VirtQueue::new(queue_size);
        virtqueue.init();
    
        // 1. Write values to UsedRing via VirtQueue
        *virtqueue.used.flags = 42;
        *virtqueue.used.idx = 1;
        for i in 0..queue_size {
            virtqueue.used.ring[i].id = i as u32;
            virtqueue.used.ring[i].len = 456;
        }
    
        // 2. Get a pointer to RawUsedRing
        let raw_used_ptr = virtqueue.used.flags as *mut u16 as *mut RawUsedRing;
    
        // 3. Directly access RawUsedRing and verify values
        let raw_used = unsafe { &*raw_used_ptr };
        assert_eq!(raw_used.flags, 42, "flags mismatch");
        assert_eq!(raw_used.idx, 1, "idx mismatch");
    
        // 4. Verify the contents of the ring
        unsafe {
            let used_ring = &mut *virtqueue.used.ring.as_mut_ptr();
            let ring = core::slice::from_raw_parts_mut(used_ring, queue_size);
            
            for i in 0..queue_size {
                assert_eq!(ring[i].id, i as u32, "ring[{}].id mismatch", i);
                assert_eq!(ring[i].len, 456, "ring[{}].len mismatch", i);
            }
        }
    }

    #[test_case]
    fn test_raw_available_ring_direct_access() {
        let queue_size = 16;
        let mut virtqueue = VirtQueue::new(queue_size);
        virtqueue.init();

        // 1. Write values to AvailableRing via VirtQueue
        *virtqueue.avail.flags = 24;
        *virtqueue.avail.idx = 1;
        for i in 0..queue_size {
            virtqueue.avail.ring[i] = i as u16;
        }

        // 2. Get a pointer to RawAvailableRing
        let raw_avail_ptr = virtqueue.avail.flags as *mut u16 as *mut RawAvailableRing;

        // 3. Directly access RawAvailableRing and verify values
        let raw_avail = unsafe { &*raw_avail_ptr };
        assert_eq!(raw_avail.flags, 24, "flags mismatch");
        assert_eq!(raw_avail.idx, 1, "idx mismatch");

        // 4. Verify the contents of the ring
        unsafe {
            let avail_ring = &mut *virtqueue.avail.ring.as_mut_ptr();
            let ring = core::slice::from_raw_parts_mut(avail_ring, queue_size);
            
            for i in 0..queue_size {
                assert_eq!(ring[i], i as u16, "ring[{}] mismatch", i);
            }

        }
    }

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

    #[test_case]
    fn test_alloc_free_desc() {
        let queue_size = 1;
        let mut virtqueue = VirtQueue::new(queue_size);
        virtqueue.init();

        // Allocate a descriptor
        let desc_idx = virtqueue.alloc_desc().unwrap();
        assert_eq!(desc_idx, 0);

        // Free the descriptor
        virtqueue.free_desc(desc_idx);
        assert_eq!(virtqueue.free_descriptors.len(), 1);
    }

    #[test_case]
    fn test_alloc_free_desc_chain() {
        let queue_size = 2;
        let mut virtqueue = VirtQueue::new(queue_size);
        virtqueue.init();

        // Allocate a chain of descriptors
        let desc_idx = virtqueue.alloc_desc_chain(2).unwrap();

        // Free the chain of descriptors
        virtqueue.free_desc_chain(desc_idx);
        assert_eq!(virtqueue.free_descriptors.len(), 2);
    }

    #[test_case]
    fn test_alloc_desc_chain_too_long() {
        let queue_size = 2;
        let mut virtqueue = VirtQueue::new(queue_size);
        virtqueue.init();

        // Allocate a chain of descriptors that is too long
        let desc_idx = virtqueue.alloc_desc_chain(3);
        assert!(desc_idx.is_none());
    }

    #[test_case]
    fn test_push_pop() {
        let queue_size = 2;
        let mut virtqueue = VirtQueue::new(queue_size);
        virtqueue.init();
        
        // 1. Allocate and configure a descriptor
        let desc_idx = virtqueue.alloc_desc().unwrap();
        virtqueue.desc[desc_idx].addr = 0x1000;
        virtqueue.desc[desc_idx].len = 100;
        
        // 2. Push to the queue
        assert!(virtqueue.push(desc_idx).is_ok());
        
        // 3. Simulate device processing the buffer
        *virtqueue.used.idx = 1;
        virtqueue.used.ring[0].id = desc_idx as u32;
        
        // 4. Pop the buffer
        let popped = virtqueue.pop();
        assert!(popped.is_some());
        assert_eq!(popped.unwrap(), desc_idx);
        
        // 5. Verify no more buffers are available
        assert!(virtqueue.pop().is_none());
    }

    #[test_case]
    fn test_push_pop_chain() {
        let queue_size = 4;
        let mut virtqueue = VirtQueue::new(queue_size);
        virtqueue.init();
        
        // 1. Allocate a chain of descriptors
        let chain_len = 3;
        let desc_idx = virtqueue.alloc_desc_chain(chain_len).unwrap();
        
        // 2. Configure the descriptors in the chain
        let mut current_idx = desc_idx;
        for i in 0..chain_len {
            virtqueue.desc[current_idx].addr = 0x1000 + (i * 0x100) as u64;
            virtqueue.desc[current_idx].len = 100;
            
            // Set appropriate flags (except for the last one)
            if i < chain_len - 1 {
                DescriptorFlag::Next.set(&mut virtqueue.desc[current_idx].flags);
                current_idx = virtqueue.desc[current_idx].next as usize;
            }
        }
        
        // 3. Push the chain to the queue
        assert!(virtqueue.push(desc_idx).is_ok());
        
        // 4. Simulate device processing the chain
        *virtqueue.used.idx = 1;
        virtqueue.used.ring[0].id = desc_idx as u32;
        virtqueue.used.ring[0].len = 300; // Total bytes processed (100 per descriptor)
        
        // 5. Pop the buffer
        let popped = virtqueue.pop();
        assert!(popped.is_some());
        assert_eq!(popped.unwrap(), desc_idx);
        
        // 6. Verify the chain is intact
        let mut current_idx = desc_idx;
        for i in 0..chain_len {
            // Check each descriptor in the chain
            assert_eq!(virtqueue.desc[current_idx].addr, 0x1000 + (i * 0x100) as u64);
            assert_eq!(virtqueue.desc[current_idx].len, 100);
            
            if i < chain_len - 1 {
                assert!(DescriptorFlag::Next.is_set(virtqueue.desc[current_idx].flags));
                current_idx = virtqueue.desc[current_idx].next as usize;
            } else {
                // Last descriptor should not have NEXT flag
                assert!(!DescriptorFlag::Next.is_set(virtqueue.desc[current_idx].flags));
            }
        }
        
        // 7. Free the chain after processing
        virtqueue.free_desc_chain(desc_idx);
        assert_eq!(virtqueue.free_descriptors.len(), queue_size);
        
        // 8. Verify no more buffers are available
        assert!(virtqueue.pop().is_none());
    }
}

