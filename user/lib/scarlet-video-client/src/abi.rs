//! Raw Scarlet video device ABI.
//!
//! These definitions mirror `kernel::device::video`. They stay private so
//! applications use the safe decoder API instead of issuing raw controls.

pub(crate) const VIDEO_DEVICE_PATH: &str = "/dev/video0";

pub(crate) const SCARLET_VIDEO_FRAME_MAGIC: &[u8; 4] = b"SVF1";
pub(crate) const SCARLET_VIDEO_FRAME_HEADER_LEN: usize = 20;
pub(crate) const SCARLET_VIDEO_PIXEL_FORMAT_NV12: u32 = 0x3432_3076;

pub(crate) const SCARLET_VIDEO_GET_BUFFER: u32 = 0x5600;
pub(crate) const SCARLET_VIDEO_SUBMIT: u32 = 0x5601;
pub(crate) const SCARLET_VIDEO_DEQUEUE: u32 = 0x5602;
pub(crate) const SCARLET_VIDEO_CREATE_SESSION: u32 = 0x5603;
pub(crate) const SCARLET_VIDEO_SUBMIT_SESSION: u32 = 0x5604;
pub(crate) const SCARLET_VIDEO_DEQUEUE_SESSION: u32 = 0x5605;
pub(crate) const SCARLET_VIDEO_DESTROY_SESSION: u32 = 0x5606;
pub(crate) const SCARLET_VIDEO_GET_CAPS: u32 = 0x5607;
#[cfg(feature = "h264-stateless-hw")]
pub(crate) const SCARLET_VIDEO_SUBMIT_H264_STATELESS: u32 = 0x5608;
#[cfg(feature = "vp9-stateless-hw")]
pub(crate) const SCARLET_VIDEO_SUBMIT_VP9_STATELESS: u32 = 0x5609;

pub(crate) const SCARLET_VIDEO_CAPS_VERSION: u32 = 1;
pub(crate) const SCARLET_VIDEO_CAP_STATEFUL_H264: u32 = 1 << 0;
pub(crate) const SCARLET_VIDEO_CAP_STATEFUL_AV1: u32 = 1 << 1;
pub(crate) const SCARLET_VIDEO_CAP_STATEFUL_HEVC: u32 = 1 << 2;
pub(crate) const SCARLET_VIDEO_CAP_STATEFUL_VP9: u32 = 1 << 3;
pub(crate) const SCARLET_VIDEO_CAP_STATELESS_H264: u32 = 1 << 8;
pub(crate) const SCARLET_VIDEO_CAP_STATELESS_VP9: u32 = 1 << 9;
pub(crate) const SCARLET_VIDEO_CAP_MAPPED_BUFFERS: u32 = 1 << 16;
pub(crate) const SCARLET_VIDEO_CAP_SESSIONS: u32 = 1 << 17;
pub(crate) const SCARLET_VIDEO_CAP_VARIABLE_MAPPED_BUFFERS: u32 = 1 << 18;

pub(crate) const SCARLET_VIDEO_FORMAT_H264: u32 = 4098;
pub(crate) const SCARLET_VIDEO_FORMAT_HEVC: u32 = 4099;
pub(crate) const SCARLET_VIDEO_FORMAT_VP9: u32 = 4102;
pub(crate) const SCARLET_VIDEO_FORMAT_AV1: u32 = 4103;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScarletVideoBufferInfo {
    pub(crate) mmap_offset: u64,
    pub(crate) mmap_len: u64,
    pub(crate) input_offset: u64,
    pub(crate) input_len: u32,
    pub(crate) output_offset: u64,
    pub(crate) output_len: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScarletVideoSubmit {
    pub(crate) input_len: u32,
    pub(crate) coded_format: u32,
    pub(crate) timestamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScarletVideoDequeuedFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixel_format: u32,
    pub(crate) payload_offset: u64,
    pub(crate) payload_len: u32,
    pub(crate) flags: u32,
    pub(crate) timestamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScarletVideoSessionInfo {
    pub(crate) stream_id: u32,
    pub(crate) padding: u32,
    pub(crate) buffer: ScarletVideoBufferInfo,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScarletVideoSessionSubmit {
    pub(crate) stream_id: u32,
    pub(crate) input_len: u32,
    pub(crate) coded_format: u32,
    pub(crate) padding: u32,
    pub(crate) timestamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScarletVideoSessionDequeuedFrame {
    pub(crate) stream_id: u32,
    pub(crate) padding: u32,
    pub(crate) frame: ScarletVideoDequeuedFrame,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScarletVideoCapabilities {
    pub(crate) version: u32,
    pub(crate) flags: u32,
    pub(crate) max_sessions: u32,
    pub(crate) output_pixel_format: u32,
    pub(crate) mapped_input_len: u32,
    pub(crate) mapped_output_len: u32,
    pub(crate) reserved: [u32; 8],
}

impl ScarletVideoCapabilities {
    pub(crate) fn has_flag(self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

#[cfg(feature = "h264-stateless-hw")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScarletVideoH264ParamPtrs {
    pub(crate) sps: u64,
    pub(crate) pps: u64,
    pub(crate) scaling_matrix: u64,
    pub(crate) pred_weights: u64,
    pub(crate) slice_params: u64,
    pub(crate) decode_params: u64,
}

#[cfg(feature = "h264-stateless-hw")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScarletVideoH264StatelessSubmit {
    pub(crate) stream_id: u32,
    pub(crate) input_len: u32,
    pub(crate) timestamp: u64,
    pub(crate) params: ScarletVideoH264ParamPtrs,
    pub(crate) flags: u32,
    pub(crate) padding: u32,
}

#[cfg(feature = "vp9-stateless-hw")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScarletVideoVp9ParamPtrs {
    pub(crate) frame: u64,
    pub(crate) probabilities: u64,
    pub(crate) tiles: u64,
}

#[cfg(feature = "vp9-stateless-hw")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScarletVideoVp9StatelessSubmit {
    pub(crate) stream_id: u32,
    pub(crate) input_len: u32,
    pub(crate) timestamp: u64,
    pub(crate) params: ScarletVideoVp9ParamPtrs,
    pub(crate) flags: u32,
    pub(crate) padding: u32,
}

const _: [(); 48] = [(); core::mem::size_of::<ScarletVideoBufferInfo>()];
const _: [(); 16] = [(); core::mem::size_of::<ScarletVideoSubmit>()];
const _: [(); 40] = [(); core::mem::size_of::<ScarletVideoDequeuedFrame>()];
const _: [(); 56] = [(); core::mem::size_of::<ScarletVideoSessionInfo>()];
const _: [(); 24] = [(); core::mem::size_of::<ScarletVideoSessionSubmit>()];
const _: [(); 48] = [(); core::mem::size_of::<ScarletVideoSessionDequeuedFrame>()];
const _: [(); 56] = [(); core::mem::size_of::<ScarletVideoCapabilities>()];
#[cfg(feature = "h264-stateless-hw")]
const _: [(); 48] = [(); core::mem::size_of::<ScarletVideoH264ParamPtrs>()];
#[cfg(feature = "h264-stateless-hw")]
const _: [(); 72] = [(); core::mem::size_of::<ScarletVideoH264StatelessSubmit>()];
#[cfg(feature = "vp9-stateless-hw")]
const _: [(); 24] = [(); core::mem::size_of::<ScarletVideoVp9ParamPtrs>()];
#[cfg(feature = "vp9-stateless-hw")]
const _: [(); 48] = [(); core::mem::size_of::<ScarletVideoVp9StatelessSubmit>()];
