//! Native Bluetooth controller V1, one adapter with multiple simultaneous Classic links.
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
//! KEY. Without KEY_SUPPLIED, CONNECT may reuse the registered bond for PEER.
//! DISCONNECT requires exactly one live link. DISCONNECT_PEER takes PEER.
//! PAIR and CONNECT complete on an authenticated, encrypted ACL. Successful
//! PAIR may set KEY_READABLE immediately. PROFILE_READY reports whether a
//! discovered HID service has configured its control and interrupt channels.
//! READ_KEY takes pairing GENERATION and PEER and returns KEY_TYPE and KEY.
//! Restrict key export to the authorized pairing caller and matching generation
//! and peer. Operations on other peers leave the key available.
//!
//! LINKS returns COUNT of connected links and a link snapshot GENERATION.
//! LINK_RESULT takes that GENERATION and zero-based INDEX. Retry LINKS on a
//! stale generation. Each row returns ADAPTER, PEER, NAME_LEN/NAME, FLAGS,
//! LAST_ERROR and the same link snapshot GENERATION.
//! PEER_STATUS takes PEER and returns the same fields with that peer's most
//! recent operation GENERATION. It remains available after failure or loss
//! until the bounded history evicts the inactive peer. BUSY, CONNECTED,
//! ENCRYPTED, PROFILE_READY and KEY_READABLE describe only that peer.
//! PAIR, CONNECT and both disconnect commands echo the selected PEER.
//! DISCONNECT_PEER returns the accepted operation GENERATION like PAIR and
//! CONNECT. Poll PEER_STATUS for that generation and a clear BUSY bit.
//! EVICT_KEY takes PEER and completes the same way. It only succeeds once the
//! peer has no outstanding attempt or physical link. An absent key is success.

use core::ops::Range;

pub const STATUS: u32 = 0x4254_0001;
pub const SCAN_START: u32 = 0x4254_0002;
pub const SCAN_RESULT: u32 = 0x4254_0003;
pub const PAIR: u32 = 0x4254_0004;
pub const CONNECT: u32 = 0x4254_0005;
pub const DISCONNECT: u32 = 0x4254_0006;
pub const READ_KEY: u32 = 0x4254_0007;
pub const LINKS: u32 = 0x4254_0008;
pub const LINK_RESULT: u32 = 0x4254_0009;
pub const PEER_STATUS: u32 = 0x4254_000a;
pub const DISCONNECT_PEER: u32 = 0x4254_000b;
pub const EVICT_KEY: u32 = 0x4254_000c;
/// Register a saved SSP bond for peer-initiated, authenticated connections.
/// PEER, KEY_TYPE, KEY and optional ADAPTER are inputs. Synchronous, idempotent.
pub const REGISTER_BOND: u32 = 0x4254_000d;

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
/// STATUS, LINK_RESULT and PEER_STATUS, from the remote HID SDP record.
pub const HID_RECONNECT_MODE: usize = 30;
pub const HID_RECONNECT_DEVICE: u8 = 1;
pub const HID_RECONNECT_HOST: u8 = 2;
// Byte 31 is reserved (zero).
pub const KEY: Range<usize> = 32..48;
pub const NAME: Range<usize> = 48..RECORD_SIZE;
/// STATUS only, in place of NAME. Changes each time a controller becomes ready,
/// including resets that retain the same Bluetooth address and device path.
pub const ADAPTER_GENERATION: Range<usize> = 48..56;

pub const READY: u32 = 1 << 0;
pub const BUSY: u32 = 1 << 1;
pub const SCANNING: u32 = 1 << 2;
pub const CONNECTED: u32 = 1 << 3;
pub const ENCRYPTED: u32 = 1 << 4;
/// Both HID channels for the discovered service are configured.
pub const PROFILE_READY: u32 = 1 << 5;
pub const KEY_READABLE: u32 = 1 << 6;
/// CONNECT input only.
pub const KEY_SUPPLIED: u32 = 1 << 7;
