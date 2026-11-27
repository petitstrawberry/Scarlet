//! Test-only controls on /dev/bt-qa-radio. Low command bit selects left (0) or right (1).
pub const REPORT: u32 = 0x4251_0010;
pub const UNPLUG: u32 = 0x4251_0020;
pub const LIGHTS: u32 = 0x4251_0030;
pub const INTERLEAVE: u32 = 0x4251_0040;
pub const ECHO_COUNT: u32 = 0x4251_0050;
pub const ACK_LIGHTS: u32 = 0x4251_0060;

/// Buttons in wire order, followed by absolute axes already oriented right/down.
pub const fn sample(buttons: u32, x: u16, y: u16) -> u64 {
    buttons as u64 | (x as u64) << 24 | (y as u64) << 36
}
