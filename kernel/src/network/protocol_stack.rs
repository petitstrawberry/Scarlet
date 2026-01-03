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

use alloc::{boxed::Box, collections::BTreeMap, string::String, sync::Arc, vec::Vec};
use spin::RwLock;

use super::socket::{SocketAddress, SocketDomain, SocketError, SocketObject, SocketProtocol, SocketType};
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
/// packet delivery. This solves the problem of how to pass destination
/// addresses and other routing info without tight coupling between layers.
///
/// # Design Philosophy
///
/// - Each layer adds the information it needs (e.g., IP adds addresses)
/// - Lower layers consume routing info to make forwarding decisions
/// - Flexible key-value store for protocol-specific information
/// - Enables proper layer composition following @petitstrawberry's guidance
///
/// # Example Flow
///
/// ```rust,ignore
/// // Application layer creates context
/// let mut ctx = LayerContext::new();
/// ctx.destination_address = Some(SocketAddress::Inet4(...));
/// ctx.protocol_number = 6; // TCP
///
/// // TCP layer sends to IP
/// tcp_layer.send(&data, &ctx, &[ip_layer])?;
///
/// // IP layer extracts destination IP, adds IP header
/// // IP layer sends to Ethernet
/// ip_layer.send(&ip_packet, &ctx, &[eth_layer])?;
///
/// // Ethernet performs ARP lookup using destination IP from ctx
/// ```
#[derive(Debug, Clone, Default)]
pub struct LayerContext {
    /// Destination address (full socket address with IP + port)
    pub destination_address: Option<SocketAddress>,
    
    /// Source address (full socket address with IP + port)
    pub source_address: Option<SocketAddress>,
    
    /// Protocol number for upper layer (e.g., 6=TCP, 17=UDP)
    pub protocol_number: u16,
    
    /// Additional protocol-specific information
    /// Used for things like TCP flags, TTL, QoS markers, etc.
    pub additional_info: BTreeMap<String, Vec<u8>>,
}

impl LayerContext {
    /// Create a new empty LayerContext
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Create context with destination and protocol
    pub fn with_destination(destination: SocketAddress, protocol: u16) -> Self {
        Self {
            destination_address: Some(destination),
            protocol_number: protocol,
            ..Default::default()
        }
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
/// **NetworkLayer (shared, stateless):**
/// - Protocol logic and packet processing
/// - Routing tables, ARP cache (shared state)
/// - Registered protocol handlers
/// - Like: ext2 driver, tmpfs implementation
///
/// **SocketObject (per-socket, stateful):**
/// - Connection state (ports, addresses)
/// - Send/receive buffers
/// - References to NetworkLayer instances
/// - Like: FileObject with seek position, flags
///
/// # Example: IP Layer (Shared Singleton)
///
/// ```rust,ignore
/// struct IpLayer {
///     protocols: RwLock<BTreeMap<u16, Arc<dyn NetworkLayer>>>, // Shared routing
///     routing_table: RwLock<RoutingTable>,  // Shared routing
///     arp_cache: RwLock<ArpCache>,          // Shared ARP cache
/// }
///
/// impl NetworkLayer for IpLayer {
///     fn register_protocol(&self, proto_num: u16, handler: Arc<dyn NetworkLayer>) {
///         self.protocols.write().insert(proto_num, handler);
///     }
///
///     fn send(&self, packet: &[u8], context: &LayerContext, 
///             next_layers: &[Arc<dyn NetworkLayer>]) -> Result<(), SocketError> {
///         // Extract destination from context
///         let dest_ip = context.destination_address
///             .as_ref()
///             .ok_or(SocketError::InvalidPacket)?
///             .get_ip();
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
    /// like destination address that may be needed for proper forwarding.
    ///
    /// # Arguments
    ///
    /// * `packet` - Packet data to send
    /// * `context` - Routing context with destination, protocol, etc.
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
    /// let ctx = LayerContext::with_destination(remote_addr, 6); // TCP
    /// tcp_layer.send(&tcp_segment, &ctx, &[ip_layer])?;
    /// ip_layer.send(&ip_packet, &ctx, &[ethernet_layer, infiniband_layer])?;
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

    // Mock protocol layers for testing the NetworkLayer trait

    /// Mock link layer (like Ethernet)
    struct MockLinkLayer {
        name: &'static str,
        packets_sent: AtomicU64,
        packets_received: AtomicU64,
        last_sent_packet: RwLock<Vec<u8>>,
    }

    impl MockLinkLayer {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                packets_sent: AtomicU64::new(0),
                packets_received: AtomicU64::new(0),
                last_sent_packet: RwLock::new(Vec::new()),
            }
        }

        fn get_last_packet(&self) -> Vec<u8> {
            self.last_sent_packet.read().clone()
        }
    }

    impl NetworkLayer for MockLinkLayer {
        fn register_protocol(&self, _proto_num: u16, _handler: Arc<dyn NetworkLayer>) {
            // Link layer doesn't register upper protocols in this simple mock
        }

        fn send(
            &self,
            packet: &[u8],
            _context: &LayerContext,
            _next_layers: &[Arc<dyn NetworkLayer>],
        ) -> Result<(), SocketError> {
            // Simulate sending packet (store it for inspection)
            *self.last_sent_packet.write() = packet.to_vec();
            self.packets_sent.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
            // Link layer just counts received packets in this mock
            self.packets_received.fetch_add(1, Ordering::SeqCst);
            // In real implementation, would strip link header and pass to network layer
            Ok(())
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    /// Mock network layer (like IP)
    struct MockNetworkLayer {
        name: &'static str,
        protocols: RwLock<BTreeMap<u16, Arc<dyn NetworkLayer>>>,
        packets_sent: AtomicU64,
        packets_received: AtomicU64,
    }

    impl MockNetworkLayer {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                protocols: RwLock::new(BTreeMap::new()),
                packets_sent: AtomicU64::new(0),
                packets_received: AtomicU64::new(0),
            }
        }
    }

    impl NetworkLayer for MockNetworkLayer {
        fn register_protocol(&self, proto_num: u16, handler: Arc<dyn NetworkLayer>) {
            self.protocols.write().insert(proto_num, handler);
        }

        fn send(
            &self,
            packet: &[u8],
            context: &LayerContext,
            next_layers: &[Arc<dyn NetworkLayer>],
        ) -> Result<(), SocketError> {
            // Add simple "IP" header (just 2 bytes: protocol number)
            let mut ip_packet = Vec::with_capacity(packet.len() + 2);
            ip_packet.extend_from_slice(&[0x45, 0x00]); // Mock IP header
            ip_packet.extend_from_slice(packet);

            self.packets_sent.fetch_add(1, Ordering::SeqCst);

            // Try to send through available lower layers
            for layer in next_layers {
                if layer.send(&ip_packet, context, &[]).is_ok() {
                    return Ok(());
                }
            }

            Err(SocketError::NoRoute)
        }

        fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
            self.packets_received.fetch_add(1, Ordering::SeqCst);

            // Parse mock IP header (first 2 bytes)
            if packet.len() < 3 {
                return Err(SocketError::InvalidPacket);
            }

            let proto_num = packet[2] as u16;
            let payload = &packet[3..];

            // Route to registered protocol handler
            let protocols = self.protocols.read();
            if let Some(handler) = protocols.get(&proto_num) {
                handler.receive(payload)
            } else {
                Err(SocketError::ProtocolNotSupported)
            }
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    /// Mock transport layer (like TCP/UDP)
    struct MockTransportLayer {
        name: &'static str,
        proto_num: u16,
        packets_sent: AtomicU64,
        packets_received: AtomicU64,
        last_received_payload: RwLock<Vec<u8>>,
    }

    impl MockTransportLayer {
        fn new(name: &'static str, proto_num: u16) -> Self {
            Self {
                name,
                proto_num,
                packets_sent: AtomicU64::new(0),
                packets_received: AtomicU64::new(0),
                last_received_payload: RwLock::new(Vec::new()),
            }
        }

        fn get_last_received(&self) -> Vec<u8> {
            self.last_received_payload.read().clone()
        }
    }

    impl NetworkLayer for MockTransportLayer {
        fn register_protocol(&self, _proto_num: u16, _handler: Arc<dyn NetworkLayer>) {
            // Transport layer typically doesn't register further protocols
        }

        fn send(
            &self,
            packet: &[u8],
            context: &LayerContext,
            next_layers: &[Arc<dyn NetworkLayer>],
        ) -> Result<(), SocketError> {
            // Add transport header (protocol number + payload)
            let mut transport_packet = Vec::with_capacity(packet.len() + 1);
            transport_packet.push(self.proto_num as u8);
            transport_packet.extend_from_slice(packet);

            self.packets_sent.fetch_add(1, Ordering::SeqCst);

            // Send to network layer
            if !next_layers.is_empty() {
                next_layers[0].send(&transport_packet, context, &next_layers[1..])
            } else {
                Err(SocketError::NoRoute)
            }
        }

        fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
            self.packets_received.fetch_add(1, Ordering::SeqCst);
            *self.last_received_payload.write() = packet.to_vec();
            Ok(())
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    #[test_case]
    fn test_network_layer_basic_send() {
        // Create a simple stack: Transport -> Link
        let link_layer = Arc::new(MockLinkLayer::new("MockEthernet"));
        let transport_layer = Arc::new(MockTransportLayer::new("MockTCP", 6));

        let payload = b"Hello, Network!";
        let context = LayerContext::new();

        // Send packet from transport to link
        let result = transport_layer.send(payload, &context, &[link_layer.clone()]);
        assert!(result.is_ok());

        // Verify packet was sent
        assert_eq!(transport_layer.packets_sent.load(Ordering::SeqCst), 1);
        assert_eq!(link_layer.packets_sent.load(Ordering::SeqCst), 1);

        // Verify packet content (should have transport header + payload)
        let sent_packet = link_layer.get_last_packet();
        assert!(sent_packet.len() > payload.len());
        assert_eq!(sent_packet[0], 6); // Protocol number
    }

    #[test_case]
    fn test_network_layer_three_tier_stack() {
        // Create a three-tier stack: Transport -> Network -> Link
        let link_layer = Arc::new(MockLinkLayer::new("MockEthernet"));
        let network_layer = Arc::new(MockNetworkLayer::new("MockIP"));
        let transport_layer = Arc::new(MockTransportLayer::new("MockTCP", 6));

        // Register transport with network layer
        network_layer.register_protocol(6, transport_layer.clone());

        let payload = b"Test Data";
        let context = LayerContext::new();

        // Send: Transport -> Network -> Link
        let result = transport_layer.send(payload, &context, &[network_layer.clone(), link_layer.clone()]);
        assert!(result.is_ok());

        // Verify all layers processed the packet
        assert_eq!(transport_layer.packets_sent.load(Ordering::SeqCst), 1);
        assert_eq!(network_layer.packets_sent.load(Ordering::SeqCst), 1);
        assert_eq!(link_layer.packets_sent.load(Ordering::SeqCst), 1);
    }

    #[test_case]
    fn test_network_layer_protocol_routing() {
        // Create network layer with multiple transport protocols
        let network_layer = Arc::new(MockNetworkLayer::new("MockIP"));
        let tcp_layer = Arc::new(MockTransportLayer::new("MockTCP", 6));
        let udp_layer = Arc::new(MockTransportLayer::new("MockUDP", 17));

        // Register both protocols
        network_layer.register_protocol(6, tcp_layer.clone());
        network_layer.register_protocol(17, udp_layer.clone());

        // Simulate receiving IP packet with TCP (proto=6)
        let tcp_packet = vec![0x45, 0x00, 6, b'T', b'C', b'P'];
        let result = network_layer.receive(&tcp_packet);
        assert!(result.is_ok());
        assert_eq!(tcp_layer.packets_received.load(Ordering::SeqCst), 1);
        assert_eq!(udp_layer.packets_received.load(Ordering::SeqCst), 0);

        // Simulate receiving IP packet with UDP (proto=17)
        let udp_packet = vec![0x45, 0x00, 17, b'U', b'D', b'P'];
        let result = network_layer.receive(&udp_packet);
        assert!(result.is_ok());
        assert_eq!(tcp_layer.packets_received.load(Ordering::SeqCst), 1);
        assert_eq!(udp_layer.packets_received.load(Ordering::SeqCst), 1);
    }

    #[test_case]
    fn test_network_layer_multiple_lower_layers() {
        // Test sending through multiple link layer options
        let ethernet = Arc::new(MockLinkLayer::new("Ethernet"));
        let infiniband = Arc::new(MockLinkLayer::new("InfiniBand"));
        let network_layer = Arc::new(MockNetworkLayer::new("IP"));

        let payload = b"Multi-path test";
        let context = LayerContext::new();

        // Network layer tries both link layers
        let result = network_layer.send(payload, &context, &[ethernet.clone(), infiniband.clone()]);
        assert!(result.is_ok());

        // First layer should succeed
        assert_eq!(ethernet.packets_sent.load(Ordering::SeqCst), 1);
        // Second layer shouldn't be tried since first succeeded
        assert_eq!(infiniband.packets_sent.load(Ordering::SeqCst), 0);
    }

    #[test_case]
    fn test_network_layer_no_route_error() {
        let transport_layer = Arc::new(MockTransportLayer::new("TCP", 6));
        let payload = b"No route";
        let context = LayerContext::new();

        // Try to send with no lower layers
        let result = transport_layer.send(payload, &context, &[]);
        assert!(result.is_err());
        assert!(matches!(result, Err(SocketError::NoRoute)));
    }

    #[test_case]
    fn test_network_layer_protocol_not_supported() {
        let network_layer = Arc::new(MockNetworkLayer::new("IP"));
        let tcp_layer = Arc::new(MockTransportLayer::new("TCP", 6));

        // Register only TCP
        network_layer.register_protocol(6, tcp_layer);

        // Try to receive packet for unregistered protocol (UDP=17)
        let udp_packet = vec![0x45, 0x00, 17, b'U', b'D', b'P'];
        let result = network_layer.receive(&udp_packet);
        assert!(result.is_err());
        assert!(matches!(result, Err(SocketError::ProtocolNotSupported)));
    }

    #[test_case]
    fn test_network_layer_end_to_end() {
        // Complete end-to-end test: send and receive through full stack
        let link_layer = Arc::new(MockLinkLayer::new("Ethernet"));
        let network_layer = Arc::new(MockNetworkLayer::new("IP"));
        let tcp_layer = Arc::new(MockTransportLayer::new("TCP", 6));

        // Build the stack
        network_layer.register_protocol(6, tcp_layer.clone());

        // Send data down the stack
        let original_data = b"End-to-end test data";
        let context = LayerContext::new();
        let result = tcp_layer.send(original_data, &context, &[network_layer.clone(), link_layer.clone()]);
        assert!(result.is_ok());

        // Get the packet from link layer
        let link_packet = link_layer.get_last_packet();
        assert!(!link_packet.is_empty());

        // Simulate receiving the same packet back up the stack
        // In real scenario, this would come from device
        // For testing, we manually construct the receive path

        // Strip mock IP header (first 2 bytes) and pass to network layer
        let ip_payload = &link_packet[2..];
        let result = network_layer.receive(ip_payload);
        assert!(result.is_ok());

        // Verify TCP layer received the data
        assert_eq!(tcp_layer.packets_received.load(Ordering::SeqCst), 1);
        let received_data = tcp_layer.get_last_received();
        assert_eq!(received_data, original_data);
    }

    #[test_case]
    fn test_layer_context_usage() {
        // Test LayerContext with destination address
        use super::socket::{Inet4SocketAddress, SocketAddress};

        let dest_addr = SocketAddress::Inet4(Inet4SocketAddress {
            addr: [192, 168, 1, 100],
            port: 80,
        });
        
        let context = LayerContext::with_destination(dest_addr.clone(), 6); // TCP
        
        assert_eq!(context.protocol_number, 6);
        assert!(context.destination_address.is_some());
        assert_eq!(context.destination_address.unwrap(), dest_addr);
    }
}
