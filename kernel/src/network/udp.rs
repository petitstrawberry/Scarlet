//! UDP protocol layer
//!
//! This module provides UDP datagram handling for the network stack.
//! It implements the NetworkLayer trait and provides UDP socket functionality.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, RwLock};

use crate::network::protocol_stack::get_network_manager;
use crate::network::protocol_stack::{LayerContext, NetworkLayer, NetworkLayerStats, SocketConfig};
use crate::network::socket::{
    SocketAddress, SocketControl, SocketError, SocketObject, SocketProtocol, SocketState,
    SocketType,
};

/// UDP header (8 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct UdpHeader {
    /// Source port
    pub src_port: u16,
    /// Destination port
    pub dst_port: u16,
    /// Length (header + data)
    pub length: u16,
    /// Checksum
    pub checksum: u16,
}

impl UdpHeader {
    /// Create a new UDP header
    pub fn new(src_port: u16, dst_port: u16, length: u16) -> Self {
        Self {
            src_port,
            dst_port,
            length,
            checksum: 0,
        }
    }

    /// Calculate UDP checksum
    pub fn calculate_checksum(&self, src_ip: [u8; 4], dst_ip: [u8; 4], data: &[u8]) -> u16 {
        let mut sum: u32 = 0;

        // Pseudo-header: src IP (4) + dst IP (4) + zero (1) + protocol (1) + UDP length (2)
        sum += u32::from_be_bytes([src_ip[0], src_ip[1]]);
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += u32::from_be_bytes([src_ip[2], src_ip[3]]);
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += u32::from_be_bytes([dst_ip[0], dst_ip[1]]);
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += u32::from_be_bytes([dst_ip[2], dst_ip[3]]);
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += 17u32; // UDP protocol number
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += self.length as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);

        // UDP header
        sum += self.src_port as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += self.dst_port as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += self.length as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += self.checksum as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);

        // Data
        for chunk in data.chunks(2) {
            if chunk.len() == 2 {
                sum += u32::from_be_bytes([chunk[0], chunk[1]]);
                sum = (sum & 0xFFFF) + (sum >> 16);
            } else if chunk.len() == 1 {
                sum += (chunk[0] as u32) << 8;
                sum = (sum & 0xFFFF) + (sum >> 16);
            }
        }

        !sum as u16
    }

    /// Serialize header to bytes
    pub fn to_bytes(&self) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        bytes[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        bytes[4..6].copy_from_slice(&self.length.to_be_bytes());
        bytes[6..8].copy_from_slice(&self.checksum.to_be_bytes());
        bytes
    }

    /// Parse header from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 {
            return None;
        }

        Some(Self {
            src_port: u16::from_be_bytes([bytes[0], bytes[1]]),
            dst_port: u16::from_be_bytes([bytes[2], bytes[3]]),
            length: u16::from_be_bytes([bytes[4], bytes[5]]),
            checksum: u16::from_be_bytes([bytes[6], bytes[7]]),
        })
    }
}

/// UDP socket
///
/// Implements SocketObject for UDP datagram communication.
pub struct UdpSocket {
    /// Local address
    local_addr: RwLock<Option<SocketAddress>>,
    /// Remote address (for connected sockets)
    remote_addr: RwLock<Option<SocketAddress>>,
    /// Send buffer
    send_buffer: Mutex<Vec<Vec<u8>>>,
    /// Receive buffer
    recv_buffer: Mutex<Vec<Vec<u8>>>,
    /// Socket state
    state: RwLock<SocketState>,
    /// Reference to UDP layer
    udp_layer: Arc<UdpLayer>,
}

impl UdpSocket {
    /// Create a new UDP socket
    pub fn new(udp_layer: Arc<UdpLayer>) -> Arc<Self> {
        Arc::new(Self {
            local_addr: RwLock::new(None),
            remote_addr: RwLock::new(None),
            send_buffer: Mutex::new(Vec::new()),
            recv_buffer: Mutex::new(Vec::new()),
            state: RwLock::new(SocketState::Unconnected),
            udp_layer,
        })
    }

    /// Deliver received datagram to this socket
    pub fn deliver_datagram(&self, data: Vec<u8>) {
        self.recv_buffer.lock().push(data);
    }
}

impl SocketObject for UdpSocket {
    fn socket_type(&self) -> SocketType {
        SocketType::Datagram
    }

    fn socket_domain(&self) -> crate::network::socket::SocketDomain {
        crate::network::socket::SocketDomain::Inet
    }

    fn socket_protocol(&self) -> SocketProtocol {
        SocketProtocol::Udp
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn sendto(
        &self,
        data: &[u8],
        address: &SocketAddress,
        _flags: u32,
    ) -> Result<usize, SocketError> {
        match address {
            SocketAddress::Inet { addr, port } => {
                // Queue the datagram for sending
                let mut buffer = self.send_buffer.lock();
                let datagram = data.to_vec();
                buffer.push(datagram);

                // Update state
                *self.remote_addr.write() = Some(address.clone());
                *self.state.write() = SocketState::Connected;

                // Try to send through UDP layer
                self.udp_layer.send_datagram(self, addr, port, datagram)?;

                Ok(data.len())
            }
            _ => Err(SocketError::InvalidAddress),
        }
    }

    fn recvfrom(
        &self,
        buffer: &mut [u8],
        _flags: u32,
    ) -> Result<(usize, SocketAddress), SocketError> {
        let mut recv_buf = self.recv_buffer.lock();

        if recv_buf.is_empty() {
            return Err(SocketError::WouldBlock);
        }

        let datagram = recv_buf.remove(0);
        let len = buffer.len().min(datagram.len());
        buffer[..len].copy_from_slice(&datagram[..len]);

        Ok((
            len,
            self.remote_addr
                .read()
                .clone()
                .unwrap_or(SocketAddress::Unspecified),
        ))
    }
}

impl SocketControl for UdpSocket {
    fn bind(&self, address: &SocketAddress) -> Result<(), SocketError> {
        match address {
            SocketAddress::Inet { addr, port } => {
                let mut config = SocketConfig::new();
                config.set("udp_local_port", &port.to_be_bytes());
                config.set("ip_local", &addr);

                // Configure UDP layer
                self.udp_layer.configure_socket(self, &config)?;

                *self.local_addr.write() = Some(address.clone());
                *self.state.write() = SocketState::Bound;
                Ok(())
            }
            _ => Err(SocketError::InvalidAddress),
        }
    }

    fn connect(&self, address: &SocketAddress) -> Result<(), SocketError> {
        match address {
            SocketAddress::Inet { .. } => {
                *self.remote_addr.write() = Some(address.clone());
                *self.state.write() = SocketState::Connected;
                Ok(())
            }
            _ => Err(SocketError::InvalidAddress),
        }
    }

    fn listen(&self, _backlog: usize) -> Result<(), SocketError> {
        Err(SocketError::NotSupported)
    }

    fn accept(&self) -> Result<Arc<dyn SocketObject>, SocketError> {
        Err(SocketError::NotSupported)
    }

    fn getpeername(&self) -> Result<SocketAddress, SocketError> {
        self.remote_addr
            .read()
            .clone()
            .ok_or(SocketError::NotConnected)
    }

    fn getsockname(&self) -> Result<SocketAddress, SocketError> {
        self.local_addr
            .read()
            .clone()
            .ok_or(SocketError::InvalidAddress)
    }

    fn shutdown(&self, _how: crate::network::socket::ShutdownHow) -> Result<(), SocketError> {
        *self.state.write() = SocketState::Closed;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        *self.state.read() == SocketState::Connected
    }

    fn state(&self) -> SocketState {
        *self.state.read()
    }
}

impl crate::ipc::StreamIpcOps for UdpSocket {
    fn is_connected(&self) -> bool {
        SocketControl::is_connected(self)
    }

    fn peer_count(&self) -> usize {
        if SocketControl::is_connected(self) {
            1
        } else {
            0
        }
    }

    fn description(&self) -> String {
        alloc::format!("UDP socket")
    }
}

impl crate::object::capability::StreamOps for UdpSocket {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, crate::object::capability::StreamError> {
        let (len, _) = self
            .recvfrom(buffer, 0)
            .map_err(|_| crate::object::capability::StreamError::Other)?;
        Ok(len)
    }

    fn write(&self, data: &[u8]) -> Result<usize, crate::object::capability::StreamError> {
        let remote_addr = self
            .remote_addr
            .read()
            .clone()
            .unwrap_or(SocketAddress::Unspecified);
        self.sendto(data, &remote_addr, 0)
            .map_err(|_| crate::object::capability::StreamError::Other)?;
        Ok(data.len())
    }
}

impl crate::object::capability::CloneOps for UdpSocket {
    fn custom_clone(&self) -> crate::object::KernelObject {
        crate::object::KernelObject::Socket(Arc::clone(self))
    }
}

/// UDP layer
///
/// Manages UDP port bindings and handles UDP datagrams.
pub struct UdpLayer {
    /// Port-to-socket mapping for receiving datagrams
    port_map: RwLock<BTreeMap<u16, alloc::sync::Weak<UdpSocket>>>,
    /// Port allocation (ephemeral ports start from 49152)
    next_ephemeral_port: Mutex<u16>,
    /// Statistics
    stats: RwLock<NetworkLayerStats>,
}

impl UdpLayer {
    /// Create a new UDP layer
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            port_map: RwLock::new(BTreeMap::new()),
            next_ephemeral_port: Mutex::new(49152),
            stats: RwLock::new(NetworkLayerStats::default()),
        })
    }

    /// Create a new UDP socket
    pub fn create_socket(self: &Arc<Self>) -> Arc<UdpSocket> {
        UdpSocket::new(Arc::clone(self))
    }

    /// Allocate an ephemeral port
    pub fn allocate_port(&self) -> u16 {
        let mut next_port = self.next_ephemeral_port.lock();
        let port = *next_port;

        *next_port = if port == 65535 { 49152 } else { port + 1 };

        port
    }

    /// Register a socket for a specific port
    pub fn register_port(&self, port: u16, socket: alloc::sync::Weak<UdpSocket>) {
        self.port_map.write().insert(port, socket);
    }

    /// Unregister a socket from a port
    pub fn unregister_port(&self, port: u16) {
        self.port_map.write().remove(&port);
    }

    /// Find socket for a destination port
    pub fn find_socket(&self, port: u16) -> Option<Arc<UdpSocket>> {
        self.port_map
            .read()
            .get(&port)
            .and_then(|weak| weak.upgrade())
    }

    /// Configure a UDP socket (bind)
    pub fn configure_socket(
        &self,
        socket: &Arc<UdpSocket>,
        config: &SocketConfig,
    ) -> Result<(), SocketError> {
        let port = config
            .get_u16("udp_local_port")
            .ok_or(SocketError::InvalidAddress)?;

        // Register the port
        self.register_port(port, Arc::downgrade(socket));

        // TODO: Configure IP layer with local address
        Ok(())
    }

    /// Send a UDP datagram
    pub fn send_datagram(
        &self,
        socket: &UdpSocket,
        dest_ip: [u8; 4],
        dest_port: u16,
        data: Vec<u8>,
    ) -> Result<(), SocketError> {
        // Get source port from local address
        let local_addr = socket.local_addr.read();
        let src_port = match *local_addr {
            Some(SocketAddress::Inet { port, .. }) => port,
            _ => {
                // Allocate ephemeral port if not bound
                return Err(SocketError::NotBound);
            }
        };

        // Build UDP header
        let total_length = (8 + data.len()) as u16;
        let mut header = UdpHeader::new(src_port, dest_port, total_length);

        // Calculate checksum
        let src_ip = match *local_addr {
            Some(SocketAddress::Inet { addr, .. }) => addr,
            _ => [0, 0, 0, 0],
        };
        header.checksum = header.calculate_checksum(src_ip, dest_ip, &data);

        // Serialize header and combine with data
        let mut udp_packet = Vec::with_capacity(8 + data.len());
        udp_packet.extend_from_slice(&header.to_bytes());
        udp_packet.extend_from_slice(&data);

        // Create context for IP layer
        let mut ip_context = LayerContext::new();
        ip_context.set("ip_dst", &dest_ip);
        ip_context.set("ip_protocol", &[17]); // UDP protocol number

            "[UDP] Send: {} bytes (src port: {}, dst: {}.{}.{}.{})",
            udp_packet.len(),
            src_port,
            dest_ip[0],
            dest_ip[1],
            dest_ip[2],
            dest_ip[3]
        );

        // Send through IP layer
        if let Some(ip_layer) = get_network_manager().get_layer("ip") {
            ip_layer.send(&udp_packet, &ip_context, &[])?;
        }

        // Update statistics
        let mut stats = self.stats.write();
        stats.packets_sent += 1;
        stats.bytes_sent += udp_packet.len() as u64;

        Ok(())
    }

    /// Receive a UDP datagram
    pub fn receive_datagram(&self, src_port: u16, dst_port: u16, data: Vec<u8>) {
            "[UDP] Recv: {} bytes (src port: {}, dst port: {})",
            data.len(),
            src_port,
            dst_port
        );

        // Update statistics
        let mut stats = self.stats.write();
        stats.packets_received += 1;
        stats.bytes_received += (8 + data.len()) as u64;

        // Find socket registered for destination port
        if let Some(socket) = self.find_socket(dst_port) {
            // Deliver datagram to socket
            socket.deliver_datagram(data);
        } else {
            // No socket bound to this port - drop the datagram
        }
    }
}

impl NetworkLayer for UdpLayer {
    fn register_protocol(&self, _proto_num: u16, _handler: Arc<dyn NetworkLayer>) {
        // UDP is typically a leaf protocol
    }

    fn send(
        &self,
        _packet: &[u8],
        _context: &LayerContext,
        _next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // UDP send is handled through send_datagram method
        Ok(())
    }

    fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
        if packet.len() < 8 {
            return Err(SocketError::InvalidPacket);
        }

        // Parse UDP header
        let header = UdpHeader::from_bytes(&packet[..8]).ok_or(SocketError::InvalidPacket)?;

        let data_offset = header.length as usize;
        if data_offset < 8 || data_offset > packet.len() {
            return Err(SocketError::InvalidPacket);
        }

        let data = &packet[8..data_offset];

        // Receive the datagram
        self.receive_datagram(header.src_port, header.dst_port, data.to_vec());

        Ok(())
    }

    fn name(&self) -> &'static str {
        "UDP"
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
    fn test_udp_header_creation() {
        let header = UdpHeader::new(1234, 5678, 100);

        assert_eq!(header.src_port, 1234);
        assert_eq!(header.dst_port, 5678);
        assert_eq!(header.length, 100);
    }

    #[test_case]
    fn test_udp_header_serialization() {
        let header = UdpHeader::new(1234, 5678, 100);
        let bytes = header.to_bytes();

        assert_eq!(bytes.len(), 8);
        assert_eq!(u16::from_be_bytes([bytes[0], bytes[1]]), 1234);
        assert_eq!(u16::from_be_bytes([bytes[2], bytes[3]]), 5678);
        assert_eq!(u16::from_be_bytes([bytes[4], bytes[5]]), 100);
    }

    #[test_case]
    fn test_udp_header_parsing() {
        let mut bytes = [0u8; 8];
        bytes[0..2].copy_from_slice(&1234u16.to_be_bytes());
        bytes[2..4].copy_from_slice(&5678u16.to_be_bytes());
        bytes[4..6].copy_from_slice(&100u16.to_be_bytes());
        bytes[6..8].copy_from_slice(&0xABCDu16.to_be_bytes());

        let header = UdpHeader::from_bytes(&bytes).unwrap();

        assert_eq!(header.src_port, 1234);
        assert_eq!(header.dst_port, 5678);
        assert_eq!(header.length, 100);
        assert_eq!(header.checksum, 0xABCD);
    }

    #[test_case]
    fn test_udp_checksum() {
        let src_ip = [192, 168, 1, 100];
        let dst_ip = [192, 168, 1, 1];
        let data = b"test";

        let mut header = UdpHeader::new(1234, 5678, (8 + data.len()) as u16);
        header.checksum = header.calculate_checksum(src_ip, dst_ip, data);

        // Just verify that checksum calculation runs without panicking
        assert_ne!(header.checksum, 0);
    }

    #[test_case]
    fn test_udp_layer_creation() {
        let udp_layer = UdpLayer::new();

        // Test port allocation
        let port1 = udp_layer.allocate_port();
        let port2 = udp_layer.allocate_port();

        assert!(port1 >= 49152 && port1 <= 65535);
        assert!(port2 >= 49152 && port2 <= 65535);
        assert_ne!(port1, port2);
    }
}
