//! Native Bluetooth controller V1, one adapter and at most one active peer per handle.
//! Each command borrows a writable, zero-initialized 288-byte record for the duration
//! of HandleControl. Fill only the specified inputs. Success returns 0. The kernel
//! zeroes unspecified output, especially KEY on every command except READ_KEY.
//! All integer fields are little-endian. ADAPTER and PEER are six-byte BD_ADDRs in
//! HCI wire order: display 01:23:45:67:89:AB as the reverse of [AB,89,67,45,23,01].
//! KEY is the 16 HCI link-key octets in their original order.
//!
//! STATUS outputs FLAGS, GENERATION, COUNT, LAST_ERROR, ADAPTER and PEER (zero
//! when absent). SCAN_START, PAIR, CONNECT and DISCONNECT each return a new
//! adapter-wide GENERATION on acceptance, including synchronous completion.
//! Poll STATUS for that generation until BUSY clears. LAST_ERROR is then the
//! operation result (zero on success). A superseding operation changes GENERATION.
//! SCANNING implies BUSY. Idle scan COUNT is the number of completed results.
//! SCAN_RESULT takes GENERATION and zero-based INDEX, returns PEER and NAME_LEN
//! bytes of NAME (no terminator). Reject stale generations and out-of-range indices.
//! PAIR takes PEER. CONNECT takes PEER and optionally KEY_SUPPLIED, KEY_TYPE and
//! KEY. DISCONNECT targets the active link. ENCRYPTED and PROFILE_READY report
//! negotiated link state independently. Successful PAIR may set KEY_READABLE.
//! READ_KEY takes pairing GENERATION and PEER and returns KEY_TYPE and KEY.
//! Restrict key export to the authorized pairing caller and matching generation
//! and peer. Clear KEY_READABLE when that key is no longer available.

use core::ops::Range;

pub const STATUS: u32 = 0x4254_0001;
pub const SCAN_START: u32 = 0x4254_0002;
pub const SCAN_RESULT: u32 = 0x4254_0003;
pub const PAIR: u32 = 0x4254_0004;
pub const CONNECT: u32 = 0x4254_0005;
pub const DISCONNECT: u32 = 0x4254_0006;
pub const READ_KEY: u32 = 0x4254_0007;

pub const RECORD_SIZE: usize = 288;
pub type Record = [u8; RECORD_SIZE];
pub type Address = [u8; 6];
pub const FLAGS: Range<usize> = 0..4;
pub const GENERATION: Range<usize> = 4..8;
pub const INDEX: Range<usize> = 8..10;
pub const COUNT: Range<usize> = 10..12;
pub const LAST_ERROR: Range<usize> = 12..16;
pub const ADAPTER: Range<usize> = 16..22;
pub const PEER: Range<usize> = 22..28;
pub const NAME_LEN: usize = 28;
pub const KEY_TYPE: usize = 29;
// Bytes 30..32 are reserved (zero).
pub const KEY: Range<usize> = 32..48;
pub const NAME: Range<usize> = 48..RECORD_SIZE;

pub const READY: u32 = 1 << 0;
pub const BUSY: u32 = 1 << 1;
pub const SCANNING: u32 = 1 << 2;
pub const CONNECTED: u32 = 1 << 3;
pub const ENCRYPTED: u32 = 1 << 4;
pub const PROFILE_READY: u32 = 1 << 5;
pub const KEY_READABLE: u32 = 1 << 6;
/// CONNECT input only.
pub const KEY_SUPPLIED: u32 = 1 << 7;
