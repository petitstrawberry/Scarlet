//! Versioned link-management ABI. Existing IPv4 v1/v2 records remain unchanged.

pub const LINK_UNKNOWN: u8 = 0;
pub const LINK_DOWN: u8 = 1;
pub const LINK_UP: u8 = 2;
pub const LINK_KIND_ETHERNET: u8 = 1;
pub const LINK_KIND_WIFI: u8 = 2;
pub const IPV4_HAS_GATEWAY: u32 = 1;
pub const IPV4_MAKE_DEFAULT: u32 = 2;
pub const IPV4_CLEAR: u32 = 4;

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
pub const TRAFFIC_CONFIGURE_METER_SET: u32 = 1;
pub const TRAFFIC_CONFIGURE_METER_REMOVE: u32 = 2;
pub const TRAFFIC_CONFIGURE_LINK_SET: u32 = 3;
pub const TRAFFIC_CONFIGURE_LINK_CLEAR: u32 = 4;
pub const TRAFFIC_CONFIGURE_GLOBAL_SET: u32 = 5;
pub const TRAFFIC_CONFIGURE_GLOBAL_CLEAR: u32 = 6;
pub const TRAFFIC_CONFIGURE_LINKS_CLEAR_METER: u32 = 7;

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
    assert!(core::mem::size_of::<NetworkUpdateIpv4V1>() == 40);
    assert!(core::mem::size_of::<NetworkTrafficConfigureV1>() == 80);
};

#[cfg(test)]
mod tests {
    use super::*;
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
