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
}