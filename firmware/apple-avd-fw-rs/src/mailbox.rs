//! AP/CM3 mailbox transport for AVD firmware.

const MAILBOX_AP_TO_CM3: usize = 0x0000_0200;
const MAILBOX_CM3_TO_AP: usize = 0x0000_0204;
const MAILBOX_STATUS: usize = 0x0000_0208;
const MAILBOX_STATUS_CM3_PENDING: u32 = 1 << 0;
const MAILBOX_STATUS_AP_PENDING: u32 = 1 << 1;

/// Send one compact firmware status message to the AP.
///
/// # Arguments
///
/// * `message` - Firmware ABI message value.
pub fn send_message(message: u32) {
    // SAFETY: The firmware runs with direct access to AVD-local MMIO.
    unsafe {
        core::ptr::write_volatile(MAILBOX_CM3_TO_AP as *mut u32, message);
        core::ptr::write_volatile(MAILBOX_STATUS as *mut u32, MAILBOX_STATUS_CM3_PENDING);
    }
}

/// Receive one AP-to-CM3 command if present.
///
/// # Returns
///
/// Raw command word, or `None` when the AP mailbox is empty.
pub fn receive_command() -> Option<u32> {
    // SAFETY: The firmware runs with direct access to AVD-local MMIO.
    let command = unsafe { core::ptr::read_volatile(MAILBOX_AP_TO_CM3 as *const u32) };
    if command == 0 {
        return None;
    }
    // SAFETY: Clearing the command word acknowledges that the firmware has
    // consumed this AP-to-CM3 command.
    unsafe {
        core::ptr::write_volatile(MAILBOX_AP_TO_CM3 as *mut u32, 0);
        core::ptr::write_volatile(MAILBOX_STATUS as *mut u32, MAILBOX_STATUS_AP_PENDING);
    }
    Some(command)
}
