//! Protocol-specific stage builders and handlers
//!
//! This module implements concrete protocol handlers and builders for the Phase 1
//! pipeline infrastructure, starting with Ethernet and IPv4 protocols.

use alloc::{
    boxed::Box,
    string::String,
    vec::Vec,
    vec,
};
use super::{
    packet::NetworkPacket,
    error::NetworkError,
    traits::{ReceiveHandler, TransmitHandler, NextAction},
    phase1::FlexibleStage,
    matchers::{EtherTypeToStage, IpProtocolToStage, EtherType, IpProtocol},
};

// ===== Ethernet Protocol Implementation =====

/// Ethernet receive handler with O(1) EtherType routing
///
/// Processes Ethernet frames by:
/// 1. Extracting 14-byte Ethernet header
/// 2. Parsing destination MAC, source MAC, and EtherType
/// 3. Using NextStageMatcher for O(1) next stage determination
/// 4. Updating packet payload for next stage
pub struct EthernetRxHandler {
    next_stage_matcher: EtherTypeToStage,
}

impl EthernetRxHandler {
    /// Create a new Ethernet receive handler with the given matcher
    pub fn new(matcher: EtherTypeToStage) -> Self {
        Self {
            next_stage_matcher: matcher,
        }
    }
}

impl ReceiveHandler for EthernetRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Validate Ethernet header size (14 bytes)
        packet.validate_payload_size(14)?;
        let payload = packet.payload();
        
        // Extract Ethernet header components
        // Bytes 0-5: Destination MAC address
        // Bytes 6-11: Source MAC address  
        // Bytes 12-13: EtherType
        let ether_type = u16::from_be_bytes([payload[12], payload[13]]);
        
        // Store Ethernet header
        packet.add_header("ethernet", payload[0..14].to_vec());
        
        // Update payload to contain remaining data (after Ethernet header)
        packet.set_payload(payload[14..].to_vec());
        
        // Use O(1) HashMap lookup to determine next stage
        let next_stage = self.next_stage_matcher.get_next_stage(ether_type)?;
        Ok(NextAction::jump_to(next_stage))
    }
}

/// Ethernet transmit handler for building Ethernet frames
///
/// Builds Ethernet frames by:
/// 1. Reading hints for destination MAC, source MAC, and EtherType
/// 2. Constructing 14-byte Ethernet header
/// 3. Prepending header to payload
/// 4. Completing transmission (no further stages needed)
pub struct EthernetTxHandler {
    default_src_mac: [u8; 6],
}

impl EthernetTxHandler {
    /// Create a new Ethernet transmit handler with default source MAC
    pub fn new(default_src_mac: [u8; 6]) -> Self {
        Self {
            default_src_mac,
        }
    }
    
    /// Create handler with a default MAC address (00:11:22:33:44:55)
    pub fn with_default_mac() -> Self {
        Self::new([0x00, 0x11, 0x22, 0x33, 0x44, 0x55])
    }
    
    /// Parse MAC address from string format (e.g., "aa:bb:cc:dd:ee:ff")
    fn parse_mac(mac_str: &str) -> Result<[u8; 6], NetworkError> {
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
    
    /// Format MAC address as string
    fn format_mac(mac: &[u8; 6]) -> String {
        alloc::format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        )
    }
}

impl TransmitHandler for EthernetTxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Read required hints
        let ethertype = packet.get_hint("ethertype")
            .ok_or_else(|| NetworkError::missing_hint("ethertype"))?;
        let dest_mac = packet.get_hint("dest_mac")
            .ok_or_else(|| NetworkError::missing_hint("dest_mac"))?;
        
        // Get source MAC (use hint or default)
        let src_mac = match packet.get_hint("src_mac") {
            Some(mac_str) => Self::parse_mac(mac_str)?,
            None => self.default_src_mac,
        };
        
        // Parse destination MAC
        let dest_mac_bytes = Self::parse_mac(dest_mac)?;
        
        // Parse EtherType
        let ethertype_u16 = if ethertype.starts_with("0x") || ethertype.starts_with("0X") {
            u16::from_str_radix(&ethertype[2..], 16)
        } else {
            ethertype.parse::<u16>()
        }.map_err(|_| NetworkError::invalid_hint_format("ethertype", ethertype))?;
        
        // Build Ethernet header (14 bytes)
        let mut ethernet_header = Vec::with_capacity(14);
        ethernet_header.extend_from_slice(&dest_mac_bytes);  // Destination MAC (6 bytes)
        ethernet_header.extend_from_slice(&src_mac);         // Source MAC (6 bytes)
        ethernet_header.extend_from_slice(&ethertype_u16.to_be_bytes()); // EtherType (2 bytes)
        
        // Prepend Ethernet header to current payload
        let current_payload = packet.payload().to_vec();
        let mut new_payload = ethernet_header;
        new_payload.extend_from_slice(&current_payload);
        packet.set_payload(new_payload);
        
        // Ethernet is typically the final layer for transmission
        Ok(NextAction::Complete)
    }
}

// ===== IPv4 Protocol Implementation =====

/// IPv4 receive handler with O(1) protocol routing
///
/// Processes IPv4 packets by:
/// 1. Extracting and validating IPv4 header (minimum 20 bytes)
/// 2. Parsing protocol field and header length
/// 3. Using NextStageMatcher for O(1) next stage determination
/// 4. Updating packet payload for next stage
pub struct IPv4RxHandler {
    next_stage_matcher: IpProtocolToStage,
}

impl IPv4RxHandler {
    /// Create a new IPv4 receive handler with the given matcher
    pub fn new(matcher: IpProtocolToStage) -> Self {
        Self {
            next_stage_matcher: matcher,
        }
    }
}

impl ReceiveHandler for IPv4RxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Validate minimum IPv4 header size (20 bytes)
        packet.validate_payload_size(20)?;
        let payload = packet.payload();
        
        // Parse IPv4 header
        let version = (payload[0] >> 4) & 0x0F;
        let ihl = payload[0] & 0x0F; // Internet Header Length in 32-bit words
        let protocol = payload[9];
        
        // Validate version
        if version != 4 {
            return Err(NetworkError::invalid_packet(
                &alloc::format!("Expected IPv4 (version 4), got version {}", version)
            ));
        }
        
        // Calculate actual header length
        let header_length = (ihl as usize) * 4;
        if header_length < 20 {
            return Err(NetworkError::invalid_packet(
                &alloc::format!("IPv4 header length too small: {}", header_length)
            ));
        }
        
        // Validate we have enough data for the full header
        packet.validate_payload_size(header_length)?;
        
        // Extract IPv4 header
        packet.add_header("ipv4", payload[0..header_length].to_vec());
        
        // Update payload to contain remaining data (after IPv4 header)
        packet.set_payload(payload[header_length..].to_vec());
        
        // Use O(1) HashMap lookup to determine next stage based on protocol
        let next_stage = self.next_stage_matcher.get_next_stage(protocol)?;
        Ok(NextAction::jump_to(next_stage))
    }
}

/// IPv4 transmit handler for building IPv4 packets
///
/// Builds IPv4 packets by:
/// 1. Reading hints for destination IP, source IP, protocol, etc.
/// 2. Constructing IPv4 header with proper checksums
/// 3. Prepending header to payload
/// 4. Setting hints for lower layer (Ethernet)
pub struct IPv4TxHandler {
    default_src_ip: [u8; 4],
}

impl IPv4TxHandler {
    /// Create a new IPv4 transmit handler with default source IP
    pub fn new(default_src_ip: [u8; 4]) -> Self {
        Self {
            default_src_ip,
        }
    }
    
    /// Create handler with a default IP address (192.168.1.1)
    pub fn with_default_ip() -> Self {
        Self::new([192, 168, 1, 1])
    }
    
    /// Parse IP address from string format (e.g., "192.168.1.1")
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
    
    /// Format IP address as string
    fn format_ip(ip: &[u8; 4]) -> String {
        alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
    }
    
    /// Calculate IPv4 header checksum
    fn calculate_checksum(header: &[u8]) -> u16 {
        let mut sum: u32 = 0;
        
        // Sum all 16-bit words in the header (except checksum field)
        for chunk in header.chunks(2) {
            if chunk.len() == 2 {
                sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
            } else {
                sum += (chunk[0] as u32) << 8;
            }
        }
        
        // Add carry bits
        while (sum >> 16) != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        
        // One's complement
        !(sum as u16)
    }
}

impl TransmitHandler for IPv4TxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Read required hints
        let dest_ip = packet.get_hint("destination_ip")
            .ok_or_else(|| NetworkError::missing_hint("destination_ip"))?;
        let protocol = packet.get_hint("protocol")
            .ok_or_else(|| NetworkError::missing_hint("protocol"))?;
        
        // Parse destination IP
        let dest_ip_bytes = Self::parse_ip(dest_ip)?;
        
        // Get source IP (use hint or default)
        let src_ip_bytes = match packet.get_hint("source_ip") {
            Some(ip_str) => Self::parse_ip(ip_str)?,
            None => self.default_src_ip,
        };
        
        // Parse protocol number
        let protocol_num = protocol.parse::<u8>()
            .map_err(|_| NetworkError::invalid_hint_format("protocol", protocol))?;
        
        let payload_len = packet.payload().len();
        if payload_len > 65515 { // Max IPv4 payload: 65535 - 20 (min header)
            return Err(NetworkError::invalid_packet("IPv4 payload too large"));
        }
        
        let total_length = 20 + payload_len; // Basic IPv4 header is 20 bytes
        
        // Build IPv4 header (20 bytes for basic header)
        let mut ipv4_header = vec![0u8; 20];
        
        // Version (4) and IHL (5 for 20-byte header)
        ipv4_header[0] = 0x45;
        
        // Type of Service / DSCP (default 0)
        ipv4_header[1] = 0x00;
        
        // Total Length
        let total_len_bytes = (total_length as u16).to_be_bytes();
        ipv4_header[2] = total_len_bytes[0];
        ipv4_header[3] = total_len_bytes[1];
        
        // Identification (simple counter, could be more sophisticated)
        ipv4_header[4] = 0x00;
        ipv4_header[5] = 0x01;
        
        // Flags and Fragment Offset (Don't Fragment = 0x4000)
        ipv4_header[6] = 0x40;
        ipv4_header[7] = 0x00;
        
        // TTL (default 64)
        ipv4_header[8] = 64;
        
        // Protocol
        ipv4_header[9] = protocol_num;
        
        // Header Checksum (will calculate after setting IPs)
        ipv4_header[10] = 0x00;
        ipv4_header[11] = 0x00;
        
        // Source IP Address
        ipv4_header[12..16].copy_from_slice(&src_ip_bytes);
        
        // Destination IP Address
        ipv4_header[16..20].copy_from_slice(&dest_ip_bytes);
        
        // Calculate and set checksum
        let checksum = Self::calculate_checksum(&ipv4_header);
        let checksum_bytes = checksum.to_be_bytes();
        ipv4_header[10] = checksum_bytes[0];
        ipv4_header[11] = checksum_bytes[1];
        
        // Prepend IPv4 header to current payload
        let current_payload = packet.payload().to_vec();
        let mut new_payload = ipv4_header;
        new_payload.extend_from_slice(&current_payload);
        packet.set_payload(new_payload);
        
        // Set hints for Ethernet layer
        packet.set_hint("ethertype", "0x0800"); // IPv4 EtherType
        packet.set_hint("dest_mac", "ff:ff:ff:ff:ff:ff"); // Broadcast for simplicity
        
        // Continue to Ethernet layer
        Ok(NextAction::jump_to("ethernet"))
    }
}

// ===== Protocol-Specific Stage Builders =====

/// Builder for Ethernet protocol stages
pub struct EthernetStage;

impl EthernetStage {
    /// Create a new Ethernet stage builder
    pub fn builder() -> EthernetStageBuilder {
        EthernetStageBuilder::new()
    }
}

/// Fluent builder for Ethernet stages
pub struct EthernetStageBuilder {
    stage_id: String,
    ethertype_routes: Vec<(u16, String)>,
    rx_enabled: bool,
    tx_enabled: bool,
    default_src_mac: [u8; 6],
}

impl EthernetStageBuilder {
    /// Create a new Ethernet stage builder
    pub fn new() -> Self {
        Self {
            stage_id: String::from("ethernet"),
            ethertype_routes: Vec::new(),
            rx_enabled: false,
            tx_enabled: false,
            default_src_mac: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
        }
    }
    
    /// Set the stage ID
    pub fn with_stage_id(mut self, id: &str) -> Self {
        self.stage_id = String::from(id);
        self
    }
    
    /// Add an EtherType routing rule
    pub fn add_ethertype_route(mut self, ethertype: u16, next_stage: &str) -> Self {
        self.ethertype_routes.push((ethertype, String::from(next_stage)));
        self
    }
    
    /// Add routing using EtherType enum
    pub fn route_to(self, ethertype: EtherType, next_stage: &str) -> Self {
        self.add_ethertype_route(ethertype.as_u16(), next_stage)
    }
    
    /// Enable receive processing for this stage
    pub fn enable_rx(mut self) -> Self {
        self.rx_enabled = true;
        self
    }
    
    /// Enable transmit processing for this stage
    pub fn enable_tx(mut self) -> Self {
        self.tx_enabled = true;
        self
    }
    
    /// Set the default source MAC address for transmission
    pub fn with_src_mac(mut self, mac: [u8; 6]) -> Self {
        self.default_src_mac = mac;
        self
    }
    
    /// Build the final FlexibleStage
    pub fn build(self) -> FlexibleStage {
        let mut stage = FlexibleStage::new(self.stage_id);
        
        // Build and set rx handler if enabled
        if self.rx_enabled {
            let mut matcher = EtherTypeToStage::new();
            for (ethertype, next_stage) in self.ethertype_routes {
                matcher = matcher.add_mapping(ethertype, &next_stage);
            }
            stage.set_rx_handler(Box::new(EthernetRxHandler::new(matcher)));
        }
        
        // Build and set tx handler if enabled
        if self.tx_enabled {
            stage.set_tx_handler(Box::new(EthernetTxHandler::new(self.default_src_mac)));
        }
        
        stage
    }
}

impl Default for EthernetStageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for IPv4 protocol stages
pub struct IPv4Stage;

impl IPv4Stage {
    /// Create a new IPv4 stage builder
    pub fn builder() -> IPv4StageBuilder {
        IPv4StageBuilder::new()
    }
}

/// Fluent builder for IPv4 stages
pub struct IPv4StageBuilder {
    stage_id: String,
    protocol_routes: Vec<(u8, String)>,
    rx_enabled: bool,
    tx_enabled: bool,
    default_src_ip: [u8; 4],
}

impl IPv4StageBuilder {
    /// Create a new IPv4 stage builder
    pub fn new() -> Self {
        Self {
            stage_id: String::from("ipv4"),
            protocol_routes: Vec::new(),
            rx_enabled: false,
            tx_enabled: false,
            default_src_ip: [192, 168, 1, 1],
        }
    }
    
    /// Set the stage ID
    pub fn with_stage_id(mut self, id: &str) -> Self {
        self.stage_id = String::from(id);
        self
    }
    
    /// Add an IP protocol routing rule
    pub fn add_protocol_route(mut self, protocol: u8, next_stage: &str) -> Self {
        self.protocol_routes.push((protocol, String::from(next_stage)));
        self
    }
    
    /// Add routing using IpProtocol enum
    pub fn route_to(self, protocol: IpProtocol, next_stage: &str) -> Self {
        self.add_protocol_route(protocol.as_u8(), next_stage)
    }
    
    /// Enable receive processing for this stage
    pub fn enable_rx(mut self) -> Self {
        self.rx_enabled = true;
        self
    }
    
    /// Enable transmit processing for this stage
    pub fn enable_tx(mut self) -> Self {
        self.tx_enabled = true;
        self
    }
    
    /// Set the default source IP address for transmission
    pub fn with_src_ip(mut self, ip: [u8; 4]) -> Self {
        self.default_src_ip = ip;
        self
    }
    
    /// Build the final FlexibleStage
    pub fn build(self) -> FlexibleStage {
        let mut stage = FlexibleStage::new(self.stage_id);
        
        // Build and set rx handler if enabled
        if self.rx_enabled {
            let mut matcher = IpProtocolToStage::new();
            for (protocol, next_stage) in self.protocol_routes {
                matcher = matcher.add_mapping(protocol, &next_stage);
            }
            stage.set_rx_handler(Box::new(IPv4RxHandler::new(matcher)));
        }
        
        // Build and set tx handler if enabled
        if self.tx_enabled {
            stage.set_tx_handler(Box::new(IPv4TxHandler::new(self.default_src_ip)));
        }
        
        stage
    }
}

impl Default for IPv4StageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test_case]
    fn test_ethernet_rx_handler() {
        let matcher = EtherTypeToStage::new()
            .add_mapping(0x0800, "ipv4")
            .add_mapping(0x86DD, "ipv6");
        
        let handler = EthernetRxHandler::new(matcher);
        
        // Create a packet with Ethernet header (IPv4 EtherType)
        let mut packet = NetworkPacket::new(
            vec![
                // Ethernet header (14 bytes)
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, // Dest MAC
                0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, // Src MAC
                0x08, 0x00, // EtherType (IPv4)
                // Payload
                0x45, 0x00, 0x00, 0x20, // Start of IPv4 header
            ],
            "test".to_string()
        );
        
        let result = handler.handle(&mut packet);
        assert!(result.is_ok());
        
        let action = result.unwrap();
        assert_eq!(action, NextAction::jump_to("ipv4"));
        
        // Check that Ethernet header was extracted
        let eth_header = packet.get_header("ethernet").unwrap();
        assert_eq!(eth_header.len(), 14);
        assert_eq!(eth_header[0..6], [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]); // Dest MAC
        assert_eq!(eth_header[6..12], [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]); // Src MAC
        assert_eq!(eth_header[12..14], [0x08, 0x00]); // EtherType
        
        // Check that payload was updated
        assert_eq!(packet.payload(), &[0x45, 0x00, 0x00, 0x20]);
    }

    #[test_case]
    fn test_ethernet_tx_handler() {
        let handler = EthernetTxHandler::with_default_mac();
        
        let mut packet = NetworkPacket::new(
            vec![0x45, 0x00, 0x00, 0x20], // IPv4 payload
            "test".to_string()
        );
        
        // Set required hints
        packet.set_hint("ethertype", "0x0800");
        packet.set_hint("dest_mac", "aa:bb:cc:dd:ee:ff");
        
        let result = handler.handle(&mut packet);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NextAction::Complete);
        
        // Check that Ethernet header was prepended
        let payload = packet.payload();
        assert_eq!(payload.len(), 18); // 14 (Ethernet) + 4 (original payload)
        
        // Check Ethernet header fields
        assert_eq!(payload[0..6], [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]); // Dest MAC
        assert_eq!(payload[6..12], [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]); // Src MAC (default)
        assert_eq!(payload[12..14], [0x08, 0x00]); // EtherType
        assert_eq!(payload[14..18], [0x45, 0x00, 0x00, 0x20]); // Original payload
    }

    #[test_case]
    fn test_ipv4_rx_handler() {
        let matcher = IpProtocolToStage::new()
            .add_mapping(6, "tcp")
            .add_mapping(17, "udp");
        
        let handler = IPv4RxHandler::new(matcher);
        
        // Create a packet with IPv4 header (TCP protocol)
        let mut packet = NetworkPacket::new(
            vec![
                // IPv4 header (20 bytes)
                0x45, // Version (4) + IHL (5)
                0x00, // DSCP + ECN
                0x00, 0x28, // Total Length (40 bytes)
                0x00, 0x01, // Identification
                0x40, 0x00, // Flags + Fragment Offset
                0x40, // TTL (64)
                0x06, // Protocol (TCP)
                0x00, 0x00, // Header Checksum (placeholder)
                192, 168, 1, 1, // Source IP
                192, 168, 1, 2, // Destination IP
                // TCP payload
                0x00, 0x50, 0x00, 0x80, // TCP header start
            ],
            "test".to_string()
        );
        
        let result = handler.handle(&mut packet);
        assert!(result.is_ok());
        
        let action = result.unwrap();
        assert_eq!(action, NextAction::jump_to("tcp"));
        
        // Check that IPv4 header was extracted
        let ipv4_header = packet.get_header("ipv4").unwrap();
        assert_eq!(ipv4_header.len(), 20);
        assert_eq!(ipv4_header[0], 0x45); // Version + IHL
        assert_eq!(ipv4_header[9], 0x06); // Protocol (TCP)
        
        // Check that payload was updated (TCP header)
        assert_eq!(packet.payload(), &[0x00, 0x50, 0x00, 0x80]);
    }

    #[test_case]
    fn test_ipv4_tx_handler() {
        let handler = IPv4TxHandler::with_default_ip();
        
        let mut packet = NetworkPacket::new(
            vec![0x00, 0x50, 0x00, 0x80], // TCP payload
            "test".to_string()
        );
        
        // Set required hints
        packet.set_hint("destination_ip", "10.0.0.1");
        packet.set_hint("protocol", "6"); // TCP
        
        let result = handler.handle(&mut packet);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NextAction::jump_to("ethernet"));
        
        // Check that hints were set for Ethernet layer
        assert_eq!(packet.get_hint("ethertype"), Some("0x0800"));
        assert_eq!(packet.get_hint("dest_mac"), Some("ff:ff:ff:ff:ff:ff"));
        
        // Check that IPv4 header was prepended
        let payload = packet.payload();
        assert_eq!(payload.len(), 24); // 20 (IPv4) + 4 (original payload)
        
        // Check IPv4 header fields
        assert_eq!(payload[0], 0x45); // Version (4) + IHL (5)
        assert_eq!(payload[9], 6); // Protocol (TCP)
        assert_eq!(payload[12..16], [192, 168, 1, 1]); // Source IP (default)
        assert_eq!(payload[16..20], [10, 0, 0, 1]); // Destination IP
        assert_eq!(payload[20..24], [0x00, 0x50, 0x00, 0x80]); // Original payload
    }

    #[test_case]
    fn test_ethernet_stage_builder() {
        let stage = EthernetStage::builder()
            .with_stage_id("eth0")
            .add_ethertype_route(0x0800, "ipv4")
            .route_to(EtherType::IPv6, "ipv6")
            .route_to(EtherType::ARP, "arp")
            .enable_rx()
            .enable_tx()
            .build();
        
        assert_eq!(stage.stage_id, "eth0");
        assert!(stage.has_rx_handler());
        assert!(stage.has_tx_handler());
    }

    #[test_case]
    fn test_ipv4_stage_builder() {
        let stage = IPv4Stage::builder()
            .with_stage_id("ip")
            .add_protocol_route(6, "tcp")
            .route_to(IpProtocol::UDP, "udp")
            .route_to(IpProtocol::ICMP, "icmp")
            .enable_rx()
            .enable_tx()
            .build();
        
        assert_eq!(stage.stage_id, "ip");
        assert!(stage.has_rx_handler());
        assert!(stage.has_tx_handler());
    }

    #[test_case]
    fn test_mac_parsing() {
        // Test valid MAC address
        let mac = EthernetTxHandler::parse_mac("aa:bb:cc:dd:ee:ff").unwrap();
        assert_eq!(mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        
        // Test invalid MAC addresses
        assert!(EthernetTxHandler::parse_mac("invalid").is_err());
        assert!(EthernetTxHandler::parse_mac("aa:bb:cc:dd:ee").is_err()); // Too few parts
        assert!(EthernetTxHandler::parse_mac("aa:bb:cc:dd:ee:ff:gg").is_err()); // Too many parts
        assert!(EthernetTxHandler::parse_mac("xx:bb:cc:dd:ee:ff").is_err()); // Invalid hex
    }

    #[test_case]
    fn test_ip_parsing() {
        // Test valid IP address
        let ip = IPv4TxHandler::parse_ip("192.168.1.1").unwrap();
        assert_eq!(ip, [192, 168, 1, 1]);
        
        // Test invalid IP addresses
        assert!(IPv4TxHandler::parse_ip("invalid").is_err());
        assert!(IPv4TxHandler::parse_ip("192.168.1").is_err()); // Too few parts
        assert!(IPv4TxHandler::parse_ip("192.168.1.1.1").is_err()); // Too many parts
        assert!(IPv4TxHandler::parse_ip("192.168.1.256").is_err()); // Invalid octet
    }

    #[test_case]
    fn test_ipv4_checksum() {
        // Test IPv4 header checksum calculation with known values
        let mut header = vec![
            0x45, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00,
            0x40, 0x06, 0x00, 0x00, 0xac, 0x10, 0x0a, 0x63,
            0xac, 0x10, 0x0a, 0x0c
        ];
        
        // Calculate checksum (bytes 10-11 should be zeroed first)
        header[10] = 0x00;
        header[11] = 0x00;
        let checksum = IPv4TxHandler::calculate_checksum(&header);
        
        // This should produce a valid checksum (exact value depends on implementation)
        assert_ne!(checksum, 0x0000); // Should not be zero for this header
        
        // Verify checksum validation by setting it in header and recalculating
        let checksum_bytes = checksum.to_be_bytes();
        header[10] = checksum_bytes[0];
        header[11] = checksum_bytes[1];
        
        // Recalculating with correct checksum should give 0
        let verify_checksum = IPv4TxHandler::calculate_checksum(&header);
        assert_eq!(verify_checksum, 0x0000);
    }

    #[test_case]
    fn test_error_handling() {
        // Test insufficient data for Ethernet
        let matcher = EtherTypeToStage::new().add_mapping(0x0800, "ipv4");
        let handler = EthernetRxHandler::new(matcher);
        
        let mut packet = NetworkPacket::new(
            vec![0x01, 0x02, 0x03], // Too short for Ethernet header
            "test".to_string()
        );
        
        let result = handler.handle(&mut packet);
        assert!(result.is_err());
        
        // Test unsupported EtherType
        let matcher = EtherTypeToStage::new().add_mapping(0x0800, "ipv4");
        let handler = EthernetRxHandler::new(matcher);
        
        let mut packet = NetworkPacket::new(
            vec![
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, // Dest MAC
                0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, // Src MAC
                0x12, 0x34, // Unsupported EtherType
                0xFF, 0xFF, // Payload
            ],
            "test".to_string()
        );
        
        let result = handler.handle(&mut packet);
        assert!(result.is_err());
        match result {
            Err(NetworkError::UnsupportedProtocol { layer, protocol }) => {
                assert_eq!(layer, "ethernet");
                assert_eq!(protocol, "0x1234");
            }
            _ => panic!("Expected UnsupportedProtocol error"),
        }
    }
}