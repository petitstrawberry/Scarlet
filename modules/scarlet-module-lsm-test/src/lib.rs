#![no_std]

use scarlet::early_println;

#[unsafe(no_mangle)]
pub static SCARLET_LSM_NAME: [u8; 24] = *b"scarlet-module-lsm-test\0";

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_lsm_init() -> Result<(), &'static str> {
    early_println!("[lsm-test] Loadable Scarlet Module loaded successfully!");
    Ok(())
}
