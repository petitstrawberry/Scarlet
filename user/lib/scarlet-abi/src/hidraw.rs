//! Native raw HID reports from registered HID devices at `/dev/hidrawN`.
//! Open, control, read and write require physical-seat management authority.
//! Each open sees input reports arriving after it opens. A read or write
//! transfers one whole report in descriptor-defined wire format (numbered
//! reports include their ID). There is no length, timestamp or transport header.
//! A short read buffer preserves the report. An overrun reports one error on
//! that reader and resumes at the oldest retained report. On unplug, readers
//! drain retained reports and get EOF. New devices can reuse the pathname.
//!
//! INFO copies an Info to the supplied address, returning zero. GET_DESCRIPTOR
//! takes a pointer to Transfer and returns the descriptor length copied. The
//! buffer must fit the complete descriptor. Feature requests also take a
//! Transfer. A transport without Feature support returns an error. Output
//! writes require CAN_OUTPUT and available queue space (write readiness).

pub const INFO: u32 = 0x4852_0001;
pub const GET_DESCRIPTOR: u32 = 0x4852_0002;
pub const GET_FEATURE: u32 = 0x4852_0003;
pub const SET_FEATURE: u32 = 0x4852_0004;
pub const SLOTS: usize = 32;
pub const MAX_NAME: usize = 248;
pub const MAX_REPORT: usize = 4096;
pub const MAX_DESCRIPTOR: usize = 4096;

pub const USB: u16 = 3;
pub const BLUETOOTH: u16 = 5;
pub const HAS_IDENTITY: u16 = 1;
pub const CAN_OUTPUT: u16 = 2;
pub const CAN_GET_FEATURE: u16 = 4;
pub const CAN_SET_FEATURE: u16 = 8;

/// Fixed-width native-endian identity. Unused transport and identity fields
/// are zero. Addresses for Bluetooth are in HCI wire order. Name bytes are
/// length-delimited and need not be UTF-8.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Info {
    pub transport: u16,
    pub flags: u16,
    pub vendor_id_source: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    pub version: u16,
    pub descriptor_len: u32,
    pub max_output_len: u32,
    pub controller: [u8; 6],
    pub peer: [u8; 6],
    pub usb_bus: u16,
    pub usb_address: u8,
    pub usb_interface: u8,
    pub name_len: u16,
    pub name: [u8; MAX_NAME],
    pub reserved: u16,
}

impl Default for Info {
    fn default() -> Self {
        Self {
            transport: 0,
            flags: 0,
            vendor_id_source: 0,
            vendor_id: 0,
            product_id: 0,
            version: 0,
            descriptor_len: 0,
            max_output_len: 0,
            controller: [0; 6],
            peer: [0; 6],
            usb_bus: 0,
            usb_address: 0,
            usb_interface: 0,
            name_len: 0,
            name: [0; MAX_NAME],
            reserved: 0,
        }
    }
}

/// User buffer address and capacity in bytes. Use report_id zero for descriptor
/// queries or unnumbered Feature reports. Reserved bytes must be zero.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Transfer {
    pub address: u64,
    pub len: u32,
    pub report_id: u8,
    pub reserved: [u8; 3],
}

const _: () = assert!(core::mem::size_of::<Info>() == 288);
const _: () = assert!(core::mem::size_of::<Transfer>() == 16);
