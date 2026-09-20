//! Shared-image output negotiation for every Scarlet video decoder.

pub const VIDEO_SET_OUTPUT_MODE: u32 = 0x560a;
pub const VIDEO_DEQUEUE_IMAGE: u32 = 0x560b;
pub const VIDEO_CAP_SHARED_IMAGES: u32 = 1 << 19;
pub const VIDEO_OUTPUT_MAPPED: u32 = 0;
pub const VIDEO_OUTPUT_SHARED_IMAGE: u32 = 1;

/// Choose output representation before the first submit on this open handle.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoOutputMode {
    pub stream_id: u32,
    pub mode: u32,
    pub reserved: [u32; 2],
}

/// A successful dequeue returns one owning ready/read-only image capability.
/// Query it with SHARED_IMAGE_QUERY. Closing it releases the consumer lease;
/// a GPU/display consumer must retain its own lease until reads retire.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoDequeuedImage {
    pub stream_id: u32,
    pub image_handle: u32,
    pub timestamp: u64,
    pub flags: u32,
    pub reserved: u32,
}

const _: [(); 16] = [(); core::mem::size_of::<VideoOutputMode>()];
const _: [(); 24] = [(); core::mem::size_of::<VideoDequeuedImage>()];
