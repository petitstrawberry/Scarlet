//! IPv4 protocol implementation with beautiful builder API

use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::format;
use hashbrown::HashMap;

use crate::network::traits::{ReceiveHandler, TransmitHandler, NextAction, NextStageMatcher};
use crate::network::packet::NetworkPacket;
use crate::network::error::NetworkError;
use crate::network::pipeline::{FlexibleStage, StageIdentifier};

/// IP Protocol constants
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IpProtocol {
    ICMP = 1,
    TCP = 6,
    UDP = 17,
    IPv6 = 41,
    ESP = 50,
    AH = 51,
    ICMPv6 = 58,
    OSPF = 89,
    SCTP = 132,
}

impl IpProtocol {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(IpProtocol::ICMP),
            6 => Some(IpProtocol::TCP),
            17 => Some(IpProtocol::UDP),
            41 => Some(IpProtocol::IPv6),
            50 => Some(IpProtocol::ESP),
            51 => Some(IpProtocol::AH),
            58 => Some(IpProtocol::ICMPv6),
            89 => Some(IpProtocol::OSPF),
            132 => Some(IpProtocol::SCTP),
            _ => None,
        }
    }
    
    pub fn name(self) -> &'static str {
        match self {
            IpProtocol::ICMP => "ICMP",
            IpProtocol::TCP => "TCP",
            IpProtocol::UDP => "UDP",
            IpProtocol::IPv6 => "IPv6",
            IpProtocol::ESP => "ESP",
            IpProtocol::AH => "AH",
            IpProtocol::ICMPv6 => "ICMPv6",
            IpProtocol::OSPF => "OSPF",
            IpProtocol::SCTP => "SCTP",
        }
    }
}

/// IP Protocol → Next stage mapping (O(1) HashMap)
#[derive(Debug, Clone)]
pub struct IpProtocolToStage {
    mapping: HashMap<u8, String>,
}

impl IpProtocolToStage {
    pub fn new() -> Self {
        Self { mapping: HashMap::new() }
    }
    
    pub fn add_mapping(mut self, protocol: u8, stage: &str) -> Self {
        self.mapping.insert(protocol, String::from(stage));
        self
    }
    
    pub fn add_mappings(mut self, mappings: &[(u8, &str)]) -> Self {
        for (protocol, stage) in mappings {
            self.mapping.insert(*protocol, String::from(*stage));
        }
        self
    }
    
    pub fn remove_mapping(&mut self, protocol: u8) -> Option<String> {
        self.mapping.remove(&protocol)
    }
    
    pub fn has_mapping(&self, protocol: u8) -> bool {
        self.mapping.contains_key(&protocol)
    }
    
    pub fn get_all_mappings(&self) -> &HashMap<u8, String> {
        &self.mapping
    }
}

impl NextStageMatcher<u8> for IpProtocolToStage {
    fn get_next_stage(&self, protocol: u8) -> Result<&str, NetworkError> {
        self.mapping.get(&protocol)
            .map(|s| s.as_str())
            .ok_or_else(|| NetworkError::unsupported_protocol("ipv4", &format!("{}", protocol)))
    }
}

/// IPv4 receive handler with O(1) routing
#[derive(Debug, Clone)]
pub struct IPv4RxHandler {
    next_stage_matcher: IpProtocolToStage,
}

impl IPv4RxHandler {
    pub fn new(matcher: IpProtocolToStage) -> Self {
        Self { next_stage_matcher: matcher }
    }
}

impl ReceiveHandler for IPv4RxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // 1. Validate minimum IPv4 header size
        packet.validate_payload_size(20)?;
        let payload = packet.payload().clone();
        
        // 2. Parse IPv4 header
        let version_ihl = payload[0];
        let version = (version_ihl >> 4) & 0xF;
        let ihl = (version_ihl & 0xF) * 4; // IHL is in 4-byte units
        
        // Version validation
        if version != 4 {
            return Err(NetworkError::unsupported_protocol("ip", &format!("version {}", version)));
        }
        
        // Validate header length
        if (ihl as usize) < 20 || payload.len() < (ihl as usize) {
            return Err(NetworkError::insufficient_payload_size(ihl as usize, payload.len()));
        }
        
        let tos = payload[1];
        let total_length = u16::from_be_bytes([payload[2], payload[3]]);
        let identification = u16::from_be_bytes([payload[4], payload[5]]);
        let flags_fragment = u16::from_be_bytes([payload[6], payload[7]]);
        let ttl = payload[8];
        let protocol = payload[9];
        let checksum = u16::from_be_bytes([payload[10], payload[11]]);
        let src_ip = [payload[12], payload[13], payload[14], payload[15]];
        let dest_ip = [payload[16], payload[17], payload[18], payload[19]];
        
        // フラグとフラグメントオフセット
        let flags = (flags_fragment >> 13) & 0x7;
        let fragment_offset = flags_fragment & 0x1FFF;
        
        // 3. Save IPv4 header information
        packet.add_header("ipv4", payload[0..(ihl as usize)].to_vec());
        packet.set_hint("ip_version", &format!("{}", version));
        packet.set_hint("ip_header_length", &format!("{}", ihl));
        packet.set_hint("ip_tos", &format!("{}", tos));
        packet.set_hint("ip_total_length", &format!("{}", total_length));
        packet.set_hint("ip_identification", &format!("{}", identification));
        packet.set_hint("ip_flags", &format!("{}", flags));
        packet.set_hint("ip_fragment_offset", &format!("{}", fragment_offset));
        packet.set_hint("ip_ttl", &format!("{}", ttl));
        packet.set_hint("ip_protocol", &format!("{}", protocol));
        packet.set_hint("ip_checksum", &format!("0x{:04x}", checksum));
        packet.set_hint("src_ip", &Self::format_ip(src_ip));
        packet.set_hint("dest_ip", &Self::format_ip(dest_ip));
        
        // 4. Update payload (remove IP header)
        packet.set_payload(payload[(ihl as usize)..].to_vec());
        
        // 5. Routing decision based on protocol (O(1) HashMap)
        let next_stage = self.next_stage_matcher.get_next_stage(protocol)?;
        Ok(NextAction::JumpTo(String::from(next_stage)))
    }
}

impl IPv4RxHandler {
    fn format_ip(ip: [u8; 4]) -> String {
        format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
    }
}

/// IPv4 transmit handler with hints-based header generation
#[derive(Debug)]
pub struct IPv4TxHandler {
    default_ttl: u8,
    default_tos: u8,
}

impl IPv4TxHandler {
    pub fn new(default_ttl: u8, default_tos: u8) -> Self {
        Self { default_ttl, default_tos }
    }
    
    pub fn new_with_defaults() -> Self {
        Self::new(64, 0)
    }
}

impl TransmitHandler for IPv4TxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // 1. Get required information from hints
        let src_ip_str = packet.get_hint("src_ip")
            .ok_or_else(|| NetworkError::missing_hint("src_ip"))?;
        let dest_ip_str = packet.get_hint("dest_ip")
            .ok_or_else(|| NetworkError::missing_hint("dest_ip"))?;
        let protocol_str = packet.get_hint("ip_protocol")
            .ok_or_else(|| NetworkError::missing_hint("ip_protocol"))?;
        
        // 2. Get optional values
        let ttl = packet.get_hint("ip_ttl")
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(self.default_ttl);
        let tos = packet.get_hint("ip_tos")
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(self.default_tos);
        
        // 3. Parse values
        let src_ip = Self::parse_ip(src_ip_str)?;
        let dest_ip = Self::parse_ip(dest_ip_str)?;
        let protocol = protocol_str.parse::<u8>()
            .map_err(|_| NetworkError::invalid_hint_format("ip_protocol", protocol_str))?;
        
        // 4. Build IPv4 header
        let payload_len = packet.payload().len();
        let total_length = (20 + payload_len) as u16; // Fixed 20-byte header
        
        let mut ipv4_header = Vec::with_capacity(20);
        ipv4_header.push(0x45); // Version=4, IHL=5 (20 bytes)
        ipv4_header.push(tos);
        ipv4_header.extend_from_slice(&total_length.to_be_bytes());
        ipv4_header.extend_from_slice(&0u16.to_be_bytes()); // Identification
        ipv4_header.extend_from_slice(&0u16.to_be_bytes()); // Flags + Fragment offset
        ipv4_header.push(ttl);
        ipv4_header.push(protocol);
        ipv4_header.extend_from_slice(&0u16.to_be_bytes()); // Checksum (will be calculated)
        ipv4_header.extend_from_slice(&src_ip);
        ipv4_header.extend_from_slice(&dest_ip);
        
        // 5. Calculate checksum
        let checksum = Self::calculate_checksum(&ipv4_header);
        ipv4_header[10] = (checksum >> 8) as u8;
        ipv4_header[11] = (checksum & 0xFF) as u8;
        
        // 6. Add header and combine with payload
        let payload = packet.payload().clone();
        packet.set_payload([ipv4_header, payload].concat());
        
        // 7. Set EtherType hint (for upper layer)
        packet.set_hint("ether_type", "0x0800");
        
        Ok(NextAction::Complete)
    }
}

impl IPv4TxHandler {
    fn parse_ip(ip_str: &str) -> Result<[u8; 4], NetworkError> {
        let parts: Vec<&str> = ip_str.split('.').collect();
        if parts.len() != 4 {
            return Err(NetworkError::invalid_hint_format("ip", ip_str));
        }
        
        let mut ip = [0u8; 4];
        for (i, part) in parts.iter().enumerate() {
            ip[i] = part.parse::<u8>()
                .map_err(|_| NetworkError::invalid_hint_format("ip", ip_str))?;
        }
        Ok(ip)
    }
    
    fn calculate_checksum(header: &[u8]) -> u16 {
        let mut sum = 0u32;
        
        // Calculate sum in 16-bit units (checksum field is calculated as 0)
        for i in (0..header.len()).step_by(2) {
            if i + 1 < header.len() {
                let word = if i == 10 { // Checksum field is calculated as 0
                    0u16
                } else {
                    u16::from_be_bytes([header[i], header[i + 1]])
                };
                sum += word as u32;
            }
        }
        
        // Carry addition
        while (sum >> 16) != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        
        // One's complement
        !(sum as u16)
    }
}

/// IPv4 stage builder for beautiful API
pub struct IPv4StageBuilder {
    stage_id: String,
    protocol_routes: Vec<(u8, String)>,
    rx_enabled: bool,
    tx_enabled: bool,
    default_ttl: u8,
    default_tos: u8,
}

impl IPv4StageBuilder {
    pub fn new() -> Self {
        Self {
            stage_id: String::from("ipv4"),
            protocol_routes: Vec::new(),
            rx_enabled: false,
            tx_enabled: false,
            default_ttl: 64,
            default_tos: 0,
        }
    }
    
    /// Set custom stage ID
    pub fn with_stage_id(mut self, id: &str) -> Self {
        self.stage_id = String::from(id);
        self
    }
    
    /// Add IP protocol routing (raw u8)
    pub fn add_protocol_route(mut self, protocol: u8, next_stage: &str) -> Self {
        self.protocol_routes.push((protocol, String::from(next_stage)));
        self
    }
    
    /// Add IP protocol routing (enum)
    pub fn route_to(self, protocol: IpProtocol, next_stage: &str) -> Self {
        self.add_protocol_route(protocol.as_u8(), next_stage)
    }
    
    /// Add IP protocol routing with type-safe stage identifier
    pub fn route_to_typed<T: StageIdentifier>(self, protocol: IpProtocol) -> Self {
        self.add_protocol_route(protocol.as_u8(), T::stage_id())
    }
    
    /// Add multiple routes at once
    pub fn add_routes(mut self, routes: &[(u8, &str)]) -> Self {
        for (protocol, stage) in routes {
            self.protocol_routes.push((*protocol, String::from(*stage)));
        }
        self
    }
    
    /// Add IP protocol route with type-safe stage identifier (raw u8)
    pub fn add_route_typed<T: StageIdentifier>(mut self, protocol: u8) -> Self {
        self.protocol_routes.push((protocol, String::from(T::stage_id())));
        self
    }
    
    /// Add multiple routes with enums
    pub fn add_enum_routes(mut self, routes: &[(IpProtocol, &str)]) -> Self {
        for (protocol, stage) in routes {
            self.protocol_routes.push((protocol.as_u8(), String::from(*stage)));
        }
        self
    }
    
    /// Add multiple routes with type-safe stage identifiers
    pub fn add_enum_routes_typed<T: StageIdentifier>(mut self, protocols: &[IpProtocol]) -> Self {
        for protocol in protocols {
            self.protocol_routes.push((protocol.as_u8(), String::from(T::stage_id())));
        }
        self
    }
    
    /// Enable receive handler
    pub fn enable_rx(mut self) -> Self {
        self.rx_enabled = true;
        self
    }
    
    /// Enable transmit handler
    pub fn enable_tx(mut self) -> Self {
        self.tx_enabled = true;
        self
    }
    
    /// Enable both handlers
    pub fn enable_both(mut self) -> Self {
        self.rx_enabled = true;
        self.tx_enabled = true;
        self
    }
    
    /// Set default TTL for transmission
    pub fn with_default_ttl(mut self, ttl: u8) -> Self {
        self.default_ttl = ttl;
        self
    }
    
    /// Set default TOS for transmission
    pub fn with_default_tos(mut self, tos: u8) -> Self {
        self.default_tos = tos;
        self
    }
    
    /// Build the stage
    pub fn build(self) -> FlexibleStage {
        // Build IpProtocolToStage matcher
        let mut matcher = IpProtocolToStage::new();
        for (protocol, stage) in self.protocol_routes {
            matcher = matcher.add_mapping(protocol, &stage);
        }
        
        let rx_handler = if self.rx_enabled {
            Some(Box::new(IPv4RxHandler::new(matcher)) as Box<dyn ReceiveHandler>)
        } else {
            None
        };
        
        let tx_handler = if self.tx_enabled {
            Some(Box::new(IPv4TxHandler::new(self.default_ttl, self.default_tos)) as Box<dyn TransmitHandler>)
        } else {
            None
        };
        
        FlexibleStage {
            stage_id: self.stage_id,
            rx_handler,
            tx_handler,
        }
    }
}

/// IPv4 stage convenience struct
pub struct IPv4Stage;

impl StageIdentifier for IPv4Stage {
    fn stage_id() -> &'static str {
        "ipv4"
    }
}

impl IPv4Stage {
    pub fn builder() -> IPv4StageBuilder {
        IPv4StageBuilder::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{NetworkPacket, NextAction};
    use alloc::{vec, string::String};

    #[test_case]
    fn test_ipv4_stage_builder() {
        let stage = IPv4Stage::builder()
            .route_to(IpProtocol::TCP, "tcp")
            .route_to(IpProtocol::UDP, "udp")
            .add_protocol_route(1, "icmp")
            .enable_both()
            .with_default_ttl(128)
            .build();
        
        assert_eq!(stage.stage_id, "ipv4");
        assert!(stage.rx_handler.is_some());
        assert!(stage.tx_handler.is_some());
    }

    #[test_case]
    fn test_ipv4_receive_processing() {
        // Create IPv4 packet with TCP payload
        let ipv4_packet = vec![
            0x45,       // Version=4, IHL=5
            0x00,       // TOS
            0x00, 0x28, // Total length = 40
            0x12, 0x34, // Identification
            0x40, 0x00, // Flags + Fragment offset (Don't Fragment)
            0x40,       // TTL = 64
            0x06,       // Protocol = TCP
            0x00, 0x00, // Checksum (will be ignored)
            // Source IP: 192.168.1.1
            192, 168, 1, 1,
            // Destination IP: 192.168.1.2
            192, 168, 1, 2,
            // TCP payload
            0x50, 0x00, 0x50, 0x50, // TCP header start
        ];
        
        let mut packet = NetworkPacket::new(ipv4_packet);
        
        let matcher = IpProtocolToStage::new()
            .add_mapping(6, "tcp")
            .add_mapping(17, "udp");
        
        let handler = IPv4RxHandler::new(matcher);
        let result = handler.handle(&mut packet).unwrap();
        
        // Should route to TCP
        assert_eq!(result, NextAction::JumpTo(String::from("tcp")));
        
        // Check hints
        assert_eq!(packet.get_hint("src_ip"), Some("192.168.1.1"));
        assert_eq!(packet.get_hint("dest_ip"), Some("192.168.1.2"));
        assert_eq!(packet.get_hint("ip_protocol"), Some("6"));
        assert_eq!(packet.get_hint("ip_ttl"), Some("64"));
        assert_eq!(packet.get_hint("ip_version"), Some("4"));
        
        // Check header saved
        assert!(packet.get_header("ipv4").is_some());
        assert_eq!(packet.get_header("ipv4").unwrap().len(), 20);
        
        // Check payload updated (should be TCP packet without IPv4 header)
        assert_eq!(packet.payload().len(), 4);
        assert_eq!(packet.payload()[0], 0x50); // TCP header start
    }

    #[test_case]
    fn test_ip_protocol_enum_constants() {
        assert_eq!(IpProtocol::TCP.as_u8(), 6);
        assert_eq!(IpProtocol::UDP.as_u8(), 17);
        assert_eq!(IpProtocol::ICMP.as_u8(), 1);
        
        assert_eq!(IpProtocol::from_u8(6), Some(IpProtocol::TCP));
        assert_eq!(IpProtocol::from_u8(17), Some(IpProtocol::UDP));
        assert_eq!(IpProtocol::from_u8(99), None);
        
        assert_eq!(IpProtocol::TCP.name(), "TCP");
        assert_eq!(IpProtocol::UDP.name(), "UDP");
    }

    #[test_case]
    fn test_o1_ip_protocol_routing() {
        // Test O(1) IP Protocol routing
        let ip_matcher = IpProtocolToStage::new()
            .add_mapping(6, "tcp")
            .add_mapping(17, "udp")
            .add_mapping(1, "icmp");
        
        assert_eq!(ip_matcher.get_next_stage(6).unwrap(), "tcp");
        assert_eq!(ip_matcher.get_next_stage(17).unwrap(), "udp");
        assert_eq!(ip_matcher.get_next_stage(1).unwrap(), "icmp");
        assert!(ip_matcher.get_next_stage(99).is_err());
    }

    #[test_case]
    fn test_ipv4_stage_builder_typed_methods() {
        use crate::network::test_helpers::{TcpProtocol, UdpProtocol};
        
        // Test the new typed routing methods
        let stage = IPv4Stage::builder()
            .route_to_typed::<TcpProtocol>(IpProtocol::TCP)
            .route_to_typed::<UdpProtocol>(IpProtocol::UDP)
            .add_route_typed::<TcpProtocol>(1) // ICMP routed to TCP stage for testing
            .enable_rx()
            .build();
        
        assert_eq!(stage.stage_id, "ipv4");
        assert!(stage.rx_handler.is_some());
        
        // Verify stage identifier consistency
        assert_eq!(TcpProtocol::stage_id(), "tcp");
        assert_eq!(UdpProtocol::stage_id(), "udp");
        assert_eq!(IPv4Stage::stage_id(), "ipv4");
    }

    #[test_case]
    fn test_ipv4_stage_builder_typed_multiple_routes() {
        use crate::network::test_helpers::{TcpProtocol, UdpProtocol};
        
        // Test adding multiple routes to the same typed stage
        let stage = IPv4Stage::builder()
            .add_enum_routes_typed::<TcpProtocol>(&[IpProtocol::TCP, IpProtocol::ICMP])
            .route_to_typed::<UdpProtocol>(IpProtocol::UDP)
            .enable_rx()
            .build();
        
        assert_eq!(stage.stage_id, "ipv4");
        assert!(stage.rx_handler.is_some());
        
        // Verify that typed routing works correctly
        // The typed methods should route to the correct stage identifiers
        assert_eq!(TcpProtocol::stage_id(), "tcp");
        assert_eq!(UdpProtocol::stage_id(), "udp");
    }
}