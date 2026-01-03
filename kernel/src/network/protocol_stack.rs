//! Protocol stack abstraction for network protocols
//!
//! This module provides the infrastructure for protocol stacks (TCP/IP, UDP, etc.)
//! to be integrated with Scarlet's socket system.
//!
//! # Design
//!
//! Protocol stacks bridge between high-level SocketObject and low-level network devices.
//! They handle protocol-specific operations like:
//! - Packet encapsulation/decapsulation
//! - Connection state management  
//! - Error handling and retransmission
//! - Flow control and congestion control
//!
//! # Architecture
//!
//! ```text
//! Application
//!     ↓
//! SocketObject (ABI-specific, e.g., Linux TCP socket)
//!     ↓
//! ProtocolStack (TCP/IP, UDP, etc.)
//!     ↓
//! NetworkDevice (Ethernet, WiFi, etc.)
//! ```
//!
//! # Layered Protocol Architecture
//!
//! The new NetworkLayer trait provides a flexible, composable protocol stack:
//!
//! ```text
//! Socket Layer
//!     ↓
//! Transport Layer (TCP=6, UDP=17)
//!     ↓ (register_protocol)
//! Network Layer (IP)
//!     ↓ (register_protocol)
//! Link Layer (Ethernet, InfiniBand)
//!     ↓
//! Physical Device
//! ```
//!
//! Each layer can:
//! - Register protocol handlers for upper layers
//! - Send packets to multiple lower layers
//! - Route based on protocol numbers

use alloc::{collections::BTreeMap, string::String, sync::Arc, vec::Vec};
use spin::RwLock;

use super::socket::{SocketDomain, SocketError, SocketObject, SocketProtocol, SocketType};
use crate::device::network::DevicePacket;

/// Protocol stack statistics
#[derive(Debug, Clone, Default)]
pub struct ProtocolStackStats {
    /// Number of packets sent
    pub packets_sent: u64,
    /// Number of bytes sent
    pub bytes_sent: u64,
    /// Number of packets received
    pub packets_received: u64,
    /// Number of bytes received
    pub bytes_received: u64,
    /// Number of packets dropped
    pub packets_dropped: u64,
    /// Number of protocol errors
    pub protocol_errors: u64,
    /// Number of active connections
    pub active_connections: u64,
}

/// Context passed between network layers for routing decisions
///
/// This structure carries routing information through the protocol stack,
/// allowing each layer to add or consume information needed for proper
/// packet delivery. This is designed to be **protocol-agnostic** and avoids
/// hard-coding specific protocol fields like IP addresses.
///
/// # Design Philosophy
///
/// - **Protocol-agnostic**: No IP addresses or protocol-specific fields in core structure
/// - **Flexible key-value store**: Each protocol layer can add/read arbitrary data
/// - **Layer composition**: Enables proper separation without tight coupling
/// - **Follows @petitstrawberry's guidance**: Generic, not tied to specific protocols
///
/// # Example Flow
///
/// ```rust,ignore
/// // TCP layer creates context with its info
/// let mut ctx = LayerContext::new();
/// ctx.set("tcp_src_port", &5000u16.to_be_bytes());
/// ctx.set("tcp_dst_port", &80u16.to_be_bytes());
///
/// // TCP layer sends to IP, IP adds its info
/// let ip_src = [192, 168, 1, 100];
/// let ip_dst = [192, 168, 1, 1];
/// ctx.set("ip_src", &ip_src);
/// ctx.set("ip_dst", &ip_dst);
/// ctx.set("ip_protocol", &[6]); // TCP
///
/// // IP layer sends to Ethernet, Ethernet performs ARP
/// // Ethernet can read "ip_dst" to determine MAC address
/// ```
#[derive(Debug, Clone, Default)]
pub struct LayerContext {
    /// Protocol-agnostic key-value store for routing information
    /// Each layer can add/read arbitrary data needed for packet delivery
    /// 
    /// Common keys (convention, not enforced):
    /// - "ip_src", "ip_dst": IPv4/IPv6 addresses
    /// - "tcp_src_port", "tcp_dst_port": TCP ports  
    /// - "udp_src_port", "udp_dst_port": UDP ports
    /// - "ip_protocol": Protocol number (6=TCP, 17=UDP)
    /// - "ttl": Time-to-live
    /// - "tos": Type of service
    pub info: BTreeMap<String, Vec<u8>>,
}

impl LayerContext {
    /// Create a new empty LayerContext
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Set a value in the context
    pub fn set(&mut self, key: &str, value: &[u8]) {
        self.info.insert(String::from(key), value.to_vec());
    }
    
    /// Get a value from the context
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.info.get(key).map(|v| v.as_slice())
    }
    
    /// Check if a key exists
    pub fn contains(&self, key: &str) -> bool {
        self.info.contains_key(key)
    }
}

/// Configuration for socket creation and layer binding
///
/// This structure carries configuration from socket creation down through
/// the protocol layers, allowing each layer to extract the information it
/// needs to properly configure the socket.
///
/// # Design Philosophy
///
/// - **Solves @petitstrawberry's question**: How does IP layer get IP address? 
///   How does TCP layer get port number? Answer: Through SocketConfig at socket creation.
/// - **Protocol-agnostic**: Generic key-value store, not tied to specific protocols
/// - **Per-socket configuration**: Each socket gets configured independently
///
/// # Example: Socket Creation with Configuration
///
/// ```rust,ignore
/// // User creates TCP socket and binds to address
/// let mut config = SocketConfig::new();
/// config.set("ip_local", &[192, 168, 1, 100]);
/// config.set("tcp_local_port", &5000u16.to_be_bytes());
///
/// // Socket factory creates socket with config
/// let socket = tcp_socket_factory(&config)?;
///
/// // Inside TcpSocket::new():
/// // - TCP layer extracts "tcp_local_port" for its state
/// // - IP layer extracts "ip_local" for source address
/// // - Ethernet layer might extract interface name
///
/// // Later, when connect() is called:
/// config.set("ip_remote", &[192, 168, 1, 1]);
/// config.set("tcp_remote_port", &80u16.to_be_bytes());
/// socket.connect(&config)?;
/// ```
#[derive(Debug, Clone, Default)]
pub struct SocketConfig {
    /// Protocol-agnostic configuration parameters
    /// 
    /// Common keys (convention, not enforced):
    /// - "ip_local": Local IP address (IPv4 or IPv6 bytes)
    /// - "ip_remote": Remote IP address
    /// - "tcp_local_port": TCP local port (u16 big-endian)
    /// - "tcp_remote_port": TCP remote port
    /// - "udp_local_port": UDP local port
    /// - "udp_remote_port": UDP remote port
    /// - "interface": Network interface name
    pub params: BTreeMap<String, Vec<u8>>,
}

impl SocketConfig {
    /// Create a new empty configuration
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Set a configuration parameter
    pub fn set(&mut self, key: &str, value: &[u8]) {
        self.params.insert(String::from(key), value.to_vec());
    }
    
    /// Get a configuration parameter
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.params.get(key).map(|v| v.as_slice())
    }
    
    /// Get a u16 value (for ports)
    pub fn get_u16(&self, key: &str) -> Option<u16> {
        self.get(key).and_then(|v| {
            if v.len() >= 2 {
                Some(u16::from_be_bytes([v[0], v[1]]))
            } else {
                None
            }
        })
    }
    
    /// Get an IPv4 address
    pub fn get_ipv4(&self, key: &str) -> Option<[u8; 4]> {
        self.get(key).and_then(|v| {
            if v.len() >= 4 {
                Some([v[0], v[1], v[2], v[3]])
            } else {
                None
            }
        })
    }
}

/// Network layer trait for composable protocol stacks
///
/// This trait enables building flexible protocol stacks where each layer
/// is independent and can be composed at runtime. Layers communicate through
/// protocol numbers (e.g., IP uses protocol 6 for TCP, 17 for UDP).
///
/// # Design Philosophy (VFS Pattern)
///
/// Following VFS architecture where filesystems are shared singletons:
/// - **NetworkLayer = FileSystemOperations**: Shared protocol implementation
/// - **SocketObject = FileObject**: Per-connection handle with references to layers
/// - **NetworkManager = VfsManager**: Global registry of protocol layer instances
///
/// Each NetworkLayer instance is shared across all sockets, similar to how
/// a filesystem (ext2, tmpfs) is shared across all file handles. Per-socket
/// state lives in SocketObject, not in NetworkLayer.
///
/// # Shared vs Per-Socket State
///
/// **NetworkLayer (shared, stateless for protocol logic):**
/// - Protocol logic and packet processing
/// - Routing tables, ARP cache (shared state)
/// - Registered protocol handlers
/// - Like: ext2 driver, tmpfs implementation
///
/// **SocketObject (per-socket, stateful):**
/// - Connection state (ports, addresses) - configured via SocketConfig
/// - Send/receive buffers
/// - References to NetworkLayer instances
/// - Like: FileObject with seek position, flags
///
/// # Socket Configuration Flow
///
/// 1. User creates socket: `socket(AF_INET, SOCK_STREAM, IPPROTO_TCP)`
/// 2. User binds: `bind(sockfd, {192.168.1.100:5000})`
/// 3. ABI creates SocketConfig with "ip_local" and "tcp_local_port"
/// 4. Socket factory creates SocketObject, passing config to each layer
/// 5. TCP layer extracts port, IP layer extracts address
/// 6. Socket handle stores references to shared layers + per-socket state
///
/// # Example: IP Layer (Shared Singleton)
///
/// ```rust,ignore
/// struct IpLayer {
///     protocols: RwLock<BTreeMap<u16, Arc<dyn NetworkLayer>>>, // Shared
///     routing_table: RwLock<RoutingTable>,  // Shared
///     arp_cache: RwLock<ArpCache>,          // Shared
/// }
///
/// impl NetworkLayer for IpLayer {
///     fn send(&self, packet: &[u8], context: &LayerContext, 
///             next_layers: &[Arc<dyn NetworkLayer>]) -> Result<(), SocketError> {
///         // Extract destination IP from protocol-agnostic context
///         let dest_ip = context.get("ip_dst")
///             .ok_or(SocketError::InvalidPacket)?;
///
///         // Add IP header with destination
///         let ip_packet = add_ip_header(packet, dest_ip);
///         
///         // Route to lower layer (Ethernet, InfiniBand, etc.)
///         for layer in next_layers {
///             if let Ok(()) = layer.send(&ip_packet, context, &[]) {
///                 return Ok(());
///             }
///         }
///         Err(SocketError::NoRoute)
///     }
///
///     fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
///         // Parse IP header
///         let (proto_num, payload) = parse_ip_header(packet)?;
///         
///         // Route to registered protocol handler
///         if let Some(handler) = self.protocols.read().get(&proto_num) {
///             handler.receive(payload)
///         } else {
///             Err(SocketError::ProtocolNotSupported)
///         }
///     }
/// }
/// ```
///
/// # Per-Task NetworkManager (Future)
///
/// Like VfsManager can be per-task for filesystem namespace isolation,
/// NetworkManager can be per-task for network namespace isolation:
///
/// ```rust,ignore
/// // Container gets isolated NetworkManager
/// let container_net = Arc::new(NetworkManager::new());
///
/// // Share Ethernet layer (driver access), but separate IP/TCP
/// let shared_eth = global_net_manager.get_layer("ethernet")?;
/// container_net.register_layer("ethernet", shared_eth);
///
/// // Container gets its own IP and TCP layer instances
/// container_net.register_layer("ip", Arc::new(IpLayer::new()));
/// container_net.register_layer("tcp", Arc::new(TcpLayer::new()));
///
/// // Assign to task
/// task.network_manager = Some(container_net);
/// ```
pub trait NetworkLayer: Send + Sync {
    /// Register a protocol handler for this layer
    ///
    /// Upper layer protocols register themselves with their protocol number.
    /// For example, TCP registers as protocol 6 with the IP layer.
    ///
    /// # Arguments
    ///
    /// * `proto_num` - Protocol number (e.g., 6 for TCP, 17 for UDP)
    /// * `handler` - Protocol handler for this protocol number
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // TCP layer registers with IP layer
    /// ip_layer.register_protocol(6, Arc::new(tcp_layer));
    /// ```
    fn register_protocol(&self, proto_num: u16, handler: Arc<dyn NetworkLayer>);

    /// Send a packet through this layer
    ///
    /// The layer encapsulates the packet with its own header and passes it
    /// to one or more lower layers. The context contains routing information
    /// in a protocol-agnostic key-value format.
    ///
    /// # Arguments
    ///
    /// * `packet` - Packet data to send
    /// * `context` - Protocol-agnostic routing context (key-value pairs)
    /// * `next_layers` - Lower layer options for transmission
    ///
    /// # Returns
    ///
    /// Ok(()) if successfully sent through at least one lower layer,
    /// Err if all lower layers failed or routing information is insufficient
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // TCP layer adds its info to context
    /// let mut ctx = LayerContext::new();
    /// ctx.set("tcp_src_port", &5000u16.to_be_bytes());
    /// ctx.set("tcp_dst_port", &80u16.to_be_bytes());
    ///
    /// tcp_layer.send(&tcp_segment, &ctx, &[ip_layer])?;
    ///
    /// // IP layer adds its info and forwards
    /// ctx.set("ip_src", &[192, 168, 1, 100]);
    /// ctx.set("ip_dst", &[192, 168, 1, 1]);
    /// ip_layer.send(&ip_packet, &ctx, &[ethernet_layer])?;
    /// ```
    fn send(&self, packet: &[u8], context: &LayerContext, next_layers: &[Arc<dyn NetworkLayer>])
        -> Result<(), SocketError>;

    /// Receive and process a packet at this layer
    ///
    /// The layer parses its header, extracts the protocol number, and routes
    /// the payload to the appropriate upper layer protocol handler.
    ///
    /// # Arguments
    ///
    /// * `packet` - Packet data received from lower layer
    ///
    /// # Returns
    ///
    /// Ok(()) if successfully processed and delivered,
    /// Err if packet is malformed or no handler for the protocol
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // IP layer receives packet, parses header, routes to TCP (proto=6)
    /// ip_layer.receive(&packet)?;
    /// ```
    fn receive(&self, packet: &[u8]) -> Result<(), SocketError>;

    /// Get layer name for debugging
    fn name(&self) -> &'static str;

    /// Get layer statistics
    fn stats(&self) -> NetworkLayerStats {
        NetworkLayerStats::default()
    }
}

/// Statistics for a network layer
#[derive(Debug, Clone, Default)]
pub struct NetworkLayerStats {
    /// Packets sent through this layer
    pub packets_sent: u64,
    /// Packets received by this layer
    pub packets_received: u64,
    /// Packets dropped due to errors
    pub packets_dropped: u64,
    /// Protocol errors encountered
    pub protocol_errors: u64,
}

/// Protocol stack trait for network protocols
///
/// This trait defines the interface for protocol stack implementations.
/// ABI modules can implement this to provide TCP/IP, UDP, or other protocol support.
///
/// # Example: TCP/IP Stack
///
/// ```rust,ignore
/// struct TcpIpStack {
///     // TCP/IP implementation details
/// }
///
/// impl ProtocolStack for TcpIpStack {
///     fn domain(&self) -> SocketDomain {
///         SocketDomain::Inet
///     }
///
///     fn create_socket(&self, socket_type: SocketType, protocol: SocketProtocol) 
///         -> Result<Arc<dyn SocketObject>, SocketError> {
///         match (socket_type, protocol) {
///             (SocketType::Stream, SocketProtocol::Tcp) => {
///                 Ok(Arc::new(TcpSocket::new(self.clone())))
///             }
///             (SocketType::Datagram, SocketProtocol::Udp) => {
///                 Ok(Arc::new(UdpSocket::new(self.clone())))
///             }
///             _ => Err(SocketError::NotSupported),
///         }
///     }
///
///     fn process_incoming_packet(&self, packet: &DevicePacket) -> Result<(), SocketError> {
///         // Parse IP header, route to appropriate socket
///         // ...
///         Ok(())
///     }
/// }
/// ```
pub trait ProtocolStack: Send + Sync {
    /// Get the protocol stack domain
    ///
    /// Returns which address family this stack handles (Inet, Inet6, etc.)
    fn domain(&self) -> SocketDomain;

    /// Create a socket for this protocol stack
    ///
    /// # Arguments
    ///
    /// * `socket_type` - Type of socket (Stream, Datagram, etc.)
    /// * `protocol` - Specific protocol (Tcp, Udp, etc.)
    ///
    /// # Returns
    ///
    /// A new socket object that uses this protocol stack
    fn create_socket(
        &self,
        socket_type: SocketType,
        protocol: SocketProtocol,
    ) -> Result<Arc<dyn SocketObject>, SocketError>;

    /// Process an incoming packet from the network device
    ///
    /// The protocol stack should parse the packet and deliver it to the
    /// appropriate socket.
    ///
    /// # Arguments
    ///
    /// * `packet` - Raw packet data from network device
    ///
    /// # Errors
    ///
    /// Returns an error if the packet is malformed or cannot be processed
    fn process_incoming_packet(&self, packet: &DevicePacket) -> Result<(), SocketError>;

    /// Send a packet through the network device
    ///
    /// The protocol stack should encapsulate the data with appropriate headers
    /// and send it through the network device.
    ///
    /// # Arguments
    ///
    /// * `packet` - Packet to send
    ///
    /// # Errors
    ///
    /// Returns an error if the packet cannot be sent
    fn send_packet(&self, packet: DevicePacket) -> Result<(), SocketError>;

    /// Get protocol stack statistics
    fn statistics(&self) -> ProtocolStackStats;

    /// Get a human-readable name for this protocol stack
    fn name(&self) -> &'static str;

    /// Check if the protocol stack supports a specific socket type and protocol
    fn supports(&self, socket_type: SocketType, protocol: SocketProtocol) -> bool;
}

/// Protocol stack manager
///
/// Manages registered protocol stacks and routes packets to appropriate stacks.
pub struct ProtocolStackManager {
    /// Registered protocol stacks by domain
    stacks: spin::RwLock<alloc::collections::BTreeMap<SocketDomain, Arc<dyn ProtocolStack>>>,
}

impl ProtocolStackManager {
    /// Create a new protocol stack manager
    pub const fn new() -> Self {
        Self {
            stacks: spin::RwLock::new(alloc::collections::BTreeMap::new()),
        }
    }

    /// Register a protocol stack
    ///
    /// # Arguments
    ///
    /// * `stack` - Protocol stack implementation to register
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let tcp_ip_stack = Arc::new(TcpIpStack::new());
    /// protocol_stack_manager.register_stack(tcp_ip_stack);
    /// ```
    pub fn register_stack(&self, stack: Arc<dyn ProtocolStack>) {
        let domain = stack.domain();
        self.stacks.write().insert(domain, stack);
    }

    /// Get a protocol stack for a specific domain
    ///
    /// # Arguments
    ///
    /// * `domain` - Socket domain (Inet, Inet6, etc.)
    ///
    /// # Returns
    ///
    /// The protocol stack for this domain, or None if not registered
    pub fn get_stack(&self, domain: SocketDomain) -> Option<Arc<dyn ProtocolStack>> {
        self.stacks.read().get(&domain).cloned()
    }

    /// Process an incoming packet
    ///
    /// Routes the packet to the appropriate protocol stack based on packet type.
    ///
    /// # Arguments
    ///
    /// * `packet` - Raw packet from network device
    ///
    /// # Errors
    ///
    /// Returns an error if no protocol stack can handle the packet
    pub fn process_packet(&self, packet: &DevicePacket) -> Result<(), SocketError> {
        // In a real implementation, we would parse the packet header to determine
        // which protocol stack should handle it (e.g., check IP version, protocol field)
        
        // For now, try each registered stack
        let stacks = self.stacks.read();
        for stack in stacks.values() {
            if let Ok(()) = stack.process_incoming_packet(packet) {
                return Ok(());
            }
        }
        
        Err(SocketError::Other("No protocol stack could handle packet".into()))
    }

    /// Get statistics for all protocol stacks
    pub fn get_all_statistics(&self) -> Vec<(String, ProtocolStackStats)> {
        let stacks = self.stacks.read();
        stacks
            .values()
            .map(|stack| (stack.name().into(), stack.statistics()))
            .collect()
    }
}

impl Default for ProtocolStackManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicU64, Ordering};

    #[test_case]
    fn test_protocol_stack_manager_creation() {
        let manager = ProtocolStackManager::new();
        assert!(manager.get_stack(SocketDomain::Inet).is_none());
    }

    #[test_case]
    fn test_protocol_stack_stats_default() {
        let stats = ProtocolStackStats::default();
        assert_eq!(stats.packets_sent, 0);
        assert_eq!(stats.bytes_sent, 0);
        assert_eq!(stats.packets_received, 0);
    }

    #[test_case]
    fn test_layer_context_generic() {
        // Test that LayerContext is protocol-agnostic
        let mut ctx = LayerContext::new();
        
        // TCP layer can add its info
        ctx.set("tcp_src_port", &5000u16.to_be_bytes());
        ctx.set("tcp_dst_port", &80u16.to_be_bytes());
        
        // IP layer can add its info
        ctx.set("ip_src", &[192, 168, 1, 100]);
        ctx.set("ip_dst", &[192, 168, 1, 1]);
        ctx.set("ip_protocol", &[6]); // TCP
        
        // Ethernet layer can add its info
        ctx.set("eth_src_mac", &[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        ctx.set("eth_dst_mac", &[0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);
        
        // Verify all info is stored
        assert_eq!(ctx.get("tcp_src_port"), Some(&[0x13, 0x88][..])); // 5000 in big-endian
        assert_eq!(ctx.get("tcp_dst_port"), Some(&[0x00, 0x50][..])); // 80 in big-endian
        assert_eq!(ctx.get("ip_src"), Some(&[192, 168, 1, 100][..]));
        assert_eq!(ctx.get("ip_dst"), Some(&[192, 168, 1, 1][..]));
        assert_eq!(ctx.get("ip_protocol"), Some(&[6][..]));
        assert!(ctx.contains("eth_src_mac"));
    }

    #[test_case]
    fn test_socket_config() {
        // Test SocketConfig for socket creation
        let mut config = SocketConfig::new();
        
        // Set local bind address and port
        config.set("ip_local", &[192, 168, 1, 100]);
        config.set("tcp_local_port", &5000u16.to_be_bytes());
        
        // Set remote address (for connect)
        config.set("ip_remote", &[192, 168, 1, 1]);
        config.set("tcp_remote_port", &80u16.to_be_bytes());
        
        // Test helper methods
        assert_eq!(config.get_ipv4("ip_local"), Some([192, 168, 1, 100]));
        assert_eq!(config.get_ipv4("ip_remote"), Some([192, 168, 1, 1]));
        assert_eq!(config.get_u16("tcp_local_port"), Some(5000));
        assert_eq!(config.get_u16("tcp_remote_port"), Some(80));
    }

    // ============================================================================
    // Realistic TCP/IP/Ethernet Mock Protocol Layers
    // ============================================================================
    
    /// Mock Ethernet layer simulating real Ethernet II frames
    ///
    /// Frame format:
    /// - Destination MAC (6 bytes)
    /// - Source MAC (6 bytes)  
    /// - EtherType (2 bytes): 0x0800 for IPv4
    /// - Payload (variable)
    /// - FCS (omitted in this mock)
    struct MockEthernetLayer {
        name: &'static str,
        mac_address: [u8; 6],
        arp_table: RwLock<BTreeMap<[u8; 4], [u8; 6]>>, // IP -> MAC mapping
        packets_sent: AtomicU64,
        packets_received: AtomicU64,
        last_sent_frame: RwLock<Vec<u8>>,
    }

    impl MockEthernetLayer {
        fn new(name: &'static str, mac: [u8; 6]) -> Self {
            let mut layer = Self {
                name,
                mac_address: mac,
                arp_table: RwLock::new(BTreeMap::new()),
                packets_sent: AtomicU64::new(0),
                packets_received: AtomicU64::new(0),
                last_sent_frame: RwLock::new(Vec::new()),
            };
            // Pre-populate some ARP entries for testing
            layer.arp_table.write().insert([192, 168, 1, 1], [0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);
            layer.arp_table.write().insert([192, 168, 1, 100], [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
            layer
        }

        fn get_last_frame(&self) -> Vec<u8> {
            self.last_sent_frame.read().clone()
        }
    }

    impl NetworkLayer for MockEthernetLayer {
        fn register_protocol(&self, _proto_num: u16, _handler: Arc<dyn NetworkLayer>) {
            // Ethernet doesn't register upper protocols in this simple mock
        }

        fn send(
            &self,
            packet: &[u8],
            context: &LayerContext,
            _next_layers: &[Arc<dyn NetworkLayer>],
        ) -> Result<(), SocketError> {
            // Extract destination IP from context for ARP lookup
            let dest_ip = context.get("ip_dst")
                .and_then(|ip| if ip.len() >= 4 { Some([ip[0], ip[1], ip[2], ip[3]]) } else { None })
                .ok_or(SocketError::InvalidPacket)?;

            // Perform ARP lookup
            let dest_mac = self.arp_table.read().get(&dest_ip).copied()
                .ok_or(SocketError::NoRoute)?;

            // Build Ethernet frame
            let mut frame = Vec::with_capacity(14 + packet.len());
            frame.extend_from_slice(&dest_mac);  // Destination MAC
            frame.extend_from_slice(&self.mac_address);  // Source MAC
            frame.extend_from_slice(&[0x08, 0x00]);  // EtherType: IPv4
            frame.extend_from_slice(packet);  // IP packet

            *self.last_sent_frame.write() = frame.clone();
            self.packets_sent.fetch_add(1, Ordering::SeqCst);
            
            Ok(())
        }

        fn receive(&self, frame: &[u8]) -> Result<(), SocketError> {
            self.packets_received.fetch_add(1, Ordering::SeqCst);
            
            // Parse Ethernet header
            if frame.len() < 14 {
                return Err(SocketError::InvalidPacket);
            }
            
            let _dest_mac = &frame[0..6];
            let _src_mac = &frame[6..12];
            let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
            let payload = &frame[14..];
            
            // For IPv4 (0x0800), would pass to IP layer
            // In this mock, just verify the frame is valid
            if ethertype == 0x0800 && !payload.is_empty() {
                Ok(())
            } else {
                Err(SocketError::ProtocolNotSupported)
            }
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    /// Mock IP layer simulating real IPv4 packets
    ///
    /// Simplified IPv4 header:
    /// - Version + IHL (1 byte): 0x45 (IPv4, 20-byte header)
    /// - TOS (1 byte): 0x00
    /// - Total Length (2 bytes)
    /// - Identification (2 bytes)
    /// - Flags + Fragment Offset (2 bytes)
    /// - TTL (1 byte)
    /// - Protocol (1 byte): 6=TCP, 17=UDP
    /// - Header Checksum (2 bytes)
    /// - Source IP (4 bytes)
    /// - Destination IP (4 bytes)
    /// - Payload (variable)
    struct MockIpLayer {
        name: &'static str,
        local_ip: [u8; 4],
        protocols: RwLock<BTreeMap<u16, Arc<dyn NetworkLayer>>>,
        packets_sent: AtomicU64,
        packets_received: AtomicU64,
    }

    impl MockIpLayer {
        fn new(name: &'static str, ip: [u8; 4]) -> Self {
            Self {
                name,
                local_ip: ip,
                protocols: RwLock::new(BTreeMap::new()),
                packets_sent: AtomicU64::new(0),
                packets_received: AtomicU64::new(0),
            }
        }
    }

    impl NetworkLayer for MockIpLayer {
        fn register_protocol(&self, proto_num: u16, handler: Arc<dyn NetworkLayer>) {
            self.protocols.write().insert(proto_num, handler);
        }

        fn send(
            &self,
            packet: &[u8],
            context: &LayerContext,
            next_layers: &[Arc<dyn NetworkLayer>],
        ) -> Result<(), SocketError> {
            // Extract addresses from context
            let src_ip = context.get("ip_src")
                .and_then(|ip| if ip.len() >= 4 { Some([ip[0], ip[1], ip[2], ip[3]]) } else { None })
                .unwrap_or(self.local_ip);
            
            let dest_ip = context.get("ip_dst")
                .and_then(|ip| if ip.len() >= 4 { Some([ip[0], ip[1], ip[2], ip[3]]) } else { None })
                .ok_or(SocketError::InvalidPacket)?;
            
            let protocol = context.get("ip_protocol")
                .and_then(|p| if !p.is_empty() { Some(p[0]) } else { None })
                .unwrap_or(6); // Default to TCP

            // Build simplified IPv4 header (20 bytes)
            let total_len = (20 + packet.len()) as u16;
            let mut ip_packet = Vec::with_capacity(20 + packet.len());
            
            ip_packet.push(0x45);  // Version=4, IHL=5 (20 bytes)
            ip_packet.push(0x00);  // TOS
            ip_packet.extend_from_slice(&total_len.to_be_bytes());  // Total Length
            ip_packet.extend_from_slice(&[0x00, 0x00]);  // Identification
            ip_packet.extend_from_slice(&[0x00, 0x00]);  // Flags + Fragment Offset
            ip_packet.push(64);  // TTL
            ip_packet.push(protocol);  // Protocol
            ip_packet.extend_from_slice(&[0x00, 0x00]);  // Checksum (simplified, not calculated)
            ip_packet.extend_from_slice(&src_ip);  // Source IP
            ip_packet.extend_from_slice(&dest_ip);  // Destination IP
            ip_packet.extend_from_slice(packet);  // Payload

            self.packets_sent.fetch_add(1, Ordering::SeqCst);

            // Forward to Ethernet layer with updated context
            for layer in next_layers {
                if layer.send(&ip_packet, context, &[]).is_ok() {
                    return Ok(());
                }
            }

            Err(SocketError::NoRoute)
        }

        fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
            self.packets_received.fetch_add(1, Ordering::SeqCst);

            // Parse IP header
            if packet.len() < 20 {
                return Err(SocketError::InvalidPacket);
            }

            let protocol = packet[9];
            let payload = &packet[20..];

            // Route to registered protocol handler
            let protocols = self.protocols.read();
            if let Some(handler) = protocols.get(&(protocol as u16)) {
                handler.receive(payload)
            } else {
                Err(SocketError::ProtocolNotSupported)
            }
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    /// Mock TCP layer simulating real TCP segments
    ///
    /// Simplified TCP header:
    /// - Source Port (2 bytes)
    /// - Destination Port (2 bytes)
    /// - Sequence Number (4 bytes)
    /// - Acknowledgment Number (4 bytes)
    /// - Data Offset + Flags (2 bytes)
    /// - Window Size (2 bytes)
    /// - Checksum (2 bytes)
    /// - Urgent Pointer (2 bytes)
    /// - Payload (variable)
    struct MockTcpLayer {
        name: &'static str,
        packets_sent: AtomicU64,
        packets_received: AtomicU64,
        last_received_payload: RwLock<Vec<u8>>,
        received_payloads: RwLock<Vec<Vec<u8>>>,  // Store all received payloads
    }

    impl MockTcpLayer {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                packets_sent: AtomicU64::new(0),
                packets_received: AtomicU64::new(0),
                last_received_payload: RwLock::new(Vec::new()),
                received_payloads: RwLock::new(Vec::new()),
            }
        }

        fn get_last_received(&self) -> Vec<u8> {
            self.last_received_payload.read().clone()
        }
        
        fn get_all_received(&self) -> Vec<Vec<u8>> {
            self.received_payloads.read().clone()
        }
    }

    impl NetworkLayer for MockTcpLayer {
        fn register_protocol(&self, _proto_num: u16, _handler: Arc<dyn NetworkLayer>) {
            // TCP doesn't register further protocols
        }

        fn send(
            &self,
            payload: &[u8],
            context: &LayerContext,
            next_layers: &[Arc<dyn NetworkLayer>],
        ) -> Result<(), SocketError> {
            // Extract ports from context
            let src_port = context.get("tcp_src_port")
                .and_then(|p| if p.len() >= 2 { Some(u16::from_be_bytes([p[0], p[1]])) } else { None })
                .unwrap_or(0);
            
            let dst_port = context.get("tcp_dst_port")
                .and_then(|p| if p.len() >= 2 { Some(u16::from_be_bytes([p[0], p[1]])) } else { None })
                .ok_or(SocketError::InvalidPacket)?;

            // Build simplified TCP header (20 bytes minimum)
            let mut tcp_segment = Vec::with_capacity(20 + payload.len());
            
            tcp_segment.extend_from_slice(&src_port.to_be_bytes());  // Source Port
            tcp_segment.extend_from_slice(&dst_port.to_be_bytes());  // Destination Port
            tcp_segment.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);  // Sequence Number
            tcp_segment.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);  // Acknowledgment Number
            tcp_segment.extend_from_slice(&[0x50, 0x18]);  // Data Offset=5 (20 bytes), Flags (PSH+ACK)
            tcp_segment.extend_from_slice(&[0xFF, 0xFF]);  // Window Size
            tcp_segment.extend_from_slice(&[0x00, 0x00]);  // Checksum (not calculated)
            tcp_segment.extend_from_slice(&[0x00, 0x00]);  // Urgent Pointer
            tcp_segment.extend_from_slice(payload);  // Payload

            self.packets_sent.fetch_add(1, Ordering::SeqCst);

            // Create new context with IP protocol field
            let mut ip_context = context.clone();
            ip_context.set("ip_protocol", &[6]); // TCP protocol number

            // Send to IP layer
            if !next_layers.is_empty() {
                next_layers[0].send(&tcp_segment, &ip_context, &next_layers[1..])
            } else {
                Err(SocketError::NoRoute)
            }
        }

        fn receive(&self, segment: &[u8]) -> Result<(), SocketError> {
            self.packets_received.fetch_add(1, Ordering::SeqCst);

            // Parse TCP header
            if segment.len() < 20 {
                return Err(SocketError::InvalidPacket);
            }

            let _src_port = u16::from_be_bytes([segment[0], segment[1]]);
            let _dst_port = u16::from_be_bytes([segment[2], segment[3]]);
            let data_offset = (segment[12] >> 4) * 4;  // Data offset in bytes
            
            if segment.len() < data_offset as usize {
                return Err(SocketError::InvalidPacket);
            }

            let payload = &segment[data_offset as usize..];
            
            *self.last_received_payload.write() = payload.to_vec();
            self.received_payloads.write().push(payload.to_vec());
            
            Ok(())
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    // ============================================================================
    // Realistic Tests with Mock TCP/IP/Ethernet
    // ============================================================================

    #[test_case]
    fn test_realistic_tcp_ip_ethernet_stack_send() {
        // Create realistic protocol stack
        let ethernet = Arc::new(MockEthernetLayer::new(
            "eth0", 
            [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]
        ));
        let ip = Arc::new(MockIpLayer::new("ip", [192, 168, 1, 100]));
        let tcp = Arc::new(MockTcpLayer::new("tcp"));

        // Register TCP with IP layer
        ip.register_protocol(6, tcp.clone());

        // Prepare context with addresses and ports
        let mut ctx = LayerContext::new();
        ctx.set("ip_src", &[192, 168, 1, 100]);
        ctx.set("ip_dst", &[192, 168, 1, 1]);
        ctx.set("tcp_src_port", &5000u16.to_be_bytes());
        ctx.set("tcp_dst_port", &80u16.to_be_bytes());

        // Send data through the stack: TCP -> IP -> Ethernet
        let payload = b"GET / HTTP/1.1\r\n";
        let result = tcp.send(payload, &ctx, &[ip.clone(), ethernet.clone()]);
        assert!(result.is_ok());

        // Verify all layers processed the packet
        assert_eq!(tcp.packets_sent.load(Ordering::SeqCst), 1);
        assert_eq!(ip.packets_sent.load(Ordering::SeqCst), 1);
        assert_eq!(ethernet.packets_sent.load(Ordering::SeqCst), 1);

        // Verify Ethernet frame structure
        let frame = ethernet.get_last_frame();
        assert!(frame.len() > 14);  // Ethernet header + IP + TCP + payload
        
        // Check Ethernet header
        assert_eq!(&frame[0..6], &[0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);  // Dest MAC (from ARP)
        assert_eq!(&frame[6..12], &[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);  // Src MAC
        assert_eq!(&frame[12..14], &[0x08, 0x00]);  // EtherType: IPv4
        
        // Check IP header starts at offset 14
        assert_eq!(frame[14], 0x45);  // IPv4 version + IHL
        assert_eq!(frame[23], 6);  // Protocol: TCP
        assert_eq!(&frame[26..30], &[192, 168, 1, 100]);  // Source IP
        assert_eq!(&frame[30..34], &[192, 168, 1, 1]);  // Destination IP
        
        // Check TCP header starts at offset 34
        assert_eq!(&frame[34..36], &5000u16.to_be_bytes());  // Source port
        assert_eq!(&frame[36..38], &80u16.to_be_bytes());  // Destination port
    }

    #[test_case]
    fn test_realistic_tcp_ip_ethernet_receive() {
        // Create protocol stack
        let ethernet = Arc::new(MockEthernetLayer::new("eth0", [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]));
        let ip = Arc::new(MockIpLayer::new("ip", [192, 168, 1, 100]));
        let tcp = Arc::new(MockTcpLayer::new("tcp"));

        // Register TCP with IP
        ip.register_protocol(6, tcp.clone());

        // Build a complete Ethernet frame with IP and TCP
        let payload = b"HTTP/1.1 200 OK\r\n";
        let mut frame = Vec::new();

        // Ethernet header
        frame.extend_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);  // Dest MAC
        frame.extend_from_slice(&[0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);  // Src MAC
        frame.extend_from_slice(&[0x08, 0x00]);  // EtherType: IPv4

        // IP header
        let ip_total_len = (20 + 20 + payload.len()) as u16;
        frame.push(0x45);  // Version + IHL
        frame.push(0x00);  // TOS
        frame.extend_from_slice(&ip_total_len.to_be_bytes());  // Total Length
        frame.extend_from_slice(&[0x00, 0x00]);  // ID
        frame.extend_from_slice(&[0x00, 0x00]);  // Flags
        frame.push(64);  // TTL
        frame.push(6);  // Protocol: TCP
        frame.extend_from_slice(&[0x00, 0x00]);  // Checksum
        frame.extend_from_slice(&[192, 168, 1, 1]);  // Source IP
        frame.extend_from_slice(&[192, 168, 1, 100]);  // Dest IP

        // TCP header
        frame.extend_from_slice(&80u16.to_be_bytes());  // Source port
        frame.extend_from_slice(&5000u16.to_be_bytes());  // Dest port
        frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);  // Sequence
        frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);  // Ack
        frame.extend_from_slice(&[0x50, 0x18]);  // Data offset + flags
        frame.extend_from_slice(&[0xFF, 0xFF]);  // Window
        frame.extend_from_slice(&[0x00, 0x00]);  // Checksum
        frame.extend_from_slice(&[0x00, 0x00]);  // Urgent pointer
        frame.extend_from_slice(payload);  // Payload

        // Receive through Ethernet layer
        let result = ethernet.receive(&frame);
        assert!(result.is_ok());

        // Extract IP packet and pass to IP layer
        let ip_packet = &frame[14..];
        let result = ip.receive(ip_packet);
        assert!(result.is_ok());

        // Verify TCP received the payload
        assert_eq!(tcp.packets_received.load(Ordering::SeqCst), 1);
        assert_eq!(tcp.get_last_received(), payload);
    }

    #[test_case]
    fn test_realistic_two_socket_communication() {
        // Simulate two sockets communicating through shared protocol layers
        
        // Create shared protocol layers
        let ethernet = Arc::new(MockEthernetLayer::new("eth0", [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]));
        let ip = Arc::new(MockIpLayer::new("ip", [192, 168, 1, 100]));
        let tcp = Arc::new(MockTcpLayer::new("tcp"));
        
        ip.register_protocol(6, tcp.clone());

        // Socket 1: Client (192.168.1.100:5000 -> 192.168.1.1:80)
        let mut client_ctx = LayerContext::new();
        client_ctx.set("ip_src", &[192, 168, 1, 100]);
        client_ctx.set("ip_dst", &[192, 168, 1, 1]);
        client_ctx.set("tcp_src_port", &5000u16.to_be_bytes());
        client_ctx.set("tcp_dst_port", &80u16.to_be_bytes());

        // Socket 2: Server (192.168.1.1:80 -> 192.168.1.100:5000)  
        let mut server_ctx = LayerContext::new();
        server_ctx.set("ip_src", &[192, 168, 1, 1]);
        server_ctx.set("ip_dst", &[192, 168, 1, 100]);
        server_ctx.set("tcp_src_port", &80u16.to_be_bytes());
        server_ctx.set("tcp_dst_port", &5000u16.to_be_bytes());

        // Client sends request
        let request = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let result = tcp.send(request, &client_ctx, &[ip.clone(), ethernet.clone()]);
        assert!(result.is_ok());

        // Server sends response
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello, World!";
        let result = tcp.send(response, &server_ctx, &[ip.clone(), ethernet.clone()]);
        assert!(result.is_ok());

        // Verify both packets were sent
        assert_eq!(tcp.packets_sent.load(Ordering::SeqCst), 2);
        assert_eq!(ip.packets_sent.load(Ordering::SeqCst), 2);
        assert_eq!(ethernet.packets_sent.load(Ordering::SeqCst), 2);
    }

    #[test_case]
    fn test_realistic_end_to_end_with_protocol_agnostic_context() {
        // Test that LayerContext is truly protocol-agnostic
        
        let ethernet = Arc::new(MockEthernetLayer::new("eth0", [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]));
        let ip = Arc::new(MockIpLayer::new("ip", [192, 168, 1, 100]));
        let tcp = Arc::new(MockTcpLayer::new("tcp"));
        
        ip.register_protocol(6, tcp.clone());

        // Create context using only generic set/get methods
        let mut ctx = LayerContext::new();
        ctx.set("ip_src", &[192, 168, 1, 100]);
        ctx.set("ip_dst", &[192, 168, 1, 1]);
        ctx.set("tcp_src_port", &5000u16.to_be_bytes());
        ctx.set("tcp_dst_port", &80u16.to_be_bytes());
        ctx.set("custom_metadata", b"arbitrary data");  // Can add any custom data

        // Verify context is protocol-agnostic
        assert!(ctx.contains("ip_src"));
        assert!(ctx.contains("tcp_src_port"));
        assert!(ctx.contains("custom_metadata"));
        assert_eq!(ctx.get("custom_metadata"), Some(&b"arbitrary data"[..]));

        // Send data
        let data = b"Test payload";
        let result = tcp.send(data, &ctx, &[ip.clone(), ethernet.clone()]);
        assert!(result.is_ok());

        // Verify packet was sent successfully
        assert_eq!(tcp.packets_sent.load(Ordering::SeqCst), 1);
        let frame = ethernet.get_last_frame();
        assert!(!frame.is_empty());
    }
}
