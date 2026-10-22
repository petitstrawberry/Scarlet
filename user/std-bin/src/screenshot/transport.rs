//! Incremental bounded framing shared by capture and host protocol tests.
use std::io::Read;
use sws_remote_protocol::{MessageHeader, decode_header};
pub struct FrameReader {
    header: [u8; MessageHeader::SIZE],
    header_filled: usize,
    parsed_header: Option<MessageHeader>,
    payload: Vec<u8>,
    payload_filled: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};
    struct Fragmented {
        bytes: Cursor<Vec<u8>>,
        blocked: bool,
    }
    impl Read for Fragmented {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.blocked = !self.blocked;
            if self.blocked {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let length = output.len().min(1);
            self.bytes.read(&mut output[..length])
        }
    }
    #[test]
    fn partial_frames_resume_without_losing_header_or_payload() {
        let header = MessageHeader::new(0x8003, 0, 8);
        let mut bytes = header.to_le_bytes().to_vec();
        bytes.extend_from_slice(&800u32.to_le_bytes());
        bytes.extend_from_slice(&600u32.to_le_bytes());
        let mut input = Fragmented {
            bytes: Cursor::new(bytes),
            blocked: false,
        };
        let mut reader = FrameReader::new();
        let mut result = None;
        for _ in 0..40 {
            if let Some(frame) = reader.poll(&mut input).unwrap() {
                result = Some(frame);
                break;
            }
        }
        let (decoded, payload) = result.unwrap();
        assert_eq!(decoded.payload_size, 8);
        assert_eq!(payload.len(), 8);
        assert!(reader.poll(&mut input).is_err() || reader.poll(&mut input).is_err());
    }
    #[test]
    fn oversized_payload_is_rejected_before_allocation() {
        let header = MessageHeader::new(0x8003, 0, u32::MAX).to_le_bytes();
        assert!(FrameReader::new().poll(&mut Cursor::new(header)).is_err());
    }
}

impl FrameReader {
    pub fn new() -> Self {
        Self {
            header: [0; MessageHeader::SIZE],
            header_filled: 0,
            parsed_header: None,
            payload: Vec::new(),
            payload_filled: 0,
        }
    }

    fn reset(&mut self) {
        self.header_filled = 0;
        self.parsed_header = None;
        self.payload.clear();
        self.payload_filled = 0;
    }

    pub fn poll(
        &mut self,
        socket: &mut impl Read,
    ) -> Result<Option<(MessageHeader, Vec<u8>)>, &'static str> {
        while self.header_filled < MessageHeader::SIZE {
            match socket.read(&mut self.header[self.header_filled..]) {
                Ok(0) => return Err("SWS remote connection closed"),
                Ok(count) => self.header_filled += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
                Err(_) => return Err("Failed to read SWS remote header"),
            }
        }
        if self.parsed_header.is_none() {
            let header = decode_header(&self.header).map_err(|_| "Invalid SWS remote header")?;
            self.payload.resize(header.payload_size as usize, 0);
            self.parsed_header = Some(header);
        }
        while self.payload_filled < self.payload.len() {
            match socket.read(&mut self.payload[self.payload_filled..]) {
                Ok(0) => return Err("SWS remote connection closed"),
                Ok(count) => self.payload_filled += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
                Err(_) => return Err("Failed to read SWS remote payload"),
            }
        }
        let header = self.parsed_header.ok_or("Missing SWS remote header")?;
        let payload = core::mem::take(&mut self.payload);
        self.reset();
        Ok(Some((header, payload)))
    }
}
