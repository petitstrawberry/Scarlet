pub mod avail;
pub mod used;
pub mod desc;

use core::{alloc::Layout, mem};

use alloc::alloc::alloc_zeroed;
use avail::{AvailableRing, RawAvailableRing};
use desc::Descriptor;
use used::{RawUsedRing, RawUsedRingEntry, UsedRing};

// struct RawVirtQueue {
//     pub desc: [Descriptor; 0], /* Flexible array member */
//     pub avail: RawAvailableRing,
//     pub padding: [u8; 0], /* Padding to align the used ring */
//     pub used: RawUsedRing,
// }

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
        *(self.avail.index) = 0;
        *(self.used.flags) = 0;
        *(self.used.index) = 0;
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

pub trait VirtioDevice {
    fn init(&mut self);
    fn reset(&mut self);
    fn read32_register(&self, offset: usize) -> u32;
    fn write32_register(&mut self, offset: usize, value: u32);
    fn read64_register(&self, offset: usize) -> u64;
    fn write64_register(&mut self, offset: usize, value: u64);
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
        assert_eq!(*virtqueue.avail.index, 0);
        assert_eq!(*virtqueue.used.index, 0);

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