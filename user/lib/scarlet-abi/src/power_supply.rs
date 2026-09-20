//! Read-only power-supply snapshots. The v1 wire format is 96 little-endian
//! bytes on every architecture; Rust struct layout is not part of this ABI.
//!
//! COUNT returns the number of boot-stable IDs in 0..count. SNAPSHOT reads a
//! 12-byte (version, size, ID) header and writes one complete snapshot back to
//! the same 96-byte buffer. Unsupported quantities remain absent, not zero.

pub const POWER_SUPPLY_ABI_VERSION: u32 = 1;
pub const POWER_SUPPLY_SNAPSHOT_SIZE: usize = 96;
pub const POWER_SUPPLY_MAX_DEVICES: usize = 16;
pub const SCTL_POWER_SUPPLY_COUNT: u32 = 0x5350_0500;
pub const SCTL_POWER_SUPPLY_SNAPSHOT: u32 = 0x5350_0501;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum SupplyKind {
    #[default]
    Unknown = 0,
    Battery = 1,
    Mains = 2,
    Usb = 3,
    Ups = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ChargeState {
    Charging = 1,
    Discharging = 2,
    NotCharging = 3,
    Full = 4,
}

/// One current observation. `online` describes an external source delivering
/// power, not cable detection. Battery current is positive into the battery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerSupplyState {
    pub present: Option<bool>,
    pub online: Option<bool>,
    pub charge_state: Option<ChargeState>,
    /// State of charge in tenths of one percent, inclusive 0..=1000.
    pub capacity_permille: Option<u32>,
    pub voltage_uv: Option<u32>,
    pub current_ua: Option<i32>,
    pub temperature_mc: Option<i32>,
    /// Configured input limit, NOT measured input current or USB-PD contract.
    pub input_current_limit_ua: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerSupplySnapshot {
    pub id: u32,
    pub kind: SupplyKind,
    pub name: [u8; 32],
    /// Monotonic time of this observation (or failed read attempt).
    pub sampled_at_ns: u64,
    /// The provider failed. All state fields are absent in this case.
    pub read_failed: bool,
    pub state: PowerSupplyState,
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

pub fn snapshot_request(id: u32) -> [u8; POWER_SUPPLY_SNAPSHOT_SIZE] {
    let mut bytes = [0; POWER_SUPPLY_SNAPSHOT_SIZE];
    bytes[..4].copy_from_slice(&POWER_SUPPLY_ABI_VERSION.to_le_bytes());
    bytes[4..8].copy_from_slice(&(POWER_SUPPLY_SNAPSHOT_SIZE as u32).to_le_bytes());
    bytes[8..12].copy_from_slice(&id.to_le_bytes());
    bytes
}

pub fn snapshot_request_id(header: &[u8]) -> Option<u32> {
    (header.len() >= 12
        && word(header, 0) == POWER_SUPPLY_ABI_VERSION
        && word(header, 4) == POWER_SUPPLY_SNAPSHOT_SIZE as u32)
        .then(|| word(header, 8))
}

impl PowerSupplySnapshot {
    pub fn name(&self) -> &str {
        let end = self.name.iter().position(|b| *b == 0).unwrap_or(32);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }

    pub fn encode(&self) -> [u8; POWER_SUPPLY_SNAPSHOT_SIZE] {
        let mut bytes = snapshot_request(self.id);
        bytes[12..16].copy_from_slice(&(self.kind as u32).to_le_bytes());
        bytes[16..20].copy_from_slice(&u32::from(self.read_failed).to_le_bytes());
        bytes[24..32].copy_from_slice(&self.sampled_at_ns.to_le_bytes());
        bytes[64..96].copy_from_slice(&self.name);
        let state = if self.read_failed {
            PowerSupplyState::default()
        } else {
            self.state
        };
        let values = [
            state.present.map(u32::from),
            state.online.map(u32::from),
            state.charge_state.map(|v| v as u32),
            state.capacity_permille.filter(|v| *v <= 1000),
            state.voltage_uv,
            state.current_ua.map(|v| v as u32),
            state.temperature_mc.map(|v| v as u32),
            state.input_current_limit_ua,
        ];
        let mut valid = 0u32;
        for (index, value) in values.into_iter().enumerate() {
            if let Some(value) = value {
                valid |= 1 << index;
                bytes[32 + 4 * index..36 + 4 * index].copy_from_slice(&value.to_le_bytes());
            }
        }
        bytes[20..24].copy_from_slice(&valid.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != POWER_SUPPLY_SNAPSHOT_SIZE {
            return None;
        }
        let id = snapshot_request_id(bytes)?;
        let kind = match word(bytes, 12) {
            0 => SupplyKind::Unknown,
            1 => SupplyKind::Battery,
            2 => SupplyKind::Mains,
            3 => SupplyKind::Usb,
            4 => SupplyKind::Ups,
            _ => return None,
        };
        let flags = word(bytes, 16);
        let valid = word(bytes, 20);
        if flags & !1 != 0 || valid & !0xff != 0 || (flags == 1 && valid != 0) {
            return None;
        }
        let value =
            |index: usize| (valid & (1u32 << index) != 0).then(|| word(bytes, 32 + index * 4));
        for index in [0, 1] {
            if value(index).is_some_and(|v| v > 1) {
                return None;
            }
        }
        let charge_state = match value(2) {
            None => None,
            Some(1) => Some(ChargeState::Charging),
            Some(2) => Some(ChargeState::Discharging),
            Some(3) => Some(ChargeState::NotCharging),
            Some(4) => Some(ChargeState::Full),
            _ => return None,
        };
        if value(3).is_some_and(|v| v > 1000) {
            return None;
        }
        let name: [u8; 32] = bytes[64..96].try_into().ok()?;
        let end = name.iter().position(|b| *b == 0)?;
        core::str::from_utf8(&name[..end]).ok()?;
        Some(Self {
            id,
            kind,
            name,
            sampled_at_ns: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
            read_failed: flags == 1,
            state: PowerSupplyState {
                present: value(0).map(|v| v == 1),
                online: value(1).map(|v| v == 1),
                charge_state,
                capacity_permille: value(3),
                voltage_uv: value(4),
                current_ua: value(5).map(|v| v as i32),
                temperature_mc: value(6).map(|v| v as i32),
                input_current_limit_ua: value(7),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_wire_units_and_unknown_are_distinct_from_zero() {
        let sample = PowerSupplySnapshot {
            id: 3,
            kind: SupplyKind::Battery,
            sampled_at_ns: 0x0102030405060708,
            state: PowerSupplyState {
                present: Some(true),
                capacity_permille: Some(0),
                current_ua: Some(-156250),
                temperature_mc: Some(-10000),
                ..Default::default()
            },
            ..Default::default()
        };
        let bytes = sample.encode();
        assert_eq!(&bytes[..12], &[1, 0, 0, 0, 96, 0, 0, 0, 3, 0, 0, 0]);
        assert_eq!(&bytes[24..32], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(PowerSupplySnapshot::decode(&bytes), Some(sample));
        assert_eq!(word(&bytes, 20), 0b01101001);
        let failed = PowerSupplySnapshot {
            read_failed: true,
            ..sample
        }
        .encode();
        assert_eq!(word(&failed, 20), 0);
        assert_eq!(
            PowerSupplySnapshot::decode(&failed).unwrap().state,
            PowerSupplyState::default()
        );
    }
    #[test]
    fn rejects_malformed_request_and_invalid_values() {
        assert_eq!(snapshot_request_id(&[0; 11]), None);
        for (offset, value) in [(0, 2), (4, 95), (12, 99), (16, 2), (20, 256)] {
            let mut bytes = PowerSupplySnapshot::default().encode();
            bytes[offset..offset + 4].copy_from_slice(&u32::to_le_bytes(value));
            assert!(PowerSupplySnapshot::decode(&bytes).is_none());
        }
        let mut bytes = PowerSupplySnapshot::default().encode();
        bytes[20] = 1;
        bytes[32] = 2;
        assert!(PowerSupplySnapshot::decode(&bytes).is_none());
        bytes[20] = 8;
        bytes[44..48].copy_from_slice(&1001u32.to_le_bytes());
        assert!(PowerSupplySnapshot::decode(&bytes).is_none());
    }
}
