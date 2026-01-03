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

use alloc::{string::String, sync::Arc, vec::Vec};

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
}
