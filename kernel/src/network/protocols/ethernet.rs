//! Ethernet protocol implementation with VLAN support and beautiful builder API

use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::format;
use hashbrown::HashMap;

use crate::network::traits::{ReceiveHandler, TransmitHandler, NextAction, NextStageMatcher};
use crate::network::packet::NetworkPacket;
use crate::network::error::NetworkError;
use crate::network::pipeline::FlexibleStage;

/// EtherType constants for type-safe routing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EtherType {
    IPv4 = 0x0800,
    IPv6 = 0x86DD,
    ARP = 0x0806,
    VLAN = 0x8100,
    PPPoEDiscovery = 0x8863,
    PPPoESession = 0x8864,
    MPLS = 0x8847,
    MPLSMulticast = 0x8848,
}

impl EtherType {
    /// Convert to u16 value
    pub fn as_u16(self) -> u16 {
        self as u16
    }
    
    /// Convert from u16 value
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0x0800 => Some(EtherType::IPv4),
            0x86DD => Some(EtherType::IPv6),
            0x0806 => Some(EtherType::ARP),
            0x8100 => Some(EtherType::VLAN),
            0x8863 => Some(EtherType::PPPoEDiscovery),
            0x8864 => Some(EtherType::PPPoESession),
            0x8847 => Some(EtherType::MPLS),
            0x8848 => Some(EtherType::MPLSMulticast),
            _ => None,
        }
    }
    
    /// Get protocol name
    pub fn name(self) -> &'static str {
        match self {
            EtherType::IPv4 => "IPv4",
            EtherType::IPv6 => "IPv6",
            EtherType::ARP => "ARP",
            EtherType::VLAN => "VLAN",
            EtherType::PPPoEDiscovery => "PPPoE Discovery",
            EtherType::PPPoESession => "PPPoE Session",
            EtherType::MPLS => "MPLS",
            EtherType::MPLSMulticast => "MPLS Multicast",
        }
    }
}

/// EtherType → 次ステージのマッピング（O(1) HashMap）
#[derive(Debug, Clone)]
pub struct EtherTypeToStage {
    mapping: HashMap<u16, String>,
}

impl EtherTypeToStage {
    pub fn new() -> Self {
        Self { mapping: HashMap::new() }
    }
    
    pub fn add_mapping(mut self, ethertype: u16, stage: &str) -> Self {
        self.mapping.insert(ethertype, String::from(stage));
        self
    }
    
    pub fn add_mappings(mut self, mappings: &[(u16, &str)]) -> Self {
        for (ethertype, stage) in mappings {
            self.mapping.insert(*ethertype, String::from(*stage));
        }
        self
    }
    
    pub fn remove_mapping(&mut self, ethertype: u16) -> Option<String> {
        self.mapping.remove(&ethertype)
    }
    
    pub fn has_mapping(&self, ethertype: u16) -> bool {
        self.mapping.contains_key(&ethertype)
    }
    
    pub fn get_all_mappings(&self) -> &HashMap<u16, String> {
        &self.mapping
    }
}

impl NextStageMatcher<u16> for EtherTypeToStage {
    fn get_next_stage(&self, ether_type: u16) -> Result<&str, NetworkError> {
        self.mapping.get(&ether_type)
            .map(|s| s.as_str())
            .ok_or_else(|| NetworkError::unsupported_protocol("ethernet", &format!("0x{:04x}", ether_type)))
    }
}

/// Ethernet receive handler with O(1) routing
#[derive(Debug, Clone)]
pub struct EthernetRxHandler {
    next_stage_matcher: EtherTypeToStage,
}

impl EthernetRxHandler {
    pub fn new(matcher: EtherTypeToStage) -> Self {
        Self { next_stage_matcher: matcher }
    }
}

impl ReceiveHandler for EthernetRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // 1. Validate minimum Ethernet header size
        packet.validate_payload_size(14)?;
        let payload = packet.payload().clone();
        
        // 2. Parse Ethernet header
        let dest_mac = &payload[0..6];
        let src_mac = &payload[6..12];
        let ether_type = u16::from_be_bytes([payload[12], payload[13]]);
        
        // 3. VLAN tag processing (optional)
        let (actual_ether_type, header_length) = if ether_type == 0x8100 {
            // VLAN tagged frame
            packet.validate_payload_size(18)?;
            let vlan_tag = u16::from_be_bytes([payload[14], payload[15]]);
            let actual_ether_type = u16::from_be_bytes([payload[16], payload[17]]);
            
            // Save VLAN information as hints
            packet.set_hint("vlan_tag", &format!("{}", vlan_tag));
            packet.set_hint("vlan_priority", &format!("{}", (vlan_tag >> 13) & 0x7));
            packet.set_hint("vlan_id", &format!("{}", vlan_tag & 0xFFF));
            
            (actual_ether_type, 18)
        } else {
            (ether_type, 14)
        };
        
        // 4. Save Ethernet header information
        packet.add_header("ethernet", payload[0..header_length].to_vec());
        packet.set_hint("src_mac", &Self::format_mac(src_mac));
        packet.set_hint("dest_mac", &Self::format_mac(dest_mac));
        packet.set_hint("ether_type", &format!("0x{:04x}", actual_ether_type));
        
        // 5. Update payload
        packet.set_payload(payload[header_length..].to_vec());
        
        // 6. Routing decision using internal O(1) HashMap
        let next_stage = self.next_stage_matcher.get_next_stage(actual_ether_type)?;
        Ok(NextAction::JumpTo(String::from(next_stage)))
    }
}

impl EthernetRxHandler {
    fn format_mac(mac: &[u8]) -> String {
        format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5])
    }
}

/// Ethernet transmit handler with hints-based header generation
#[derive(Debug)]
pub struct EthernetTxHandler {
    default_src_mac: [u8; 6],
}

impl EthernetTxHandler {
    pub fn new(default_src_mac: [u8; 6]) -> Self {
        Self { default_src_mac }
    }
    
    pub fn new_with_default() -> Self {
        Self::new([0x00, 0x11, 0x22, 0x33, 0x44, 0x55])
    }
}

impl TransmitHandler for EthernetTxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // 1. Get required information from hints
        let ethertype_str = packet.get_hint("ether_type")
            .or_else(|| packet.get_hint("ethertype"))
            .ok_or_else(|| NetworkError::missing_hint("ether_type"))?;
            
        let dest_mac_str = packet.get_hint("dest_mac")
            .ok_or_else(|| NetworkError::missing_hint("dest_mac"))?;
            
        let src_mac_str = packet.get_hint("src_mac");
        
        // 2. 値をパース
        let ether_type = Self::parse_ether_type(ethertype_str)?;
        let dest_mac = Self::parse_mac(dest_mac_str)?;
        let src_mac = if let Some(mac_str) = src_mac_str {
            Self::parse_mac(mac_str)?
        } else {
            self.default_src_mac
        };
        
        // 3. VLAN tag processing (optional)
        let mut ethernet_header = Vec::with_capacity(18);
        ethernet_header.extend_from_slice(&dest_mac);
        ethernet_header.extend_from_slice(&src_mac);
        
        if let Some(vlan_tag_str) = packet.get_hint("vlan_tag") {
            // VLAN tagged frame
            let vlan_tag = vlan_tag_str.parse::<u16>()
                .map_err(|_| NetworkError::invalid_hint_format("vlan_tag", vlan_tag_str))?;
            
            ethernet_header.extend_from_slice(&0x8100u16.to_be_bytes()); // VLAN EtherType
            ethernet_header.extend_from_slice(&vlan_tag.to_be_bytes());  // VLAN Tag
        }
        
        ethernet_header.extend_from_slice(&ether_type.to_be_bytes());
        
        // 4. Add header and combine with payload
        let payload = packet.payload().clone();
        packet.set_payload([ethernet_header, payload].concat());
        
        // 5. Transmission complete
        Ok(NextAction::Complete)
    }
}

impl EthernetTxHandler {
    fn parse_ether_type(ethertype_str: &str) -> Result<u16, NetworkError> {
        if ethertype_str.starts_with("0x") {
            u16::from_str_radix(&ethertype_str[2..], 16)
        } else {
            ethertype_str.parse::<u16>()
        }.map_err(|_| NetworkError::invalid_hint_format("ether_type", ethertype_str))
    }
    
    pub fn parse_mac(mac_str: &str) -> Result<[u8; 6], NetworkError> {
        let parts: Vec<&str> = mac_str.split(':').collect();
        if parts.len() != 6 {
            return Err(NetworkError::invalid_hint_format("mac", mac_str));
        }
        
        let mut mac = [0u8; 6];
        for (i, part) in parts.iter().enumerate() {
            mac[i] = u8::from_str_radix(part, 16)
                .map_err(|_| NetworkError::invalid_hint_format("mac", mac_str))?;
        }
        Ok(mac)
    }
}

/// Ethernet stage builder for beautiful API
pub struct EthernetStageBuilder {
    stage_id: String,
    ethertype_routes: Vec<(u16, String)>,
    rx_enabled: bool,
    tx_enabled: bool,
    default_src_mac: [u8; 6],
}

impl EthernetStageBuilder {
    pub fn new() -> Self {
        Self {
            stage_id: String::from("ethernet"),
            ethertype_routes: Vec::new(),
            rx_enabled: false,
            tx_enabled: false,
            default_src_mac: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
        }
    }
    
    /// Set custom stage ID
    pub fn with_stage_id(mut self, id: &str) -> Self {
        self.stage_id = String::from(id);
        self
    }
    
    /// Add EtherType routing (raw u16)
    pub fn add_ethertype_route(mut self, ethertype: u16, next_stage: &str) -> Self {
        self.ethertype_routes.push((ethertype, String::from(next_stage)));
        self
    }
    
    /// Add EtherType routing (enum)
    pub fn route_to(self, ethertype: EtherType, next_stage: &str) -> Self {
        self.add_ethertype_route(ethertype.as_u16(), next_stage)
    }
    
    /// Add multiple routes at once
    pub fn add_routes(mut self, routes: &[(u16, &str)]) -> Self {
        for (ethertype, stage) in routes {
            self.ethertype_routes.push((*ethertype, String::from(*stage)));
        }
        self
    }
    
    /// Add multiple routes with enums
    pub fn add_enum_routes(mut self, routes: &[(EtherType, &str)]) -> Self {
        for (ethertype, stage) in routes {
            self.ethertype_routes.push((ethertype.as_u16(), String::from(*stage)));
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
    
    /// Set default source MAC address for transmission
    pub fn with_src_mac(mut self, mac: [u8; 6]) -> Self {
        self.default_src_mac = mac;
        self
    }
    
    /// Set default source MAC address from string
    pub fn with_src_mac_str(mut self, mac_str: &str) -> Result<Self, NetworkError> {
        self.default_src_mac = EthernetTxHandler::parse_mac(mac_str)?;
        Ok(self)
    }
    
    /// Build the stage
    pub fn build(self) -> FlexibleStage {
        // Build EtherTypeToStage matcher
        let mut matcher = EtherTypeToStage::new();
        for (ethertype, stage) in self.ethertype_routes {
            matcher = matcher.add_mapping(ethertype, &stage);
        }
        
        let rx_handler = if self.rx_enabled {
            Some(Box::new(EthernetRxHandler::new(matcher)) as Box<dyn ReceiveHandler>)
        } else {
            None
        };
        
        let tx_handler = if self.tx_enabled {
            Some(Box::new(EthernetTxHandler::new(self.default_src_mac)) as Box<dyn TransmitHandler>)
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

/// Ethernet stage convenience struct
pub struct EthernetStage;

impl EthernetStage {
    pub fn builder() -> EthernetStageBuilder {
        EthernetStageBuilder::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{NetworkPacket, NextAction};
    use alloc::vec;

    #[test_case]
    fn test_ethernet_stage_builder() {
        let stage = EthernetStage::builder()
            .route_to(EtherType::IPv4, "ipv4")
            .route_to(EtherType::ARP, "arp")
            .add_ethertype_route(0x86DD, "ipv6")
            .enable_both()
            .with_src_mac([0x02, 0x00, 0x00, 0x00, 0x00, 0x01])
            .build();
        
        assert_eq!(stage.stage_id, "ethernet");
        assert!(stage.rx_handler.is_some());
        assert!(stage.tx_handler.is_some());
    }

    #[test_case]
    fn test_ethernet_receive_processing() {
        // Create Ethernet frame with IPv4 payload
        let mut ethernet_frame = vec![
            // Destination MAC: 02:00:00:00:00:02
            0x02, 0x00, 0x00, 0x00, 0x00, 0x02,
            // Source MAC: 02:00:00:00:00:01
            0x02, 0x00, 0x00, 0x00, 0x00, 0x01,
            // EtherType: IPv4 (0x0800)
            0x08, 0x00,
            // Payload
            0x45, 0x00, 0x00, 0x20, // IPv4 header start
        ];
        ethernet_frame.extend_from_slice(&[0; 16]); // Padding
        
        let mut packet = NetworkPacket::new(ethernet_frame);
        
        let matcher = EtherTypeToStage::new()
            .add_mapping(0x0800, "ipv4")
            .add_mapping(0x0806, "arp");
        
        let handler = EthernetRxHandler::new(matcher);
        let result = handler.handle(&mut packet).unwrap();
        
        // Should route to IPv4
        assert_eq!(result, NextAction::JumpTo("ipv4".to_string()));
        
        // Check hints
        assert_eq!(packet.get_hint("src_mac"), Some("02:00:00:00:00:01"));
        assert_eq!(packet.get_hint("dest_mac"), Some("02:00:00:00:00:02"));
        assert_eq!(packet.get_hint("ether_type"), Some("0x0800"));
        
        // Check header saved
        assert!(packet.get_header("ethernet").is_some());
        assert_eq!(packet.get_header("ethernet").unwrap().len(), 14);
        
        // Check payload updated (should be IPv4 packet without ethernet header)
        assert_eq!(packet.payload().len(), 20);
        assert_eq!(packet.payload()[0], 0x45); // IPv4 version + IHL
    }

    #[test_case]
    fn test_ethernet_vlan_processing() {
        // Create VLAN-tagged Ethernet frame
        let vlan_frame = vec![
            // Destination MAC
            0x02, 0x00, 0x00, 0x00, 0x00, 0x02,
            // Source MAC
            0x02, 0x00, 0x00, 0x00, 0x00, 0x01,
            // VLAN EtherType (0x8100)
            0x81, 0x00,
            // VLAN Tag (Priority=1, VLAN ID=100)
            0x20, 0x64,
            // Actual EtherType: IPv4 (0x0800)
            0x08, 0x00,
            // Payload
            0x45, 0x00,
        ];
        
        let mut packet = NetworkPacket::new(vlan_frame);
        
        let matcher = EtherTypeToStage::new()
            .add_mapping(0x0800, "ipv4");
        
        let handler = EthernetRxHandler::new(matcher);
        let result = handler.handle(&mut packet).unwrap();
        
        // Should route to IPv4
        assert_eq!(result, NextAction::JumpTo("ipv4".to_string()));
        
        // Check VLAN hints
        assert_eq!(packet.get_hint("vlan_tag"), Some("8292")); // 0x2064 = 8292
        assert_eq!(packet.get_hint("vlan_priority"), Some("1"));
        assert_eq!(packet.get_hint("vlan_id"), Some("100"));
        assert_eq!(packet.get_hint("ether_type"), Some("0x0800"));
        
        // Check header includes VLAN tag
        assert_eq!(packet.get_header("ethernet").unwrap().len(), 18);
    }

    #[test_case]
    fn test_ethernet_enum_constants() {
        assert_eq!(EtherType::IPv4.as_u16(), 0x0800);
        assert_eq!(EtherType::ARP.as_u16(), 0x0806);
        assert_eq!(EtherType::IPv6.as_u16(), 0x86DD);
        
        assert_eq!(EtherType::from_u16(0x0800), Some(EtherType::IPv4));
        assert_eq!(EtherType::from_u16(0x0806), Some(EtherType::ARP));
        assert_eq!(EtherType::from_u16(0x9999), None);
        
        assert_eq!(EtherType::IPv4.name(), "IPv4");
        assert_eq!(EtherType::ARP.name(), "ARP");
    }

    #[test_case]
    fn test_o1_hashmap_routing() {
        // Test O(1) EtherType routing
        let ethernet_matcher = EtherTypeToStage::new()
            .add_mapping(0x0800, "ipv4")
            .add_mapping(0x0806, "arp")
            .add_mapping(0x86DD, "ipv6");
        
        assert_eq!(ethernet_matcher.get_next_stage(0x0800).unwrap(), "ipv4");
        assert_eq!(ethernet_matcher.get_next_stage(0x0806).unwrap(), "arp");
        assert_eq!(ethernet_matcher.get_next_stage(0x86DD).unwrap(), "ipv6");
        assert!(ethernet_matcher.get_next_stage(0x9999).is_err());
    }
}