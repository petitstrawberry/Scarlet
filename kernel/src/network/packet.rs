use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::string::String;

use crate::network::error::NetworkError;

/// Packet direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketDirection {
    /// Incoming packet (from network device to application)
    Incoming,
    /// Outgoing packet (from application to network device)
    Outgoing,
}

impl PacketDirection {
    /// Get the opposite direction
    pub fn opposite(self) -> Self {
        match self {
            PacketDirection::Incoming => PacketDirection::Outgoing,
            PacketDirection::Outgoing => PacketDirection::Incoming,
        }
    }
}

/// Network packet structure
/// Manages payload (actual data), headers (each layer's headers), and hints (transmission instruction info)
#[derive(Debug, Clone)]
pub struct NetworkPacket {
    /// Packet payload part
    payload: Vec<u8>,
    /// Header information for each protocol layer (layer name -> header data)
    headers: BTreeMap<String, Vec<u8>>,
    /// Hints for inter-layer information passing during transmission (key -> value)
    hints: BTreeMap<String, String>,
    /// Packet direction
    direction: PacketDirection,
}

impl NetworkPacket {
    /// Create a new NetworkPacket
    pub fn new(payload: Vec<u8>) -> Self {
        Self {
            payload,
            headers: BTreeMap::new(),
            hints: BTreeMap::new(),
            direction: PacketDirection::Outgoing, // Default direction
        }
    }

    /// Create an empty NetworkPacket
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Get payload
    pub fn payload(&self) -> &Vec<u8> {
        &self.payload
    }

    /// Set payload
    pub fn set_payload(&mut self, payload: Vec<u8>) {
        self.payload = payload;
    }

    /// Validate payload size
    pub fn validate_payload_size(&self, required_size: usize) -> Result<(), NetworkError> {
        if self.payload.len() < required_size {
            return Err(NetworkError::insufficient_payload_size(required_size, self.payload.len()));
        }
        Ok(())
    }

    /// Add header
    pub fn add_header(&mut self, layer: &str, header: Vec<u8>) {
        self.headers.insert(String::from(layer), header);
    }

    /// Get header
    pub fn get_header(&self, layer: &str) -> Option<&Vec<u8>> {
        self.headers.get(layer)
    }

    /// Get all headers
    pub fn headers(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.headers
    }

    /// Set hint
    pub fn set_hint(&mut self, key: &str, value: &str) {
        self.hints.insert(String::from(key), String::from(value));
    }

    /// Get hint
    pub fn get_hint(&self, key: &str) -> Option<&str> {
        self.hints.get(key).map(|s| s.as_str())
    }

    /// Get all hints
    pub fn hints(&self) -> &BTreeMap<String, String> {
        &self.hints
    }

    /// Get total packet data size (payload + all headers)
    pub fn total_size(&self) -> usize {
        let headers_size: usize = self.headers.values().map(|h| h.len()).sum();
        self.payload.len() + headers_size
    }

    /// Get packet direction
    pub fn direction(&self) -> PacketDirection {
        self.direction
    }

    /// Set packet direction
    pub fn set_direction(&mut self, direction: PacketDirection) {
        self.direction = direction;
    }
}
