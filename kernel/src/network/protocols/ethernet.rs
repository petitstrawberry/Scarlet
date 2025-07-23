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
        // 1. Ethernetヘッダー最小サイズ検証
        packet.validate_payload_size(14)?;
        let payload = packet.payload().clone();
        
        // 2. Ethernetヘッダー解析
        let dest_mac = &payload[0..6];
        let src_mac = &payload[6..12];
        let ether_type = u16::from_be_bytes([payload[12], payload[13]]);
        
        // 3. VLANタグ処理（オプション）
        let (actual_ether_type, header_length) = if ether_type == 0x8100 {
            // VLAN tagged frame
            packet.validate_payload_size(18)?;
            let vlan_tag = u16::from_be_bytes([payload[14], payload[15]]);
            let actual_ether_type = u16::from_be_bytes([payload[16], payload[17]]);
            
            // VLAN情報をhintとして保存
            packet.set_hint("vlan_tag", &format!("{}", vlan_tag));
            packet.set_hint("vlan_priority", &format!("{}", (vlan_tag >> 13) & 0x7));
            packet.set_hint("vlan_id", &format!("{}", vlan_tag & 0xFFF));
            
            (actual_ether_type, 18)
        } else {
            (ether_type, 14)
        };
        
        // 4. Ethernetヘッダー情報を保存
        packet.add_header("ethernet", payload[0..header_length].to_vec());
        packet.set_hint("src_mac", &Self::format_mac(src_mac));
        packet.set_hint("dest_mac", &Self::format_mac(dest_mac));
        packet.set_hint("ether_type", &format!("0x{:04x}", actual_ether_type));
        
        // 5. ペイロード更新
        packet.set_payload(payload[header_length..].to_vec());
        
        // 6. 内部でのルーティング決定（O(1) HashMap）
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
        // 1. hintsから必要情報取得
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
        
        // 3. VLANタグ処理（オプション）
        let mut ethernet_header = Vec::with_capacity(18);
        ethernet_header.extend_from_slice(&dest_mac);
        ethernet_header.extend_from_slice(&src_mac);
        
        if let Some(vlan_tag_str) = packet.get_hint("vlan_tag") {
            // VLANタグ付きフレーム
            let vlan_tag = vlan_tag_str.parse::<u16>()
                .map_err(|_| NetworkError::invalid_hint_format("vlan_tag", vlan_tag_str))?;
            
            ethernet_header.extend_from_slice(&0x8100u16.to_be_bytes()); // VLAN EtherType
            ethernet_header.extend_from_slice(&vlan_tag.to_be_bytes());  // VLAN Tag
        }
        
        ethernet_header.extend_from_slice(&ether_type.to_be_bytes());
        
        // 4. ヘッダー追加してペイロード結合
        let payload = packet.payload().clone();
        packet.set_payload([ethernet_header, payload].concat());
        
        // 5. 送信完了
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
        // EtherTypeToStage マッチャー構築
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