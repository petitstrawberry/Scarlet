//! NextStageMatcher implementations for O(1) protocol routing
//!
//! This module provides HashMap-based matchers for high-performance routing
//! decisions within network protocol handlers.

use hashbrown::HashMap;
use alloc::string::String;
use super::{
    error::NetworkError,
    traits::NextStageMatcher,
};

// ===== Protocol Constants =====

/// Ethernet protocol types (EtherType values)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EtherType {
    /// IPv4 protocol
    IPv4 = 0x0800,
    /// IPv6 protocol  
    IPv6 = 0x86DD,
    /// Address Resolution Protocol
    ARP = 0x0806,
}

impl EtherType {
    /// Convert EtherType to u16 value
    pub fn as_u16(self) -> u16 {
        self as u16
    }
    
    /// Try to convert u16 to EtherType
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0x0800 => Some(EtherType::IPv4),
            0x86DD => Some(EtherType::IPv6),
            0x0806 => Some(EtherType::ARP),
            _ => None,
        }
    }
}

/// IP protocol numbers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IpProtocol {
    /// Internet Control Message Protocol
    ICMP = 1,
    /// Transmission Control Protocol
    TCP = 6,
    /// User Datagram Protocol
    UDP = 17,
}

impl IpProtocol {
    /// Convert IpProtocol to u8 value
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    
    /// Try to convert u8 to IpProtocol
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(IpProtocol::ICMP),
            6 => Some(IpProtocol::TCP),
            17 => Some(IpProtocol::UDP),
            _ => None,
        }
    }
}

// ===== HashMap-based Matchers =====

/// O(1) EtherType to next stage mapping
#[derive(Debug, Clone)]
pub struct EtherTypeToStage {
    mapping: HashMap<u16, String>,
}

impl EtherTypeToStage {
    /// Create a new empty EtherType matcher
    pub fn new() -> Self {
        Self {
            mapping: HashMap::new(),
        }
    }
    
    /// Add a mapping from EtherType to stage name
    pub fn add_mapping(mut self, ether_type: u16, stage_name: &str) -> Self {
        self.mapping.insert(ether_type, String::from(stage_name));
        self
    }
    
    /// Add a mapping using the EtherType enum
    pub fn add_ethertype(self, ether_type: EtherType, stage_name: &str) -> Self {
        self.add_mapping(ether_type.as_u16(), stage_name)
    }
}

impl NextStageMatcher<u16> for EtherTypeToStage {
    fn get_next_stage(&self, ether_type: u16) -> Result<&str, NetworkError> {
        self.mapping.get(&ether_type)
            .map(|s| s.as_str())
            .ok_or_else(|| NetworkError::unsupported_protocol("ethernet", &alloc::format!("0x{:04x}", ether_type)))
    }
}

impl Default for EtherTypeToStage {
    fn default() -> Self {
        Self::new()
    }
}

/// O(1) IP protocol to next stage mapping
#[derive(Debug, Clone)]
pub struct IpProtocolToStage {
    mapping: HashMap<u8, String>,
}

impl IpProtocolToStage {
    /// Create a new empty IP protocol matcher
    pub fn new() -> Self {
        Self {
            mapping: HashMap::new(),
        }
    }
    
    /// Add a mapping from IP protocol number to stage name
    pub fn add_mapping(mut self, protocol: u8, stage_name: &str) -> Self {
        self.mapping.insert(protocol, String::from(stage_name));
        self
    }
    
    /// Add a mapping using the IpProtocol enum
    pub fn add_protocol(self, protocol: IpProtocol, stage_name: &str) -> Self {
        self.add_mapping(protocol.as_u8(), stage_name)
    }
}

impl NextStageMatcher<u8> for IpProtocolToStage {
    fn get_next_stage(&self, protocol: u8) -> Result<&str, NetworkError> {
        self.mapping.get(&protocol)
            .map(|s| s.as_str())
            .ok_or_else(|| NetworkError::unsupported_protocol("ipv4", &alloc::format!("{}", protocol)))
    }
}

impl Default for IpProtocolToStage {
    fn default() -> Self {
        Self::new()
    }
}

/// Port range specification for TCP/UDP routing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRange {
    /// Start of port range (inclusive)
    pub start: u16,
    /// End of port range (inclusive)  
    pub end: u16,
}

impl PortRange {
    /// Create a new port range
    pub fn new(start: u16, end: u16) -> Self {
        Self { start, end }
    }
    
    /// Create a range for a single port
    pub fn single(port: u16) -> Self {
        Self { start: port, end: port }
    }
    
    /// Check if a port is within this range
    pub fn contains(&self, port: u16) -> bool {
        port >= self.start && port <= self.end
    }
}

/// O(1) port range to next stage mapping
#[derive(Debug, Clone)]
pub struct PortRangeToStage {
    mapping: HashMap<u16, String>,
}

impl PortRangeToStage {
    /// Create a new empty port range matcher
    pub fn new() -> Self {
        Self {
            mapping: HashMap::new(),
        }
    }
    
    /// Add a mapping for a single port
    pub fn add_port(mut self, port: u16, stage_name: &str) -> Self {
        self.mapping.insert(port, String::from(stage_name));
        self
    }
}

impl NextStageMatcher<u16> for PortRangeToStage {
    fn get_next_stage(&self, port: u16) -> Result<&str, NetworkError> {
        self.mapping.get(&port)
            .map(|s| s.as_str())
            .ok_or_else(|| NetworkError::unsupported_protocol("tcp_udp", &alloc::format!("port {}", port)))
    }
}

impl Default for PortRangeToStage {
    fn default() -> Self {
        Self::new()
    }
}