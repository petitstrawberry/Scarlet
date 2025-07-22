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
    /// VLAN-tagged frame
    VLAN = 0x8100,
    /// Wake-on-LAN
    WOL = 0x0842,
    /// MPLS unicast
    MPLS = 0x8847,
    /// MPLS multicast
    MPLSMulticast = 0x8848,
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
            0x8100 => Some(EtherType::VLAN),
            0x0842 => Some(EtherType::WOL),
            0x8847 => Some(EtherType::MPLS),
            0x8848 => Some(EtherType::MPLSMulticast),
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
    /// Internet Group Management Protocol
    IGMP = 2,
    /// Transmission Control Protocol
    TCP = 6,
    /// User Datagram Protocol
    UDP = 17,
    /// IPv6-in-IPv4 tunneling
    IPv6 = 41,
    /// ICMPv6
    ICMPv6 = 58,
    /// No Next Header for IPv6
    NoNextHeader = 59,
    /// IPv6 Destination Options
    IPv6DestOpts = 60,
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
            2 => Some(IpProtocol::IGMP),
            6 => Some(IpProtocol::TCP),
            17 => Some(IpProtocol::UDP),
            41 => Some(IpProtocol::IPv6),
            58 => Some(IpProtocol::ICMPv6),
            59 => Some(IpProtocol::NoNextHeader),
            60 => Some(IpProtocol::IPv6DestOpts),
            _ => None,
        }
    }
}

// ===== HashMap-based Matchers =====

/// O(1) EtherType to next stage mapping
///
/// Maps Ethernet protocol types to pipeline stage names using HashMap
/// for constant-time lookups.
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
    
    /// Add multiple mappings from a slice
    pub fn add_mappings(mut self, mappings: &[(u16, &str)]) -> Self {
        for &(ether_type, stage_name) in mappings {
            self.mapping.insert(ether_type, String::from(stage_name));
        }
        self
    }
    
    /// Remove a mapping
    pub fn remove_mapping(&mut self, ether_type: u16) -> Option<String> {
        self.mapping.remove(&ether_type)
    }
    
    /// Check if a mapping exists
    pub fn has_mapping(&self, ether_type: u16) -> bool {
        self.mapping.contains_key(&ether_type)
    }
    
    /// Get all mapped EtherTypes
    pub fn mapped_ethertypes(&self) -> alloc::vec::Vec<u16> {
        self.mapping.keys().copied().collect()
    }
    
    /// Get the number of mappings
    pub fn mapping_count(&self) -> usize {
        self.mapping.len()
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
///
/// Maps IP protocol numbers to pipeline stage names using HashMap
/// for constant-time lookups.
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
    
    /// Add multiple mappings from a slice
    pub fn add_mappings(mut self, mappings: &[(u8, &str)]) -> Self {
        for &(protocol, stage_name) in mappings {
            self.mapping.insert(protocol, String::from(stage_name));
        }
        self
    }
    
    /// Remove a mapping
    pub fn remove_mapping(&mut self, protocol: u8) -> Option<String> {
        self.mapping.remove(&protocol)
    }
    
    /// Check if a mapping exists
    pub fn has_mapping(&self, protocol: u8) -> bool {
        self.mapping.contains_key(&protocol)
    }
    
    /// Get all mapped protocols
    pub fn mapped_protocols(&self) -> alloc::vec::Vec<u8> {
        self.mapping.keys().copied().collect()
    }
    
    /// Get the number of mappings
    pub fn mapping_count(&self) -> usize {
        self.mapping.len()
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
    
    /// Get the size of this range
    pub fn size(&self) -> u32 {
        (self.end as u32) - (self.start as u32) + 1
    }
}

/// O(1) port range to next stage mapping
///
/// Maps TCP/UDP port numbers to pipeline stage names. Uses HashMap with
/// the port number as key for O(1) lookup. For port ranges, currently
/// requires exact matches, but could be extended for range lookups.
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
    
    /// Add a mapping for a port range
    /// Note: This creates individual entries for each port in the range
    /// which works for small ranges but may not scale for large ranges
    pub fn add_range(mut self, range: PortRange, stage_name: &str) -> Self {
        for port in range.start..=range.end {
            self.mapping.insert(port, String::from(stage_name));
        }
        self
    }
    
    /// Add multiple port mappings from a slice
    pub fn add_ports(mut self, mappings: &[(u16, &str)]) -> Self {
        for &(port, stage_name) in mappings {
            self.mapping.insert(port, String::from(stage_name));
        }
        self
    }
    
    /// Remove a port mapping
    pub fn remove_port(&mut self, port: u16) -> Option<String> {
        self.mapping.remove(&port)
    }
    
    /// Check if a port mapping exists
    pub fn has_port(&self, port: u16) -> bool {
        self.mapping.contains_key(&port)
    }
    
    /// Get all mapped ports
    pub fn mapped_ports(&self) -> alloc::vec::Vec<u16> {
        self.mapping.keys().copied().collect()
    }
    
    /// Get the number of port mappings
    pub fn mapping_count(&self) -> usize {
        self.mapping.len()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_ether_type_enum() {
        assert_eq!(EtherType::IPv4.as_u16(), 0x0800);
        assert_eq!(EtherType::IPv6.as_u16(), 0x86DD);
        assert_eq!(EtherType::ARP.as_u16(), 0x0806);
        
        assert_eq!(EtherType::from_u16(0x0800), Some(EtherType::IPv4));
        assert_eq!(EtherType::from_u16(0x86DD), Some(EtherType::IPv6));
        assert_eq!(EtherType::from_u16(0x1234), None);
    }

    #[test_case]
    fn test_ip_protocol_enum() {
        assert_eq!(IpProtocol::TCP.as_u8(), 6);
        assert_eq!(IpProtocol::UDP.as_u8(), 17);
        assert_eq!(IpProtocol::ICMP.as_u8(), 1);
        
        assert_eq!(IpProtocol::from_u8(6), Some(IpProtocol::TCP));
        assert_eq!(IpProtocol::from_u8(17), Some(IpProtocol::UDP));
        assert_eq!(IpProtocol::from_u8(255), None);
    }

    #[test_case]
    fn test_ether_type_to_stage() {
        let matcher = EtherTypeToStage::new()
            .add_mapping(0x0800, "ipv4")
            .add_ethertype(EtherType::IPv6, "ipv6")
            .add_mapping(0x0806, "arp");
        
        assert_eq!(matcher.get_next_stage(0x0800).unwrap(), "ipv4");
        assert_eq!(matcher.get_next_stage(0x86DD).unwrap(), "ipv6");
        assert_eq!(matcher.get_next_stage(0x0806).unwrap(), "arp");
        
        // Test unsupported protocol
        let result = matcher.get_next_stage(0x1234);
        assert!(result.is_err());
        match result {
            Err(NetworkError::UnsupportedProtocol { layer, protocol }) => {
                assert_eq!(layer, "ethernet");
                assert_eq!(protocol, "0x1234");
            }
            _ => panic!("Expected UnsupportedProtocol error"),
        }
        
        assert!(matcher.has_mapping(0x0800));
        assert!(!matcher.has_mapping(0x1234));
        assert_eq!(matcher.mapping_count(), 3);
    }

    #[test_case]
    fn test_ip_protocol_to_stage() {
        let matcher = IpProtocolToStage::new()
            .add_mapping(6, "tcp")
            .add_protocol(IpProtocol::UDP, "udp")
            .add_mapping(1, "icmp");
        
        assert_eq!(matcher.get_next_stage(6).unwrap(), "tcp");
        assert_eq!(matcher.get_next_stage(17).unwrap(), "udp");
        assert_eq!(matcher.get_next_stage(1).unwrap(), "icmp");
        
        // Test unsupported protocol
        let result = matcher.get_next_stage(255);
        assert!(result.is_err());
        match result {
            Err(NetworkError::UnsupportedProtocol { layer, protocol }) => {
                assert_eq!(layer, "ipv4");
                assert_eq!(protocol, "255");
            }
            _ => panic!("Expected UnsupportedProtocol error"),
        }
        
        assert!(matcher.has_mapping(6));
        assert!(!matcher.has_mapping(255));
        assert_eq!(matcher.mapping_count(), 3);
    }

    #[test_case]
    fn test_port_range() {
        let range = PortRange::new(80, 90);
        assert!(range.contains(80));
        assert!(range.contains(85));
        assert!(range.contains(90));
        assert!(!range.contains(79));
        assert!(!range.contains(91));
        assert_eq!(range.size(), 11);
        
        let single = PortRange::single(443);
        assert!(single.contains(443));
        assert!(!single.contains(442));
        assert!(!single.contains(444));
        assert_eq!(single.size(), 1);
    }

    #[test_case]
    fn test_port_range_to_stage() {
        let matcher = PortRangeToStage::new()
            .add_port(80, "http")
            .add_port(443, "https")
            .add_range(PortRange::new(20, 21), "ftp");
        
        assert_eq!(matcher.get_next_stage(80).unwrap(), "http");
        assert_eq!(matcher.get_next_stage(443).unwrap(), "https");
        assert_eq!(matcher.get_next_stage(20).unwrap(), "ftp");
        assert_eq!(matcher.get_next_stage(21).unwrap(), "ftp");
        
        // Test unmapped port
        let result = matcher.get_next_stage(8080);
        assert!(result.is_err());
        
        assert!(matcher.has_port(80));
        assert!(matcher.has_port(20));
        assert!(matcher.has_port(21));
        assert!(!matcher.has_port(8080));
        assert_eq!(matcher.mapping_count(), 4); // 80, 443, 20, 21
    }

    #[test_case]
    fn test_multiple_mappings() {
        let ether_mappings = [(0x0800, "ipv4"), (0x86DD, "ipv6"), (0x0806, "arp")];
        let matcher = EtherTypeToStage::new().add_mappings(&ether_mappings);
        
        assert_eq!(matcher.mapping_count(), 3);
        assert_eq!(matcher.get_next_stage(0x0800).unwrap(), "ipv4");
        assert_eq!(matcher.get_next_stage(0x86DD).unwrap(), "ipv6");
        assert_eq!(matcher.get_next_stage(0x0806).unwrap(), "arp");
        
        let ip_mappings = [(6, "tcp"), (17, "udp"), (1, "icmp")];
        let ip_matcher = IpProtocolToStage::new().add_mappings(&ip_mappings);
        
        assert_eq!(ip_matcher.mapping_count(), 3);
        assert_eq!(ip_matcher.get_next_stage(6).unwrap(), "tcp");
        assert_eq!(ip_matcher.get_next_stage(17).unwrap(), "udp");
        assert_eq!(ip_matcher.get_next_stage(1).unwrap(), "icmp");
    }

    #[test_case]
    fn test_matcher_mutation() {
        let mut matcher = EtherTypeToStage::new()
            .add_mapping(0x0800, "ipv4")
            .add_mapping(0x86DD, "ipv6");
        
        assert_eq!(matcher.mapping_count(), 2);
        
        // Remove a mapping
        let removed = matcher.remove_mapping(0x0800);
        assert_eq!(removed, Some("ipv4".to_string()));
        assert_eq!(matcher.mapping_count(), 1);
        assert!(!matcher.has_mapping(0x0800));
        
        // Try to remove non-existent mapping
        let removed = matcher.remove_mapping(0x1234);
        assert_eq!(removed, None);
    }

    #[test_case]
    fn test_mapped_keys() {
        let matcher = EtherTypeToStage::new()
            .add_mapping(0x0800, "ipv4")
            .add_mapping(0x86DD, "ipv6")
            .add_mapping(0x0806, "arp");
        
        let mut ethertypes = matcher.mapped_ethertypes();
        ethertypes.sort();
        assert_eq!(ethertypes, vec![0x0806, 0x0800, 0x86DD]);
        
        let ip_matcher = IpProtocolToStage::new()
            .add_mapping(6, "tcp")
            .add_mapping(17, "udp");
        
        let mut protocols = ip_matcher.mapped_protocols();
        protocols.sort();
        assert_eq!(protocols, vec![6, 17]);
        
        let port_matcher = PortRangeToStage::new()
            .add_port(80, "http")
            .add_port(443, "https");
        
        let mut ports = port_matcher.mapped_ports();
        ports.sort();
        assert_eq!(ports, vec![80, 443]);
    }
}