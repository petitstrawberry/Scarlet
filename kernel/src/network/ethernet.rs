//! Ethernet protocol layer
//!
//! This module provides Ethernet II frame handling for the network stack.
//! It implements the NetworkLayer trait for Ethernet encapsulation/decapsulation.
//!
//! # Design
//!
//! The EthernetLayer manages:
//! - Multiple network interfaces with their MAC addresses
//! - Interface selection for outgoing packets
//! - Device access for sending/receiving frames
//!
//! This design supports multiple network interfaces (eth0, eth1, wlan0, etc.).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::device::network::DevicePacket;
use crate::device::network::MacAddress;
use crate::early_println;
use crate::network::protocol_stack::{LayerContext, NetworkLayer, NetworkLayerStats};
use crate::network::socket::SocketError;
use crate::network::NetworkInterface;

/// Ethernet frame header (14 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct EthernetHeader {
    /// Destination MAC address (6 bytes)
    pub dest_mac: [u8; 6],
    /// Source MAC address (6 bytes)
    pub src_mac: [u8; 6],
    /// EtherType (2 bytes) - protocol identifier
    pub ether_type: u16,
}

impl EthernetHeader {
    /// Create a new Ethernet header
    pub fn new(dest_mac: [u8; 6], src_mac: [u8; 6], ether_type: u16) -> Self {
        Self {
            dest_mac,
            src_mac,
            ether_type,
        }
    }

    /// Serialize header to bytes
    pub fn to_bytes(&self) -> [u8; 14] {
        let mut bytes = [0u8; 14];
        bytes[0..6].copy_from_slice(&self.dest_mac);
        bytes[6..12].copy_from_slice(&self.src_mac);
        bytes[12..14].copy_from_slice(&self.ether_type.to_be_bytes());
        bytes
    }

    /// Parse header from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 14 {
            return None;
        }
        let mut dest_mac = [0u8; 6];
        let mut src_mac = [0u8; 6];
        dest_mac.copy_from_slice(&bytes[0..6]);
        src_mac.copy_from_slice(&bytes[6..12]);
        let ether_type = u16::from_be_bytes([bytes[12], bytes[13]]);
        Some(Self {
            dest_mac,
            src_mac,
            ether_type,
        })
    }

    /// Get EtherType as big-endian bytes
    pub fn ether_type_be(&self) -> [u8; 2] {
        self.ether_type.to_be_bytes()
    }
}

/// Ethernet EtherType constants
pub mod ether_type {
    /// IPv4 protocol
    pub const IPV4: u16 = 0x0800;
    /// ARP protocol
    pub const ARP: u16 = 0x0806;
    /// IPv6 protocol
    pub const IPV6: u16 = 0x86DD;
    /// VLAN-tagged frame (802.1Q)
    pub const VLAN: u16 = 0x8100;
}

/// Maximum Transmission Unit for Ethernet (standard)
pub const ETHERNET_MTU: usize = 1500;

/// Minimum Ethernet frame size (64 bytes including FCS)
pub const ETHERNET_MIN_SIZE: usize = 64;

/// Ethernet header size
pub const ETHERNET_HEADER_SIZE: usize = 14;

/// Ethernet interface information
#[derive(Debug, Clone)]
pub struct EthernetInterfaceInfo {
    /// Interface name (e.g., "eth0", "wlan0")
    pub name: String,
    /// MAC address
    pub mac: MacAddress,
    /// Maximum Transmission Unit
    pub mtu: usize,
}

/// Ethernet layer
///
/// Handles Ethernet II frame encapsulation and decapsulation.
/// Manages multiple interfaces and routes frames based on EtherType field.
pub struct EthernetLayer {
    /// Registered interfaces: name -> info
    interfaces: RwLock<BTreeMap<String, EthernetInterfaceInfo>>,
    /// Interface devices: name -> device (kept separate for Arc<dyn> handling)
    devices: RwLock<BTreeMap<String, Arc<dyn NetworkInterface>>>,
    /// Default interface name
    default_interface: RwLock<Option<String>>,
    /// Protocol handlers registered by EtherType
    protocols: RwLock<BTreeMap<u16, Arc<dyn NetworkLayer>>>,
    /// Statistics
    stats: RwLock<NetworkLayerStats>,
}

impl EthernetLayer {
    /// Create a new Ethernet layer
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            interfaces: RwLock::new(BTreeMap::new()),
            devices: RwLock::new(BTreeMap::new()),
            default_interface: RwLock::new(None),
            protocols: RwLock::new(BTreeMap::new()),
            stats: RwLock::new(NetworkLayerStats::default()),
        })
    }

    /// Initialize and register the Ethernet layer with NetworkManager
    ///
    /// This is the first layer to be initialized as it has no dependencies.
    /// Other layers (IPv4, ARP) will register their protocols with this layer.
    pub fn init(network_manager: &crate::network::NetworkManager) {
        let layer = Self::new();
        network_manager.register_layer("ethernet", layer);
    }

    /// Register a network interface
    pub fn register_interface(
        &self,
        name: &str,
        mac: MacAddress,
        device: Arc<dyn NetworkInterface>,
    ) {
        let info = EthernetInterfaceInfo {
            name: name.to_string(),
            mac,
            mtu: ETHERNET_MTU,
        };

        self.interfaces.write().insert(name.to_string(), info);
        self.devices.write().insert(name.to_string(), device);

        // First interface becomes default
        if self.default_interface.read().is_none() {
            *self.default_interface.write() = Some(name.to_string());
        }
    }

    /// Unregister a network interface
    pub fn unregister_interface(&self, name: &str) {
        self.interfaces.write().remove(name);
        self.devices.write().remove(name);

        // If this was the default, pick another
        let mut default = self.default_interface.write();
        if default.as_deref() == Some(name) {
            *default = self.interfaces.read().keys().next().cloned();
        }
    }

    /// Get interface info by name
    pub fn get_interface(&self, name: &str) -> Option<EthernetInterfaceInfo> {
        self.interfaces.read().get(name).cloned()
    }

    /// Get MAC address for an interface
    pub fn get_mac(&self, name: &str) -> Option<MacAddress> {
        self.interfaces.read().get(name).map(|i| i.mac)
    }

    /// Get default interface name
    pub fn get_default_interface(&self) -> Option<String> {
        self.default_interface.read().clone()
    }

    /// Set default interface
    pub fn set_default_interface(&self, name: &str) {
        if self.interfaces.read().contains_key(name) {
            *self.default_interface.write() = Some(name.to_string());
        }
    }

    /// Get all interface names
    pub fn list_interfaces(&self) -> Vec<String> {
        self.interfaces.read().keys().cloned().collect()
    }

    /// Get device for an interface
    fn get_device(&self, name: &str) -> Option<Arc<dyn NetworkInterface>> {
        self.devices.read().get(name).cloned()
    }

    /// Resolve destination MAC address for sending
    ///
    /// This method determines the destination MAC address for an outgoing packet:
    /// 1. If explicit `eth_dst_mac` is in context, use it directly
    /// 2. If destination is broadcast IP (255.255.255.255), use broadcast MAC
    /// 3. Otherwise, look up in ARP cache (using `next_hop` if set, else `dst_ip`)
    ///
    /// # Arguments
    ///
    /// * `context` - Layer context with addressing info
    /// * `interface` - Interface name for per-interface ARP cache lookup
    ///
    /// # Returns
    ///
    /// MAC address to use as destination, or error if resolution fails
    fn resolve_dest_mac(
        &self,
        context: &LayerContext,
        interface: &str,
    ) -> Result<[u8; 6], SocketError> {
        // 1. Check for explicit destination MAC in context
        if let Some(mac_bytes) = context.get("eth_dst_mac") {
            if mac_bytes.len() >= 6 {
                let mut mac = [0u8; 6];
                mac.copy_from_slice(&mac_bytes[..6]);
                return Ok(mac);
            }
        }

        // 2. Check if destination IP is broadcast
        if let Some(dst_ip_bytes) = context.get("dst_ip") {
            if dst_ip_bytes.len() >= 4 {
                // Broadcast IP (255.255.255.255) → Broadcast MAC
                if dst_ip_bytes[0] == 255
                    && dst_ip_bytes[1] == 255
                    && dst_ip_bytes[2] == 255
                    && dst_ip_bytes[3] == 255
                {
                    return Ok([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
                }
            }
        }

        // 3. Determine which IP to resolve (next_hop from gateway, or direct dst_ip)
        let resolve_ip = context.get("next_hop").or_else(|| context.get("dst_ip"));

        if let Some(ip_bytes) = resolve_ip {
            if ip_bytes.len() >= 4 {
                let ip = crate::network::ipv4::Ipv4Address::from_bytes([
                    ip_bytes[0],
                    ip_bytes[1],
                    ip_bytes[2],
                    ip_bytes[3],
                ]);

                // Look up in ARP cache via NetworkManager (interface-aware)
                if let Some(arp_layer) =
                    crate::network::protocol_stack::get_network_manager().get_layer("arp")
                {
                    if let Some(arp) = arp_layer
                        .as_any()
                        .downcast_ref::<crate::network::arp::ArpLayer>()
                    {
                        // Use interface-aware lookup
                        if let Some(mac) = arp.lookup_on_interface(interface, ip) {
                            return Ok(mac);
                        }

                        // Not in cache - trigger ARP request
                        early_println!(
                            "[Ethernet] ARP cache miss for {}.{}.{}.{} on {}, need resolution",
                            ip_bytes[0],
                            ip_bytes[1],
                            ip_bytes[2],
                            ip_bytes[3],
                            interface
                        );

                        // Trigger ARP request with interface info
                        let mut arp_context = LayerContext::new();
                        arp_context.set("interface", interface.as_bytes());
                        let _ = arp.send_request(ip, &arp_context, &[]);

                        return Err(SocketError::WouldBlock);
                    }
                }
            }
        }

        // No way to determine destination MAC
        early_println!("[Ethernet] Cannot resolve destination MAC: no dst_ip or eth_dst_mac");
        Err(SocketError::NoRoute)
    }

    /// Receive a frame on a specific interface
    ///
    /// This method should be called by drivers to process incoming frames.
    /// It passes the interface name to upper layers via context.
    pub fn receive_on_interface(&self, frame: &[u8], interface: &str) -> Result<(), SocketError> {
        if frame.len() < ETHERNET_HEADER_SIZE {
            return Err(SocketError::InvalidPacket);
        }

        let header = EthernetHeader::from_bytes(&frame[..ETHERNET_HEADER_SIZE])
            .ok_or(SocketError::InvalidPacket)?;

        early_println!(
            "[Ethernet] RX on {}: {} bytes (type=0x{:04X})",
            interface,
            frame.len(),
            header.ether_type
        );

        let payload = &frame[ETHERNET_HEADER_SIZE..];

        let mut stats = self.stats.write();
        stats.packets_received += 1;
        stats.bytes_received += frame.len() as u64;
        drop(stats);

        // Build context with interface info and source MAC
        let mut context = LayerContext::new();
        context.set("interface", interface.as_bytes());
        context.set("eth_src_mac", &header.src_mac);
        context.set("eth_dst_mac", &header.dest_mac);

        let protocols = self.protocols.read();
        if let Some(handler) = protocols.get(&header.ether_type) {
            handler.receive(payload, Some(&context))
        } else {
            Ok(())
        }
    }
}

impl NetworkLayer for EthernetLayer {
    fn register_protocol(&self, proto_num: u16, handler: Arc<dyn NetworkLayer>) {
        self.protocols.write().insert(proto_num, handler);
    }

    fn send(
        &self,
        packet: &[u8],
        context: &LayerContext,
        _next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // Get interface from context, or use default
        let interface_name = context
            .get("interface")
            .and_then(|b| core::str::from_utf8(b).ok())
            .map(String::from)
            .or_else(|| self.get_default_interface())
            .ok_or(SocketError::NoRoute)?;

        // Get source MAC from interface
        let src_mac = self.get_mac(&interface_name).ok_or(SocketError::NoRoute)?;

        // Determine destination MAC
        let dest_mac = match self.resolve_dest_mac(context, &interface_name) {
            Ok(mac) => mac,
            Err(SocketError::WouldBlock) => {
                // ARP resolution pending - queue the packet for later transmission
                // Get the IP address we're resolving (next_hop or dst_ip)
                let resolve_ip = context.get("next_hop").or_else(|| context.get("dst_ip"));
                if let Some(ip_bytes) = resolve_ip {
                    if ip_bytes.len() >= 4 {
                        let ip = crate::network::ipv4::Ipv4Address::from_bytes([
                            ip_bytes[0],
                            ip_bytes[1],
                            ip_bytes[2],
                            ip_bytes[3],
                        ]);

                        // Queue packet in ARP layer for later transmission
                        if let Some(arp_layer) =
                            crate::network::protocol_stack::get_network_manager().get_layer("arp")
                        {
                            if let Some(arp) = arp_layer
                                .as_any()
                                .downcast_ref::<crate::network::arp::ArpLayer>()
                            {
                                early_println!(
                                    "[Ethernet] Queuing packet ({} bytes) for ARP resolution of {}.{}.{}.{}",
                                    packet.len(),
                                    ip_bytes[0],
                                    ip_bytes[1],
                                    ip_bytes[2],
                                    ip_bytes[3]
                                );
                                arp.queue_packet_on_interface(&interface_name, ip, packet.to_vec());
                            }
                        }
                    }
                }
                // Return WouldBlock so caller knows packet is queued, not sent
                return Err(SocketError::WouldBlock);
            }
            Err(e) => return Err(e),
        };

        // Get EtherType
        let ether_type = if let Some(eth_type) = context.get("eth_type") {
            if eth_type.len() >= 2 {
                u16::from_be_bytes([eth_type[0], eth_type[1]])
            } else if !eth_type.is_empty() {
                eth_type[0] as u16
            } else {
                ether_type::IPV4
            }
        } else {
            ether_type::IPV4
        };

        // Build frame
        let header = EthernetHeader::new(dest_mac, *src_mac.as_bytes(), ether_type);
        let total_size = ETHERNET_HEADER_SIZE + packet.len();

        let mut frame = Vec::with_capacity(total_size);
        frame.extend_from_slice(&header.to_bytes());
        frame.extend_from_slice(packet);

        // Pad to minimum frame size
        let min_payload = ETHERNET_MIN_SIZE.saturating_sub(4);
        if frame.len() < min_payload {
            frame.resize(min_payload, 0);
        }
        let frame_len = frame.len();

        // Send through device
        if let Some(device) = self.get_device(&interface_name) {
            early_println!(
                "[Ethernet] Sending {} bytes via {} to {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} (type=0x{:04X})",
                frame_len,
                interface_name,
                dest_mac[0],
                dest_mac[1],
                dest_mac[2],
                dest_mac[3],
                dest_mac[4],
                dest_mac[5],
                ether_type
            );
            let pkt = DevicePacket::with_data(frame);
            device.send(pkt).map_err(|e| {
                early_println!("[Ethernet] Send failed: {}", e);
                SocketError::Other("send failed".into())
            })?;
            early_println!("[Ethernet] Send succeeded");
        } else {
            early_println!("[Ethernet] No device for interface {}", interface_name);
            return Err(SocketError::NoRoute);
        }

        let mut stats = self.stats.write();
        stats.packets_sent += 1;
        stats.bytes_sent += total_size as u64;

        Ok(())
    }

    fn receive(&self, frame: &[u8], context: Option<&LayerContext>) -> Result<(), SocketError> {
        // If context has interface, use it; otherwise use default
        let interface = context
            .and_then(|c| c.get("interface"))
            .and_then(|b| core::str::from_utf8(b).ok())
            .map(String::from)
            .or_else(|| self.get_default_interface());

        if let Some(iface) = interface {
            self.receive_on_interface(frame, &iface)
        } else {
            // Fallback: receive without interface context
            if frame.len() < ETHERNET_HEADER_SIZE {
                return Err(SocketError::InvalidPacket);
            }

            let header = EthernetHeader::from_bytes(&frame[..ETHERNET_HEADER_SIZE])
                .ok_or(SocketError::InvalidPacket)?;

            early_println!(
                "[Ethernet] RX: {} bytes (type=0x{:04X})",
                frame.len(),
                header.ether_type
            );

            let payload = &frame[ETHERNET_HEADER_SIZE..];

            let mut stats = self.stats.write();
            stats.packets_received += 1;
            stats.bytes_received += frame.len() as u64;

            let protocols = self.protocols.read();
            if let Some(handler) = protocols.get(&header.ether_type) {
                handler.receive(payload, None)
            } else {
                Ok(())
            }
        }
    }

    fn name(&self) -> &'static str {
        "Ethernet"
    }

    fn stats(&self) -> NetworkLayerStats {
        self.stats.read().clone()
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_ethernet_header_serialization() {
        let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let dest_mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let ether_type = 0x0800; // IPv4

        let header = EthernetHeader::new(dest_mac, src_mac, ether_type);
        let bytes = header.to_bytes();

        assert_eq!(bytes.len(), 14);
        assert_eq!(&bytes[0..6], &dest_mac);
        assert_eq!(&bytes[6..12], &src_mac);
        assert_eq!(u16::from_be_bytes([bytes[12], bytes[13]]), ether_type);
    }

    #[test_case]
    fn test_ethernet_header_parsing() {
        let mut bytes = [0u8; 14];
        bytes[0..6].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        bytes[6..12].copy_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        bytes[12..14].copy_from_slice(&ether_type::IPV4.to_be_bytes());

        let header = EthernetHeader::from_bytes(&bytes).unwrap();
        assert_eq!(header.dest_mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        assert_eq!(header.src_mac, [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        assert_eq!(header.ether_type, 0x0800);
    }

    #[test_case]
    fn test_ethernet_header_invalid_length() {
        let bytes = [0u8; 10];
        assert!(EthernetHeader::from_bytes(&bytes).is_none());
    }

    #[test_case]
    fn test_ether_type_constants() {
        assert_eq!(ether_type::IPV4, 0x0800);
        assert_eq!(ether_type::ARP, 0x0806);
        assert_eq!(ether_type::IPV6, 0x86DD);
        assert_eq!(ether_type::VLAN, 0x8100);
    }

    #[test_case]
    fn test_ethernet_layer_creation() {
        let eth_layer = EthernetLayer::new();
        // New layer has no interfaces initially
        assert!(eth_layer.get_default_interface().is_none());
        assert!(eth_layer.list_interfaces().is_empty());
    }

    #[test_case]
    fn test_ethernet_layer_register_interface() {
        let eth_layer = EthernetLayer::new();
        let mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

        // Create a mock device - we can't easily test without one,
        // so we test interface registration by checking get_interface
        // Note: In real usage, you'd pass an actual Arc<dyn NetworkInterface>

        // For now, test that interface info struct works
        let info = EthernetInterfaceInfo {
            name: "eth0".into(),
            mac,
            mtu: ETHERNET_MTU,
        };
        assert_eq!(info.name, "eth0");
        assert_eq!(info.mac, mac);
        assert_eq!(info.mtu, ETHERNET_MTU);
    }

    #[test_case]
    fn test_ethernet_layer_default_interface() {
        let eth_layer = EthernetLayer::new();
        // Initially no default
        assert!(eth_layer.get_default_interface().is_none());
    }
}
