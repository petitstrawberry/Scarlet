//! Rounded surface-local input and backdrop regions. Coordinates and radii
//! are physical pixels; the receiver clips them to the owned surface.

use super::ProtocolError;
use std::vec::Vec;

pub const MAX_REGIONS: usize = 16;
pub const INPUT: u32 = 1;
pub const BACKDROP: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceRegion {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub corner_radius: u32,
    pub blur_radius: u32,
    pub flags: u32,
}

fn valid(region: &SurfaceRegion) -> bool {
    region.width > 0
        && region.height > 0
        && region.x >= 0
        && region.y >= 0
        && i64::from(region.x) + i64::from(region.width) <= i64::from(i32::MAX)
        && i64::from(region.y) + i64::from(region.height) <= i64::from(i32::MAX)
        && region.blur_radius <= 64
        && region.corner_radius <= region.width.min(region.height) / 2
        && region.flags & !(INPUT | BACKDROP) == 0
}

pub fn payload(
    window_id: u32,
    restrict_input: bool,
    regions: &[SurfaceRegion],
) -> Result<Vec<u8>, ProtocolError> {
    if regions.len() > MAX_REGIONS || !regions.iter().all(valid) {
        return Err(ProtocolError::MalformedPayload);
    }
    let mut result = Vec::with_capacity(12 + regions.len() * 28);
    for value in [window_id, u32::from(restrict_input), regions.len() as u32] {
        result.extend_from_slice(&value.to_le_bytes());
    }
    for region in regions {
        for value in [
            region.x as u32,
            region.y as u32,
            region.width,
            region.height,
            region.corner_radius,
            region.blur_radius,
            region.flags,
        ] {
            result.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(result)
}

pub fn parse(bytes: &[u8]) -> Result<(u32, bool, Vec<SurfaceRegion>), ProtocolError> {
    if bytes.len() < 12 {
        return Err(ProtocolError::MalformedPayload);
    }
    let read = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let count = read(8) as usize;
    if read(4) > 1 || count > MAX_REGIONS || bytes.len() != 12 + count * 28 {
        return Err(ProtocolError::MalformedPayload);
    }
    let mut regions = Vec::with_capacity(count);
    for index in 0..count {
        let offset = 12 + index * 28;
        let region = SurfaceRegion {
            x: read(offset) as i32,
            y: read(offset + 4) as i32,
            width: read(offset + 8),
            height: read(offset + 12),
            corner_radius: read(offset + 16),
            blur_radius: read(offset + 20),
            flags: read(offset + 24),
        };
        if !valid(&region) {
            return Err(ProtocolError::MalformedPayload);
        }
        regions.push(region);
    }
    Ok((read(0), read(4) != 0, regions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_empty_input_region() {
        let region = SurfaceRegion {
            x: 20,
            y: 8,
            width: 100,
            height: 44,
            corner_radius: 10,
            blur_radius: 8,
            flags: INPUT | BACKDROP,
        };
        assert_eq!(
            parse(&payload(42, true, &[region]).unwrap()).unwrap(),
            (42, true, vec![region])
        );
        assert_eq!(
            parse(&payload(42, true, &[]).unwrap()).unwrap(),
            (42, true, vec![])
        );
    }

    #[test]
    fn malformed_and_unbounded_regions_are_rejected() {
        assert!(parse(&[0; 11]).is_err());
        let region = SurfaceRegion {
            x: 0,
            y: 0,
            width: 100,
            height: 44,
            corner_radius: 10,
            blur_radius: 8,
            flags: BACKDROP,
        };
        let bytes = payload(1, false, &[region]).unwrap();
        for size in 0..bytes.len() {
            assert!(parse(&bytes[..size]).is_err());
        }
        for invalid in [
            SurfaceRegion { x: -1, ..region },
            SurfaceRegion {
                blur_radius: 65,
                ..region
            },
            SurfaceRegion { flags: 4, ..region },
            SurfaceRegion {
                corner_radius: 30,
                ..region
            },
        ] {
            assert!(payload(1, false, &[invalid]).is_err());
        }
        assert!(payload(1, false, &[region; MAX_REGIONS + 1]).is_err());
    }
}
