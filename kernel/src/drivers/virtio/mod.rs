pub mod avail;
pub mod used;
pub mod desc;

use core::{alloc::Layout, mem};

use alloc::alloc::alloc_zeroed;
use avail::AvailableRing;
use desc::Descriptor;
use used::{UsedRing, UsedRingEntry};

pub struct VirtQueue<'a> {
    pub desc: &'a mut [Descriptor],
    pub avail: &'a mut AvailableRing,
    pub used: &'a mut UsedRing,
}

impl<'a> VirtQueue<'a> {
    pub fn new(queue_size: usize) -> Self {
        /* Calculate the size of each ring */
        let desc_size = queue_size * mem::size_of::<Descriptor>();
        let avail_size = mem::size_of::<AvailableRing>() + queue_size * mem::size_of::<u16>();
        let used_size = mem::size_of::<UsedRing>() + queue_size * mem::size_of::<UsedRingEntry>();

        /* Make layout for the virtqueue */
        /* The size is the sum of the sizes of the descriptor table, available ring, and used ring */
        let layout = Layout::from_size_align(
            desc_size + avail_size + used_size,
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
        let avail_ptr = unsafe { desc_ptr.add(queue_size) as *mut AvailableRing };
        let avail = unsafe { &mut *avail_ptr };

        /* Create the used ring */
        let used_ptr = unsafe {
            (avail_ptr as *mut u8).add(mem::size_of::<AvailableRing>() + queue_size * mem::size_of::<u16>()) as *mut UsedRing
        };
        let used = unsafe { &mut *used_ptr };

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

        self.avail.flags = 0;
        self.avail.index = 0;
        self.used.flags = 0;
        self.used.index = 0;
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
        let avail_size = mem::size_of::<AvailableRing>() + self.desc.len() * mem::size_of::<u16>();
        let used_size = mem::size_of::<UsedRing>() + self.desc.len() * mem::size_of::<UsedRingEntry>();

        desc_size + avail_size + used_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_virtqueue() {
        let queue_size = 8;
        let mut virtqueue = VirtQueue::new(queue_size);
        virtqueue.init();

        let total_size = mem::size_of::<Descriptor>() * queue_size
            + mem::size_of::<AvailableRing>() + queue_size * mem::size_of::<u16>()
            + mem::size_of::<UsedRing>() + queue_size * mem::size_of::<UsedRingEntry>();

        assert_eq!(virtqueue.desc.len(), queue_size);
        assert_eq!(virtqueue.avail.index, 0);
        assert_eq!(virtqueue.used.index, 0);

        // Check the size of the allocated memory
        let allocated_size = virtqueue.get_raw_size();
        assert_eq!(allocated_size, total_size);

        // Check the next index of each descriptor
        for i in 0..queue_size {
            assert_eq!(virtqueue.desc[i].next, (i as u16 + 1) % queue_size as u16);
        }
    }
}