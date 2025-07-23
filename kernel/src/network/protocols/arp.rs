//! ARP protocol implementation with beautiful builder API

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::format;

use crate::network::traits::{PacketHandler, NextAction};
use crate::network::packet::NetworkPacket;
use crate::network::error::NetworkError;
use crate::network::pipeline::{FlexibleStage, StageIdentifier};

/// ARP operation codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ArpOperation {
    Request = 1,
    Reply = 2,
}

impl ArpOperation {
    pub fn as_u16(self) -> u16 {
        self as u16
    }
    
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(ArpOperation::Request),
            2 => Some(ArpOperation::Reply),
            _ => None,
        }
    }
    
    pub fn name(self) -> &'static str {
        match self {
            ArpOperation::Request => "Request",
            ArpOperation::Reply => "Reply",
        }
    }
}

/// ARP receive handler
#[derive(Debug, Clone)]
pub struct ArpRxHandler;

impl ArpRxHandler {
    pub fn new() -> Self {
        Self
    }
}

impl PacketHandler for ArpRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // 1. Validate minimum ARP packet size
        packet.validate_payload_size(28)?;
        let payload = packet.payload().clone();
        
        // 2. Parse ARP header
        let hardware_type = u16::from_be_bytes([payload[0], payload[1]]);
        let protocol_type = u16::from_be_bytes([payload[2], payload[3]]);
        let hardware_length = payload[4];
        let protocol_length = payload[5];
        let operation = u16::from_be_bytes([payload[6], payload[7]]);
        
        // 3. Basic validation (only support Ethernet/IPv4)
        if hardware_type != 1 {
            return Err(NetworkError::unsupported_protocol("arp", &format!("hardware_type {}", hardware_type)));
        }
        if protocol_type != 0x0800 {
            return Err(NetworkError::unsupported_protocol("arp", &format!("protocol_type 0x{:04x}", protocol_type)));
        }
        if hardware_length != 6 || protocol_length != 4 {
            return Err(NetworkError::unsupported_protocol("arp", "invalid address lengths"));
        }
        
        // 4. Parse addresses
        let sender_mac = &payload[8..14];
        let sender_ip = &payload[14..18];
        let target_mac = &payload[18..24];
        let target_ip = &payload[24..28];
        
        // 5. Save ARP information as hints
        packet.add_header("arp", payload[0..28].to_vec());
        packet.set_hint("arp_hardware_type", &format!("{}", hardware_type));
        packet.set_hint("arp_protocol_type", &format!("0x{:04x}", protocol_type));
        packet.set_hint("arp_operation", &format!("{}", operation));
        packet.set_hint("arp_sender_mac", &Self::format_mac(sender_mac));
        packet.set_hint("arp_sender_ip", &Self::format_ip(sender_ip));
        packet.set_hint("arp_target_mac", &Self::format_mac(target_mac));
        packet.set_hint("arp_target_ip", &Self::format_ip(target_ip));
        
        if let Some(op) = ArpOperation::from_u16(operation) {
            packet.set_hint("arp_operation_name", op.name());
        }
        
        // 6. Update payload (remove ARP header)
        packet.set_payload(payload[28..].to_vec());
        
        // 7. ARP processing complete (normally ends here)
        Ok(NextAction::Complete)
    }
}

impl ArpRxHandler {
    fn format_mac(mac: &[u8]) -> String {
        format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5])
    }
    
    fn format_ip(ip: &[u8]) -> String {
        format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
    }
}

/// ARP transmit handler
#[derive(Debug)]
pub struct ArpTxHandler;

impl ArpTxHandler {
    pub fn new() -> Self {
        Self
    }
}

impl PacketHandler for ArpTxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // 1. Get required information from hints
        let operation_str = packet.get_hint("arp_operation")
            .ok_or_else(|| NetworkError::missing_hint("arp_operation"))?;
        let sender_mac_str = packet.get_hint("arp_sender_mac")
            .ok_or_else(|| NetworkError::missing_hint("arp_sender_mac"))?;
        let sender_ip_str = packet.get_hint("arp_sender_ip")
            .ok_or_else(|| NetworkError::missing_hint("arp_sender_ip"))?;
        let target_mac_str = packet.get_hint("arp_target_mac")
            .ok_or_else(|| NetworkError::missing_hint("arp_target_mac"))?
            .to_string();
        let target_ip_str = packet.get_hint("arp_target_ip")
            .ok_or_else(|| NetworkError::missing_hint("arp_target_ip"))?;
        
        // 2. Parse values
        let operation = operation_str.parse::<u16>()
            .map_err(|_| NetworkError::invalid_hint_format("arp_operation", operation_str))?;
        let sender_mac = Self::parse_mac(sender_mac_str)?;
        let sender_ip = Self::parse_ip(sender_ip_str)?;
        let target_mac = Self::parse_mac(&target_mac_str)?;
        let target_ip = Self::parse_ip(target_ip_str)?;
        
        // 3. Build ARP packet
        let mut arp_packet = Vec::with_capacity(28);
        arp_packet.extend_from_slice(&1u16.to_be_bytes());      // Hardware type (Ethernet)
        arp_packet.extend_from_slice(&0x0800u16.to_be_bytes()); // Protocol type (IPv4)
        arp_packet.push(6);                                     // Hardware length
        arp_packet.push(4);                                     // Protocol length
        arp_packet.extend_from_slice(&operation.to_be_bytes()); // Operation
        arp_packet.extend_from_slice(&sender_mac);              // Sender MAC
        arp_packet.extend_from_slice(&sender_ip);               // Sender IP
        arp_packet.extend_from_slice(&target_mac);              // Target MAC
        arp_packet.extend_from_slice(&target_ip);               // Target IP
        
        // 4. Set payload
        packet.set_payload(arp_packet);
        
        // 5. Set hints for upper layer (Ethernet)
        packet.set_hint("ether_type", "0x0806");
        packet.set_hint("dest_mac", &target_mac_str);
        
        Ok(NextAction::Complete)
    }
}

impl ArpTxHandler {
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
}

/// ARP stage builder for beautiful API
pub struct ArpStageBuilder {
    stage_id: String,
    rx_enabled: bool,
    tx_enabled: bool,
}

impl ArpStageBuilder {
    pub fn new() -> Self {
        Self {
            stage_id: String::from("arp"),
            rx_enabled: false,
            tx_enabled: false,
        }
    }
    
    /// Set custom stage ID
    pub fn with_stage_id(mut self, id: &str) -> Self {
        self.stage_id = String::from(id);
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
    
    /// Build the stage
    pub fn build(self) -> FlexibleStage {
        let rx_handler = if self.rx_enabled {
            Some(Box::new(ArpRxHandler::new()) as Box<dyn PacketHandler>)
        } else {
            None
        };
        
        let tx_handler = if self.tx_enabled {
            Some(Box::new(ArpTxHandler::new()) as Box<dyn PacketHandler>)
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

/// ARP stage convenience struct
pub struct ArpStage;

impl StageIdentifier for ArpStage {
    fn stage_id() -> &'static str {
        "arp"
    }
}

impl ArpStage {
    pub fn builder() -> ArpStageBuilder {
        ArpStageBuilder::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{NetworkPacket, NextAction};
    use alloc::{vec, string::String};

    #[test_case]
    fn test_arp_stage_builder() {
        let stage = ArpStage::builder()
            .enable_both()
            .build();
        
        assert_eq!(stage.stage_id, "arp");
        assert!(stage.rx_handler.is_some());
        assert!(stage.tx_handler.is_some());
    }

    #[test_case]
    fn test_arp_receive_processing() {
        // Create ARP request packet
        let arp_packet = vec![
            // Hardware type: Ethernet (1)
            0x00, 0x01,
            // Protocol type: IPv4 (0x0800)
            0x08, 0x00,
            // Hardware length: 6
            0x06,
            // Protocol length: 4
            0x04,
            // Operation: Request (1)
            0x00, 0x01,
            // Sender MAC: 02:00:00:00:00:01
            0x02, 0x00, 0x00, 0x00, 0x00, 0x01,
            // Sender IP: 192.168.1.1
            192, 168, 1, 1,
            // Target MAC: 00:00:00:00:00:00
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Target IP: 192.168.1.2
            192, 168, 1, 2,
        ];
        
        let mut packet = NetworkPacket::new(arp_packet);
        
        let handler = ArpRxHandler::new();
        let result = handler.handle(&mut packet).unwrap();
        
        // ARP processing should complete
        assert_eq!(result, NextAction::Complete);
        
        // Check hints
        assert_eq!(packet.get_hint("arp_operation"), Some("1"));
        assert_eq!(packet.get_hint("arp_operation_name"), Some("Request"));
        assert_eq!(packet.get_hint("arp_sender_mac"), Some("02:00:00:00:00:01"));
        assert_eq!(packet.get_hint("arp_sender_ip"), Some("192.168.1.1"));
        assert_eq!(packet.get_hint("arp_target_mac"), Some("00:00:00:00:00:00"));
        assert_eq!(packet.get_hint("arp_target_ip"), Some("192.168.1.2"));
        
        // Check header saved
        assert!(packet.get_header("arp").is_some());
        assert_eq!(packet.get_header("arp").unwrap().len(), 28);
    }
}