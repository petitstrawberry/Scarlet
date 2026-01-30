//! TCP protocol implementation
//!
//! This module provides TCP (Transmission Control Protocol) functionality for the Scarlet kernel.

use crate::network::inet::{Ipv4Address, Ipv4Protocol};
use crate::network::protocol_stack::{LayerContext, NetworkLayer, SocketError};
use crate::network::socket::{SocketAddress, SocketControl, SocketObject, SocketState};
use crate::object::capability::stream::{ReadOps, WriteOps};
use crate::sync::spinlock::SpinLock;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};

/// TCP connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

/// TCP flags
#[allow(dead_code)]
mod tcp_flags {
    pub const FIN: u8 = 0x01;
    pub const SYN: u8 = 0x02;
    pub const RST: u8 = 0x04;
    pub const PSH: u8 = 0x08;
    pub const ACK: u8 = 0x10;
    pub const URG: u8 = 0x20;
}

/// TCP header (minimum 20 bytes)
#[derive(Debug, Clone)]
pub struct TcpHeader {
    pub source_port: u16,
    pub dest_port: u16,
    pub seq_number: u32,
    pub ack_number: u32,
    pub data_offset_flags: u16, // Data offset (4 bits) + flags (12 bits)
    pub window_size: u16,
    pub checksum: u16,
    pub urgent_pointer: u16,
}

impl TcpHeader {
    /// Create a new TCP header
    pub fn new(source_port: u16, dest_port: u16) -> Self {
        Self {
            source_port,
            dest_port,
            seq_number: 0,
            ack_number: 0,
            data_offset_flags: 0x5000, // Data offset = 5 (20 bytes), no flags
            window_size: 65535,
            checksum: 0,
            urgent_pointer: 0,
        }
    }

    /// Serialize header to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(20);
        bytes.extend_from_slice(&self.source_port.to_be_bytes());
        bytes.extend_from_slice(&self.dest_port.to_be_bytes());
        bytes.extend_from_slice(&self.seq_number.to_be_bytes());
        bytes.extend_from_slice(&self.ack_number.to_be_bytes());
        bytes.extend_from_slice(&self.data_offset_flags.to_be_bytes());
        bytes.extend_from_slice(&self.window_size.to_be_bytes());
        bytes.extend_from_slice(&self.checksum.to_be_bytes());
        bytes.extend_from_slice(&self.urgent_pointer.to_be_bytes());
        bytes
    }

    /// Parse header from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 20 {
            return None;
        }

        Some(Self {
            source_port: u16::from_be_bytes([bytes[0], bytes[1]]),
            dest_port: u16::from_be_bytes([bytes[2], bytes[3]]),
            seq_number: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            ack_number: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            data_offset_flags: u16::from_be_bytes([bytes[12], bytes[13]]),
            window_size: u16::from_be_bytes([bytes[14], bytes[15]]),
            checksum: u16::from_be_bytes([bytes[16], bytes[17]]),
            urgent_pointer: u16::from_be_bytes([bytes[18], bytes[19]]),
        })
    }

    /// Get TCP flags
    pub fn flags(&self) -> u8 {
        (self.data_offset_flags & 0x3F) as u8
    }

    /// Set TCP flags
    pub fn set_flags(&mut self, flags: u8) {
        self.data_offset_flags = (self.data_offset_flags & 0xFFC0) | (flags as u16 & 0x3F);
    }

    /// Get header length in bytes
    pub fn header_length(&self) -> usize {
        ((self.data_offset_flags >> 12) as usize) * 4
    }
}

/// TCP socket implementation
pub struct TcpSocket {
    state: SpinLock<TcpState>,
    local_addr: SpinLock<Option<Ipv4Address>>,
    local_port: AtomicU16,
    remote_addr: SpinLock<Option<Ipv4Address>>,
    remote_port: AtomicU16,
    
    // Sequence numbers
    send_seq: AtomicU32,
    recv_seq: AtomicU32,
    
    // Data buffers
    send_buffer: SpinLock<VecDeque<u8>>,
    recv_buffer: SpinLock<VecDeque<u8>>,
    
    // Reference to TCP layer for sending packets
    tcp_layer: Weak<TcpLayer>,
}

impl TcpSocket {
    /// Create a new TCP socket
    pub fn new(tcp_layer: Weak<TcpLayer>) -> Self {
        Self {
            state: SpinLock::new(TcpState::Closed),
            local_addr: SpinLock::new(None),
            local_port: AtomicU16::new(0),
            remote_addr: SpinLock::new(None),
            remote_port: AtomicU16::new(0),
            send_seq: AtomicU32::new(0),
            recv_seq: AtomicU32::new(0),
            send_buffer: SpinLock::new(VecDeque::new()),
            recv_buffer: SpinLock::new(VecDeque::new()),
            tcp_layer,
        }
    }

    /// Get current TCP state
    pub fn get_state(&self) -> TcpState {
        *self.state.lock()
    }

    /// Set TCP state
    pub fn set_state(&self, new_state: TcpState) {
        *self.state.lock() = new_state;
    }

    /// Send data through the socket
    fn send_data(&self, data: &[u8]) -> Result<usize, SocketError> {
        if self.get_state() != TcpState::Established {
            return Err(SocketError::NotConnected);
        }

        // Add to send buffer
        let mut buffer = self.send_buffer.lock();
        buffer.extend(data);
        
        // TODO: Actually send the data through TCP layer
        // For now, just return success
        Ok(data.len())
    }

    /// Receive data from the socket
    fn recv_data(&self, buffer: &mut [u8]) -> Result<usize, SocketError> {
        if self.get_state() != TcpState::Established {
            return Err(SocketError::NotConnected);
        }

        let mut recv_buf = self.recv_buffer.lock();
        let len = buffer.len().min(recv_buf.len());
        
        for i in 0..len {
            buffer[i] = recv_buf.pop_front().unwrap();
        }
        
        Ok(len)
    }

    /// Deliver received data to this socket
    pub fn deliver_data(&self, data: &[u8]) {
        let mut buffer = self.recv_buffer.lock();
        buffer.extend(data);
    }
}

impl SocketObject for TcpSocket {
    fn socket_type(&self) -> crate::network::socket::SocketType {
        crate::network::socket::SocketType::Stream
    }

    fn domain(&self) -> crate::network::socket::SocketDomain {
        crate::network::socket::SocketDomain::Inet
    }

    fn state(&self) -> SocketState {
        match self.get_state() {
            TcpState::Closed => SocketState::Unconnected,
            TcpState::Listen => SocketState::Listening,
            TcpState::Established => SocketState::Connected,
            _ => SocketState::Unconnected,
        }
    }
}

impl SocketControl for TcpSocket {
    fn bind(&self, address: &SocketAddress) -> Result<(), SocketError> {
        match address {
            SocketAddress::Inet { addr, port } => {
                *self.local_addr.lock() = Some(*addr);
                self.local_port.store(*port, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(SocketError::InvalidAddress),
        }
    }

    fn listen(&self, _backlog: usize) -> Result<(), SocketError> {
        if self.local_port.load(Ordering::SeqCst) == 0 {
            return Err(SocketError::NotBound);
        }
        self.set_state(TcpState::Listen);
        Ok(())
    }

    fn connect(&self, address: &SocketAddress) -> Result<(), SocketError> {
        match address {
            SocketAddress::Inet { addr, port } => {
                *self.remote_addr.lock() = Some(*addr);
                self.remote_port.store(*port, Ordering::SeqCst);
                
                // TODO: Implement TCP 3-way handshake
                // For now, just set state to established
                self.set_state(TcpState::Established);
                Ok(())
            }
            _ => Err(SocketError::InvalidAddress),
        }
    }

    fn accept(&self) -> Result<Arc<dyn SocketObject>, SocketError> {
        if self.get_state() != TcpState::Listen {
            return Err(SocketError::InvalidState);
        }
        
        // TODO: Implement actual accept from connection backlog
        Err(SocketError::WouldBlock)
    }

    fn shutdown(&self, _how: crate::network::socket::ShutdownMode) -> Result<(), SocketError> {
        // TODO: Implement proper TCP shutdown
        self.set_state(TcpState::Closed);
        Ok(())
    }

    fn local_address(&self) -> Option<SocketAddress> {
        let addr = self.local_addr.lock();
        let port = self.local_port.load(Ordering::SeqCst);
        addr.as_ref()
            .map(|a| SocketAddress::Inet { addr: *a, port })
    }

    fn remote_address(&self) -> Option<SocketAddress> {
        let addr = self.remote_addr.lock();
        let port = self.remote_port.load(Ordering::SeqCst);
        addr.as_ref()
            .map(|a| SocketAddress::Inet { addr: *a, port })
    }
}

impl ReadOps for TcpSocket {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, &'static str> {
        self.recv_data(buffer).map_err(|_| "Read failed")
    }
}

impl WriteOps for TcpSocket {
    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        self.send_data(buffer).map_err(|_| "Write failed")
    }

    fn flush(&self) -> Result<(), &'static str> {
        // TODO: Implement flush
        Ok(())
    }
}

/// TCP protocol layer
pub struct TcpLayer {
    /// Port-to-socket mapping for receiving packets
    port_map: SpinLock<alloc::collections::BTreeMap<u16, Weak<TcpSocket>>>,
}

impl TcpLayer {
    /// Create a new TCP layer
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            port_map: SpinLock::new(alloc::collections::BTreeMap::new()),
        })
    }

    /// Register a socket for a specific port
    pub fn register_port(&self, port: u16, socket: Weak<TcpSocket>) {
        let mut map = self.port_map.lock();
        map.insert(port, socket);
    }

    /// Unregister a socket from a port
    pub fn unregister_port(&self, port: u16) {
        let mut map = self.port_map.lock();
        map.remove(&port);
    }

    /// Create a new TCP socket
    pub fn create_socket(self: &Arc<Self>) -> Arc<TcpSocket> {
        Arc::new(TcpSocket::new(Arc::downgrade(self)))
    }
}

impl NetworkLayer for TcpLayer {
    fn send(
        &self,
        packet: &[u8],
        context: &LayerContext,
        _next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // Extract TCP information from context
        let _src_port = context.get_u16("tcp_src_port").ok_or(SocketError::NoRoute)?;
        let _dst_port = context.get_u16("tcp_dst_port").ok_or(SocketError::NoRoute)?;

        // TODO: Implement actual packet sending through IP layer
        // For now, just log that we received the packet
        crate::println!(
            "[TCP] Send packet: {} bytes (not actually sent yet)",
            packet.len()
        );

        Ok(())
    }

    fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
        // Parse TCP header
        let header = TcpHeader::from_bytes(packet).ok_or(SocketError::InvalidPacket)?;

        // Find the socket registered for this destination port
        let map = self.port_map.lock();
        if let Some(socket_weak) = map.get(&header.dest_port) {
            if let Some(socket) = socket_weak.upgrade() {
                // Deliver data to the socket
                let payload_offset = header.header_length();
                if packet.len() > payload_offset {
                    socket.deliver_data(&packet[payload_offset..]);
                }
                return Ok(());
            }
        }

        Err(SocketError::ProtocolNotSupported)
    }

    fn register_protocol(&self, _proto_num: u16, _handler: Arc<dyn NetworkLayer>) {
        // TCP is typically a leaf protocol, so this is usually not used
    }

    fn configure(
        &self,
        _config: &crate::network::protocol_stack::SocketConfig,
        _next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // TODO: Extract and register local port from config
        Ok(())
    }

    fn name(&self) -> &str {
        "TCP"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn stats(&self) -> crate::network::protocol_stack::NetworkLayerStats {
        crate::network::protocol_stack::NetworkLayerStats {
            packets_sent: 0,
            packets_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
            errors: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_tcp_header_serialization() {
        let mut header = TcpHeader::new(8080, 80);
        header.seq_number = 1000;
        header.ack_number = 2000;
        header.set_flags(tcp_flags::SYN | tcp_flags::ACK);

        let bytes = header.to_bytes();
        assert_eq!(bytes.len(), 20);

        let parsed = TcpHeader::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.source_port, 8080);
        assert_eq!(parsed.dest_port, 80);
        assert_eq!(parsed.seq_number, 1000);
        assert_eq!(parsed.ack_number, 2000);
        assert_eq!(parsed.flags(), tcp_flags::SYN | tcp_flags::ACK);
    }

    #[test_case]
    fn test_tcp_socket_creation() {
        let tcp_layer = TcpLayer::new();
        let socket = tcp_layer.create_socket();
        assert_eq!(socket.get_state(), TcpState::Closed);
    }

    #[test_case]
    fn test_tcp_socket_bind() {
        let tcp_layer = TcpLayer::new();
        let socket = tcp_layer.create_socket();

        let addr = SocketAddress::Inet {
            addr: Ipv4Address::new(127, 0, 0, 1),
            port: 8080,
        };

        assert!(socket.bind(&addr).is_ok());
        assert_eq!(socket.local_port.load(Ordering::SeqCst), 8080);
    }

    #[test_case]
    fn test_tcp_socket_state_transitions() {
        let tcp_layer = TcpLayer::new();
        let socket = tcp_layer.create_socket();

        assert_eq!(socket.get_state(), TcpState::Closed);

        socket.set_state(TcpState::Listen);
        assert_eq!(socket.get_state(), TcpState::Listen);

        socket.set_state(TcpState::Established);
        assert_eq!(socket.get_state(), TcpState::Established);
    }
}
