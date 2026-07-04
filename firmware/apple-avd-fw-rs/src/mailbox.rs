//! Minimal AP mailbox write side for AVD firmware.

const MAILBOX_STATUS: usize = 0x0000_0000;
const MAILBOX_DATA: usize = 0x0000_0004;

/// Send one compact firmware status message to the AP.
///
/// # Arguments
///
/// * `message` - Firmware ABI message value.
pub fn send_message(message: u32) {
    // These offsets are placeholders until the AVD mailbox register map is
    // finalized. Keeping all writes behind this function lets the kernel bring-up
    // patch replace the transport without touching IRQ or panic paths.
    // SAFETY: The firmware runs with direct access to AVD-local MMIO.
    unsafe {
        core::ptr::write_volatile(MAILBOX_DATA as *mut u32, message);
        core::ptr::write_volatile(MAILBOX_STATUS as *mut u32, 1);
    }
}
