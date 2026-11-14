//! Bounded CPU scanout stream. One request is acknowledged before the next.
//! All integers are little endian; payload pixels are premultiplied BGRA8.
pub const HEADER: usize = 32;
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const HELLO: u16 = 1;
pub const FRAME: u16 = 2;
pub const VISIBLE: u16 = 3;
pub const CLOSE: u16 = 4;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: u16,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}
impl Header {
    pub fn decode(b: &[u8]) -> Result<Self, &'static str> {
        if b.len() != HEADER
            || &b[..4] != b"SADF"
            || u16::from_le_bytes(b[4..6].try_into().unwrap()) != 1
        {
            return Err("display protocol header");
        }
        let h = Self {
            kind: u16::from_le_bytes(b[6..8].try_into().unwrap()),
            sequence: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            width: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            height: u32::from_le_bytes(b[20..24].try_into().unwrap()),
            bytes: u32::from_le_bytes(b[28..32].try_into().unwrap()) as usize,
        };
        let stride = u32::from_le_bytes(b[24..28].try_into().unwrap());
        let pixels = u64::from(h.width) * u64::from(h.height) * 4;
        let valid = match h.kind {
            HELLO | FRAME => {
                (2..=4096).contains(&h.width)
                    && (2..=4096).contains(&h.height)
                    && pixels <= MAX_BYTES as u64
                    && stride == h.width * 4
                    && h.bytes == if h.kind == FRAME { pixels as usize } else { 0 }
            }
            VISIBLE => h.width <= 1 && h.height == 0 && stride == 0 && h.bytes == 0,
            CLOSE => h.width == 0 && h.height == 0 && stride == 0 && h.bytes == 0,
            _ => false,
        };
        if !valid || h.sequence == 0 {
            return Err("display protocol layout");
        }
        Ok(h)
    }
    pub fn acknowledgement(self, status: u32) -> [u8; 16] {
        let mut b = [0; 16];
        b[..4].copy_from_slice(b"SADA");
        b[4..8].copy_from_slice(&status.to_le_bytes());
        b[8..].copy_from_slice(&self.sequence.to_le_bytes());
        b
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_checks_dimensions_length_version_and_ack_identity() {
        let mut b = [0; 32];
        b[..4].copy_from_slice(b"SADF");
        b[4..6].copy_from_slice(&1u16.to_le_bytes());
        b[6..8].copy_from_slice(&FRAME.to_le_bytes());
        b[8..16].copy_from_slice(&7u64.to_le_bytes());
        b[16..20].copy_from_slice(&640u32.to_le_bytes());
        b[20..24].copy_from_slice(&480u32.to_le_bytes());
        b[24..28].copy_from_slice(&2560u32.to_le_bytes());
        b[28..32].copy_from_slice(&1228800u32.to_le_bytes());
        let h = Header::decode(&b).unwrap();
        assert_eq!(h.bytes, 1228800);
        assert_eq!(&h.acknowledgement(0)[8..], &7u64.to_le_bytes());
        b[28] = 1;
        assert!(Header::decode(&b).is_err());
        b[28] = 0;
        b[5] = 1;
        assert!(Header::decode(&b).is_err());
    }
}
