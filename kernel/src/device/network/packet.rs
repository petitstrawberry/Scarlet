//! Network packet structure for the flexible pipeline
//!
//! This module provides the NetworkPacket structure that supports the flexible pipeline
//! architecture by maintaining separate headers and payload data.

use hashbrown::HashMap;
use alloc::{string::String, vec::Vec};
use super::error::NetworkError;

/// Time information for packet metadata
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instant {
    /// Microseconds since system boot
    pub micros: u64,
}

impl Instant {
    /// Create a new instant with the current time
    pub fn now() -> Self {
        // In a real implementation, this would get the actual system time
        // For now, we'll use a placeholder implementation
        Self { micros: 0 }
    }

    /// Create an instant from microseconds
    pub fn from_micros(micros: u64) -> Self {
        Self { micros }
    }
}

/// Network packet with flexible header/payload separation
///
/// This packet structure enables the flexible pipeline processing by:
/// - Maintaining headers from each processing stage separately
/// - Keeping the current payload for the next stage to process
/// - Preserving metadata like receive time and device information
#[derive(Debug, Clone)]
pub struct NetworkPacket {
    /// Current payload data (data for the next stage to process)
    payload: Vec<u8>,
    /// Headers extracted by each stage, indexed by stage name
    headers: HashMap<String, Vec<u8>>,
    /// Timestamp when packet was received
    received_at: Instant,
    /// Name of the device that received this packet
    device_name: String,
}

impl NetworkPacket {
    /// Create a new network packet with initial data
    pub fn new(data: Vec<u8>, device_name: String) -> Self {
        Self {
            payload: data,
            headers: HashMap::new(),
            received_at: Instant::now(),
            device_name,
        }
    }

    /// Get the current payload data (read-only)
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Set new payload data (typically after a stage processes part of it)
    pub fn set_payload(&mut self, payload: Vec<u8>) {
        self.payload = payload;
    }

    /// Add header data from a processing stage
    ///
    /// # Arguments
    /// * `stage_name` - Name of the stage (e.g., "ethernet", "ipv4", "tcp")
    /// * `header_data` - The header bytes extracted by this stage
    pub fn add_header(&mut self, stage_name: &str, header_data: Vec<u8>) {
        self.headers.insert(String::from(stage_name), header_data);
    }

    /// Get header data for a specific stage
    ///
    /// # Arguments  
    /// * `stage_name` - Name of the stage to get headers for
    ///
    /// # Returns
    /// Header data as a byte slice, or None if stage headers not found
    pub fn get_header(&self, stage_name: &str) -> Option<&[u8]> {
        self.headers.get(stage_name).map(|v| v.as_slice())
    }

    /// Get all header names that have been stored
    pub fn header_names(&self) -> Vec<&str> {
        self.headers.keys().map(|k| k.as_str()).collect()
    }

    /// Get the packet receive timestamp
    pub fn received_at(&self) -> Instant {
        self.received_at
    }

    /// Get the device name that received this packet
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Get total packet size (all headers + current payload)
    pub fn total_size(&self) -> usize {
        let headers_size: usize = self.headers.values().map(|h| h.len()).sum();
        headers_size + self.payload.len()
    }

    /// Get the size of stored headers
    pub fn headers_size(&self) -> usize {
        self.headers.values().map(|h| h.len()).sum()
    }

    /// Parse a big-endian u16 from a stage's header at the given offset
    ///
    /// # Arguments
    /// * `stage_name` - Name of the stage whose header to parse
    /// * `offset` - Byte offset within the header
    ///
    /// # Returns
    /// The parsed u16 value, or None if insufficient data
    pub fn parse_u16_be(&self, stage_name: &str, offset: usize) -> Option<u16> {
        let header = self.get_header(stage_name)?;
        if header.len() < offset + 2 {
            return None;
        }
        Some(u16::from_be_bytes([header[offset], header[offset + 1]]))
    }

    /// Parse a big-endian u32 from a stage's header at the given offset
    ///
    /// # Arguments
    /// * `stage_name` - Name of the stage whose header to parse
    /// * `offset` - Byte offset within the header
    ///
    /// # Returns  
    /// The parsed u32 value, or None if insufficient data
    pub fn parse_u32_be(&self, stage_name: &str, offset: usize) -> Option<u32> {
        let header = self.get_header(stage_name)?;
        if header.len() < offset + 4 {
            return None;
        }
        Some(u32::from_be_bytes([
            header[offset],
            header[offset + 1], 
            header[offset + 2],
            header[offset + 3]
        ]))
    }

    /// Parse bytes from a stage's header
    ///
    /// # Arguments
    /// * `stage_name` - Name of the stage whose header to parse
    /// * `offset` - Byte offset within the header
    /// * `len` - Number of bytes to extract
    ///
    /// # Returns
    /// The requested bytes as a slice, or None if insufficient data
    pub fn parse_bytes(&self, stage_name: &str, offset: usize, len: usize) -> Option<&[u8]> {
        let header = self.get_header(stage_name)?;
        if header.len() < offset + len {
            return None;
        }
        Some(&header[offset..offset + len])
    }

    /// Validate packet has minimum required payload size
    pub fn validate_payload_size(&self, min_size: usize) -> Result<(), NetworkError> {
        if self.payload.len() < min_size {
            Err(NetworkError::insufficient_data(min_size, self.payload.len()))
        } else {
            Ok(())
        }
    }

    /// Create a copy of this packet (for broadcasting, etc.)
    pub fn clone_packet(&self) -> NetworkPacket {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{vec, string::String};

    #[test_case]
    fn test_packet_creation() {
        let data = vec![0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let packet = NetworkPacket::new(data.clone(), String::from("eth0"));
        
        assert_eq!(packet.payload(), &data);
        assert_eq!(packet.device_name(), "eth0");
        assert_eq!(packet.total_size(), 6);
        assert_eq!(packet.headers_size(), 0);
    }

    #[test_case]
    fn test_header_operations() {
        let data = vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let mut packet = NetworkPacket::new(data, String::from("eth0"));

        // Add ethernet header
        packet.add_header("ethernet", vec![0x01, 0x02, 0x03, 0x04]);
        assert_eq!(packet.get_header("ethernet"), Some(&[0x01, 0x02, 0x03, 0x04][..]));
        assert!(packet.get_header("ipv4").is_none());

        // Add IPv4 header
        packet.add_header("ipv4", vec![0x45, 0x00, 0x00, 0x20]);
        assert_eq!(packet.get_header("ipv4"), Some(&[0x45, 0x00, 0x00, 0x20][..]));

        // Check header names
        let names = packet.header_names();
        assert!(names.contains(&"ethernet"));
        assert!(names.contains(&"ipv4"));
        assert_eq!(names.len(), 2);

        // Check sizes
        assert_eq!(packet.headers_size(), 8); // 4 + 4 bytes
        assert_eq!(packet.total_size(), 14); // 8 headers + 6 payload
    }

    #[test_case]
    fn test_payload_operations() {
        let initial_data = vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        let mut packet = NetworkPacket::new(initial_data, String::from("test"));

        // Simulate ethernet stage: remove first 4 bytes as header
        let ethernet_header = packet.payload()[0..4].to_vec();
        packet.add_header("ethernet", ethernet_header);
        packet.set_payload(packet.payload()[4..].to_vec());

        assert_eq!(packet.get_header("ethernet"), Some(&[0x01, 0x02, 0x03, 0x04][..]));
        assert_eq!(packet.payload(), &[0x05, 0x06]);
        assert_eq!(packet.total_size(), 6); // Original size preserved
    }

    #[test_case]
    fn test_parsing_helpers() {
        let mut packet = NetworkPacket::new(vec![], String::from("test"));
        
        // Add a header with various data types
        packet.add_header("test", vec![
            0x12, 0x34,             // u16 at offset 0
            0x56, 0x78, 0x9A, 0xBC, // u32 at offset 2
            0xDE, 0xAD, 0xBE, 0xEF  // more bytes at offset 6
        ]);

        // Test u16 parsing
        assert_eq!(packet.parse_u16_be("test", 0), Some(0x1234));
        assert_eq!(packet.parse_u16_be("test", 8), Some(0xBEEF));
        assert_eq!(packet.parse_u16_be("test", 9), None); // Not enough data

        // Test u32 parsing  
        assert_eq!(packet.parse_u32_be("test", 2), Some(0x56789ABC));
        assert_eq!(packet.parse_u32_be("test", 6), Some(0xDEADBEEF));
        assert_eq!(packet.parse_u32_be("test", 7), None); // Not enough data

        // Test bytes parsing
        assert_eq!(packet.parse_bytes("test", 0, 2), Some(&[0x12, 0x34][..]));
        assert_eq!(packet.parse_bytes("test", 6, 4), Some(&[0xDE, 0xAD, 0xBE, 0xEF][..]));
        assert_eq!(packet.parse_bytes("test", 8, 4), None); // Not enough data

        // Test non-existent stage
        assert_eq!(packet.parse_u16_be("nonexistent", 0), None);
    }

    #[test_case]
    fn test_validation() {
        let data = vec![0x01, 0x02, 0x03];
        let packet = NetworkPacket::new(data, String::from("test"));

        // Should pass with smaller requirement
        assert!(packet.validate_payload_size(2).is_ok());
        assert!(packet.validate_payload_size(3).is_ok());

        // Should fail with larger requirement
        let result = packet.validate_payload_size(5);
        assert!(result.is_err());
        match result {
            Err(NetworkError::InsufficientData { required, available }) => {
                assert_eq!(required, 5);
                assert_eq!(available, 3);
            }
            _ => panic!("Expected InsufficientData error"),
        }
    }

    #[test_case]
    fn test_packet_clone() {
        let data = vec![0xAA, 0xBB];
        let mut packet = NetworkPacket::new(data.clone(), String::from("test"));
        packet.add_header("eth", vec![0x01, 0x02]);

        let cloned = packet.clone_packet();
        assert_eq!(cloned.payload(), packet.payload());
        assert_eq!(cloned.get_header("eth"), packet.get_header("eth"));
        assert_eq!(cloned.device_name(), packet.device_name());
    }

    #[test_case]
    fn test_instant() {
        let instant1 = Instant::from_micros(12345);
        let instant2 = Instant::from_micros(12345);
        assert_eq!(instant1, instant2);
        assert_eq!(instant1.micros, 12345);

        // Test that now() doesn't panic
        let _now = Instant::now();
    }
}