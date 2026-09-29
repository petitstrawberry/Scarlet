//! Atomic, ordered surface scenes over extension buffer objects.
//!
//! Layers are bottom-to-top, in physical scene coordinates. Source rectangles
//! use 24.8 fixed point in transformed buffer pixels; transform values match
//! the eight orthogonal Wayland transforms. A scene owns its selected buffers
//! until replaced, including buffers whose object was subsequently destroyed.
use crate::{ProtocolError, read_i32, read_u32, read_u64};
use std::vec::Vec;

pub const MAX_LAYERS: usize = 256;
pub const MAX_PIXELS: usize = 16 * 1024 * 1024;
const LAYER_BYTES: usize = 44;
const HEADER_BYTES: usize = 36;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer {
    pub surface_id: u32,
    pub buffer_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub source_x: i32,
    pub source_y: i32,
    pub source_width: i32,
    pub source_height: i32,
    pub transform: u32,
}
impl Layer {
    pub fn valid(&self) -> bool {
        self.surface_id != 0
            && self.buffer_id != 0
            && self.width != 0
            && self.height != 0
            && self.width <= 16384
            && self.height <= 16384
            && self.transform < 8
            && self.source_x >= 0
            && self.source_y >= 0
            && self.source_width > 0
            && self.source_height > 0
    }
    pub fn fits_buffer(&self, width: u32, height: u32) -> bool {
        let (w, h) = if self.transform & 1 != 0 {
            (height, width)
        } else {
            (width, height)
        };
        self.valid()
            && i64::from(self.source_x) + i64::from(self.source_width) <= i64::from(w) * 256
            && i64::from(self.source_y) + i64::from(self.source_height) <= i64::from(h) * 256
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub external_client_id: u32,
    pub window_id: u32,
    pub serial: u64,
    /// Origin of the complete scene relative to the managed root surface.
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
}
impl Commit {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.serial == 0
            || self.layers.len() > MAX_LAYERS
            || self.width > 16384
            || self.height > 16384
            || self.width as u64 * self.height as u64 > MAX_PIXELS as u64
            || (!self.layers.is_empty() && (self.width == 0 || self.height == 0))
        {
            return Err(ProtocolError::MalformedPayload);
        }
        for (i, l) in self.layers.iter().enumerate() {
            if !l.valid()
                || self.layers[..i]
                    .iter()
                    .any(|v| v.surface_id == l.surface_id)
                || l.x < 0
                || l.y < 0
                || i64::from(l.x) + i64::from(l.width) > i64::from(self.width)
                || i64::from(l.y) + i64::from(l.height) > i64::from(self.height)
            {
                return Err(ProtocolError::MalformedPayload);
            }
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;
        let mut out = Vec::with_capacity(HEADER_BYTES + self.layers.len() * LAYER_BYTES);
        out.extend_from_slice(&self.external_client_id.to_le_bytes());
        out.extend_from_slice(&self.window_id.to_le_bytes());
        out.extend_from_slice(&self.serial.to_le_bytes());
        for v in [
            self.origin_x as u32,
            self.origin_y as u32,
            self.width,
            self.height,
            self.layers.len() as u32,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for l in &self.layers {
            for v in [
                l.surface_id,
                l.buffer_id,
                l.x as u32,
                l.y as u32,
                l.width,
                l.height,
                l.source_x as u32,
                l.source_y as u32,
                l.source_width as u32,
                l.source_height as u32,
                l.transform,
            ] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let count = read_u32(bytes, 32)? as usize;
        if count > MAX_LAYERS || bytes.len() != HEADER_BYTES + count * LAYER_BYTES {
            return Err(ProtocolError::MalformedPayload);
        }
        let mut layers = Vec::with_capacity(count);
        for i in 0..count {
            let b = &bytes[HEADER_BYTES + i * LAYER_BYTES..];
            layers.push(Layer {
                surface_id: read_u32(b, 0)?,
                buffer_id: read_u32(b, 4)?,
                x: read_i32(b, 8)?,
                y: read_i32(b, 12)?,
                width: read_u32(b, 16)?,
                height: read_u32(b, 20)?,
                source_x: read_i32(b, 24)?,
                source_y: read_i32(b, 28)?,
                source_width: read_i32(b, 32)?,
                source_height: read_i32(b, 36)?,
                transform: read_u32(b, 40)?,
            });
        }
        let result = Self {
            external_client_id: read_u32(bytes, 0)?,
            window_id: read_u32(bytes, 4)?,
            serial: read_u64(bytes, 8)?,
            origin_x: read_i32(bytes, 16)?,
            origin_y: read_i32(bytes, 20)?,
            width: read_u32(bytes, 24)?,
            height: read_u32(bytes, 28)?,
            layers,
        };
        result.validate()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn commit() -> Commit {
        Commit {
            external_client_id: 7,
            window_id: 8,
            serial: 9,
            origin_x: -10,
            origin_y: 0,
            width: 20,
            height: 10,
            layers: std::vec![Layer {
                surface_id: 1,
                buffer_id: 2,
                x: 0,
                y: 0,
                width: 20,
                height: 10,
                source_x: 128,
                source_y: 0,
                source_width: 256,
                source_height: 256,
                transform: 0
            }],
        }
    }
    #[test]
    fn scene_wire_roundtrip_and_truncation() {
        let c = commit();
        let wire = c.encode().unwrap();
        assert_eq!(Commit::decode(&wire).unwrap(), c);
        for len in 0..wire.len() {
            assert!(Commit::decode(&wire[..len]).is_err());
        }
        let mut extended = wire;
        extended.push(0);
        assert!(Commit::decode(&extended).is_err());
    }
    #[test]
    fn reject_duplicates_overflow_and_unknown_transforms() {
        let mut c = commit();
        c.layers.push(c.layers[0]);
        assert!(c.encode().is_err());
        let mut c = commit();
        c.layers[0].transform = 8;
        assert!(c.encode().is_err());
        let mut c = commit();
        c.layers[0].x = i32::MAX;
        assert!(c.encode().is_err());
        let mut c = commit();
        c.width = u32::MAX;
        assert!(c.encode().is_err());
        let mut c = commit();
        c.serial = 0;
        assert!(c.encode().is_err());
    }
    #[test]
    fn transformed_crop_uses_rotated_extent() {
        let mut layer = commit().layers[0];
        layer.source_x = 0;
        layer.source_width = 256;
        layer.source_height = 512;
        layer.transform = 1;
        assert!(layer.fits_buffer(2, 1));
        layer.transform = 0;
        assert!(!layer.fits_buffer(2, 1));
    }
    #[test]
    fn empty_scene_unmaps() {
        let mut c = commit();
        c.layers.clear();
        c.width = 0;
        c.height = 0;
        assert_eq!(Commit::decode(&c.encode().unwrap()).unwrap(), c);
    }
}
