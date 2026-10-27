//! Native Wi-Fi control V1. Commands use one bounded inline buffer and no pointers
//! inside payloads. All records have explicit padding and accept every bit pattern.

pub const ABI_VERSION: u16 = 1;
pub const HEADER_SIZE: usize = 64;
pub const MAX_BUFFER: usize = 65536;
pub const MAX_SESSIONS: usize = 16;
pub const MAX_REQUESTS: usize = 64;
pub const MAX_EVENTS: usize = 64;
pub const MAX_SCANS: usize = 4;
pub const MAX_TIMEOUT_MS: u32 = 60000;
pub const DEFAULT_TIMEOUT_MS: u32 = 5000;

pub const GET_INFO: u32 = 0x5749_0001;
pub const GET_CAPABILITIES: u32 = 0x5749_0002;
pub const GET_STATUS: u32 = 0x5749_0003;
pub const CLAIM_CONTROLLER: u32 = 0x5749_0004;
pub const RELEASE_CONTROLLER: u32 = 0x5749_0005;
pub const SUBSCRIBE_EVENTS: u32 = 0x5749_0006;
pub const DRAIN_EVENTS: u32 = 0x5749_0007;
pub const RESYNC: u32 = 0x5749_0008;
pub const SCAN: u32 = 0x5749_0010;
pub const GET_SCAN_RESULTS: u32 = 0x5749_0011;
pub const RELEASE_SCAN_RESULTS: u32 = 0x5749_0012;
pub const CONNECT: u32 = 0x5749_0013;
pub const DISCONNECT: u32 = 0x5749_0014;
pub const CANCEL_REQUEST: u32 = 0x5749_0015;
pub const GET_REQUEST: u32 = 0x5749_0016;
pub const ACK_REQUEST: u32 = 0x5749_0017;
pub const CONNECT_RSN: u32 = 0x5749_0018;
// Authentication transport and confirmed key operations; credentials remain reserved.
pub const TX_EAPOL: u32 = 0x5749_0020;
pub const INSTALL_KEY: u32 = 0x5749_0021;
pub const DELETE_KEY: u32 = 0x5749_0022;
pub const SET_CONTROLLED_PORT: u32 = 0x5749_0023;
pub const SET_CREDENTIAL: u32 = 0x5749_0024;
pub const RX_EAPOL: u32 = 0x5749_0025;
pub const GET_SECURITY_STATUS: u32 = 0x5749_0026;
pub const GET_ASSOCIATION_RSN: u32 = 0x5749_0027;
pub const MOCK_SECURITY_STATS: u32 = 0x5749_ff02;
// Available only with the explicitly enabled test backend, never on a real radio.
pub const MOCK_STEP: u32 = 0x5749_ff00;
pub const MOCK_DATA_TEST: u32 = 0x5749_ff01;
pub const DATA_STATS: u32 = 0;
pub const DATA_HOLD: u32 = 1;
pub const DATA_RELEASE: u32 = 2;
pub const DATA_REPLAY_STALE: u32 = 3;

pub const OK: i32 = 0;
pub const ACCEPTED: i32 = 1;
pub const WOULD_BLOCK: i32 = 2;
pub const BUSY: i32 = 3;
pub const NOT_SUPPORTED: i32 = 4;
pub const INVALID: i32 = 5;
pub const STALE_DEVICE: i32 = 6;
pub const STALE_CONNECTION: i32 = 7;
pub const STATE_CHANGED: i32 = 8;
pub const NOT_OWNER: i32 = 9;
pub const BUFFER_TOO_SMALL: i32 = 10;
pub const NOT_FOUND: i32 = 11;
pub const DEVICE_GONE: i32 = 12;
pub const RESOURCE_LIMIT: i32 = 13;
pub const CANCELED: i32 = 14;
pub const TIMED_OUT: i32 = 15;
pub const FAILED: i32 = 16;

pub const RADIO_READY: u32 = 1;
pub const RADIO_REMOVED: u32 = 2;
pub const IDLE: u32 = 0;
pub const AUTHENTICATING: u32 = 1;
pub const ASSOCIATING: u32 = 2;
pub const ASSOCIATED: u32 = 3;
pub const AUTHORIZED: u32 = 4;
pub const DISCONNECTING: u32 = 5;
pub const MLME_FIRMWARE: u32 = 1;
pub const MLME_HOST: u32 = 2;
pub const AKM_OPEN: u32 = 1;
/// External authenticator plumbing. This is not a claim of WPA interoperability.
pub const AKM_EXTERNAL: u32 = 2;
pub const CIPHER_CCMP128: u32 = 1;
pub const KEY_PAIRWISE: u32 = 1;
pub const KEY_GROUP: u32 = 2;
pub const MAX_EAPOL: usize = 1024;
pub const MAX_EAPOL_QUEUE: usize = 32;
pub const SECURITY_PAIRWISE: u32 = 1;
pub const SECURITY_GROUP: u32 = 2;
pub const SECURITY_BLOCKED: u32 = 4;
pub const SECURITY_RETIRE_FAILED: u32 = 8;
pub const FEATURE_MOCK: u32 = 1;
pub const FEATURE_DATA: u32 = 2;
pub const FEATURE_RSN: u32 = 32;
pub const RSN_CCMP_PSK_IE: [u8; 22] = [
    48, 20, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 2, 0, 0,
];
pub const FEATURE_EAPOL: u32 = 4;
pub const FEATURE_KEYS: u32 = 8;
pub const FEATURE_CONTROL_PORT: u32 = 16;
pub const FEATURE_AUTH: u32 = FEATURE_EAPOL | FEATURE_KEYS | FEATURE_CONTROL_PORT;
/// The firmware performs the WPA2-Personal 4-way handshake itself;
/// credentials never cross this ABI, the driver derives or reads them locally.
pub const FEATURE_FW_PSK: u32 = 64;
pub const EVENT_STATE: u32 = 1;
pub const EVENT_COMPLETE: u32 = 2;
pub const EVENT_SCAN: u32 = 3;
pub const EVENT_RESET: u32 = 4;
pub const EVENT_REMOVED: u32 = 5;
pub const EVENT_OVERFLOW: u32 = 6;
pub const EVENT_EAPOL: u32 = 7;
pub const EVENT_MASK_ALL: u32 = 0x7f;
pub const STEP_SUCCESS: u32 = 1;
pub const STEP_FAILURE: u32 = 2;
pub const STEP_RESET: u32 = 3;
pub const STEP_REMOVE: u32 = 4;
pub const STEP_ASSOCIATED: u32 = 5;
pub const STEP_RETIRE_FAILURE: u32 = 6;
pub const STEP_RETIRE_RECOVER: u32 = 7;

/// A wire record with no padding or restricted bit patterns.
///
/// # Safety
/// Every byte must be initialized, with no implicit padding, references, enums or
/// bools. All bit patterns must be valid; every field must have fixed-width layout.
pub unsafe trait Wire: Copy + Default {
    fn bytes(&self) -> &[u8] {
        // SAFETY: implementors guarantee initialized padding-free storage.
        unsafe {
            core::slice::from_raw_parts(
                self as *const Self as *const u8,
                core::mem::size_of::<Self>(),
            )
        }
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != core::mem::size_of::<Self>() {
            return None;
        }
        // SAFETY: exact-size read; unaligned input is supported; every bit pattern is valid.
        Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Self) })
    }
}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub abi_version: u16,
    pub header_len: u16,
    pub buffer_len: u32,
    pub request_id: u64,
    pub link_id: u64,
    pub device_epoch: u64,
    pub connection_id: u64,
    pub expected_state_seq: u64,
    pub flags: u32,
    pub result: i32,
    pub reserved: u64,
}
impl Header {
    pub fn new(buffer_len: usize) -> Self {
        Self {
            abi_version: ABI_VERSION,
            header_len: HEADER_SIZE as u16,
            buffer_len: buffer_len as u32,
            ..Self::default()
        }
    }
    pub fn valid(&self) -> bool {
        self.abi_version == ABI_VERSION
            && self.header_len as usize == HEADER_SIZE
            && (HEADER_SIZE..=MAX_BUFFER).contains(&(self.buffer_len as usize))
            && self.flags == 0
            && self.reserved == 0
            && self.result == 0
    }
}
// SAFETY: fixed-width fields cover all 64 bytes, with no implicit padding.
unsafe impl Wire for Header {}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub mlme: u32,
    pub akm_mask: u32,
    pub features: u32,
    pub max_sessions: u32,
    pub max_requests: u32,
    pub max_events: u32,
    pub max_timeout_ms: u32,
    pub reserved: u32,
}
// SAFETY: eight u32 fields, no padding or invalid bit patterns.
unsafe impl Wire for Capabilities {}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub availability: u32,
    pub phase: u32,
    pub connect_request: u64,
    pub scan_request: u64,
    pub network_generation: u64,
    pub link_epoch: u64,
    /// Last notification sequence for this session, including dropped events.
    pub event_sequence: u64,
    pub last_result: i32,
    pub reserved: u32,
}
// SAFETY: explicit fields fill all 56 bytes.
unsafe impl Wire for Status {}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub timeout_ms: u32,
    pub reserved: u32,
}
// SAFETY: two unrestricted u32 fields.
unsafe impl Wire for Scan {}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Connect {
    pub ssid: [u8; 32],
    pub ssid_len: u32,
    pub akm: u32,
    pub timeout_ms: u32,
    pub reserved: u32,
}
// SAFETY: fixed-width fields cover all 48 bytes.
unsafe impl Wire for Connect {}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Id {
    pub id: u64,
}
// SAFETY: single unrestricted u64.
unsafe impl Wire for Id {}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanPage {
    pub scan_id: u64,
    pub cursor: u32,
    pub reserved: u32,
}
// SAFETY: explicit fields fill 16 bytes.
unsafe impl Wire for ScanPage {}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bss {
    pub ssid: [u8; 32],
    pub bssid: [u8; 6],
    pub ssid_len: u8,
    pub reserved: u8,
    pub frequency_mhz: u32,
    pub signal_mbm: i32,
    pub akm: u32,
    pub reserved2: u32,
    pub seen_ns: u64,
}
// SAFETY: fields explicitly fill all 64 bytes.
unsafe impl Wire for Bss {}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequestInfo {
    pub id: u64,
    pub connection_id: u64,
    pub object_id: u64,
    pub command: u32,
    pub result: i32,
}
// SAFETY: explicit fields fill 32 bytes.
unsafe impl Wire for RequestInfo {}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub link_id: u64,
    pub device_epoch: u64,
    pub connection_id: u64,
    pub request_id: u64,
    pub state_seq: u64,
    pub network_generation: u64,
    pub kind: u32,
    pub result: i32,
    pub phase: u32,
    pub reserved: u32,
    pub object_id: u64,
    pub reserved2: u64,
}
// SAFETY: explicit fields fill 96 bytes.
unsafe impl Wire for Event {}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Token {
    pub session: u64,
    pub request: u64,
    pub epoch: u64,
    pub connection: u64,
}
// SAFETY: four u64 fields.
unsafe impl Wire for Token {}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MockStep {
    pub token: Token,
    pub step: u32,
    pub reserved: u32,
}
// SAFETY: explicit fields fill 40 bytes.
unsafe impl Wire for MockStep {}

/// Test-only bounded packet-queue control and counters. Never exposed by a
/// real backend. Input counters must be zero; output contains kernel counters.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DataTest {
    pub operation: u32,
    pub reserved: u32,
    pub tx_queued: u64,
    pub rx_queued: u64,
    pub tx_dropped: u64,
    pub rx_dropped: u64,
    pub tx_submitted: u64,
    pub rx_delivered: u64,
    pub arp_queued: u64,
    pub retirements: u64,
}
// SAFETY: explicit fixed-width fields cover all 72 bytes, with no invalid bits.
unsafe impl Wire for DataTest {}

/// EAPOL PDU only (Ethernet header supplied/validated by the kernel).
/// TX: sequence/reserved/tail zero; PROTECTED requests CCMP protection.
/// RX: PROTECTED is set only after authenticated unicast PTK decryption.
#[repr(C, align(8))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Eapol {
    pub sequence: u64,
    pub peer: [u8; 6],
    pub reserved: u16,
    pub length: u32,
    pub flags: u32,
    pub payload: [u8; MAX_EAPOL],
}
impl Default for Eapol {
    fn default() -> Self {
        Self {
            sequence: 0,
            peer: [0; 6],
            reserved: 0,
            length: 0,
            flags: 0,
            payload: [0; MAX_EAPOL],
        }
    }
}
// SAFETY: explicit fixed-width fields fill 1048 bytes, all patterns valid.
unsafe impl Wire for Eapol {}

/// Version is monotonically increasing per slot for the connection, including
/// failed/deleted installs. Only 16-byte CCMP keys and a six-byte RX PN are supported.
#[repr(C, align(8))]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyInstall {
    pub version: u64,
    pub cipher: u32,
    pub kind: u32,
    pub index: u32,
    pub key_len: u32,
    pub peer: [u8; 6],
    pub reserved: [u8; 2],
    pub rx_sequence: [u8; 8],
    pub key: [u8; 32],
}
// SAFETY: explicit fixed-width fields fill 72 bytes, all patterns valid.
unsafe impl Wire for KeyInstall {}
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyRef {
    pub version: u64,
    pub kind: u32,
    pub index: u32,
}
// SAFETY: explicit fields fill 16 bytes.
unsafe impl Wire for KeyRef {}
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortControl {
    pub key_revision: u64,
    pub authorized: u32,
    pub reserved: u32,
}
// SAFETY: explicit fields fill 16 bytes.
unsafe impl Wire for PortControl {}
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SecurityStatus {
    pub key_revision: u64,
    pub rx_sequence: u64,
    pub keys_installed: u32,
    pub keys_pending: u32,
    pub eapol_pending: u32,
    pub flags: u32,
    pub peer: [u8; 6],
    pub reserved: [u8; 2],
}
// SAFETY: explicit fields fill 40 bytes.
unsafe impl Wire for SecurityStatus {}
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MockSecurityStats {
    pub installed: u32,
    pub pending: u32,
    pub retired: u64,
}
// SAFETY: explicit fields fill 16 bytes.
unsafe impl Wire for MockSecurityStats {}

/// Explicit WPA2-Personal/CCMP association request. No passphrase crosses the ABI.
#[repr(C, align(8))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RsnConnect {
    pub connect: Connect,
    pub ie_len: u32,
    pub reserved: u32,
    pub ie: [u8; 64],
}
impl Default for RsnConnect {
    fn default() -> Self {
        Self {
            connect: Connect::default(),
            ie_len: 0,
            reserved: 0,
            ie: [0; 64],
        }
    }
}
// SAFETY: explicit fields cover 120 bytes, all bit patterns valid.
unsafe impl Wire for RsnConnect {}
/// Backend-reported AP security IE bundle and selected association IE for the
/// live connection. The AP buffer is one complete RSNE, optionally followed
/// by one complete RSNXE; legacy callers may provide only the RSNE. No ABI size change.
#[repr(C, align(8))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AssociationRsn {
    pub own_len: u32,
    pub ap_len: u32,
    pub peer: [u8; 6],
    pub reserved: [u8; 2],
    pub own_ie: [u8; 64],
    pub ap_ie: [u8; 64],
}
impl Default for AssociationRsn {
    fn default() -> Self {
        Self {
            own_len: 0,
            ap_len: 0,
            peer: [0; 6],
            reserved: [0; 2],
            own_ie: [0; 64],
            ap_ie: [0; 64],
        }
    }
}
// SAFETY: explicit fields cover 144 bytes, all bit patterns valid.
unsafe impl Wire for AssociationRsn {}
const _: () = {
    assert!(core::mem::size_of::<RsnConnect>() == 120);
    assert!(core::mem::size_of::<AssociationRsn>() == 144);
    assert!(core::mem::size_of::<Eapol>() == 1048);
    assert!(core::mem::offset_of!(Eapol, payload) == 24);
    assert!(core::mem::size_of::<KeyInstall>() == 72);
    assert!(core::mem::size_of::<KeyRef>() == 16);
    assert!(core::mem::size_of::<PortControl>() == 16);
    assert!(core::mem::size_of::<SecurityStatus>() == 40);
    assert!(core::mem::size_of::<MockSecurityStats>() == 16);
    assert!(core::mem::size_of::<DataTest>() == 72);
    assert!(core::mem::size_of::<Header>() == 64);
    assert!(core::mem::offset_of!(Header, reserved) == 56);
    assert!(core::mem::size_of::<Capabilities>() == 32);
    assert!(core::mem::size_of::<Status>() == 56);
    assert!(core::mem::offset_of!(Status, event_sequence) == 40);
    assert!(core::mem::size_of::<Connect>() == 48);
    assert!(core::mem::size_of::<ScanPage>() == 16);
    assert!(core::mem::size_of::<Bss>() == 64);
    assert!(core::mem::offset_of!(Bss, seen_ns) == 56);
    assert!(core::mem::size_of::<RequestInfo>() == 32);
    assert!(core::mem::size_of::<Event>() == 96);
    assert!(core::mem::size_of::<MockStep>() == 40);
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_roundtrip_unaligned_and_validation() {
        let mut header = Header::new(HEADER_SIZE);
        header.request_id = 0x123456789abcdef0;
        let mut storage = [0u8; HEADER_SIZE + 1];
        storage[1..].copy_from_slice(header.bytes());
        assert_eq!(Header::decode(&storage[1..]), Some(header));
        assert!(header.valid());
        header.reserved = 1;
        assert!(!header.valid());
        assert!(Header::decode(&storage).is_none());
    }
}

/// Software CCMP-128 data protection with key-scoped packet numbers.
pub const FEATURE_CCMP: u32 = 64;
pub const GET_CCMP_STATUS: u32 = 0x5749_0028;
/// Counters only, never key bytes. PN values are diagnostic snapshots.
#[derive(Clone, Copy, Default)]
#[repr(C, align(8))]
pub struct CcmpStatus {
    pub tx_pn: u64,
    pub rx_accepted: u64,
    pub replay_dropped: u64,
    pub integrity_dropped: u64,
    pub invalid_dropped: u64,
    pub retired: u64,
    pub largest_rx_pn: u64,
    pub keys: u32,
    pub reserved: u32,
}
unsafe impl Wire for CcmpStatus {}
const _: () = assert!(core::mem::size_of::<CcmpStatus>() == 64);

#[cfg(test)]
mod ccmp_contract_tests {
    use super::*;
    #[test]
    fn ccmp_status_has_one_bounded_pointer_independent_layout() {
        assert_eq!(core::mem::size_of::<CcmpStatus>(), 64);
        assert_eq!(core::mem::align_of::<CcmpStatus>(), 8);
        let status = CcmpStatus {
            tx_pn: (1u64 << 48) - 1,
            keys: 2,
            ..CcmpStatus::default()
        };
        let decoded = CcmpStatus::decode(status.bytes()).unwrap();
        assert_eq!(
            (decoded.tx_pn, decoded.keys, decoded.reserved),
            (status.tx_pn, 2, 0)
        );
        assert!(CcmpStatus::decode(&status.bytes()[..63]).is_none());
    }
}

/// Optional authenticated EAPOL transport and atomic GTK replacement.
pub const FEATURE_PROTECTED_EAPOL: u32 = 128;
pub const FEATURE_GROUP_REKEY: u32 = 256;
pub const EAPOL_PROTECTED: u32 = 1;
pub const REPLACE_GROUP_KEY: u32 = 0x5749_0029;

#[cfg(test)]
mod protected_eapol_contract_tests {
    use super::*;
    #[test]
    fn protection_metadata_keeps_the_bounded_eapol_wire_layout() {
        let mut frame = Eapol::default();
        frame.flags = EAPOL_PROTECTED;
        frame.peer = [2, 0, 0, 0, 0, 1];
        frame.length = 4;
        frame.payload[..4].copy_from_slice(&[2, 3, 0, 0]);
        let decoded = Eapol::decode(frame.bytes()).unwrap();
        assert_eq!((decoded.flags, decoded.length), (1, 4));
        assert_eq!(core::mem::size_of::<Eapol>(), 1048);
        assert_eq!(
            FEATURE_GROUP_REKEY & (FEATURE_PROTECTED_EAPOL | FEATURE_CCMP | FEATURE_AUTH),
            0
        );
    }
}
