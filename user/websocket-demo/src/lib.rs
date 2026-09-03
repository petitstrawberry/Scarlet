//! Shared defaults and platform support for the Scarlet WebSocket demo.

/// Default address used by `ws-server`.
pub const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:8081";

/// Default endpoint used by `ws-client`.
pub const DEFAULT_ENDPOINT: &str = "ws://127.0.0.1:8081";

/// Channel capacity for the broadcast bus.
pub const BROADCAST_CAPACITY: usize = 64;

/// Custom error code for the getrandom backend.
const CUSTOM_RANDOM_ERROR_CODE: u16 = 1;

/// Custom getrandom 0.4 backend for Scarlet.
///
/// The workspace sets `--cfg getrandom_backend="custom"` for Scarlet targets.
/// getrandom 0.4 then calls this function via `extern "Rust"`.
#[unsafe(no_mangle)]
pub extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    let buffer = unsafe { core::slice::from_raw_parts_mut(dest, len) };
    let mut offset = 0usize;
    while offset < buffer.len() {
        let result = scarlet_sys::syscall3(
            scarlet_sys::Syscall::GetRandom,
            buffer[offset..].as_mut_ptr() as usize,
            buffer.len() - offset,
            scarlet_sys::GET_RANDOM_FLAG_REQUIRE_ENTROPY,
        );
        if result == usize::MAX || result == 0 || result > buffer.len() - offset {
            return Err(getrandom::Error::new_custom(CUSTOM_RANDOM_ERROR_CODE));
        }
        offset += result;
    }
    Ok(())
}
