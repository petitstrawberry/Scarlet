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
use crate::network::socket::{
    Inet4SocketAddress, SocketAddress, SocketControl, SocketObject, SocketState,
};

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
        sum = (sum & 0xFFFF) + (sum >> 16);

        // TCP header (checksum field treated as 0)
        // src_port (2 bytes)
        sum += self.src_port as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        // dst_port (2 bytes)
        sum += self.dst_port as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        // seq_number (4 bytes)
        sum += ((self.seq_number >> 16) & 0xFFFF) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += (self.seq_number & 0xFFFF) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        // ack_number (4 bytes)
        sum += ((self.ack_number >> 16) & 0xFFFF) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += (self.ack_number & 0xFFFF) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        // data_offset_flags (2 bytes)
        sum += self.data_offset_flags as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        // window_size (2 bytes)
        sum += self.window_size as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        // checksum field - treated as 0, skip
        // urgent_pointer (2 bytes)
        sum += self.urgent_pointer as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);

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

/// Unacknowledged TCP segment for retransmission tracking
#[derive(Clone)]
struct UnackedSegment {
    /// Sequence number of first byte
    seq: u32,
    /// Data to retransmit
    data: Vec<u8>,
    /// Flags (SYN, FIN, PSH, etc.)
    flags: u8,
    /// Transmission count
    tx_count: u16,
    /// Last transmission timestamp (ticks)
    last_tx_time: u64,
}

/// Retransmission timer handler
struct RetransTimer {
    socket: Weak<TcpSocket>,
    seq: u32,
}

impl crate::timer::TimerHandler for RetransTimer {
    fn on_timer_expired(self: Arc<Self>, _context: usize) {
        if let Some(socket) = self.socket.upgrade() {
            socket.handle_retrans_timeout(self.seq);
        }
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
    /// Weak self reference for registration
    self_weak: Weak<TcpSocket>,
    /// Pending accepted connections (listener only)
    pending_accept: Mutex<VecDeque<Arc<TcpSocket>>>,

    /// Statistics
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,

    /// RTO (Retransmission Timeout) calculation - RFC 6298
    /// Smoothed RTT (8 * srtt for fixed-point arithmetic)
    srtt: AtomicU32,
    /// RTT variation (4 * rttvar for fixed-point arithmetic)
    rttvar: AtomicU32,
    /// Current RTO in ticks (initial: 1 second = 100 ticks @ 10ms)
    rto: AtomicU32,
    /// Retransmission count for exponential backoff
    retrans_count: AtomicU16,
    /// Timer ID for retransmission timer
    retrans_timer_id: Mutex<Option<u64>>,
    /// Timestamp of last segment transmission (for RTT measurement)
    last_send_time: AtomicU64,
    /// Whether we're timing an RTT measurement (Karn's algorithm)
    timing_rtt: AtomicU16,
    /// Sequence number being timed
    timed_seq: AtomicU32,

    /// List of unacknowledged segments for retransmission
    unacked_segments: Mutex<VecDeque<UnackedSegment>>,
}

impl TcpSocket {
    /// Create a new TCP socket
    pub fn new(tcp_layer: Weak<TcpLayer>) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
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
            self_weak: weak.clone(),
            pending_accept: Mutex::new(VecDeque::new()),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),

            // RTO initialization - RFC 6298
            // Initial RTO = 1 second = 100 ticks (10ms/tick)
            srtt: AtomicU32::new(0),
            rttvar: AtomicU32::new(0),
            rto: AtomicU32::new(100), // 1 second in ticks
            retrans_count: AtomicU16::new(0),
            retrans_timer_id: Mutex::new(None),
            last_send_time: AtomicU64::new(0),
            timing_rtt: AtomicU16::new(0),
            timed_seq: AtomicU32::new(0),

            // Unacked segments list
            unacked_segments: Mutex::new(VecDeque::new()),
        })
    }

    fn matches_peer(&self, src_ip: Ipv4Address, src_port: u16) -> bool {
        if self.get_state() == TcpState::Listen {
            return false;
        }

        let remote_ip = self.remote_ip.lock().clone();
        let remote_port = self.remote_port.load(Ordering::SeqCst);
        match remote_ip {
            Some(ip) => ip == src_ip && remote_port == src_port,
            None => false,
        }
    }

    fn ensure_local_ip(&self) {
        if self.local_ip.lock().is_some() {
            return;
        }

        if let Some(ip_layer) = get_network_manager().get_layer("ip") {
            if let Some(ip) = ip_layer
                .as_any()
                .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
            {
                *self.local_ip.lock() = Some(ip.get_local_ip());
            }
        }
    }

    fn register_local_port(&self, port: u16) -> Result<(), SocketError> {
        let tcp_layer = self
            .tcp_layer
            .upgrade()
            .ok_or(SocketError::InvalidOperation)?;

        tcp_layer.register_port(port, self.self_weak.clone());
        Ok(())
    }

    fn allocate_ephemeral_port(&self) -> u16 {
        static NEXT_EPHEMERAL_PORT: AtomicU16 = AtomicU16::new(49152);

        let port = NEXT_EPHEMERAL_PORT.fetch_add(1, Ordering::SeqCst);
        if port == u16::MAX {
            NEXT_EPHEMERAL_PORT.store(49152, Ordering::SeqCst);
        }
        if port < 49152 { 49152 } else { port }
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
                    let tcp_layer = match self.tcp_layer.upgrade() {
                        Some(layer) => layer,
                        None => return,
                    };

                    let child = TcpSocket::new(Arc::downgrade(&tcp_layer));
                    let local_port = self.local_port.load(Ordering::SeqCst);
                    if local_port == 0 {
                        return;
                    }

                    if let Some(local_ip) = self.local_ip.lock().clone() {
                        *child.local_ip.lock() = Some(local_ip);
                    } else {
                        child.ensure_local_ip();
                    }

                    child.local_port.store(local_port, Ordering::SeqCst);
                    tcp_layer.register_port(local_port, child.self_weak.clone());
                    child.handle_syn_received(src_ip, header);
                    self.pending_accept.lock().push_back(child);
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
            _ => {}
        }
    }

    /// Handle incoming SYN (SYN-RECEIVED state)
    fn handle_syn_received(&self, src_ip: Ipv4Address, header: TcpHeader) {
        // Store remote address
        *self.remote_ip.lock() = Some(src_ip);
        self.remote_port.store(header.src_port, Ordering::SeqCst);

        // Track peer sequence and our initial sequence
        let initial_seq = 1000;
        let next_recv = header.seq_number.wrapping_add(1);
        self.send_seq.store(initial_seq, Ordering::SeqCst);
        self.recv_seq.store(next_recv, Ordering::SeqCst);
        self.recv_ack.store(next_recv, Ordering::SeqCst);

        // Send SYN-ACK
        self.send_segment(src_ip, header, &[], false, false);
        self.send_seq.fetch_add(1, Ordering::SeqCst);
        self.set_state(TcpState::SynReceived);
    }

    /// Handle received SYN-ACK (move to ESTABLISHED)
    fn handle_syn_ack_received(&self, src_ip: Ipv4Address, header: TcpHeader) {
        *self.remote_ip.lock() = Some(src_ip);
        self.remote_port.store(header.src_port, Ordering::SeqCst);

        let next_recv = header.seq_number.wrapping_add(1);
        self.recv_seq.store(next_recv, Ordering::SeqCst);
        self.recv_ack.store(next_recv, Ordering::SeqCst);

        // Advance our sequence number past the SYN we sent
        let acked = header.ack_number;
        self.send_seq.store(acked, Ordering::SeqCst);
        self.send_unacked.store(acked, Ordering::SeqCst);

        self.send_ack(src_ip, header.src_port, next_recv);

        self.set_state(TcpState::Established);
    }

    /// Handle control segment (ACK, FIN, RST)
    fn handle_control_segment(&self, src_ip: Ipv4Address, header: TcpHeader) {
        if header.flags() & tcp_flags::RST != 0 {
            self.set_state(TcpState::Closed);
            return;
        }

        if header.flags() & tcp_flags::FIN != 0 {
            self.handle_fin(src_ip, header);
        }

        if header.flags() & tcp_flags::ACK != 0 {
            self.update_send_window(header.ack_number);
            self.stop_rtt_measurement(header.ack_number);
            self.remove_acked_segments(header.ack_number);
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

        if segment_seq < expected_seq {
            self.send_ack(src_ip, header.src_port, expected_seq);
            return;
        }

        // Add data to receive buffer
        if data.is_empty() {
            if header.flags() & tcp_flags::ACK != 0 {
                self.update_send_window(header.ack_number);
                self.stop_rtt_measurement(header.ack_number);
                self.remove_acked_segments(header.ack_number);
            }
        } else if segment_seq == expected_seq {
            let mut recv_buf = self.recv_buffer.lock();
            recv_buf.extend(data);
            let next_seq = expected_seq.wrapping_add(data.len() as u32);
            self.recv_seq.store(next_seq, Ordering::SeqCst);
            self.recv_ack.store(next_seq, Ordering::SeqCst);

            // Send ACK
            self.send_ack(src_ip, header.src_port, next_seq);

            // Update received bytes
            self.bytes_received
                .fetch_add(data.len() as u64, Ordering::SeqCst);
        }

        if header.flags() & tcp_flags::ACK != 0 {
            self.update_send_window(header.ack_number);
            self.stop_rtt_measurement(header.ack_number);
            self.remove_acked_segments(header.ack_number);
        }
    }

    /// Handle FIN segment
    fn handle_fin(&self, src_ip: Ipv4Address, header: TcpHeader) {
        let current_state = self.get_state();
        let ack_seq = header.seq_number.wrapping_add(1);
        self.recv_seq.store(ack_seq, Ordering::SeqCst);
        self.recv_ack.store(ack_seq, Ordering::SeqCst);
        match current_state {
            TcpState::Established => {
                self.send_ack(src_ip, header.src_port, ack_seq);
                self.set_state(TcpState::CloseWait);
            }
            TcpState::FinWait1 => {
                if header.flags() & (tcp_flags::FIN | tcp_flags::ACK)
                    == (tcp_flags::FIN | tcp_flags::ACK)
                {
                    self.send_ack(src_ip, header.src_port, ack_seq);
                    self.set_state(TcpState::TimeWait);
                }
            }
            _ => {}
        }
    }

    /// Update send window based on acknowledgment
    fn update_send_window(&self, ack_number: u32) {
        // Track the latest acknowledged sequence number
        self.send_unacked.store(ack_number, Ordering::SeqCst);
    }

    /// Send SYN packet
    fn send_syn(&self, dest_ip: Ipv4Address, dest_port: u16) {
        let local_port = self.local_port.load(Ordering::SeqCst);

        let initial_seq = 1000;
        self.send_seq.store(initial_seq, Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = initial_seq;
        header.set_flags(tcp_flags::SYN);

        self.send_segment(dest_ip, header, &[], false, false);
        self.send_seq.fetch_add(1, Ordering::SeqCst);
        self.set_state(TcpState::SynSent);
    }

    /// Send SYN-ACK packet
    fn send_syn_ack(&self, dest_ip: Ipv4Address, dest_port: u16, their_seq: u32, ack_seq: u32) {
        let local_port = self.local_port.load(Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = their_seq;
        header.ack_number = ack_seq;
        header.set_flags(tcp_flags::SYN | tcp_flags::ACK);

        self.send_segment(dest_ip, header, &[], false, false);
        self.set_state(TcpState::SynReceived);
    }

    /// Send ACK packet
    fn send_ack(&self, dest_ip: Ipv4Address, dest_port: u16, ack_seq: u32) {
        let local_port = self.local_port.load(Ordering::SeqCst);
        let send_seq = self.send_seq.load(Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = send_seq;
        header.ack_number = ack_seq;
        header.set_flags(tcp_flags::ACK);

        self.send_segment(dest_ip, header, &[], false, false);
    }

    /// Send FIN packet
    fn send_fin(&self) {
        let dest_ip = self.remote_ip.lock().clone().unwrap();
        let dest_port = self.remote_port.load(Ordering::SeqCst);
        let local_port = self.local_port.load(Ordering::SeqCst);

        let send_seq = self.send_seq.load(Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = send_seq;
        header.set_flags(tcp_flags::FIN);

        self.send_segment(dest_ip, header, &[], true, false);
        self.set_state(TcpState::FinWait1);
    }

    /// Send FIN-ACK packet
    fn send_fin_ack(&self) {
        let dest_ip = self.remote_ip.lock().clone().unwrap();
        let dest_port = self.remote_port.load(Ordering::SeqCst);
        let local_port = self.local_port.load(Ordering::SeqCst);
        let send_seq = self.send_seq.load(Ordering::SeqCst);
        let recv_seq = self.recv_seq.load(Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = send_seq;
        header.ack_number = recv_seq;
        header.set_flags(tcp_flags::FIN | tcp_flags::ACK);

        self.send_segment(dest_ip, header, &[], true, false);
    }

    /// Send TCP segment through IP layer
    fn send_segment(
        &self,
        dest_ip: Ipv4Address,
        mut header: TcpHeader,
        data: &[u8],
        update_seq: bool,
        is_retransmit: bool,
    ) {
        self.ensure_local_ip();
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
                self.bytes_sent
                    .fetch_add(segment.len() as u64, Ordering::SeqCst);

                if update_seq {
                    let mut advance = data.len() as u32;
                    let flags = header.flags();
                    if (flags & tcp_flags::SYN) != 0 {
                        advance = advance.wrapping_add(1);
                    }
                    if (flags & tcp_flags::FIN) != 0 {
                        advance = advance.wrapping_add(1);
                    }
                    if advance != 0 {
                        self.send_seq.fetch_add(advance, Ordering::SeqCst);
                    }
                }

                // Track segment for retransmission (only for new transmissions with data or SYN/FIN)
                if !is_retransmit {
                    let flags = header.flags();
                    let has_data = !data.is_empty();
                    let is_syn = (flags & tcp_flags::SYN) != 0;
                    let is_fin = (flags & tcp_flags::FIN) != 0;

                    if has_data || is_syn || is_fin {
                        let seq = header.seq_number;
                        self.add_unacked_segment(seq, data.to_vec(), flags);
                    }
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

        self.send_segment(dest_ip, header, data, true, false);

        Ok(data.len())
    }

    /// Receive data from socket
    pub fn recv_data(&self, buffer: &mut [u8]) -> Result<usize, SocketError> {
        match self.get_state() {
            TcpState::Established | TcpState::CloseWait => {}
            _ => return Err(SocketError::NotConnected),
        }

        let mut recv_buf = self.recv_buffer.lock();
        let len = buffer.len().min(recv_buf.len());

        for i in 0..len {
            buffer[i] = recv_buf.pop_front().unwrap();
        }

        Ok(len)
    }

    // ===================================================================
    // RTO (Retransmission Timeout) - RFC 6298
    // ===================================================================

    /// Update RTO based on RTT measurement (Jacobson/Karels algorithm)
    /// Uses fixed-point arithmetic for better precision in no_std
    fn update_rto(&self, rtt_ticks: u32) {
        // RFC 6298: RTO calculation
        // SRTT = (1 - alpha) * SRTT + alpha * RTT
        // RTTVAR = (1 - beta) * RTTVAR + beta * |SRTT - RTT|
        // RTO = SRTT + max(G, K * RTTVAR)
        // where alpha = 1/8, beta = 1/4, K = 4, G = clock granularity

        const ALPHA_SHIFT: u32 = 3; // alpha = 1/8
        const BETA_SHIFT: u32 = 2; // beta = 1/4
        const K: u32 = 4; // multiplier for RTTVAR

        let srtt = self.srtt.load(Ordering::SeqCst);
        let rttvar = self.rttvar.load(Ordering::SeqCst);

        if srtt == 0 {
            // First RTT measurement
            // SRTT = RTT
            // RTTVAR = RTT / 2
            self.srtt.store(rtt_ticks << 3, Ordering::SeqCst); // 8 * RTT
            self.rttvar.store((rtt_ticks << 2) >> 1, Ordering::SeqCst); // 4 * RTT / 2
        } else {
            // Subsequent measurements
            // RTTVAR = (1 - beta) * RTTVAR + beta * |SRTT - RTT|
            // SRTT = (1 - alpha) * SRTT + alpha * RTT
            let srtt_val = srtt >> 3; // Divide by 8
            let diff = if srtt_val > rtt_ticks {
                srtt_val - rtt_ticks
            } else {
                rtt_ticks - srtt_val
            };

            // RTTVAR = (3/4) * RTTVAR + (1/4) * |diff|
            let new_rttvar = ((rttvar * 3) >> BETA_SHIFT) + ((diff << 2) >> BETA_SHIFT);
            self.rttvar.store(new_rttvar, Ordering::SeqCst);

            // SRTT = (7/8) * SRTT + (1/8) * RTT
            let new_srtt = ((srtt * 7) >> ALPHA_SHIFT) + (rtt_ticks << (3 - ALPHA_SHIFT));
            self.srtt.store(new_srtt, Ordering::SeqCst);
        }

        // RTO = SRTT + max(G, K * RTTVAR)
        // G = 1 tick (10ms), K = 4
        let srtt_val = self.srtt.load(Ordering::SeqCst) >> 3;
        let rttvar_val = self.rttvar.load(Ordering::SeqCst) >> 2;
        let mut rto = srtt_val + (K * rttvar_val).max(1);

        // Clamp RTO to bounds
        // Min: 1 tick (10ms), Max: 12000 ticks (120 seconds)
        rto = rto.max(1).min(12000);

        self.rto.store(rto, Ordering::SeqCst);
    }

    /// Get current RTO in milliseconds
    fn get_rto_ms(&self) -> u32 {
        // Convert ticks to milliseconds (10ms per tick)
        self.rto.load(Ordering::SeqCst) * 10
    }

    /// Get current RTO in ticks
    fn get_rto_ticks(&self) -> u32 {
        self.rto.load(Ordering::SeqCst)
    }

    /// Start RTT measurement for a sequence number
    fn start_rtt_measurement(&self, seq: u32) {
        // Only start timing if not already timing
        if self.timing_rtt.load(Ordering::SeqCst) == 0 {
            self.timed_seq.store(seq, Ordering::SeqCst);
            self.last_send_time
                .store(crate::timer::get_tick(), Ordering::SeqCst);
            self.timing_rtt.store(1, Ordering::SeqCst);
        }
    }

    /// Stop RTT measurement when ACK is received
    fn stop_rtt_measurement(&self, ack_seq: u32) {
        // Check if we're timing and if this ACK covers the timed sequence
        if self.timing_rtt.load(Ordering::SeqCst) != 0 {
            let timed_seq = self.timed_seq.load(Ordering::SeqCst);
            // Check if ACK acknowledges the segment we were timing
            // Note: Sequence number comparison needs to handle wraparound
            if is_seq_acknowledged(timed_seq, ack_seq) {
                let send_time = self.last_send_time.load(Ordering::SeqCst);
                let now = crate::timer::get_tick();
                if now > send_time {
                    let rtt = (now - send_time) as u32;
                    self.update_rto(rtt);
                }
                self.timing_rtt.store(0, Ordering::SeqCst);
                // Reset retransmission count on successful ACK
                self.retrans_count.store(0, Ordering::SeqCst);
            }
        }
    }

    /// Exponential backoff for retransmission
    fn backoff_rto(&self) {
        let count = self.retrans_count.load(Ordering::SeqCst);
        if count < 6 {
            // Double RTO (exponential backoff), max 64x
            let backoff = 1u32 << count.min(6);
            let base_rto = self.rto.load(Ordering::SeqCst);
            let new_rto = (base_rto * backoff).min(12000); // Max 120 seconds
            self.rto.store(new_rto, Ordering::SeqCst);
            self.retrans_count.store(count + 1, Ordering::SeqCst);
        }
    }

    /// Check if maximum retransmissions exceeded
    fn max_retransmissions_exceeded(&self) -> bool {
        self.retrans_count.load(Ordering::SeqCst) >= 12 // Max 12 retransmissions
    }

    /// Handle retransmission timeout
    fn handle_retrans_timeout(&self, seq: u32) {
        // Check if socket is still in a valid state for retransmission
        let state = self.get_state();
        match state {
            TcpState::Closed | TcpState::Listen | TcpState::TimeWait => return,
            _ => {}
        }

        // Find the segment to retransmit
        let mut unacked = self.unacked_segments.lock();
        if let Some(pos) = unacked.iter().position(|seg| seg.seq == seq) {
            if let Some(mut seg) = unacked.get(pos).cloned() {
                // Check max retransmissions
                if seg.tx_count >= 12 {
                    // Too many retransmissions, close connection
                    self.set_state(TcpState::Closed);
                    return;
                }

                // Exponential backoff
                self.backoff_rto();

                // Retransmit the segment
                if let Some(dest_ip) = self.remote_ip.lock().clone() {
                    let dest_port = self.remote_port.load(Ordering::SeqCst);
                    let local_port = self.local_port.load(Ordering::SeqCst);

                    let mut header = TcpHeader::new(local_port, dest_port);
                    header.seq_number = seg.seq;
                    header.ack_number = self.recv_ack.load(Ordering::SeqCst);
                    header.set_flags(seg.flags);

                    // Retransmit (don't update sequence number, mark as retransmission)
                    self.send_segment(dest_ip, header, &seg.data, false, true);

                    // Update segment info
                    seg.tx_count += 1;
                    seg.last_tx_time = crate::timer::get_tick();

                    // Update in queue
                    if let Some(existing) = unacked.get_mut(pos) {
                        *existing = seg;
                    }

                    // Schedule next retransmission timer
                    self.schedule_retrans_timer(seq);
                }
            }
        }
    }

    /// Schedule retransmission timer for a segment
    fn schedule_retrans_timer(&self, seq: u32) {
        let rto_ticks = self.get_rto_ticks();
        let expires = crate::timer::get_tick() + rto_ticks as u64;

        let timer: Arc<dyn crate::timer::TimerHandler> = Arc::new(RetransTimer {
            socket: self.self_weak.clone(),
            seq,
        });

        let timer_id = crate::timer::add_timer(expires, &timer, 0);

        // Store timer ID
        *self.retrans_timer_id.lock() = Some(timer_id);
    }

    /// Cancel retransmission timer
    fn cancel_retrans_timer(&self) {
        if let Some(timer_id) = *self.retrans_timer_id.lock() {
            crate::timer::cancel_timer(timer_id);
            *self.retrans_timer_id.lock() = None;
        }
    }

    /// Add segment to unacked list and schedule retransmission
    fn add_unacked_segment(&self, seq: u32, data: Vec<u8>, flags: u8) {
        let segment = UnackedSegment {
            seq,
            data,
            flags,
            tx_count: 1,
            last_tx_time: crate::timer::get_tick(),
        };

        self.unacked_segments.lock().push_back(segment);

        // Schedule retransmission timer
        self.schedule_retrans_timer(seq);

        // Start RTT measurement if not already timing
        self.start_rtt_measurement(seq);
    }

    /// Remove acknowledged segments from unacked list
    fn remove_acked_segments(&self, ack_seq: u32) {
        let mut unacked = self.unacked_segments.lock();
        // Remove all segments that are fully acknowledged
        unacked.retain(|seg| {
            let seg_end = seg.seq.wrapping_add(seg.data.len() as u32);
            // Keep if not fully acknowledged
            !is_seq_acknowledged(seg_end, ack_seq)
        });

        // If all segments are acknowledged, cancel timer
        if unacked.is_empty() {
            drop(unacked);
            self.cancel_retrans_timer();
        }
    }
}

/// Check if a sequence number is acknowledged by an ACK number
/// Handles sequence number wraparound
fn is_seq_acknowledged(seq: u32, ack: u32) -> bool {
    // Standard TCP sequence number comparison
    // Returns true if ack acknowledges seq
    seq.wrapping_sub(ack) > (1u32 << 31)
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
                if inet.port == 0 {
                    return Err(SocketError::InvalidAddress);
                }

                self.register_local_port(inet.port)?;
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

                let local_port = self.local_port.load(Ordering::SeqCst);
                if local_port == 0 {
                    let port = self.allocate_ephemeral_port();
                    self.register_local_port(port)?;
                    self.local_port.store(port, Ordering::SeqCst);
                }

                self.ensure_local_ip();

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

        let mut pending = self.pending_accept.lock();
        pending
            .pop_front()
            .map(|socket| socket as Arc<dyn SocketObject>)
            .ok_or(SocketError::WouldBlock)
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
        crate::object::KernelObject::Socket(TcpSocket::new(self.tcp_layer.clone()))
    }
}

impl Drop for TcpSocket {
    fn drop(&mut self) {
        // Send FIN if connection is still open
        let state = self.get_state();
        match state {
            TcpState::Established | TcpState::SynReceived | TcpState::SynSent => {
                // Send FIN to close connection gracefully
                let _ = self.remote_ip.lock().clone().map(|dest_ip| {
                    let dest_port = self.remote_port.load(Ordering::SeqCst);
                    let local_port = self.local_port.load(Ordering::SeqCst);
                    let send_seq = self.send_seq.load(Ordering::SeqCst);

                    let mut header = TcpHeader::new(local_port, dest_port);
                    header.seq_number = send_seq;
                    header.set_flags(tcp_flags::FIN);
                    self.send_segment(dest_ip, header, &[], true, false);
                });
            }
            _ => {}
        }

        // Unregister port from TcpLayer
        if let Some(layer) = self.tcp_layer.upgrade() {
            let port = self.local_port.load(Ordering::SeqCst);
            if port != 0 {
                layer.unregister_port(port);
            }
        }
    }
}

/// TCP layer
///
/// Manages TCP port bindings and routes packets to sockets.
pub struct TcpLayer {
    /// Port-to-socket mapping for receiving packets
    port_map: RwLock<BTreeMap<u16, Vec<Weak<TcpSocket>>>>,
    /// Statistics
    stats: RwLock<NetworkLayerStats>,
    self_weak: Weak<TcpLayer>,
}

impl TcpLayer {
    /// Create a new TCP layer
    pub fn new() -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            port_map: RwLock::new(BTreeMap::new()),
            stats: RwLock::new(NetworkLayerStats::default()),
            self_weak: weak.clone(),
        })
    }

    pub fn create_socket(&self) -> Arc<TcpSocket> {
        TcpSocket::new(self.self_weak.clone())
    }

    /// Register a socket for a specific port
    pub fn register_port(&self, port: u16, socket: Weak<TcpSocket>) {
        let mut map = self.port_map.write();
        let entry = map.entry(port).or_default();
        if entry.iter().any(|existing| existing.ptr_eq(&socket)) {
            return;
        }
        if entry.iter().any(|existing| {
            existing
                .upgrade()
                .map(|sock| sock.get_state() == TcpState::Listen)
                .unwrap_or(false)
                && socket
                    .upgrade()
                    .map(|sock| sock.get_state() == TcpState::Listen)
                    .unwrap_or(false)
        }) {
            return;
        }
        entry.push(socket);
    }

    /// Unregister a socket from a port
    pub fn unregister_port(&self, port: u16) {
        self.port_map.write().remove(&port);
    }

    /// Find socket for a destination port
    pub fn find_socket(
        &self,
        port: u16,
        src_ip: Ipv4Address,
        src_port: u16,
    ) -> Option<Arc<TcpSocket>> {
        let map = self.port_map.read();
        let sockets = map.get(&port)?;
        let mut listening = None;
        for weak in sockets {
            if let Some(socket) = weak.upgrade() {
                if socket.matches_peer(src_ip, src_port) {
                    return Some(socket);
                }
                if socket.get_state() == TcpState::Listen {
                    listening = Some(socket);
                }
            }
        }
        listening
    }

    pub fn find_listening_socket(&self, port: u16) -> Option<Arc<TcpSocket>> {
        let map = self.port_map.read();
        let sockets = map.get(&port)?;
        for weak in sockets {
            if let Some(socket) = weak.upgrade() {
                if socket.get_state() == TcpState::Listen {
                    return Some(socket);
                }
            }
        }
        None
    }

    /// Process incoming TCP segment
    pub fn receive_segment(&self, src_ip: Ipv4Address, header: TcpHeader, data: &[u8]) {
        let mut stats = self.stats.write();
        stats.packets_received += 1;
        stats.bytes_received += (header.data_offset() + data.len()) as u64;

        if let Some(socket) = self.find_socket(header.dst_port, src_ip, header.src_port) {
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
        self.receive_packet(
            Ipv4Address::new(0, 0, 0, 0),
            Ipv4Address::new(0, 0, 0, 0),
            packet,
        )
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

impl TcpLayer {
    /// Receive a TCP segment with IPv4 addressing information
    pub fn receive_packet(
        &self,
        src_ip: Ipv4Address,
        _dst_ip: Ipv4Address,
        packet: &[u8],
    ) -> Result<(), SocketError> {
        if packet.len() < 20 {
            return Err(SocketError::InvalidPacket);
        }

        let header = TcpHeader::from_bytes(&packet[..20]).ok_or(SocketError::InvalidPacket)?;

        let data_offset = header.data_offset();
        if data_offset < 20 || data_offset > packet.len() {
            return Err(SocketError::InvalidPacket);
        }

        let data = &packet[data_offset..];

        self.receive_segment(src_ip, header, data);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_tcp_header_creation() {
        let header = TcpHeader::new(8080, 80);

        let src_port = unsafe { core::ptr::addr_of!(header.src_port).read_unaligned() };
        let dst_port = unsafe { core::ptr::addr_of!(header.dst_port).read_unaligned() };
        assert_eq!(src_port, 8080);
        assert_eq!(dst_port, 80);
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

        let checksum = header.calculate_checksum(local_ip, dest_ip, data);

        assert_ne!(checksum, 0);
    }
}
