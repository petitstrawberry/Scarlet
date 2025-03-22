pub mod avail;
pub mod used;
pub mod desc;

use avail::AvailableRing;
use desc::DescriptorTable;
use used::UsedRing;

pub struct VirtQueue {
    pub descriptor_table: DescriptorTable,
    pub available_ring: AvailableRing,
    pub used_ring: UsedRing,
    pub last_used_index: u16,
}

impl VirtQueue {
    pub fn new(size: usize) -> Self {
        Self {
            descriptor_table: DescriptorTable::new(size),
            available_ring: AvailableRing::new(size),
            used_ring: UsedRing::new(size),
            last_used_index: 0,
        }
    }

    pub fn init(&mut self) {
        self.descriptor_table.init();
        self.available_ring.init();
        self.used_ring.init();
    }
}