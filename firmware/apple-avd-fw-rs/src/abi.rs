//! Firmware mailbox ABI shared conceptually with the Scarlet kernel.

/// Video pipe decode completed.
pub const MSG_VP_DONE: u32 = 0x0000_0100;
/// Post-process pipe completed.
pub const MSG_PP_DONE: u32 = 0x0000_1000;
/// Unexpected IRQ vector.
pub const MSG_UNKNOWN_IRQ: u32 = 0x0001_0000;
