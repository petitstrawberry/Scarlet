//! Firmware mailbox ABI shared conceptually with the Scarlet kernel.

/// Firmware initialized and waiting for work.
pub const MSG_READY: u32 = 0x0000_0001;
/// Firmware panic or hardfault.
pub const MSG_PANIC: u32 = 0x0000_0002;
/// Video pipe decode completed.
pub const MSG_VP_DONE: u32 = 0x0000_0100;
/// Video pipe decode error.
pub const MSG_VP_ERROR: u32 = 0x0000_0200;
/// Post-process pipe completed.
pub const MSG_PP_DONE: u32 = 0x0000_1000;
/// Unexpected IRQ vector.
pub const MSG_UNKNOWN_IRQ: u32 = 0x0001_0000;

/// AP-to-CM3 H.264 decode command kind.
pub const CMD_H264_DECODE: u32 = 0x10;
/// High-byte shift for AP-to-CM3 command kind.
pub const CMD_KIND_SHIFT: u32 = 24;
/// Low-bit mask for AP-to-CM3 command tags.
pub const CMD_TAG_MASK: u32 = 0x0000_ffff;

/// Return the AP-to-CM3 command kind.
///
/// # Arguments
///
/// * `command` - Raw AP-to-CM3 command word.
///
/// # Returns
///
/// Command kind field.
pub const fn command_kind(command: u32) -> u32 {
    command >> CMD_KIND_SHIFT
}

/// Return the AP-to-CM3 command tag.
///
/// # Arguments
///
/// * `command` - Raw AP-to-CM3 command word.
///
/// # Returns
///
/// Low tag bits.
pub const fn command_tag(command: u32) -> u32 {
    command & CMD_TAG_MASK
}
