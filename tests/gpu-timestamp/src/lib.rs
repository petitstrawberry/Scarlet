//! Host exercise of the production kernel request validation/response logic.
//! The surrounding capability/usercopy adapters are stand-ins, not hardware.
#![allow(dead_code)]

pub const GPU_RESULT_SUCCESS: u32 = 0;
pub const GPU_RESULT_INVALID_ABI: u32 = 1;
pub const GPU_RESULT_INVALID_ARGUMENT: u32 = 2;
pub const GPU_RESULT_INVALID_STATE: u32 = 4;
pub const GPU_RESULT_UNSUPPORTED: u32 = 5;
pub const GPU_RESULT_DEVICE_LOST: u32 = 7;

pub enum GpuBackendSubmitError {
    Rejected(&'static str),
    Unavailable(&'static str),
    DeviceLost(&'static str),
}
pub trait GpuBackendContext {
    fn timestamp_ns(&self) -> Result<Option<u64>, GpuBackendSubmitError>;
}

mod execution {
    fn read_user_value<T: Copy>(_: usize) -> Result<T, &'static str> {
        Err("host usercopy is not provided")
    }
    fn write_user_value<T: Copy>(_: usize, _: &T) -> Result<(), &'static str> {
        Err("host usercopy is not provided")
    }
    mod timestamp {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../kernel/src/device/gpu/execution/timestamp.rs"
        ));
    }
}
