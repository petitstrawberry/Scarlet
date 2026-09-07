extern crate alloc;

use alloc::{format, string::String};
use core::sync::atomic::{AtomicUsize, Ordering};

pub mod device;
pub mod pci;
pub mod queue;

static BLOCK_COUNTER: AtomicUsize = AtomicUsize::new(0);
static NET_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Allocate the next VirtIO block device name.
///
/// # Returns
///
/// A transport-wide unique `vblkN` device name.
pub(crate) fn next_block_device_name() -> String {
    format!("vblk{}", BLOCK_COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// Allocate the next VirtIO network device name.
///
/// # Returns
///
/// A transport-wide unique `vethN` device name.
pub(crate) fn next_net_device_name() -> String {
    format!("veth{}", NET_COUNTER.fetch_add(1, Ordering::SeqCst))
}

pub mod features {
    pub const VIRTIO_F_ANY_LAYOUT: u32 = 27;
    pub const VIRTIO_RING_F_INDIRECT_DESC: u32 = 28;
    pub const VIRTIO_RING_F_EVENT_IDX: u32 = 29;
    pub const VIRTIO_F_VERSION_1: u32 = 32;
}
