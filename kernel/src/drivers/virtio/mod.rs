pub mod queue;
pub mod device;

pub mod features {
    pub const VIRTIO_F_ANY_LAYOUT: u32 = 27;
    pub const VIRTIO_RING_F_INDIRECT_DESC: u32 = 28;
    pub const VIRTIO_RING_F_EVENT_IDX: u32 = 29;
}