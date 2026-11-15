//! Additive context GPU-clock query. No host-clock substitution is permitted.

pub const GPU_CONTEXT_TIMESTAMP: u32 = 0x4775;
pub const GPU_TIMESTAMP_ABI_VERSION: u32 = 1;
/// GPU_QUERY_INFO execution_support: retired-context device GPU clock sampling.
pub const GPU_EXECUTION_SUPPORT_TIMESTAMPS: u32 = 1 << 12;

/// Sample the device's single GPU clock after retiring preceding context work.
/// The backend must retain power and serialization through the sample. No
/// relation to the host epoch is implied. Flags and reserved must be zero.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GpuContextTimestamp {
    pub abi_version: u32,
    pub flags: u32,
    pub result: u32,
    pub reserved: u32,
    pub timestamp_ns: u64,
}

impl GpuContextTimestamp {
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_TIMESTAMP_ABI_VERSION,
            flags: 0,
            result: 0,
            reserved: 0,
            timestamp_ns: 0,
        }
    }
}

impl Default for GpuContextTimestamp {
    fn default() -> Self {
        Self::new()
    }
}

const _: () = assert!(core::mem::size_of::<GpuContextTimestamp>() == 24);
const _: () = assert!(core::mem::offset_of!(GpuContextTimestamp, timestamp_ns) == 16);
