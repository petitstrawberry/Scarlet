//! ARP (Address Resolution Protocol)
//!
//! This module provides ARP implementation for resolving IP addresses to MAC addresses.
//! It implements the NetworkLayer trait and manages an ARP cache.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::{Mutex, RwLock};

use crate::early_println;
use crate::network::ipv4::Ipv4Address;
use crate::network::protocol_stack::get_network_manager;
use crate::network::protocol_stack::{LayerContext, NetworkLayer, NetworkLayerStats};
use crate::network::socket::SocketError;

/// ARP operation types
pub mod operation {
    /// ARP request
    pub const REQUEST: u16 = 0x0001;
    /// ARP reply
    pub const REPLY: u16 = 0x0002;
}

/// Hardware type (Ethernet)
pub const HTYPE_ETHERNET: u16 = 0x0001;

/// Protocol type (IPv4)
pub const PTYPE_IPV4: u16 = 0x0800;

/// Hardware address length for Ethernet (6 bytes)
pub const HLEN_ETHERNET: u8 = 6;

/// Protocol address length for IPv4 (4 bytes)
pub const PLEN_IPV4: u8 = 4;

/// ARP packet header (28 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct ArpPacket {
    /// Hardware type (e.g., Ethernet)
    pub htype: u16,
    /// Protocol type (e.g., IPv4)
    pub ptype: u16,
    /// Hardware address length
    pub hlen: u8,
    /// Protocol address length
    pub plen: u8,
    /// Operation (request/reply)
    pub operation: u16,
    /// Sender hardware address
    pub sender_mac: [u8; 6],
    /// Sender protocol address
    pub sender_ip: [u8; 4],
    /// Target hardware address (zeros in request)
    pub target_mac: [u8; 6],
    /// Target protocol address
    pub target_ip: [u8; 4],
}

impl ArpPacket {
    /// Create a new ARP packet
    pub fn new(operation: u16) -> Self {
        Self {
            htype: HTYPE_ETHERNET,
            ptype: PTYPE_IPV4,
            hlen: HLEN_ETHERNET,
            plen: PLEN_IPV4,
            operation,
            sender_mac: [0; 6],
            sender_ip: [0; 4],
            target_mac: [0; 6],
            target_ip: [0; 4],
        }
    }

    /// Create an ARP request
    pub fn request(sender_ip: [u8; 4], target_ip: [u8; 4]) -> Self {
        let mut packet = Self::new(operation::REQUEST);
        packet.sender_ip = sender_ip;
        packet.target_ip = target_ip;
        packet
    }

    /// Create an ARP reply
    pub fn reply(sender_mac: [u8; 6], sender_ip: [u8; 4], target_mac: [u8; 6]) -> Self {
        let mut packet = Self::new(operation::REPLY);
        packet.sender_mac = sender_mac;
        packet.sender_ip = sender_ip;
        packet.target_mac = target_mac;
        packet.target_ip = sender_ip; // Target IP = sender IP in reply
        packet
    }

    /// Serialize ARP packet to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(28);
        bytes.extend_from_slice(&self.htype.to_be_bytes());
        bytes.extend_from_slice(&self.ptype.to_be_bytes());
        bytes.push(self.hlen);
        bytes.push(self.plen);
        bytes.extend_from_slice(&self.operation.to_be_bytes());
        bytes.extend_from_slice(&self.sender_mac);
        bytes.extend_from_slice(&self.sender_ip);
        bytes.extend_from_slice(&self.target_mac);
        bytes.extend_from_slice(&self.target_ip);
        bytes
    }

    /// Parse ARP packet from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 28 {
            return None;
        }

        Some(Self {
            htype: u16::from_be_bytes([bytes[0], bytes[1]]),
            ptype: u16::from_be_bytes([bytes[2], bytes[3]]),
            hlen: bytes[4],
            plen: bytes[5],
            operation: u16::from_be_bytes([bytes[6], bytes[7]]),
            sender_mac: [
                bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13],
            ],
            sender_ip: [bytes[14], bytes[15], bytes[16], bytes[17]],
            target_mac: [
                bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
            ],
            target_ip: [bytes[24], bytes[25], bytes[26], bytes[27]],
        })
    }

    /// Check if this is an ARP request
    pub fn is_request(&self) -> bool {
        self.operation == operation::REQUEST
    }

    /// Check if this is an ARP reply
    pub fn is_reply(&self) -> bool {
        self.operation == operation::REPLY
    }
}

/// ARP cache entry
#[derive(Debug, Clone)]
pub struct ArpCacheEntry {
    /// IP address
    pub ip_address: Ipv4Address,
    /// MAC address
    pub mac_address: [u8; 6],
    /// Timestamp when entry was created
    pub timestamp: u64,
    /// Entry state
    pub state: ArpEntryState,
}

/// ARP entry state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpEntryState {
    /// Entry is valid
    Valid,
    /// Entry is pending (waiting for ARP reply)
    Pending,
    /// Entry has expired
    Expired,
}

impl ArpCacheEntry {
    /// Create a new ARP cache entry
    pub fn new(ip_address: Ipv4Address, mac_address: [u8; 6]) -> Self {
        Self {
            ip_address,
            mac_address,
            timestamp: 0,
            state: ArpEntryState::Valid,
        }
    }

    /// Create a pending ARP cache entry
    pub fn pending(ip_address: Ipv4Address) -> Self {
        Self {
            ip_address,
            mac_address: [0; 6],
            timestamp: 0,
            state: ArpEntryState::Pending,
        }
    }
}

/// ARP cache entry with packet queue
#[derive(Debug)]
struct ArpPendingEntry {
    /// Cache entry
    entry: ArpCacheEntry,
    /// Packets waiting for this ARP resolution
    packet_queue: Mutex<Vec<Vec<u8>>>,
}

/// ARP layer
///
/// Manages ARP cache and handles ARP requests/replies.
/// Implements NetworkLayer trait for integration with protocol stack.
pub struct ArpLayer {
    /// ARP cache (IP -> MAC)
    cache: RwLock<BTreeMap<u32, ArpCacheEntry>>,
    /// Pending ARP resolutions (waiting for replies)
    pending: RwLock<BTreeMap<u32, ArpPendingEntry>>,
    /// Local MAC address (for replies)
    local_mac: RwLock<[u8; 6]>,
    /// Local IP address
    local_ip: RwLock<Ipv4Address>,
    /// Packet timeout (in ticks)
    timeout_ticks: u64,
    /// Cache timeout (in ticks)
    cache_timeout: u64,
    /// Statistics
    stats: RwLock<NetworkLayerStats>,
    /// Counter for packet queueing
    packet_counter: AtomicU32,
}

impl ArpLayer {
    /// Create a new ARP layer
    pub fn new(local_mac: [u8; 6], local_ip: Ipv4Address) -> Arc<Self> {
        Arc::new(Self {
            cache: RwLock::new(BTreeMap::new()),
            pending: RwLock::new(BTreeMap::new()),
            local_mac: RwLock::new(local_mac),
            local_ip: RwLock::new(local_ip),
            timeout_ticks: 1000,  // 1 second
            cache_timeout: 60000, // 1 minute
            stats: RwLock::new(NetworkLayerStats::default()),
            packet_counter: AtomicU32::new(0),
        })
    }

    /// Look up MAC address for an IP address
    pub fn lookup(&self, ip_address: Ipv4Address) -> Option<[u8; 6]> {
        let cache = self.cache.read();
        let ip_key = u32::from_be_bytes(ip_address.0);

        cache.get(&ip_key).and_then(|entry| {
            if entry.state == ArpEntryState::Valid {
                Some(entry.mac_address)
            } else {
                None
            }
        })
    }

    /// Add entry to ARP cache
    pub fn add_entry(&self, ip_address: Ipv4Address, mac_address: [u8; 6]) {
        let mut cache = self.cache.write();
        let ip_key = u32::from_be_bytes(ip_address.0);
        cache.insert(
            ip_key,
            ArpCacheEntry {
                ip_address,
                mac_address,
                timestamp: self.get_timestamp(),
                state: ArpEntryState::Valid,
            },
        );
    }

    /// Remove entry from ARP cache
    pub fn remove_entry(&self, ip_address: Ipv4Address) {
        let mut cache = self.cache.write();
        let ip_key = u32::from_be_bytes(ip_address.0);
        cache.remove(&ip_key);
    }

    /// Send ARP request
    pub fn send_request(
        &self,
        target_ip: Ipv4Address,
        _context: &LayerContext,
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        let local_ip = *self.local_ip.read();
        let local_mac = *self.local_mac.read();

        // Create ARP request packet
        let arp_packet = ArpPacket::request(local_ip.0, target_ip.0);

        let target_ip_bytes = target_ip.0;
        early_println!(
            "[ARP] Sending request for {}.{}.{}.{}",
            target_ip_bytes[0],
            target_ip_bytes[1],
            target_ip_bytes[2],
            target_ip_bytes[3]
        );

        // Add to pending list
        let ip_key = u32::from_be_bytes(target_ip.0);
        let mut pending = self.pending.write();
        pending.insert(
            ip_key,
            ArpPendingEntry {
                entry: ArpCacheEntry::pending(target_ip),
                packet_queue: Mutex::new(Vec::new()),
            },
        );
        drop(pending);

        // Build Ethernet frame for ARP broadcast
        let mut eth_context = LayerContext::new();
        eth_context.set("eth_dst_mac", &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        eth_context.set("eth_src_mac", &local_mac);
        eth_context.set("ip_protocol", &arp_packet.htype.to_be_bytes()); // EtherType = ARP

        // ARP packet bytes
        let arp_bytes = arp_packet.to_bytes();

        // Send through Ethernet layer
        if !next_layers.is_empty() {
            next_layers[0].send(&arp_bytes, &eth_context, &next_layers[1..])?;

            // Update statistics
            let mut stats = self.stats.write();
            stats.packets_sent += 1;
            stats.bytes_sent += (28 + 14) as u64; // ARP (28) + Ethernet header (14)
        }

        Ok(())
    }

    /// Process received ARP packet
    pub fn receive_packet(&self, arp_bytes: &[u8]) -> Result<(), SocketError> {
        // Parse ARP packet
        let arp_packet = ArpPacket::from_bytes(arp_bytes).ok_or(SocketError::InvalidPacket)?;

        let local_ip = *self.local_ip.read();
        let local_mac = *self.local_mac.read();

        // Get sender and target IP addresses
        let sender_ip = Ipv4Address::from_bytes(arp_packet.sender_ip);
        let target_ip = Ipv4Address::from_bytes(arp_packet.target_ip);

        // Process ARP request
        if arp_packet.is_request() {
            if target_ip == local_ip {
                // Request is for us - send reply
                let reply = ArpPacket::reply(local_mac, local_ip.0, arp_packet.sender_mac);

                let sender_ip_bytes = sender_ip.0;
                early_println!(
                    "[ARP] Received request for {}.{}.{}.{}",
                    sender_ip_bytes[0],
                    sender_ip_bytes[1],
                    sender_ip_bytes[2],
                    sender_ip_bytes[3]
                );

                // Build Ethernet frame for unicast reply
                let mut eth_context = LayerContext::new();
                eth_context.set("eth_dst_mac", &arp_packet.sender_mac);
                eth_context.set("eth_src_mac", &local_mac);
                eth_context.set("ip_protocol", &arp_packet.htype.to_be_bytes());

                // Get Ethernet layer from NetworkManager
                if let Some(eth_layer) = get_network_manager().get_layer("ethernet") {
                    let reply_bytes = reply.to_bytes();
                    eth_layer.send(&reply_bytes, &eth_context, &[])?;
                }
            }
        }
        // Process ARP reply
        else if arp_packet.is_reply() {
            if sender_ip != local_ip {
                // Check if we have a pending request for this IP
                let ip_key = u32::from_be_bytes(sender_ip.0);
                let mut pending = self.pending.write();

                if let Some(mut pending_entry) = pending.remove(&ip_key) {
                    // Update cache with resolved MAC
                    self.add_entry(sender_ip, arp_packet.sender_mac);

                    let sender_ip_bytes = sender_ip.0;
                    early_println!(
                        "[ARP] Received reply for {}.{}.{}.{} -> {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                        sender_ip_bytes[0],
                        sender_ip_bytes[1],
                        sender_ip_bytes[2],
                        sender_ip_bytes[3],
                        arp_packet.sender_mac[0],
                        arp_packet.sender_mac[1],
                        arp_packet.sender_mac[2],
                        arp_packet.sender_mac[3],
                        arp_packet.sender_mac[4],
                        arp_packet.sender_mac[5]
                    );

                    // Send queued packets
                    let mut queue = pending_entry.packet_queue.lock();
                    if let Some(eth_layer) = get_network_manager().get_layer("ethernet") {
                        for packet_bytes in queue.drain(..) {
                            // Update context with resolved MAC
                            let mut eth_context = LayerContext::new();
                            eth_context.set("eth_dst_mac", &arp_packet.sender_mac);
                            eth_context.set("eth_src_mac", &local_mac);

                            // Re-send the queued packet
                            if let Err(e) = eth_layer.send(&packet_bytes, &eth_context, &[]) {
                            }
                        }
                    }

                    // Update entry state to valid
                    pending_entry.entry.state = ArpEntryState::Valid;
                    pending_entry.entry.mac_address = arp_packet.sender_mac;
                } else {
                    // Not in pending list, but cache the reply anyway
                    let sender_ip_bytes = sender_ip.0;
                    early_println!(
                        "[ARP] Received unsolicited reply for {}.{}.{}.{}",
                        sender_ip_bytes[0],
                        sender_ip_bytes[1],
                        sender_ip_bytes[2],
                        sender_ip_bytes[3]
                    );
                    self.add_entry(sender_ip, arp_packet.sender_mac);
                }

                drop(pending);

                let mut stats = self.stats.write();
                stats.packets_received += 1;
                stats.bytes_received += (arp_bytes.len() + 14) as u64;
            }
        }

        Ok(())
    }

    /// Queue a packet waiting for ARP resolution
    pub fn queue_packet(&self, ip_address: Ipv4Address, packet: Vec<u8>) {
        let ip_key = u32::from_be_bytes(ip_address.0);
        let mut pending = self.pending.write();

        if let Some(entry) = pending.get_mut(&ip_key) {
            entry.packet_queue.lock().push(packet);
        }

        drop(pending);
    }

    /// Get current timestamp (placeholder - should use actual system time)
    fn get_timestamp(&self) -> u64 {
        self.packet_counter.fetch_add(1, Ordering::SeqCst) as u64
    }

    /// Get local MAC address
    pub fn get_local_mac(&self) -> [u8; 6] {
        *self.local_mac.read()
    }

    /// Check if IP address is resolved
    pub fn is_resolved(&self, ip_address: Ipv4Address) -> bool {
        self.lookup(ip_address).is_some()
    }
}

impl NetworkLayer for ArpLayer {
    fn register_protocol(&self, _proto_num: u16, _handler: Arc<dyn NetworkLayer>) {
        // ARP doesn't register upper protocols
    }

    fn send(
        &self,
        _packet: &[u8],
        _context: &LayerContext,
        _next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // ARP send is handled through specific methods
        Ok(())
    }

    fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
        let mut stats = self.stats.write();
        stats.packets_received += 1;
        stats.bytes_received += packet.len() as u64;

        self.receive_packet(packet)
    }

    fn name(&self) -> &'static str {
        "ARP"
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
    fn test_arp_packet_request() {
        let sender_ip = [192, 168, 1, 100];
        let target_ip = [192, 168, 1, 1];

        let packet = ArpPacket::request(sender_ip, target_ip);

        assert!(packet.is_request());
        assert!(!packet.is_reply());
        assert_eq!(packet.sender_ip, sender_ip);
        assert_eq!(packet.target_ip, target_ip);
        let operation_value =
            unsafe { core::ptr::addr_of!(packet.operation).read_unaligned() };
        assert_eq!(operation_value, operation::REQUEST);
    }

    #[test_case]
    fn test_arp_packet_reply() {
        let sender_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let sender_ip = [192, 168, 1, 1];

        let packet = ArpPacket::reply(sender_mac, sender_ip, sender_mac);

        assert!(!packet.is_request());
        assert!(packet.is_reply());
        assert_eq!(packet.sender_mac, sender_mac);
        assert_eq!(packet.target_mac, sender_mac);
        let operation_value =
            unsafe { core::ptr::addr_of!(packet.operation).read_unaligned() };
        assert_eq!(operation_value, operation::REPLY);
    }

    #[test_case]
    fn test_arp_packet_serialization() {
        let sender_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let sender_ip = [192, 168, 1, 1];
        let target_ip = [192, 168, 1, 2];

        let packet = ArpPacket::reply(sender_mac, sender_ip, sender_mac);
        let bytes = packet.to_bytes();

        assert_eq!(bytes.len(), 28);
        assert_eq!(&bytes[0..2], HTYPE_ETHERNET.to_be_bytes()); // htype
        assert_eq!(&bytes[2..4], PTYPE_IPV4.to_be_bytes()); // ptype
        assert_eq!(bytes[4], HLEN_ETHERNET); // hlen
        assert_eq!(bytes[5], PLEN_IPV4); // plen
        assert_eq!(&bytes[6..8], operation::REPLY.to_be_bytes()); // operation
        assert_eq!(&bytes[8..14], &sender_mac); // sender_mac
        assert_eq!(&bytes[14..18], &sender_ip); // sender_ip
        assert_eq!(&bytes[18..24], &sender_mac); // target_mac
        assert_eq!(&bytes[24..28], &sender_ip); // target_ip
    }

    #[test_case]
    fn test_arp_packet_parsing() {
        let sender_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let sender_ip = [192, 168, 1, 1];

        let original = ArpPacket::reply(sender_mac, sender_ip, sender_mac);
        let bytes = original.to_bytes();

        let parsed = ArpPacket::from_bytes(&bytes).unwrap();

        let parsed_htype = unsafe { core::ptr::addr_of!(parsed.htype).read_unaligned() };
        let original_htype = unsafe { core::ptr::addr_of!(original.htype).read_unaligned() };
        assert_eq!(parsed_htype, original_htype);
        let parsed_ptype = unsafe { core::ptr::addr_of!(parsed.ptype).read_unaligned() };
        let original_ptype = unsafe { core::ptr::addr_of!(original.ptype).read_unaligned() };
        assert_eq!(parsed_ptype, original_ptype);
        assert_eq!(parsed.hlen, original.hlen);
        assert_eq!(parsed.plen, original.plen);
        let parsed_operation =
            unsafe { core::ptr::addr_of!(parsed.operation).read_unaligned() };
        let original_operation =
            unsafe { core::ptr::addr_of!(original.operation).read_unaligned() };
        assert_eq!(parsed_operation, original_operation);
        assert_eq!(parsed.sender_mac, original.sender_mac);
        assert_eq!(parsed.sender_ip, original.sender_ip);
        assert_eq!(parsed.target_mac, original.target_mac);
        assert_eq!(parsed.target_ip, original.target_ip);
    }

    #[test_case]
    fn test_arp_cache_entry() {
        let ip = Ipv4Address::new(192, 168, 1, 1);
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];

        let entry = ArpCacheEntry::new(ip, mac);

        assert_eq!(entry.ip_address, ip);
        assert_eq!(entry.mac_address, mac);
        assert_eq!(entry.state, ArpEntryState::Valid);
    }

    #[test_case]
    fn test_arp_constants() {
        assert_eq!(HTYPE_ETHERNET, 0x0001);
        assert_eq!(PTYPE_IPV4, 0x0800);
        assert_eq!(HLEN_ETHERNET, 6);
        assert_eq!(PLEN_IPV4, 4);
    }
}
