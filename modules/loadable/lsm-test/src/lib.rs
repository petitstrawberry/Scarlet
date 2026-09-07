#![no_std]

use scarlet::println;

#[unsafe(no_mangle)]
pub static SCARLET_LSM_NAME: [u8; 9] = *b"lsm-test\0";

#[unsafe(no_mangle)]
pub static SCARLET_LSM_BUILD_INFO: [u8; 72] = {
    let s = concat!(env!("RUSTC_VERSION"), ";", env!("TARGET"), "\0");
    let bytes: &[u8] = s.as_bytes();
    let mut arr = [0u8; 72];
    let mut i = 0;
    while i < bytes.len() && i < 72 {
        arr[i] = bytes[i];
        i += 1;
    }
    arr
};

#[unsafe(no_mangle)]
pub static SCARLET_LSM_DEPENDS: [u8; 256] = {
    let s = concat!(env!("SCARLET_LSM_DEPENDS"), "\0");
    let bytes: &[u8] = s.as_bytes();
    let mut arr = [0u8; 256];
    let mut i = 0;
    while i < bytes.len() && i < 256 {
        arr[i] = bytes[i];
        i += 1;
    }
    arr
};

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_lsm_init() -> Result<(), &'static str> {
    println!("[lsm-test] Loadable Scarlet Module loaded successfully!");
    Ok(())
}
