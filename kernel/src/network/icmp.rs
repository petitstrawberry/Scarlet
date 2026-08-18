//! ICMP protocol layer
//!
//! This module provides ICMP handling for network stack.
//! It implements NetworkLayer trait for ICMP messages.

use crate::sync::{IrqRwSpinLock, IrqSpinLock};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

use crate::early_println;

use crate::network::ipv4::Ipv4Address;
use crate::network::protocol_stack::get_network_manager;
use crate::network::protocol_stack::{LayerContext, NetworkLayer, NetworkLayerStats};
use crate::network::socket::SocketError;
use crate::network::socket::{
    Inet4SocketAddress, SocketAddress, SocketControl, SocketObject, SocketProtocol, SocketState,
    SocketType,
};

/// ICMP message types
pub mod message_type {
    /// Echo reply
    pub const ECHO_REPLY: u8 = 0;
    /// Destination unreachable
    pub const DESTINATION_UNREACHABLE: u8 = 3;
    /// Source quench
    pub const SOURCE_QUENCH: u8 = 4;
    /// Redirect
    pub const REDIRECT: u8 = 5;
    /// Echo request
    pub const ECHO_REQUEST: u8 = 8;
    /// Time exceeded
    pub const TIME_EXCEEDED: u8 = 11;
    /// Parameter problem
    pub const PARAMETER_PROBLEM: u8 = 12;
    /// Timestamp request
    pub const TIMESTAMP_REQUEST: u8 = 13;
    /// Timestamp reply
    pub const TIMESTAMP_REPLY: u8 = 14;
}

/// ICMP codes
pub mod code {
    /// No code
    pub const NO_CODE: u8 = 0;

    // Destination unreachable codes
    pub const NET_UNREACHABLE: u8 = 0;
    pub const HOST_UNREACHABLE: u8 = 1;
    pub const PROTOCOL_UNREACHABLE: u8 = 2;
    pub const PORT_UNREACHABLE: u8 = 3;
    pub const FRAGMENTATION_NEEDED: u8 = 4;
    pub const SOURCE_ROUTE_FAILED: u8 = 5;
}

/// ICMP header (4 bytes minimum)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct IcmpHeader {
    /// Message type
    pub message_type: u8,
    /// Message code
    pub code: u8,
    /// Checksum
    pub checksum: u16,
    /// Rest of header (varies by type)
    pub rest: [u8; 4],
}

impl IcmpHeader {
    /// Create a new ICMP header
    pub fn new(message_type: u8, code: u8) -> Self {
        Self {
            message_type,
            code,
            checksum: 0,
            rest: [0; 4],
        }
    }

    /// Calculate checksum
    pub fn calculate_checksum(&self, data: &[u8]) -> u16 {
        let mut sum: u32 = 0;

        // Header
        let header_bytes =
            unsafe { core::slice::from_raw_parts(self as *const IcmpHeader as *const u8, 8) };
        for chunk in header_bytes.chunks(2) {
            if chunk.len() == 2 {
                sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
            } else if chunk.len() == 1 {
                sum += (chunk[0] as u32) << 8;
            }
        }

        // Data
        for chunk in data.chunks(2) {
            if chunk.len() == 2 {
                sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
            } else if chunk.len() == 1 {
                sum += (chunk[0] as u32) << 8;
            }
        }

        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }

        !sum as u16
    }

    /// Serialize header to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8);
        bytes.push(self.message_type);
        bytes.push(self.code);
        bytes.extend_from_slice(&self.checksum.to_be_bytes());
        bytes.extend_from_slice(&self.rest);
        bytes
    }

    /// Parse header from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 {
            return None;
        }

        Some(Self {
            message_type: bytes[0],
            code: bytes[1],
            checksum: u16::from_be_bytes([bytes[2], bytes[3]]),
            rest: [bytes[4], bytes[5], bytes[6], bytes[7]],
        })
    }
}

/// ICMP Echo request/reply header
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct IcmpEcho {
    /// Identifier
    pub identifier: u16,
    /// Sequence number
    pub sequence: u16,
}

impl IcmpEcho {
    /// Create a new ICMP Echo header
    pub fn new(identifier: u16, sequence: u16) -> Self {
        Self {
            identifier,
            sequence,
        }
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> [u8; 4] {
        let mut bytes = [0u8; 4];
        bytes[0..2].copy_from_slice(&self.identifier.to_be_bytes());
        bytes[2..4].copy_from_slice(&self.sequence.to_be_bytes());
        bytes
    }

    /// Parse from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }

        Some(Self {
            identifier: u16::from_be_bytes([bytes[0], bytes[1]]),
            sequence: u16::from_be_bytes([bytes[2], bytes[3]]),
        })
    }
}

/// ICMP layer
///
/// Handles ICMP messages for network diagnostics.
pub struct IcmpLayer {
    /// Statistics
    stats: IrqRwSpinLock<NetworkLayerStats>,
    /// ICMP sockets by identifier
    sockets: IrqRwSpinLock<BTreeMap<u16, Weak<IcmpSocket>>>,
    /// Identifier allocator
    next_identifier: AtomicU16,
    self_weak: Weak<IcmpLayer>,
}

impl IcmpLayer {
    fn compute_checksum(packet: &[u8]) -> u16 {
        let mut sum: u32 = 0;
        let mut chunks = packet.chunks_exact(2);
        for chunk in &mut chunks {
            sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
        }
        if let Some(&byte) = chunks.remainder().first() {
            sum += (byte as u32) << 8;
        }
        while (sum >> 16) != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        !(sum as u16)
    }

    fn verify_checksum(packet: &[u8]) -> bool {
        if packet.len() < 4 {
            return false;
        }
        let expected = u16::from_be_bytes([packet[2], packet[3]]);
        let mut check = packet.to_vec();
        check[2] = 0;
        check[3] = 0;
        Self::compute_checksum(&check) == expected
    }

    /// Create a new ICMP layer
    pub fn new() -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            stats: IrqRwSpinLock::new(NetworkLayerStats::default()),
            sockets: IrqRwSpinLock::new(BTreeMap::new()),
            next_identifier: AtomicU16::new(1),
            self_weak: weak.clone(),
        })
    }

    /// Initialize and register the ICMP layer with NetworkManager
    ///
    /// Registers with NetworkManager and registers itself with Ipv4Layer
    /// for protocol number 1 (ICMP).
    ///
    /// # Panics
    ///
    /// Panics if Ipv4Layer is not registered (must be initialized first).
    pub fn init(network_manager: &crate::network::NetworkManager) {
        let layer = Self::new();
        network_manager.register_layer("icmp", layer.clone());

        // Register with IPv4 layer for ICMP packets (protocol 1)
        let ipv4 = network_manager
            .get_layer("ip")
            .expect("Ipv4Layer must be initialized before IcmpLayer");
        ipv4.register_protocol(crate::network::ipv4::protocol::ICMP as u16, layer);
    }

    pub fn create_socket(&self) -> Arc<IcmpSocket> {
        let identifier = self.next_identifier.fetch_add(1, Ordering::SeqCst);
        let socket = IcmpSocket::new(self.self_weak.clone(), identifier);
        self.sockets
            .write()
            .insert(identifier, Arc::downgrade(&socket));
        socket
    }

    /// Send an ICMP Echo Request (ping)
    pub fn send_ping_request(
        &self,
        dest_ip: Ipv4Address,
        identifier: u16,
        sequence: u16,
        data: &[u8],
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // Build ICMP Echo Request header
        let echo = IcmpEcho::new(identifier, sequence);
        let mut header = IcmpHeader::new(message_type::ECHO_REQUEST, code::NO_CODE);
        header.rest = echo.to_bytes();

        // Calculate checksum
        let mut icmp_packet = Vec::with_capacity(8 + data.len());
        header.checksum = 0;
        icmp_packet.extend_from_slice(&header.to_bytes());
        icmp_packet.extend_from_slice(data);
        let checksum = Self::compute_checksum(&icmp_packet);
        icmp_packet[2] = (checksum >> 8) as u8;
        icmp_packet[3] = checksum as u8;

        // Create IP context
        let mut ip_context = LayerContext::new();
        ip_context.set("ip_dst", &dest_ip.0);
        ip_context.set("ip_protocol", &[1]); // ICMP protocol

        let dest_ip_bytes = dest_ip.0;
        early_println!(
            "[ICMP] Ping {}.{}.{}.{} (id={}, seq={}, data_len={})",
            dest_ip_bytes[0],
            dest_ip_bytes[1],
            dest_ip_bytes[2],
            dest_ip_bytes[3],
            identifier,
            sequence,
            data.len()
        );

        // Send through IP layer
        if !next_layers.is_empty() {
            next_layers[0].send(&icmp_packet, &ip_context, &next_layers[1..])?;
        } else if let Some(ip_layer) = get_network_manager().get_layer("ip") {
            ip_layer.send(&icmp_packet, &ip_context, &[])?;

            // Update statistics
            let mut stats = self.stats.write();
            stats.packets_sent += 1;
            stats.bytes_sent += icmp_packet.len() as u64;
        } else {
            return Err(SocketError::NoRoute);
        }

        Ok(())
    }

    /// Send an ICMP Echo Reply
    pub fn send_ping_reply(
        &self,
        dest_ip: Ipv4Address,
        identifier: u16,
        sequence: u16,
        data: &[u8],
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        self.send_ping_reply_from(None, dest_ip, identifier, sequence, data, next_layers)
    }

    fn send_ping_reply_from(
        &self,
        source_ip: Option<Ipv4Address>,
        dest_ip: Ipv4Address,
        identifier: u16,
        sequence: u16,
        data: &[u8],
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // Build ICMP Echo Reply header
        let echo = IcmpEcho::new(identifier, sequence);
        let mut header = IcmpHeader::new(message_type::ECHO_REPLY, code::NO_CODE);
        header.rest = echo.to_bytes();

        // Calculate checksum
        let mut icmp_packet = Vec::with_capacity(8 + data.len());
        header.checksum = 0;
        icmp_packet.extend_from_slice(&header.to_bytes());
        icmp_packet.extend_from_slice(data);
        let checksum = Self::compute_checksum(&icmp_packet);
        icmp_packet[2] = (checksum >> 8) as u8;
        icmp_packet[3] = checksum as u8;

        // Create IP context
        let mut ip_context = LayerContext::new();
        ip_context.set("ip_dst", &dest_ip.0);
        ip_context.set("ip_protocol", &[1]); // ICMP protocol
        if let Some(source_ip) = source_ip {
            ip_context.set("ip_src", &source_ip.0);
        }

        let dest_ip_bytes = dest_ip.0;
        early_println!(
            "[ICMP] Pong {}.{}.{}.{} (id={}, seq={}, data_len={})",
            dest_ip_bytes[0],
            dest_ip_bytes[1],
            dest_ip_bytes[2],
            dest_ip_bytes[3],
            identifier,
            sequence,
            data.len()
        );

        // Send through IP layer
        if !next_layers.is_empty() {
            next_layers[0].send(&icmp_packet, &ip_context, &next_layers[1..])?;
        } else if let Some(ip_layer) = get_network_manager().get_layer("ip") {
            ip_layer.send(&icmp_packet, &ip_context, &[])?;

            // Update statistics
            let mut stats = self.stats.write();
            stats.packets_sent += 1;
            stats.bytes_sent += icmp_packet.len() as u64;
        } else {
            return Err(SocketError::NoRoute);
        }

        Ok(())
    }

    /// Process received ICMP packet
    pub fn receive_packet(
        &self,
        packet: &[u8],
        src_ip: Ipv4Address,
        dst_ip: Ipv4Address,
    ) -> Result<(), SocketError> {
        if packet.len() < 8 {
            return Err(SocketError::InvalidPacket);
        }

        if !Self::verify_checksum(packet) {
            let mut stats = self.stats.write();
            stats.protocol_errors += 1;
            return Err(SocketError::InvalidPacket);
        }

        early_println!(
            "[ICMP] RX: {} bytes src={}.{}.{}.{} dst={}.{}.{}.{}",
            packet.len(),
            src_ip.0[0],
            src_ip.0[1],
            src_ip.0[2],
            src_ip.0[3],
            dst_ip.0[0],
            dst_ip.0[1],
            dst_ip.0[2],
            dst_ip.0[3]
        );

        // Parse ICMP header
        let header = IcmpHeader::from_bytes(&packet[..8]).ok_or(SocketError::InvalidPacket)?;

        let data = &packet[8..];

        early_println!(
            "[ICMP] Recv: type={}, code={}, len={}",
            header.message_type,
            header.code,
            packet.len()
        );

        // Update statistics
        {
            let mut stats = self.stats.write();
            stats.packets_received += 1;
            stats.bytes_received += packet.len() as u64;
        }

        match header.message_type {
            message_type::ECHO_REQUEST => {
                if header.code != code::NO_CODE {
                    return Ok(());
                }
                // Handle ping request - send reply
                let identifier = u16::from_be_bytes([header.rest[0], header.rest[1]]);
                let sequence = u16::from_be_bytes([header.rest[2], header.rest[3]]);
                early_println!(
                    "[ICMP] Ping request from (id={}, seq={})",
                    identifier,
                    sequence
                );

                if let Some(ip_layer) = get_network_manager().get_layer("ip") {
                    let _ = self.send_ping_reply_from(
                        Some(dst_ip),
                        src_ip,
                        identifier,
                        sequence,
                        data,
                        &[ip_layer],
                    );
                }
            }
            message_type::ECHO_REPLY => {
                if header.code != code::NO_CODE {
                    return Ok(());
                }
                let identifier = u16::from_be_bytes([header.rest[0], header.rest[1]]);
                let sequence = u16::from_be_bytes([header.rest[2], header.rest[3]]);
                let payload = data.to_vec();
                self.deliver_echo_reply(identifier, payload, src_ip, sequence);
            }
            _ => {}
        }

        Ok(())
    }

    fn deliver_echo_reply(
        &self,
        identifier: u16,
        payload: Vec<u8>,
        src_ip: Ipv4Address,
        sequence: u16,
    ) {
        if let Some(socket) = self
            .sockets
            .read()
            .get(&identifier)
            .and_then(|weak| weak.upgrade())
        {
            socket.deliver_reply(payload, src_ip, sequence);
        }
    }
}

pub struct IcmpSocket {
    icmp_layer: Weak<IcmpLayer>,
    identifier: u16,
    sequence: AtomicU16,
    expected_sequence: AtomicU16,
    local_addr: IrqSpinLock<Option<SocketAddress>>,
    remote_addr: IrqRwSpinLock<Option<SocketAddress>>,
    recv_queue: IrqSpinLock<VecDeque<(Vec<u8>, SocketAddress)>>,
    recv_waker: crate::sync::waker::Waker,
    nonblocking: IrqRwSpinLock<bool>,
}

impl IcmpSocket {
    fn new(icmp_layer: Weak<IcmpLayer>, identifier: u16) -> Arc<Self> {
        Arc::new(Self {
            icmp_layer,
            identifier,
            sequence: AtomicU16::new(0),
            expected_sequence: AtomicU16::new(0),
            local_addr: IrqSpinLock::new(None),
            remote_addr: IrqRwSpinLock::new(None),
            recv_queue: IrqSpinLock::new(VecDeque::new()),
            recv_waker: crate::sync::waker::Waker::new_interruptible("icmp_recv"),
            nonblocking: IrqRwSpinLock::new(false),
        })
    }

    fn deliver_reply(&self, payload: Vec<u8>, src_ip: Ipv4Address, sequence: u16) {
        let expected = self.expected_sequence.load(Ordering::SeqCst);
        if sequence != expected {
            return;
        }
        let addr = SocketAddress::Inet(Inet4SocketAddress::new(src_ip.0, 0));
        self.recv_queue.lock().push_back((payload, addr));
        self.recv_waker.wake_one();
    }
}

impl Drop for IcmpSocket {
    fn drop(&mut self) {
        if let Some(layer) = self.icmp_layer.upgrade() {
            let mut sockets = layer.sockets.write();
            if let Some(existing) = sockets.get(&self.identifier)
                && existing.as_ptr() == self as *const Self
            {
                sockets.remove(&self.identifier);
            }
        }

        self.recv_waker.wake_all();
        crate::network::NetworkManager::get_manager()
            .remove_socket_by_ptr(self as *const Self as usize);
    }
}

impl SocketObject for IcmpSocket {
    fn socket_type(&self) -> SocketType {
        SocketType::Datagram
    }

    fn socket_domain(&self) -> crate::network::socket::SocketDomain {
        crate::network::socket::SocketDomain::Inet4
    }

    fn socket_protocol(&self) -> SocketProtocol {
        SocketProtocol::Icmp
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_control_ops(&self) -> Option<&dyn crate::object::capability::ControlOps> {
        Some(self)
    }

    fn sendto(
        &self,
        data: &[u8],
        address: &SocketAddress,
        _flags: u32,
    ) -> Result<usize, SocketError> {
        let target = match address {
            SocketAddress::Inet(inet) => *inet,
            SocketAddress::Unspecified => match self.remote_addr.read().clone() {
                Some(SocketAddress::Inet(inet)) => inet,
                _ => return Err(SocketError::InvalidAddress),
            },
            _ => return Err(SocketError::InvalidAddress),
        };

        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst);
        self.expected_sequence.store(sequence, Ordering::SeqCst);
        let dest_ip = Ipv4Address::from_bytes(target.addr);

        if let Some(ip_layer) = get_network_manager().get_layer("ip") {
            if let Some(ipv4) = ip_layer
                .as_any()
                .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
            {
                // Route selection also chooses the correct source interface.
                let has_ip = ipv4.select_source(dest_ip).is_some();
                if !has_ip {
                    early_println!("[ICMP] send blocked: local IP unset");
                    return Err(SocketError::NotConnected);
                }
            }
            if let Some(icmp_layer) = self.icmp_layer.upgrade() {
                match icmp_layer.send_ping_request(
                    dest_ip,
                    self.identifier,
                    sequence,
                    data,
                    &[ip_layer],
                ) {
                    Ok(()) => return Ok(data.len()),
                    Err(SocketError::WouldBlock) => {
                        // Packet queued for ARP resolution - treat as success
                        return Ok(data.len());
                    }
                    Err(e) => return Err(e),
                }
            } else {
                early_println!("[ICMP] send failed: ICMP layer unavailable");
            }
        } else {
            early_println!("[ICMP] send failed: IP layer unavailable");
        }

        Err(SocketError::NoRoute)
    }

    fn recvfrom(
        &self,
        buffer: &mut [u8],
        _flags: u32,
    ) -> Result<(usize, SocketAddress), SocketError> {
        use crate::task::mytask;

        loop {
            if let Some((data, addr)) = self.recv_queue.lock().pop_front() {
                let len = buffer.len().min(data.len());
                buffer[..len].copy_from_slice(&data[..len]);
                return Ok((len, addr));
            }

            if *self.nonblocking.read() {
                return Err(SocketError::WouldBlock);
            }

            if let Some(task) = mytask() {
                self.recv_waker.wait(task.get_id(), task.get_trapframe());
            } else {
                return Err(SocketError::WouldBlock);
            }
        }
    }
}

impl crate::object::capability::ControlOps for IcmpSocket {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            crate::network::socket::socket_ctl::SCTL_SOCKET_SET_NONBLOCK => {
                *self.nonblocking.write() = arg != 0;
                Ok(0)
            }
            crate::network::socket::socket_ctl::SCTL_SOCKET_GET_NONBLOCK => {
                Ok(if *self.nonblocking.read() { 1 } else { 0 })
            }
            _ => Err("Unsupported socket control command"),
        }
    }

    fn supported_control_commands(&self) -> alloc::vec::Vec<(u32, &'static str)> {
        alloc::vec![
            (
                crate::network::socket::socket_ctl::SCTL_SOCKET_SET_NONBLOCK,
                "Set non-blocking mode",
            ),
            (
                crate::network::socket::socket_ctl::SCTL_SOCKET_GET_NONBLOCK,
                "Get non-blocking mode",
            ),
        ]
    }
}

impl SocketControl for IcmpSocket {
    fn bind(&self, address: &SocketAddress) -> Result<(), SocketError> {
        match address {
            SocketAddress::Inet(_) => {
                *self.local_addr.lock() = Some(address.clone());
                Ok(())
            }
            SocketAddress::Unspecified => Ok(()),
            _ => Err(SocketError::InvalidAddress),
        }
    }

    fn connect(&self, address: &SocketAddress) -> Result<(), SocketError> {
        match address {
            SocketAddress::Inet(_) => {
                *self.remote_addr.write() = Some(address.clone());
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
            .lock()
            .clone()
            .ok_or(SocketError::InvalidAddress)
    }

    fn shutdown(&self, _how: crate::network::socket::ShutdownHow) -> Result<(), SocketError> {
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.remote_addr.read().is_some()
    }

    fn state(&self) -> SocketState {
        if self.is_connected() {
            SocketState::Connected
        } else {
            SocketState::Unconnected
        }
    }
}

impl crate::ipc::StreamIpcOps for IcmpSocket {
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
        alloc::format!("ICMP socket")
    }
}

impl crate::object::capability::StreamOps for IcmpSocket {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, crate::object::capability::StreamError> {
        let (len, _) = self.recvfrom(buffer, 0).map_err(|err| {
            crate::object::capability::StreamError::Other(format!("icmp recv error: {:?}", err))
        })?;
        Ok(len)
    }

    fn write(&self, data: &[u8]) -> Result<usize, crate::object::capability::StreamError> {
        let addr = self.remote_addr.read().clone().ok_or_else(|| {
            crate::object::capability::StreamError::Other("icmp not connected".into())
        })?;
        self.sendto(data, &addr, 0).map_err(|err| {
            crate::object::capability::StreamError::Other(format!("icmp send error: {:?}", err))
        })?;
        Ok(data.len())
    }
}

impl NetworkLayer for IcmpLayer {
    fn register_protocol(&self, _proto_num: u16, _handler: Arc<dyn NetworkLayer>) {
        // ICMP is typically a leaf protocol
    }

    fn send(
        &self,
        _packet: &[u8],
        _context: &LayerContext,
        _next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // ICMP send is handled through specific methods
        Ok(())
    }

    fn receive(&self, _packet: &[u8], _context: Option<&LayerContext>) -> Result<(), SocketError> {
        Ok(())
    }

    fn name(&self) -> &'static str {
        "ICMP"
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
    fn test_icmp_header_creation() {
        let header = IcmpHeader::new(message_type::ECHO_REQUEST, code::NO_CODE);

        assert_eq!(header.message_type, message_type::ECHO_REQUEST);
        assert_eq!(header.code, code::NO_CODE);
        assert_eq!(header.rest, [0, 0, 0, 0]);
    }

    #[test_case]
    fn test_icmp_echo_header() {
        let echo = IcmpEcho::new(1234, 5678);

        let identifier = unsafe { core::ptr::addr_of!(echo.identifier).read_unaligned() };
        let sequence = unsafe { core::ptr::addr_of!(echo.sequence).read_unaligned() };
        assert_eq!(identifier, 1234);
        assert_eq!(sequence, 5678);
    }

    #[test_case]
    fn test_icmp_echo_serialization() {
        let echo = IcmpEcho::new(1234, 5678);
        let bytes = echo.to_bytes();

        assert_eq!(bytes.len(), 4);
        assert_eq!(u16::from_be_bytes([bytes[0], bytes[1]]), 1234);
        assert_eq!(u16::from_be_bytes([bytes[2], bytes[3]]), 5678);
    }

    #[test_case]
    fn test_icmp_echo_parsing() {
        let mut bytes = [0u8; 4];
        bytes[0..2].copy_from_slice(&1234u16.to_be_bytes());
        bytes[2..4].copy_from_slice(&5678u16.to_be_bytes());

        let echo = IcmpEcho::from_bytes(&bytes).unwrap();

        let identifier = unsafe { core::ptr::addr_of!(echo.identifier).read_unaligned() };
        let sequence = unsafe { core::ptr::addr_of!(echo.sequence).read_unaligned() };
        assert_eq!(identifier, 1234);
        assert_eq!(sequence, 5678);
    }

    #[test_case]
    fn test_icmp_header_parsing() {
        let mut bytes = [0u8; 8];
        bytes[0] = message_type::ECHO_REQUEST;
        bytes[1] = code::NO_CODE;
        bytes[2..4].copy_from_slice(&0x1234u16.to_be_bytes()); // Checksum
        bytes[4..8].copy_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Rest

        let header = IcmpHeader::from_bytes(&bytes).unwrap();

        assert_eq!(header.message_type, message_type::ECHO_REQUEST);
        assert_eq!(header.code, code::NO_CODE);
        assert_eq!(header.rest, [0, 0, 0, 0]);
    }

    #[test_case]
    fn test_icmp_header_too_short() {
        let bytes = [0u8; 4];
        assert!(IcmpHeader::from_bytes(&bytes).is_none());
    }

    #[test_case]
    fn test_icmp_echo_checksum_known_vector() {
        let mut header = IcmpHeader::new(message_type::ECHO_REQUEST, code::NO_CODE);
        header.rest = IcmpEcho::new(0x1234, 0x0001).to_bytes();
        let data = b"scarlet";

        let mut packet = header.to_bytes();
        packet.extend_from_slice(data);
        header.checksum = IcmpLayer::compute_checksum(&packet);

        let mut checked = header.to_bytes();
        checked.extend_from_slice(data);
        assert!(IcmpLayer::verify_checksum(&checked));
    }

    #[test_case]
    fn test_message_type_constants() {
        assert_eq!(message_type::ECHO_REPLY, 0);
        assert_eq!(message_type::DESTINATION_UNREACHABLE, 3);
        assert_eq!(message_type::ECHO_REQUEST, 8);
        assert_eq!(message_type::TIME_EXCEEDED, 11);
    }

    #[test_case]
    fn test_code_constants() {
        assert_eq!(code::NO_CODE, 0);
        assert_eq!(code::NET_UNREACHABLE, 0);
        assert_eq!(code::HOST_UNREACHABLE, 1);
        assert_eq!(code::PORT_UNREACHABLE, 3);
    }
}
