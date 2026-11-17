//! Monitor identity from EDID, independent of its transport connector.
//! EDID base layout: Linux include/drm/drm_edid.h (MIT), struct edid and
//! detailed_non_pixel. A hash is a fingerprint, never proof of a unique unit.
use alloc::{format, string::String};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorIdentity {
    pub key: String,
    pub name: String,
    pub has_serial: bool,
    pub physical_size_mm: Option<(u32, u32)>,
}

impl MonitorIdentity {
    pub fn from_edid(bytes: &[u8]) -> Option<Self> {
        let base = bytes.get(..128)?;
        if base[..8] != [0, 255, 255, 255, 255, 255, 255, 0]
            || base.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0 { return None; }
        let manufacturer = u16::from_be_bytes([base[8], base[9]]);
        let product = u16::from_le_bytes([base[10], base[11]]);
        let numeric = u32::from_le_bytes(base[12..16].try_into().ok()?);
        let descriptor = |kind: u8| -> Option<String> {
            let field = base[54..126].chunks_exact(18).find(|d| d[0..3] == [0, 0, 0] && d[3] == kind)?;
            let end = field[5..18].iter().position(|b| *b == 0 || *b == b'\n').unwrap_or(13);
            let text = core::str::from_utf8(&field[5..5 + end]).ok()?.trim();
            (!text.is_empty() && text.bytes().all(|b| (32..127).contains(&b))).then(|| String::from(text))
        };
        let serial = descriptor(0xff).filter(|s| s.bytes().any(|b| b != b'0' && b != b' '));
        let name = descriptor(0xfc).unwrap_or_else(|| format!("Display {:04x}:{:04x}", manufacturer, product));
        let (key, has_serial) = if let Some(serial) = serial {
            let mut hex = String::new();
            for byte in serial.as_bytes() { use core::fmt::Write; let _ = write!(hex, "{:02x}", byte); }
            (format!("edid:{:04x}:{:04x}:s:{}", manufacturer, product, hex), true)
        } else if numeric != 0 && numeric != u32::MAX {
            (format!("edid:{:04x}:{:04x}:n:{:08x}", manufacturer, product, numeric), true)
        } else {
            let hash = Sha256::digest(base);
            let mut hex = String::new();
            for byte in hash { use core::fmt::Write; let _ = write!(hex, "{:02x}", byte); }
            (format!("edid-sha256:{}", hex), false)
        };
        let physical_size_mm = (base[21] != 0 && base[22] != 0).then_some((u32::from(base[21]) * 10, u32::from(base[22]) * 10));
        Some(Self { key, name, has_serial, physical_size_mm })
    }
}
