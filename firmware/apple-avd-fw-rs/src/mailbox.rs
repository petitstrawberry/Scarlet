//! AP/CM3 mailbox transport for AVD firmware.

const CM3_MBOX_BASE: usize = 0x5001_0000;
const MAILBOX_CM3_TO_AP: usize = CM3_MBOX_BASE + 0x60;
const CM3_BOOT: usize = CM3_MBOX_BASE + 0x90;

/// Mark the firmware as booted in the AP-visible CM3 boot flag.
pub fn signal_booted() {
    // SAFETY: The firmware runs with direct access to AVD-local MMIO.
    unsafe {
        core::ptr::write_volatile(CM3_BOOT as *mut u32, 1);
    }
}

/// Send one compact firmware status message to the AP.
///
/// # Arguments
///
/// * `message` - Firmware ABI message value.
pub fn send_message(message: u32) {
    // SAFETY: The firmware runs with direct access to AVD-local MMIO.
    unsafe {
        core::ptr::write_volatile(MAILBOX_CM3_TO_AP as *mut u32, message);
    }
}
