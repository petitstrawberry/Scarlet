//! Shared transcript for the synthetic H4 controller and guest. Low command bit selects the peer.
pub const HID: u32 = 0;
pub const OTHER: u32 = 1;
pub const HID_ADDR: [u8; 6] = [0x10, 0, 0, 0, 0, 2];
pub const OTHER_ADDR: [u8; 6] = [0x20, 0, 0, 0, 0, 2];
pub const ADAPTER: [u8; 6] = [1, 0, 0, 0, 0, 2];
pub const HID_NAME: &str = "QA report device";
pub const OTHER_NAME: &str = "QA serial adapter";

pub const SEND_INPUT: u32 = 0x4251_0020;
pub const UNPLUG: u32 = 0x4251_0030;
pub const CONNECTION_REQUESTS: u32 = 0x4251_0040;
pub const OPEN_CHANNELS: u32 = 0x4251_0050;
pub const OUTPUT_COUNT: u32 = 0x4251_0060;
pub const FAIL_DISCONNECT: u32 = 0x4251_0070;
pub const WAKE: u32 = 0x4251_0080;
pub const PAGE_SCAN_ENABLED: u32 = 0x4251_0090;
pub const REJECTED_PAGES: u32 = 0x4251_00a0;
pub const CONTROLLER_FAULT: u32 = 0x4251_00b0;
pub const HOST_PAGES: u32 = 0x4251_00c0;
pub const WAKE_BOTH: u32 = 0x4251_00d0;
pub const COMPLETE_WAKE: u32 = 0x4251_00e0;

// Numbered keyboard input plus two vendor-defined Output reports.
pub const DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x85, 0x16, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0,
    0x25, 1, 0x75, 1, 0x95, 8, 0x81, 2, 0x75, 8, 0x95, 1, 0x81, 1, 0x19, 0, 0x29, 0x65, 0x15, 0,
    0x25, 0x65, 0x75, 8, 0x95, 6, 0x81, 0, 0x06, 0x00, 0xff, 0x09, 0x01, 0x85, 0x47, 0x75, 8, 0x95,
    60, 0x91, 2, 0x09, 0x02, 0x85, 0x18, 0x95, 3, 0x91, 2, 0xc0,
];
pub const PRESS: &[u8] = &[0x16, 0, 0, 4, 0, 0, 0, 0, 0];
pub const RELEASE: &[u8] = &[0x16, 0, 0, 0, 0, 0, 0, 0, 0];
pub const OUTPUT_LONG: &[u8] = &[0x47; 61];
pub const OUTPUT_SHORT: &[u8] = &[0x18, 0xa2, 0, 0xff];
