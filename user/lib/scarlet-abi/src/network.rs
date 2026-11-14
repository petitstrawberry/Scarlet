//! Versioned link-management ABI. Existing IPv4 v1/v2 records remain unchanged.

pub const LINK_UNKNOWN: u8 = 0;
pub const LINK_DOWN: u8 = 1;
pub const LINK_UP: u8 = 2;
pub const LINK_KIND_ETHERNET: u8 = 1;
pub const LINK_KIND_WIFI: u8 = 2;
pub const IPV4_HAS_GATEWAY: u32 = 1;
pub const IPV4_MAKE_DEFAULT: u32 = 2;
pub const IPV4_CLEAR: u32 = 4;
pub const LINK_STATISTICS_VERSION_V1: u32 = 1;
pub const LINK_STATISTICS_SIZE_V1: u32 = 64;
pub const LINK_STATISTICS_LAYER_LINK: u32 = 1;

/// One registered link. IDs are nonzero and never reused during a boot.
/// Generation changes for observed carrier, MAC, MTU, kind or driver epoch changes.
/// Wi-Fi reports UP only after association and key installation finish.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkLinkInfoV1 {
    pub id: u64,
    pub generation: u64,
    pub name: [u8; 32],
    pub mac_address: [u8; 6],
    pub kind: u8,
    pub state: u8,
    pub mtu: u32,
    pub reserved: u32,
}

impl NetworkLinkInfoV1 {
    pub fn interface_name(&self) -> Option<&str> {
        let end = self.name.iter().position(|byte| *byte == 0)?;
        core::str::from_utf8(&self.name[..end]).ok()
    }

    pub fn is_up(&self) -> bool {
        self.state == LINK_UP
    }

    /// Identify the exact link incarnation for asynchronous work.
    pub fn same_incarnation(&self, other: &Self) -> bool {
        self.id == other.id && self.generation == other.generation
    }
}

/// Read-only, incarnation-checked counter snapshot for one registered link.
/// The caller seeds `id` and `generation` from [`NetworkLinkInfoV1`]; every
/// other field except `version` and `size` must be zero. The kernel replaces
/// the complete record on success without creating a second link namespace.
/// Success returns zero; invalid requests, absent links, stale incarnations and
/// user-copy failures return `-EINVAL`, `-ENODEV`, `-EAGAIN` and `-EFAULT`.
/// The four counters are one interface's current link-level observations; the
/// record does not claim an atomic snapshot across links or with the device.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkReadLinkStatisticsV1 {
    pub version: u32,
    pub size: u32,
    pub id: u64,
    pub generation: u64,
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub tx_bytes: u64,
    pub layer: u32,
    pub reserved: u32,
}

impl NetworkReadLinkStatisticsV1 {
    /// Construct the zero-payload request for one existing link incarnation.
    pub fn query(id: u64, generation: u64) -> Self {
        Self {
            version: LINK_STATISTICS_VERSION_V1,
            size: LINK_STATISTICS_SIZE_V1,
            id,
            generation,
            ..Default::default()
        }
    }

    /// A request may select a link but cannot supply output counters or a layer.
    pub fn is_query(&self) -> bool {
        self.version == LINK_STATISTICS_VERSION_V1
            && self.size == LINK_STATISTICS_SIZE_V1
            && self.rx_packets == 0
            && self.rx_bytes == 0
            && self.tx_packets == 0
            && self.tx_bytes == 0
            && self.layer == 0
            && self.reserved == 0
    }
}

/// Atomically reject an IPv4 update for an obsolete link incarnation.
/// CLEAR requires all address fields and metric to be zero, and no other flags.
/// Configuration requires carrier UP; CLEAR also works while carrier is down.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default)]
pub struct NetworkUpdateIpv4V1 {
    pub id: u64,
    pub generation: u64,
    pub address: [u8; 4],
    pub netmask: [u8; 4],
    pub gateway: [u8; 4],
    pub flags: u32,
    pub metric: u32,
    pub reserved: u32,
}

pub const TRAFFIC_CONFIGURE_VERSION_V1: u32 = 1;
/// Read the supported operation mask without changing meters or attachments.
/// All payload fields must be zero. Success returns one bit per operation.
pub const TRAFFIC_CONFIGURE_QUERY: u32 = 0;
pub const TRAFFIC_CONFIGURE_METER_SET: u32 = 1;
pub const TRAFFIC_CONFIGURE_METER_REMOVE: u32 = 2;
pub const TRAFFIC_CONFIGURE_LINK_SET: u32 = 3;
pub const TRAFFIC_CONFIGURE_LINK_CLEAR: u32 = 4;
pub const TRAFFIC_CONFIGURE_GLOBAL_SET: u32 = 5;
pub const TRAFFIC_CONFIGURE_GLOBAL_CLEAR: u32 = 6;
pub const TRAFFIC_CONFIGURE_LINKS_CLEAR_METER: u32 = 7;
pub const TRAFFIC_CONFIGURE_SUPPORTED_V1: u64 = (1 << TRAFFIC_CONFIGURE_METER_SET)
    | (1 << TRAFFIC_CONFIGURE_METER_REMOVE)
    | (1 << TRAFFIC_CONFIGURE_LINK_SET)
    | (1 << TRAFFIC_CONFIGURE_LINK_CLEAR)
    | (1 << TRAFFIC_CONFIGURE_GLOBAL_SET)
    | (1 << TRAFFIC_CONFIGURE_GLOBAL_CLEAR)
    | (1 << TRAFFIC_CONFIGURE_LINKS_CLEAR_METER);

/// Management adapter request for a named packet meter or its attachment to
/// one link/global scope. Names are UTF-8, NUL-padded and bounded.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkTrafficConfigureV1 {
    pub version: u32,
    pub operation: u32,
    pub remaining: u64,
    pub subject: [u8; 32],
    pub meter: [u8; 32],
}

impl NetworkTrafficConfigureV1 {
    pub fn subject_name(&self) -> Option<&str> {
        let end = self.subject.iter().position(|byte| *byte == 0)?;
        core::str::from_utf8(&self.subject[..end]).ok()
    }
    pub fn meter_name(&self) -> Option<&str> {
        let end = self.meter.iter().position(|byte| *byte == 0)?;
        core::str::from_utf8(&self.meter[..end]).ok()
    }
}

const _: () = {
    assert!(core::mem::size_of::<NetworkLinkInfoV1>() == 64);
    assert!(core::mem::offset_of!(NetworkLinkInfoV1, mtu) == 56);
    assert!(core::mem::size_of::<NetworkReadLinkStatisticsV1>() == 64);
    assert!(core::mem::size_of::<NetworkUpdateIpv4V1>() == 40);
    assert!(core::mem::size_of::<NetworkTrafficConfigureV1>() == 80);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_statistics_request_cannot_prefill_output_fields() {
        let mut request = NetworkReadLinkStatisticsV1::query(7, 3);
        assert!(request.is_query());

        request.rx_packets = 1;
        assert!(!request.is_query());
        request.rx_packets = 0;
        request.rx_bytes = 1;
        assert!(!request.is_query());
        request.rx_bytes = 0;
        request.tx_packets = 1;
        assert!(!request.is_query());
        request.tx_packets = 0;
        request.tx_bytes = 1;
        assert!(!request.is_query());
        request.tx_bytes = 0;
        request.layer = LINK_STATISTICS_LAYER_LINK;
        assert!(!request.is_query());
        request.layer = 0;
        request.reserved = 1;
        assert!(!request.is_query());
        request.reserved = 0;
        request.size = LINK_STATISTICS_SIZE_V1 - 1;
        assert!(!request.is_query());
        request.size = LINK_STATISTICS_SIZE_V1;
        request.version = LINK_STATISTICS_VERSION_V1 + 1;
        assert!(!request.is_query());
    }

    #[test]
    fn incarnation_changes_even_when_name_is_reused() {
        let first = NetworkLinkInfoV1 {
            id: 1,
            generation: 1,
            ..Default::default()
        };
        let replacement = NetworkLinkInfoV1 { id: 2, ..first };
        let reconnected = NetworkLinkInfoV1 {
            generation: 3,
            ..first
        };
        assert!(!first.same_incarnation(&replacement));
        assert!(!first.same_incarnation(&reconnected));
        assert!(first.same_incarnation(&first));
    }
}
