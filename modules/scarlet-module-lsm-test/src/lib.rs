#![no_std]

use scarlet::early_println;

#[no_mangle]
pub extern "C" fn scarlet_lsm_init() -> Result<(), &'static str> {
    early_println!("[lsm-test] Loadable Scarlet Module loaded successfully!");
    Ok(())
}
