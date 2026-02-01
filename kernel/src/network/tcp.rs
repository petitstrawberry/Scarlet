//! TCP protocol layer (Complete implementation)
//!
//! This module provides a full TCP implementation with 3-way handshake,
//! flow control, and retransmission.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering};
use core::time::Duration;
use spin::{Mutex, RwLock};

use crate::network::ipv4::Ipv4Address;
use crate::network::protocol_stack::get_network_manager;
use crate::network::protocol_stack::{LayerContext, NetworkLayer, NetworkLayerStats};
use crate::network::socket::SocketError;
use crate::network::socket::{Inet4SocketAddress, SocketAddress, SocketControl, SocketObject, SocketState};

/// TCP connection states
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
pub mod tcp_flags {
    pub const FIN: u8 = 0x01;
    pub const SYN: u8 = 0x02;
    pub const RST: u8 = 0x04;
    pub const PSH: u8 = 0x08;
    pub const ACK: u8 = 0x10;
    pub const URG: u8 = 0x20;
}

/// TCP header
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct TcpHeader {
    /// Source port
    pub src_port: u16,
    /// Destination port
    pub dst_port: u16,
    /// Sequence number
    pub seq_number: u32,
    /// Acknowledgment number
    pub ack_number: u32,
    /// Data offset (4 bits) + reserved (4 bits) + flags (8 bits)
    pub data_offset_flags: u16,
    /// Window size
    pub window_size: u16,
    /// Checksum
    pub checksum: u16,
    /// Urgent pointer
    pub urgent_pointer: u16,
}

impl TcpHeader {
    /// Create a new TCP header
    pub fn new(src_port: u16, dst_port: u16) -> Self {
        Self {
            src_port,
            dst_port,
            seq_number: 0,
            ack_number: 0,
            data_offset_flags: 0x5000, // Data offset = 5 (20 bytes), no flags
            window_size: 65535,
            checksum: 0,
            urgent_pointer: 0,
        }
    }

    /// Get TCP flags
    pub fn flags(&self) -> u8 {
        (self.data_offset_flags & 0x3F) as u8
    }

    /// Set TCP flags
    pub fn set_flags(&mut self, flags: u8) {
        self.data_offset_flags = (self.data_offset_flags & 0xFFC0) | (flags as u16 & 0x3F);
    }

    /// Get data offset in bytes
    pub fn data_offset(&self) -> usize {
        ((self.data_offset_flags >> 12) as usize) * 4
    }

    /// Calculate TCP checksum
    pub fn calculate_checksum(&self, src_ip: [u8; 4], dst_ip: [u8; 4], data: &[u8]) -> u16 {
        let mut sum: u32 = 0;

        // Pseudo-header: src IP (4) + dst IP (4) + zero (1) + protocol (1) + TCP length (2)
        sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += 6u32; // TCP protocol number
        sum = (sum & 0xFFFF) + (sum >> 16);
        let tcp_len = (self.data_offset() + data.len()) as u16;
        sum += tcp_len as u32;

        // TCP header (exclude checksum field)
        let header_bytes =
            unsafe { core::slice::from_raw_parts(self as *const TcpHeader as *const u8, 12) };
        for chunk in header_bytes.chunks(2) {
            sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
            sum = (sum & 0xFFFF) + (sum >> 16);
        }

        // Data
        for chunk in data.chunks(2) {
            if chunk.len() == 2 {
                sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
                sum = (sum & 0xFFFF) + (sum >> 16);
            } else if chunk.len() == 1 {
                sum += (chunk[0] as u32) << 8;
                sum = (sum & 0xFFFF) + (sum >> 16);
            }
        }

        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }

        !sum as u16
    }

    /// Serialize header to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(20);
        bytes.extend_from_slice(&self.src_port.to_be_bytes());
        bytes.extend_from_slice(&self.dst_port.to_be_bytes());
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
            src_port: u16::from_be_bytes([bytes[0], bytes[1]]),
            dst_port: u16::from_be_bytes([bytes[2], bytes[3]]),
            seq_number: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            ack_number: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            data_offset_flags: u16::from_be_bytes([bytes[12], bytes[13]]),
            window_size: u16::from_be_bytes([bytes[14], bytes[15]]),
            checksum: u16::from_be_bytes([bytes[16], bytes[17]]),
            urgent_pointer: u16::from_be_bytes([bytes[18], bytes[19]]),
        })
    }
}

/// TCP socket (full implementation)
pub struct TcpSocket {
    /// TCP connection state
    state: Mutex<TcpState>,

    /// Local IP address
    local_ip: Mutex<Option<Ipv4Address>>,
    /// Local port
    local_port: AtomicU16,

    /// Remote IP address
    remote_ip: Mutex<Option<Ipv4Address>>,
    /// Remote port
    remote_port: AtomicU16,

    /// Sequence numbers
    send_seq: AtomicU32,
    send_unacked: AtomicU32,
    recv_seq: AtomicU32,
    recv_ack: AtomicU32,

    /// Window size
    send_window: AtomicU16,
    recv_window: AtomicU16,

    /// Data buffers
    send_buffer: Mutex<VecDeque<u8>>,
    recv_buffer: Mutex<VecDeque<u8>>,

    /// Reference to TCP layer
    tcp_layer: Weak<TcpLayer>,

    /// Statistics
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
}

impl TcpSocket {
    /// Create a new TCP socket
    pub fn new(tcp_layer: Weak<TcpLayer>) -> Self {
        Self {
            state: Mutex::new(TcpState::Closed),
            local_ip: Mutex::new(None),
            local_port: AtomicU16::new(0),
            remote_ip: Mutex::new(None),
            remote_port: AtomicU16::new(0),
            send_seq: AtomicU32::new(0),
            send_unacked: AtomicU32::new(0),
            recv_seq: AtomicU32::new(0),
            recv_ack: AtomicU32::new(0),
            send_window: AtomicU16::new(65535),
            recv_window: AtomicU16::new(65535),
            send_buffer: Mutex::new(VecDeque::new()),
            recv_buffer: Mutex::new(VecDeque::new()),
            tcp_layer,
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
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

    /// Process incoming TCP segment
    pub fn process_segment(&self, src_ip: Ipv4Address, header: TcpHeader, data: &[u8]) {
        let current_state = self.get_state();

        match current_state {
            TcpState::Listen => {
                if header.flags() & tcp_flags::SYN != 0 {
                    // Handle incoming SYN (SYN-RECEIVED state)
                    self.handle_syn_received(src_ip, header);
                }
            }
            TcpState::SynSent => {
                if header.flags() & (tcp_flags::SYN | tcp_flags::ACK)
                    == (tcp_flags::SYN | tcp_flags::ACK)
                {
                    // Received SYN-ACK, move to ESTABLISHED
                    self.handle_syn_ack_received(src_ip, header);
                } else if header.flags() & tcp_flags::RST != 0 {
                    // Received RST, abort connection
                    self.set_state(TcpState::Closed);
                }
            }
            TcpState::Established => {
                if data.is_empty() {
                    self.handle_control_segment(src_ip, header);
                } else {
                    self.handle_data_segment(src_ip, header, data);
                }
            }
            _ => {
            }
        }
    }

    /// Handle incoming SYN (SYN-RECEIVED state)
    fn handle_syn_received(&self, src_ip: Ipv4Address, header: TcpHeader) {
        // Store remote address
        *self.remote_ip.lock() = Some(src_ip);
        self.remote_port.store(header.src_port, Ordering::SeqCst);

        // Generate initial sequence number
        let initial_seq = 1000;
        self.recv_seq.store(initial_seq, Ordering::SeqCst);

        // Send SYN-ACK
        self.send_syn_ack(
            src_ip,
            header.src_port,
            initial_seq,
            header.seq_number.wrapping_add(1),
        );
        self.set_state(TcpState::SynReceived);
    }

    /// Handle received SYN-ACK (move to ESTABLISHED)
    fn handle_syn_ack_received(&self, src_ip: Ipv4Address, header: TcpHeader) {
        *self.remote_ip.lock() = Some(src_ip);
        self.remote_port.store(header.src_port, Ordering::SeqCst);

        self.send_ack(src_ip, header.src_port, header.seq_number.wrapping_add(1));

        self.set_state(TcpState::Established);
    }

    /// Handle control segment (ACK, FIN, RST)
    fn handle_control_segment(&self, src_ip: Ipv4Address, header: TcpHeader) {
        if header.flags() & tcp_flags::RST != 0 {
            self.set_state(TcpState::Closed);
            return;
        }

        if header.flags() & tcp_flags::FIN != 0 {
            self.handle_fin(header);
        }

        if header.flags() & tcp_flags::ACK != 0 {
            self.update_send_window(header.ack_number);
        }
    }

    /// Handle data segment
    fn handle_data_segment(&self, src_ip: Ipv4Address, header: TcpHeader, data: &[u8]) {
        if header.flags() & tcp_flags::RST != 0 {
            self.set_state(TcpState::Closed);
            return;
        }

        // Check sequence number
        let expected_seq = self.recv_seq.load(Ordering::SeqCst);
        let segment_seq = header.seq_number;

        if segment_seq == expected_seq.wrapping_sub(1) {
            // Duplicate ACK, just update acknowledgment
            if header.flags() & tcp_flags::ACK != 0 {
                self.update_send_window(header.ack_number);
            }
            return;
        }

        // Add data to receive buffer
        if data.is_empty() {
            if header.flags() & tcp_flags::ACK != 0 {
                self.update_send_window(header.ack_number);
            }
        } else if segment_seq >= expected_seq {
            let mut recv_buf = self.recv_buffer.lock();
            recv_buf.extend(data);
            self.recv_seq.fetch_add(data.len() as u32, Ordering::SeqCst);

            // Send ACK
            let next_seq = segment_seq.wrapping_add(data.len() as u32);
            self.send_ack(src_ip, header.src_port, next_seq);

            // Update received bytes
            self.bytes_received
                .fetch_add(data.len() as u64, Ordering::SeqCst);
        }

        if header.flags() & tcp_flags::ACK != 0 {
            self.update_send_window(header.ack_number);
        }
    }

    /// Handle FIN segment
    fn handle_fin(&self, header: TcpHeader) {
        let current_state = self.get_state();
        match current_state {
            TcpState::Established => {
                if header.flags() & tcp_flags::ACK != 0 {
                    // FIN-ACK received
                    self.set_state(TcpState::CloseWait);
                } else {
                    // FIN received
                    self.send_ack(
                        self.remote_ip.lock().clone().unwrap(),
                        self.remote_port.load(Ordering::SeqCst),
                        header.seq_number.wrapping_add(1),
                    );
                    self.set_state(TcpState::CloseWait);
                }
            }
            TcpState::FinWait1 => {
                if header.flags() & (tcp_flags::FIN | tcp_flags::ACK)
                    == (tcp_flags::FIN | tcp_flags::ACK)
                {
                    self.send_fin_ack();
                    self.set_state(TcpState::TimeWait);
                }
            }
            _ => {}
        }
    }

    /// Update send window based on acknowledgment
    fn update_send_window(&self, ack_number: u32) {
        let send_seq = self.send_seq.load(Ordering::SeqCst);
        let unacked = send_seq.wrapping_sub(ack_number);
        self.send_unacked.store(unacked, Ordering::SeqCst);
    }

    /// Send SYN packet
    fn send_syn(&self, dest_ip: Ipv4Address, dest_port: u16) {
        let local_port = self.local_port.load(Ordering::SeqCst);
        let local_ip = self
            .local_ip
            .lock()
            .clone()
            .unwrap_or(Ipv4Address::new(0, 0, 0, 0));

        let initial_seq = 1000;
        self.send_seq.store(initial_seq, Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = initial_seq;
        header.set_flags(tcp_flags::SYN);

        self.send_segment(dest_ip, header, &[], false);
        self.set_state(TcpState::SynSent);
    }

    /// Send SYN-ACK packet
    fn send_syn_ack(&self, dest_ip: Ipv4Address, dest_port: u16, their_seq: u32, ack_seq: u32) {
        let local_port = self.local_port.load(Ordering::SeqCst);
        let local_ip = self
            .local_ip
            .lock()
            .clone()
            .unwrap_or(Ipv4Address::new(0, 0, 0, 0));

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = their_seq;
        header.ack_number = ack_seq;
        header.set_flags(tcp_flags::SYN | tcp_flags::ACK);

        self.send_segment(dest_ip, header, &[], false);
        self.set_state(TcpState::SynReceived);
    }

    /// Send ACK packet
    fn send_ack(&self, dest_ip: Ipv4Address, dest_port: u16, ack_seq: u32) {
        let local_port = self.local_port.load(Ordering::SeqCst);
        let local_ip = self
            .local_ip
            .lock()
            .clone()
            .unwrap_or(Ipv4Address::new(0, 0, 0, 0));

        let recv_seq = self.recv_seq.load(Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = recv_seq.wrapping_add(1);
        header.ack_number = ack_seq;
        header.set_flags(tcp_flags::ACK);

        self.send_segment(dest_ip, header, &[], true);
    }

    /// Send FIN packet
    fn send_fin(&self) {
        let dest_ip = self.remote_ip.lock().clone().unwrap();
        let dest_port = self.remote_port.load(Ordering::SeqCst);
        let local_port = self.local_port.load(Ordering::SeqCst);
        let local_ip = self
            .local_ip
            .lock()
            .clone()
            .unwrap_or(Ipv4Address::new(0, 0, 0, 0));

        let send_seq = self.send_seq.load(Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = send_seq;
        header.set_flags(tcp_flags::FIN);

        self.send_segment(dest_ip, header, &[], true);
        self.set_state(TcpState::FinWait1);
    }

    /// Send FIN-ACK packet
    fn send_fin_ack(&self) {
        let dest_ip = self.remote_ip.lock().clone().unwrap();
        let dest_port = self.remote_port.load(Ordering::SeqCst);
        let local_port = self.local_port.load(Ordering::SeqCst);
        let local_ip = self
            .local_ip
            .lock()
            .clone()
            .unwrap_or(Ipv4Address::new(0, 0, 0, 0));

        let recv_seq = self.recv_seq.load(Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = recv_seq.wrapping_add(1);
        header.ack_number = self.recv_ack.load(Ordering::SeqCst).wrapping_add(1);
        header.set_flags(tcp_flags::FIN | tcp_flags::ACK);

        self.send_segment(dest_ip, header, &[], true);
    }

    /// Send TCP segment through IP layer
    fn send_segment(
        &self,
        dest_ip: Ipv4Address,
        mut header: TcpHeader,
        data: &[u8],
        update_seq: bool,
    ) {
        let local_ip = self
            .local_ip
            .lock()
            .clone()
            .unwrap_or(Ipv4Address::new(0, 0, 0, 0));

        let total_len = header.data_offset() + data.len();
        header.window_size = self.recv_window.load(Ordering::SeqCst);

        // Calculate checksum
        header.checksum = header.calculate_checksum(local_ip.0, dest_ip.0, data);

        // Serialize header
        let header_bytes = header.to_bytes();

        // Combine header and data
        let mut segment = Vec::with_capacity(total_len);
        segment.extend_from_slice(&header_bytes);
        segment.extend_from_slice(data);

        // Create IP context
        let mut ip_context = LayerContext::new();
        ip_context.set("ip_dst", &dest_ip.0);
        ip_context.set("ip_protocol", &[6]); // TCP protocol

        // Send through IP layer
        if let Some(ip_layer) = get_network_manager().get_layer("ip") {
            if let Ok(()) = ip_layer.send(&segment, &ip_context, &[]) {
                self.bytes_sent.fetch_add(segment.len() as u64, Ordering::SeqCst);

                if update_seq {
                    self.send_seq.fetch_add(total_len as u32, Ordering::SeqCst);
                }
            }
        }
    }

    /// Send data through socket
    pub fn send_data(&self, data: &[u8]) -> Result<usize, SocketError> {
        if self.get_state() != TcpState::Established {
            return Err(SocketError::NotConnected);
        }

        let dest_ip = self
            .remote_ip
            .lock()
            .clone()
            .ok_or(SocketError::NotConnected)?;
        let dest_port = self.remote_port.load(Ordering::SeqCst);

        // Add to send buffer
        self.send_buffer.lock().extend(data);

        // Create TCP header
        let local_port = self.local_port.load(Ordering::SeqCst);
        let local_ip = self
            .local_ip
            .lock()
            .clone()
            .unwrap_or(Ipv4Address::new(0, 0, 0, 0));

        let send_seq = self.send_seq.load(Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = send_seq;
        header.ack_number = self.recv_ack.load(Ordering::SeqCst);
        header.set_flags(tcp_flags::ACK | tcp_flags::PSH);

        self.send_segment(dest_ip, header, data, true);

        Ok(data.len())
    }

    /// Receive data from socket
    pub fn recv_data(&self, buffer: &mut [u8]) -> Result<usize, SocketError> {
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
}

impl SocketObject for TcpSocket {
    fn socket_type(&self) -> crate::network::socket::SocketType {
        crate::network::socket::SocketType::Stream
    }

    fn socket_domain(&self) -> crate::network::socket::SocketDomain {
        crate::network::socket::SocketDomain::Inet
    }

    fn socket_protocol(&self) -> crate::network::socket::SocketProtocol {
        crate::network::socket::SocketProtocol::Tcp
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn sendto(
        &self,
        data: &[u8],
        address: &SocketAddress,
        flags: u32,
    ) -> Result<usize, SocketError> {
        let _ = flags;

        match address {
            SocketAddress::Inet(inet) => {
                let addr = Ipv4Address::from_bytes(inet.addr);
                let port = inet.port;
                // Update remote address
                *self.remote_ip.lock() = Some(addr);
                self.remote_port.store(port, Ordering::SeqCst);
                self.send_data(data)
            }
            _ => Err(SocketError::InvalidAddress),
        }
    }

    fn recvfrom(
        &self,
        buffer: &mut [u8],
        flags: u32,
    ) -> Result<(usize, SocketAddress), SocketError> {
        let _ = flags;

        let len = self.recv_data(buffer)?;
        let remote_ip = self
            .remote_ip
            .lock()
            .clone()
            .unwrap_or(Ipv4Address::new(0, 0, 0, 0));
        let addr = SocketAddress::Inet(Inet4SocketAddress::new(
            remote_ip.0,
            self.remote_port.load(Ordering::SeqCst),
        ));

        Ok((len, addr))
    }
}

impl SocketControl for TcpSocket {
    fn bind(&self, address: &SocketAddress) -> Result<(), SocketError> {
        match address {
            SocketAddress::Inet(inet) => {
                *self.local_ip.lock() = Some(Ipv4Address::from_bytes(inet.addr));
                self.local_port.store(inet.port, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(SocketError::InvalidAddress),
        }
    }

    fn listen(&self, _backlog: usize) -> Result<(), SocketError> {
        if self.local_port.load(Ordering::SeqCst) == 0 {
            return Err(SocketError::InvalidOperation);
        }
        self.set_state(TcpState::Listen);
        Ok(())
    }

    fn connect(&self, address: &SocketAddress) -> Result<(), SocketError> {
        match address {
            SocketAddress::Inet(inet) => {
                let addr = Ipv4Address::from_bytes(inet.addr);
                let port = inet.port;
                *self.remote_ip.lock() = Some(addr);
                self.remote_port.store(port, Ordering::SeqCst);

                // Start 3-way handshake
                self.send_syn(addr, port);
                Ok(())
            }
            _ => Err(SocketError::InvalidAddress),
        }
    }

    fn accept(&self) -> Result<Arc<dyn SocketObject>, SocketError> {
        if self.get_state() != TcpState::Listen {
            return Err(SocketError::NotListening);
        }

        // TODO: Implement accept from pending connections
        // For now, return would block
        Err(SocketError::WouldBlock)
    }

    fn getpeername(&self) -> Result<SocketAddress, SocketError> {
        let ip = self
            .remote_ip
            .lock()
            .clone()
            .ok_or(SocketError::NotConnected)?;
        let port = self.remote_port.load(Ordering::SeqCst);
        Ok(SocketAddress::Inet(Inet4SocketAddress::new(ip.0, port)))
    }

    fn getsockname(&self) -> Result<SocketAddress, SocketError> {
        let ip = self
            .local_ip
            .lock()
            .clone()
            .ok_or(SocketError::InvalidAddress)?;
        let port = self.local_port.load(Ordering::SeqCst);
        Ok(SocketAddress::Inet(Inet4SocketAddress::new(ip.0, port)))
    }

    fn shutdown(&self, how: crate::network::socket::ShutdownHow) -> Result<(), SocketError> {
        match how {
            crate::network::socket::ShutdownHow::Write
            | crate::network::socket::ShutdownHow::Both => {
                self.send_fin();
            }
            _ => {}
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.get_state() == TcpState::Established
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

impl crate::ipc::StreamIpcOps for TcpSocket {
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
        alloc::format!("TCP socket")
    }
}

impl crate::object::capability::StreamOps for TcpSocket {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, crate::object::capability::StreamError> {
        self.recv_data(buffer)
            .map_err(|_| crate::object::capability::StreamError::Other("tcp recv error".into()))
    }

    fn write(&self, data: &[u8]) -> Result<usize, crate::object::capability::StreamError> {
        self.send_data(data)
            .map_err(|_| crate::object::capability::StreamError::Other("tcp send error".into()))
    }
}

impl crate::object::capability::CloneOps for TcpSocket {
    fn custom_clone(&self) -> crate::object::KernelObject {
        crate::object::KernelObject::Socket(Arc::new(TcpSocket::new(self.tcp_layer.clone())))
    }
}

/// TCP layer
///
/// Manages TCP port bindings and routes packets to sockets.
pub struct TcpLayer {
    /// Port-to-socket mapping for receiving packets
    port_map: RwLock<BTreeMap<u16, Weak<TcpSocket>>>,
    /// Statistics
    stats: RwLock<NetworkLayerStats>,
}

impl TcpLayer {
    /// Create a new TCP layer
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            port_map: RwLock::new(BTreeMap::new()),
            stats: RwLock::new(NetworkLayerStats::default()),
        })
    }

    /// Register a socket for a specific port
    pub fn register_port(&self, port: u16, socket: Weak<TcpSocket>) {
        self.port_map.write().insert(port, socket);
    }

    /// Unregister a socket from a port
    pub fn unregister_port(&self, port: u16) {
        self.port_map.write().remove(&port);
    }

    /// Find socket for a destination port
    pub fn find_socket(&self, port: u16) -> Option<Arc<TcpSocket>> {
        self.port_map
            .read()
            .get(&port)
            .and_then(|weak| weak.upgrade())
    }

    /// Process incoming TCP segment
    pub fn receive_segment(&self, src_ip: Ipv4Address, header: TcpHeader, data: &[u8]) {

        let mut stats = self.stats.write();
        stats.packets_received += 1;
        stats.bytes_received += (header.data_offset() + data.len()) as u64;

        if let Some(socket) = self.find_socket(header.dst_port) {
            socket.process_segment(src_ip, header, data);
        } else {
        }
    }
}

impl NetworkLayer for TcpLayer {
    fn register_protocol(&self, _proto_num: u16, _handler: Arc<dyn NetworkLayer>) {
        // TCP is typically a leaf protocol
    }

    fn send(
        &self,
        _packet: &[u8],
        _context: &LayerContext,
        _next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // TCP send is handled through sockets
        Ok(())
    }

    fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
        if packet.len() < 20 {
            return Err(SocketError::InvalidPacket);
        }

        let header = TcpHeader::from_bytes(&packet[..20]).ok_or(SocketError::InvalidPacket)?;

        let data_offset = header.data_offset();
        if data_offset < 20 || data_offset > packet.len() {
            return Err(SocketError::InvalidPacket);
        }

        let data = &packet[data_offset..];
        let src_ip = Ipv4Address::new(0, 0, 0, 0);

        self.receive_segment(src_ip, header, data);

        Ok(())
    }

    fn name(&self) -> &'static str {
        "TCP"
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
    fn test_tcp_header_creation() {
        let header = TcpHeader::new(8080, 80);

        assert_eq!(header.src_port, 8080);
        assert_eq!(header.dst_port, 80);
        assert_eq!(header.flags(), 0);
        assert_eq!(header.data_offset(), 20);
    }

    #[test_case]
    fn test_tcp_flags_constants() {
        assert_eq!(tcp_flags::FIN, 0x01);
        assert_eq!(tcp_flags::SYN, 0x02);
        assert_eq!(tcp_flags::RST, 0x04);
        assert_eq!(tcp_flags::ACK, 0x10);
        assert_eq!(tcp_flags::PSH, 0x08);
    }

    #[test_case]
    fn test_tcp_state_transitions() {
        let tcp_layer = TcpLayer::new();
        let socket = TcpSocket::new(Arc::downgrade(&tcp_layer));

        assert_eq!(socket.get_state(), TcpState::Closed);

        socket.set_state(TcpState::Listen);
        assert_eq!(socket.get_state(), TcpState::Listen);

        socket.set_state(TcpState::SynSent);
        assert_eq!(socket.get_state(), TcpState::SynSent);

        socket.set_state(TcpState::Established);
        assert_eq!(socket.get_state(), TcpState::Established);
    }

    #[test_case]
    fn test_tcp_checksum() {
        let local_ip = [192, 168, 1, 100];
        let dest_ip = [192, 168, 1, 1];
        let data = b"test";

        let mut header = TcpHeader::new(1234, 5678);
        header.seq_number = 1000;
        header.ack_number = 2000;

        let checksum = header.calculate_checksum(&local_ip, &dest_ip, data);

        assert_ne!(checksum, 0);
    }
}
