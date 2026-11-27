//! Incremental H4 framing. After a framing error the transport must be reset:
//! a packet indicator inside an ACL payload is indistinguishable from a header.
extern crate alloc;
use alloc::vec::Vec;

pub const COMMAND: u8 = 1;
pub const ACL: u8 = 2;
pub const EVENT: u8 = 4;
const MAX_ACL: usize = 1024;

#[derive(Default)]
pub struct Decoder {
    frame: Vec<u8>,
    total: Option<usize>,
}

impl Decoder {
    pub fn feed(
        &mut self,
        input: &[u8],
        mut deliver: impl FnMut(u8, &[u8]),
    ) -> Result<(), &'static str> {
        for &byte in input {
            self.frame.push(byte);
            let kind = self.frame[0];
            let header = match kind {
                EVENT => 3,
                ACL => 5,
                3 => 4, // SCO, ignored by the Classic HID host.
                _ => return Err("invalid H4 packet indicator"),
            };
            if self.frame.len() == header {
                let len = match kind {
                    ACL => usize::from(u16::from_le_bytes([self.frame[3], self.frame[4]])),
                    _ => usize::from(byte),
                };
                if kind == ACL && len > MAX_ACL {
                    return Err("oversized H4 ACL frame");
                }
                self.total = Some(header + len);
            }
            if self.total == Some(self.frame.len()) {
                deliver(kind, &self.frame[1..]);
                self.total = None;
                self.frame.clear();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_uart_splits_and_multiple_frames() {
        let stream = [4, 14, 4, 1, 3, 12, 0, 2, 1, 0x20, 2, 0, 4, 2, 4, 1, 0];
        for split in 0..=stream.len() {
            let mut rx = Decoder::default();
            let mut frames = Vec::new();
            for chunk in [&stream[..split], &stream[split..]] {
                rx.feed(chunk, |kind, bytes| frames.push((kind, bytes.to_vec())))
                    .unwrap();
            }
            assert_eq!(
                frames,
                [
                    (4, stream[1..7].to_vec()),
                    (2, stream[8..14].to_vec()),
                    (4, stream[15..].to_vec())
                ]
            );
        }
    }

    #[test]
    fn bogus_length_cannot_allocate_or_inject_a_nested_event() {
        let mut rx = Decoder::default();
        assert!(
            rx.feed(&[2, 1, 0, 255, 255, 4, 14, 0], |_, _| panic!())
                .is_err()
        );
        assert_eq!(rx.frame.len(), 5);
    }
}
