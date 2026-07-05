//! AP/CM3 mailbox transport for AVD firmware.

const MAILBOX_AP_TO_CM3: usize = 0x000a_8054;
const MAILBOX_CM3_TO_AP: usize = 0x000a_8064;

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
    }
    Some(command)
}
