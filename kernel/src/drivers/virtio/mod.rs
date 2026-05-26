pub mod device;
pub mod pci;
pub mod queue;

pub mod features {
    pub const VIRTIO_F_ANY_LAYOUT: u32 = 27;
    pub const VIRTIO_RING_F_INDIRECT_DESC: u32 = 28;
    pub const VIRTIO_RING_F_EVENT_IDX: u32 = 29;
    pub const VIRTIO_F_VERSION_1: u32 = 32;
}
