//! TCP protocol layer (Complete implementation)
//!
//! This module provides a full TCP implementation with 3-way handshake,
//! flow control, and retransmission.

use crate::sync::{IrqRwSpinLock, IrqSpinLock, WaitResult};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::network::ipv4::Ipv4Address;
use crate::network::protocol_stack::get_network_manager;
use crate::network::protocol_stack::{LayerContext, NetworkLayer, NetworkLayerStats};
use crate::network::socket::SocketError;
use crate::network::socket::{
    Inet4SocketAddress, SocketAddress, SocketControl, SocketObject, SocketState,
};
use crate::sched::scheduler::current_task_id;

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

const fn tcp_receive_side_open(state: TcpState) -> bool {
    matches!(
        state,
        TcpState::Established | TcpState::FinWait1 | TcpState::FinWait2
    )
}

const fn tcp_receive_side_eof(state: TcpState) -> bool {
    matches!(
        state,
        TcpState::CloseWait
            | TcpState::Closing
            | TcpState::LastAck
            | TcpState::TimeWait
            | TcpState::Closed
    )
}

const fn tcp_send_side_open(state: TcpState) -> bool {
    matches!(state, TcpState::Established | TcpState::CloseWait)
}

const fn tcp_connection_present(state: TcpState) -> bool {
    matches!(
        state,
        TcpState::Established
            | TcpState::FinWait1
            | TcpState::FinWait2
            | TcpState::CloseWait
            | TcpState::Closing
            | TcpState::LastAck
    )
}

const fn state_after_peer_fin(state: TcpState, local_fin_acknowledged: bool) -> TcpState {
    match state {
        TcpState::Established => TcpState::CloseWait,
        TcpState::FinWait1 if local_fin_acknowledged => TcpState::TimeWait,
        TcpState::FinWait1 => TcpState::Closing,
        TcpState::FinWait2 => TcpState::TimeWait,
        _ => state,
    }
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

/// Buffer size limits (prevent memory exhaustion)
const MAX_SEND_BUFFER_SIZE: usize = 65536; // 64KB
const MAX_RECV_BUFFER_SIZE: usize = 128 * 1024; // Two advertised receive windows
const MAX_UNACKED_SEGMENTS: usize = 256; // Limit unacked segment list
const IPV4_HEADER_SIZE: usize = 20;
const TCP_HEADER_SIZE: usize = 20;
const TCP_MAX_SEGMENT_DATA: usize =
    crate::network::ethernet::ETHERNET_MTU - IPV4_HEADER_SIZE - TCP_HEADER_SIZE;
const WINDOW_UPDATE_THRESHOLD: u16 = 8192;
const LOG_TCP_HTTPS: bool = false;
const MAX_SOCKET_TIMEOUT_MS: usize = i32::MAX as usize;

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
        let tcp_len = (self.data_offset() + data.len()) as u16;
        let mut pseudo = Vec::with_capacity(12 + 20 + data.len());
        pseudo.extend_from_slice(&src_ip);
        pseudo.extend_from_slice(&dst_ip);
        pseudo.push(0);
        pseudo.push(6); // TCP protocol number
        pseudo.extend_from_slice(&tcp_len.to_be_bytes());

        let mut header = *self;
        header.checksum = 0;
        pseudo.extend_from_slice(&header.to_bytes());
        pseudo.extend_from_slice(data);

        let mut sum: u32 = 0;
        for chunk in pseudo.chunks(2) {
            if chunk.len() == 2 {
                sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
            } else {
                sum += (chunk[0] as u32) << 8;
            }
            sum = (sum & 0xFFFF) + (sum >> 16);
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

/// Out-of-order TCP segment for reassembly
#[derive(Clone)]
struct OutOfOrderSegment {
    /// Sequence number of first byte
    seq: u32,
    /// Segment data
    data: Vec<u8>,
    /// TCP flags carried by the segment
    flags: u8,
}

impl PartialEq for OutOfOrderSegment {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq
    }
}

impl Eq for OutOfOrderSegment {}

impl PartialOrd for OutOfOrderSegment {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        self.seq.partial_cmp(&other.seq)
    }
}

impl Ord for OutOfOrderSegment {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.seq.cmp(&other.seq)
    }
}

/// Retransmission timer handler
struct RetransTimer {
    socket: Weak<TcpSocket>,
    seq: u32,
}

impl crate::timer::TimerHandler for RetransTimer {
    fn on_timer_expired(self: Arc<Self>, _context: usize) {
        if let Some(socket) = self.socket.upgrade() {
            let handler: Arc<dyn crate::timer::TimerHandler> = self.clone();
            if socket.take_active_retrans_timer(&handler) {
                socket.handle_retrans_timeout(self.seq);
            }
        }
    }
}

struct ActiveRetransTimer {
    handle: crate::timer::TimerHandle,
    handler: Arc<dyn crate::timer::TimerHandler>,
}

/// TCP socket (full implementation)
pub struct TcpSocket {
    /// TCP connection state
    state: IrqSpinLock<TcpState>,

    /// Local IP address
    local_ip: IrqSpinLock<Option<Ipv4Address>>,
    /// Local port
    pub(crate) local_port: AtomicU16,

    /// Remote IP address
    remote_ip: IrqSpinLock<Option<Ipv4Address>>,
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
    send_buffer: IrqSpinLock<VecDeque<u8>>,
    recv_buffer: IrqSpinLock<VecDeque<u8>>,
    /// Serializes payload writes so sequence numbers and rollback stay ordered.
    transmit_lock: IrqSpinLock<()>,

    /// Reference to TCP layer
    tcp_layer: Weak<TcpLayer>,
    /// Weak self reference for registration
    self_weak: Weak<TcpSocket>,
    /// Pending accepted connections (listener only)
    pending_accept: IrqSpinLock<VecDeque<Arc<TcpSocket>>>,
    /// Half-open connections waiting for the final ACK (listener only).
    pending_syn: IrqSpinLock<VecDeque<Arc<TcpSocket>>>,
    /// Maximum backlog size (from listen())
    max_backlog: AtomicUsize,

    /// Statistics
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,

    /// RTO (Retransmission Timeout) calculation - RFC 6298
    /// Smoothed RTT in nanoseconds, scaled by eight for fixed-point arithmetic.
    srtt_ns: AtomicU64,
    /// RTT variation in nanoseconds, scaled by four for fixed-point arithmetic.
    rttvar_ns: AtomicU64,
    /// Current retransmission timeout in nanoseconds.
    rto_ns: AtomicU64,
    /// Retransmission count for exponential backoff
    retrans_count: AtomicU16,
    /// Active retransmission timer and its strongly retained callback.
    retrans_timer: IrqSpinLock<Option<ActiveRetransTimer>>,
    /// Timestamp of last segment transmission (for RTT measurement)
    last_send_time: AtomicU64,
    /// Whether we're timing an RTT measurement (Karn's algorithm)
    timing_rtt: AtomicU16,
    /// Sequence number being timed
    timed_seq: AtomicU32,

    /// List of unacknowledged segments for retransmission
    unacked_segments: IrqSpinLock<VecDeque<UnackedSegment>>,

    /// Out-of-order segments for reassembly (sorted by sequence number)
    out_of_order: IrqSpinLock<BTreeMap<u32, OutOfOrderSegment>>,

    /// Waker for blocking accept() operations
    accept_waker: IrqSpinLock<Option<Arc<crate::sync::Waker>>>,
    /// Waker for blocking recv() operations
    recv_waker: IrqSpinLock<Option<Arc<crate::sync::Waker>>>,
    /// Waker for blocking send() operations
    send_waker: IrqSpinLock<Option<Arc<crate::sync::Waker>>>,
    /// Block mode: true for blocking, false for non-blocking
    blocking_mode: AtomicBool,
    /// Read timeout in milliseconds. Zero means no timeout.
    read_timeout_ms: AtomicU64,
    /// Write timeout in milliseconds. Zero means no timeout.
    write_timeout_ms: AtomicU64,
    /// Direct peer for in-kernel loopback connections.
    loopback_peer: IrqSpinLock<Weak<TcpSocket>>,
    /// Listening socket that should receive this socket once the handshake completes.
    accept_listener: IrqSpinLock<Weak<TcpSocket>>,
    /// Whether this socket has already been queued for accept().
    accept_queued: AtomicBool,

    /// Duplicate ACK count for Fast Retransmit
    dup_ack_count: AtomicU16,
    /// Last ACK sequence number for detecting duplicates
    last_ack_seq: AtomicU32,
}

impl TcpSocket {
    const INITIAL_RTO_NS: u64 = crate::timer::ms_to_ns(1_000);
    const MIN_RTO_NS: u64 = crate::timer::ms_to_ns(10);
    const MAX_RTO_NS: u64 = crate::timer::ms_to_ns(120_000);
    const MAX_SEGMENT_TRANSMISSIONS: u16 = 12;

    /// Safely downcast a SocketObject to TcpSocket using Any trait
    ///
    /// Returns None if socket is not a TcpSocket.
    /// This is completely safe and does not use any unsafe code.
    pub fn from_socket_object(socket: &dyn SocketObject) -> Option<&Self> {
        socket.as_any().downcast_ref::<TcpSocket>()
    }

    /// Blocking accept - waits for a connection
    pub fn accept_blocking(
        &self,
        task_id: usize,
        trapframe: &mut crate::arch::Trapframe,
    ) -> Result<Arc<dyn SocketObject>, SocketError> {
        if self.get_state() != TcpState::Listen {
            return Err(SocketError::NotListening);
        }

        let nonblocking = !self.blocking_mode.load(Ordering::SeqCst);

        loop {
            {
                let mut pending = self.pending_accept.lock();
                if let Some(socket) = pending.pop_front() {
                    return Ok(socket as Arc<dyn SocketObject>);
                }
            }

            if nonblocking {
                return Err(SocketError::WouldBlock);
            }

            let waker = {
                let mut waker_lock = self.accept_waker.lock();
                waker_lock
                    .get_or_insert_with(|| {
                        Arc::new(crate::sync::Waker::new_interruptible("tcp_accept"))
                    })
                    .clone()
            };

            {
                let mut pending = self.pending_accept.lock();
                if let Some(socket) = pending.pop_front() {
                    return Ok(socket as Arc<dyn SocketObject>);
                }
            }

            if waker.wait_result(task_id, trapframe) == WaitResult::Interrupted {
                return Err(SocketError::Interrupted);
            }
        }
    }

    /// Create a new TCP socket
    pub fn new(tcp_layer: Weak<TcpLayer>) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            state: IrqSpinLock::new(TcpState::Closed),
            local_ip: IrqSpinLock::new(None),
            local_port: AtomicU16::new(0),
            remote_ip: IrqSpinLock::new(None),
            remote_port: AtomicU16::new(0),
            send_seq: AtomicU32::new(0),
            send_unacked: AtomicU32::new(0),
            recv_seq: AtomicU32::new(0),
            recv_ack: AtomicU32::new(0),
            send_window: AtomicU16::new(65535),
            recv_window: AtomicU16::new(65535),
            send_buffer: IrqSpinLock::new(VecDeque::new()),
            recv_buffer: IrqSpinLock::new(VecDeque::new()),
            transmit_lock: IrqSpinLock::new(()),
            tcp_layer,
            self_weak: weak.clone(),
            pending_accept: IrqSpinLock::new(VecDeque::new()),
            pending_syn: IrqSpinLock::new(VecDeque::new()),
            max_backlog: AtomicUsize::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),

            // RTO initialization - RFC 6298
            srtt_ns: AtomicU64::new(0),
            rttvar_ns: AtomicU64::new(0),
            rto_ns: AtomicU64::new(Self::INITIAL_RTO_NS),
            retrans_count: AtomicU16::new(0),
            retrans_timer: IrqSpinLock::new(None),
            last_send_time: AtomicU64::new(0),
            timing_rtt: AtomicU16::new(0),
            timed_seq: AtomicU32::new(0),

            // Unacked segments list
            unacked_segments: IrqSpinLock::new(VecDeque::new()),

            // Out-of-order segments map
            out_of_order: IrqSpinLock::new(BTreeMap::new()),

            // Blocking support
            accept_waker: IrqSpinLock::new(None),
            recv_waker: IrqSpinLock::new(None),
            send_waker: IrqSpinLock::new(None),
            blocking_mode: AtomicBool::new(true), // Default to blocking mode
            read_timeout_ms: AtomicU64::new(0),
            write_timeout_ms: AtomicU64::new(0),
            loopback_peer: IrqSpinLock::new(Weak::new()),
            accept_listener: IrqSpinLock::new(Weak::new()),
            accept_queued: AtomicBool::new(false),

            // Fast Retransmit - duplicate ACK tracking
            dup_ack_count: AtomicU16::new(0),
            last_ack_seq: AtomicU32::new(0),
        })
    }

    fn queue_established_accept(&self) {
        let listener = match self.accept_listener.lock().upgrade() {
            Some(listener) => listener,
            None => return,
        };
        let socket = match self.self_weak.upgrade() {
            Some(socket) => socket,
            None => return,
        };

        {
            let max_backlog = listener.max_backlog.load(Ordering::SeqCst);
            let mut pending = listener.pending_accept.lock();
            if pending.len() >= max_backlog {
                return;
            }
            if self.accept_queued.swap(true, Ordering::SeqCst) {
                return;
            }
            pending.push_back(socket);
        }

        {
            let self_ptr = self as *const TcpSocket;
            let mut pending_syn = listener.pending_syn.lock();
            pending_syn.retain(|socket| Arc::as_ptr(socket) != self_ptr);
        }

        if let Some(waker) = listener.accept_waker.lock().as_ref() {
            waker.wake_one();
        }
    }

    fn deliver_loopback_data(&self, data: &[u8]) -> Result<Option<usize>, SocketError> {
        let peer = match self.loopback_peer.lock().upgrade() {
            Some(peer) => peer,
            None => return Ok(None),
        };

        {
            let mut recv_buf = peer.recv_buffer.lock();
            if recv_buf.len() + data.len() > MAX_RECV_BUFFER_SIZE {
                return Err(SocketError::WouldBlock);
            }
            recv_buf.extend(data);
        }

        peer.bytes_received
            .fetch_add(data.len() as u64, Ordering::SeqCst);
        self.bytes_sent
            .fetch_add(data.len() as u64, Ordering::SeqCst);

        if let Some(waker) = peer.recv_waker.lock().as_ref() {
            waker.wake_one();
        }

        Ok(Some(data.len()))
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
        let mut local_ip = self.local_ip.lock();
        let needs_update = match *local_ip {
            Some(ip) => ip.0 == [0, 0, 0, 0],
            None => true,
        };
        if !needs_update {
            return;
        }

        let manager = get_network_manager();
        if let Some(default_iface) = manager.get_default_interface() {
            if let Some(ip_layer) = manager.get_layer("ip") {
                if let Some(ip) = ip_layer
                    .as_any()
                    .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
                {
                    if let Some(addr) = ip.get_primary_ip(default_iface.name()) {
                        *local_ip = Some(addr);
                    }
                }
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

    fn try_loopback_connect(&self, addr: Ipv4Address, port: u16) -> Result<bool, SocketError> {
        if addr.0[0] != 127 {
            return Ok(false);
        }

        let tcp_layer = self
            .tcp_layer
            .upgrade()
            .ok_or(SocketError::InvalidOperation)?;
        let listener = match tcp_layer.find_listening_socket(port) {
            Some(listener) => listener,
            None => return Ok(false),
        };

        let local_port = self.local_port.load(Ordering::SeqCst);
        if local_port == 0 {
            return Err(SocketError::InvalidOperation);
        }

        let local_ip = self
            .local_ip
            .lock()
            .clone()
            .unwrap_or(Ipv4Address::new(127, 0, 0, 1));
        let listener_ip = listener
            .local_ip
            .lock()
            .clone()
            .unwrap_or(Ipv4Address::new(127, 0, 0, 1));

        let child = TcpSocket::new(Arc::downgrade(&tcp_layer));
        *child.local_ip.lock() = Some(listener_ip);
        child.local_port.store(port, Ordering::SeqCst);
        *child.remote_ip.lock() = Some(local_ip);
        child.remote_port.store(local_port, Ordering::SeqCst);
        child.send_seq.store(1000, Ordering::SeqCst);
        child.send_unacked.store(1000, Ordering::SeqCst);
        child.recv_seq.store(1000, Ordering::SeqCst);
        child.recv_ack.store(1000, Ordering::SeqCst);
        child.set_state(TcpState::Established);
        child.accept_queued.store(true, Ordering::SeqCst);
        *child.loopback_peer.lock() = self.self_weak.clone();

        *self.remote_ip.lock() = Some(listener_ip);
        self.remote_port.store(port, Ordering::SeqCst);
        self.send_seq.store(1000, Ordering::SeqCst);
        self.send_unacked.store(1000, Ordering::SeqCst);
        self.recv_seq.store(1000, Ordering::SeqCst);
        self.recv_ack.store(1000, Ordering::SeqCst);
        self.set_state(TcpState::Established);
        *self.loopback_peer.lock() = child.self_weak.clone();

        {
            let max_backlog = listener.max_backlog.load(Ordering::SeqCst);
            let mut pending = listener.pending_accept.lock();
            if pending.len() >= max_backlog {
                return Err(SocketError::ConnectionRefused);
            }
            pending.push_back(child);
        }

        if let Some(waker) = listener.accept_waker.lock().as_ref() {
            waker.wake_one();
        }

        Ok(true)
    }

    fn allocate_ephemeral_port(&self) -> u16 {
        static NEXT_EPHEMERAL_PORT: AtomicU16 = AtomicU16::new(49152);

        let port = NEXT_EPHEMERAL_PORT.fetch_add(1, Ordering::SeqCst);
        if port == u16::MAX {
            NEXT_EPHEMERAL_PORT.store(49152, Ordering::SeqCst);
        }
        if port < 49152 { 49152 } else { port }
    }

    fn timeout_ms_to_ns(timeout_ms: u64) -> Option<u64> {
        if timeout_ms == 0 {
            None
        } else {
            Some(timeout_ms.saturating_mul(crate::timer::NANOSECONDS_PER_MILLISECOND))
        }
    }

    fn read_timeout_ns(&self) -> Option<u64> {
        Self::timeout_ms_to_ns(self.read_timeout_ms.load(Ordering::SeqCst))
    }

    fn write_timeout_ns(&self) -> Option<u64> {
        Self::timeout_ms_to_ns(self.write_timeout_ms.load(Ordering::SeqCst))
    }

    fn set_read_timeout_ms(&self, timeout_ms: usize) -> Result<(), SocketError> {
        if timeout_ms > MAX_SOCKET_TIMEOUT_MS {
            return Err(SocketError::InvalidArgument);
        }
        self.read_timeout_ms
            .store(timeout_ms as u64, Ordering::SeqCst);
        Ok(())
    }

    fn set_write_timeout_ms(&self, timeout_ms: usize) -> Result<(), SocketError> {
        if timeout_ms > MAX_SOCKET_TIMEOUT_MS {
            return Err(SocketError::InvalidArgument);
        }
        self.write_timeout_ms
            .store(timeout_ms as u64, Ordering::SeqCst);
        Ok(())
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
        let src_port = header.src_port;
        let dst_port = header.dst_port;
        if should_log_tcp_https(src_port, dst_port) {
            let seq = header.seq_number;
            let ack = header.ack_number;
            let flags = header.flags();
            let expected = self.recv_seq.load(Ordering::SeqCst);
            let send_unacked = self.send_unacked.load(Ordering::SeqCst);
            let send_seq = self.send_seq.load(Ordering::SeqCst);
            crate::println!(
                "[tcp] rx {}.{}.{}.{}:{} -> local:{} state={:?} flags=0x{:02x} seq={} ack={} len={} expected={} send={}..{}",
                src_ip.0[0],
                src_ip.0[1],
                src_ip.0[2],
                src_ip.0[3],
                src_port,
                dst_port,
                current_state,
                flags,
                seq,
                ack,
                data.len(),
                expected,
                send_unacked,
                send_seq
            );
        }

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
                    *child.accept_listener.lock() = self.self_weak.clone();
                    child.handle_syn_received(src_ip, header);
                    tcp_layer.register_port(local_port, child.self_weak.clone());
                    self.pending_syn.lock().push_back(Arc::clone(&child));
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
                    self.handle_rst();
                }
            }
            TcpState::SynReceived => {
                if header.flags() & tcp_flags::RST != 0 {
                    self.handle_rst();
                    return;
                }

                if header.flags() & tcp_flags::ACK != 0 {
                    let expected_ack = self.send_seq.load(Ordering::SeqCst);
                    let ack_number = header.ack_number;
                    if ack_number == expected_ack {
                        self.update_send_window(ack_number);
                        self.set_state(TcpState::Established);
                        self.queue_established_accept();

                        if !data.is_empty() {
                            self.handle_data_segment(src_ip, header, data);
                        } else if header.flags() & tcp_flags::FIN != 0 {
                            self.handle_fin(src_ip, header);
                        }
                    } else if should_log_tcp_https(src_port, dst_port) {
                        crate::println!(
                            "[tcp] syn-received ACK mismatch: ack={} expected={}",
                            ack_number,
                            expected_ack
                        );
                    }
                }
            }
            state if tcp_receive_side_open(state) => {
                if data.is_empty() {
                    self.handle_control_segment(src_ip, header);
                } else {
                    self.handle_data_segment(src_ip, header, data);
                }
            }
            TcpState::CloseWait | TcpState::Closing | TcpState::LastAck => {
                if data.is_empty() {
                    self.handle_control_segment(src_ip, header);
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
        let initial_seq = 1000u32;
        let next_recv = header.seq_number.wrapping_add(1);
        self.send_seq
            .store(initial_seq.wrapping_add(1), Ordering::SeqCst);
        self.recv_seq.store(next_recv, Ordering::SeqCst);
        self.recv_ack.store(next_recv, Ordering::SeqCst);
        self.set_state(TcpState::SynReceived);

        // Send SYN-ACK
        let local_port = self.local_port.load(Ordering::SeqCst);
        let remote_port = header.src_port;
        let mut syn_ack = TcpHeader::new(local_port, remote_port);
        syn_ack.seq_number = initial_seq;
        syn_ack.ack_number = next_recv;
        syn_ack.set_flags(tcp_flags::SYN | tcp_flags::ACK);
        let _ = self.send_segment(src_ip, syn_ack, &[], false, false);
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
        self.update_send_window(acked);
        self.stop_rtt_measurement(acked);
        self.remove_acked_segments(acked);

        self.set_state(TcpState::Established);
        if let Some(waker) = self.send_waker.lock().as_ref() {
            waker.wake_all();
        }

        self.send_ack(src_ip, header.src_port, next_recv);
    }

    /// Handle RST (Reset) - properly cleanup connection
    fn handle_rst(&self) {
        // Cancel retransmission timers
        self.cancel_retrans_timer();

        // Clear send buffer
        self.send_buffer.lock().clear();

        // Clear receive buffer
        self.recv_buffer.lock().clear();

        // Clear unacked segments
        self.unacked_segments.lock().clear();

        // Clear out-of-order segments
        self.out_of_order.lock().clear();

        // Reset sequence numbers
        self.send_seq.store(0, Ordering::SeqCst);
        self.send_unacked.store(0, Ordering::SeqCst);
        self.recv_seq.store(0, Ordering::SeqCst);
        self.recv_ack.store(0, Ordering::SeqCst);

        // Reset window sizes
        self.send_window.store(65535, Ordering::SeqCst);
        self.recv_window.store(65535, Ordering::SeqCst);

        // Reset RTO state
        self.srtt_ns.store(0, Ordering::SeqCst);
        self.rttvar_ns.store(0, Ordering::SeqCst);
        self.rto_ns.store(Self::INITIAL_RTO_NS, Ordering::SeqCst);
        self.retrans_count.store(0, Ordering::SeqCst);
        self.timing_rtt.store(0, Ordering::SeqCst);

        // Clear addresses
        *self.local_ip.lock() = None;
        *self.remote_ip.lock() = None;
        self.local_port.store(0, Ordering::SeqCst);
        self.remote_port.store(0, Ordering::SeqCst);

        // Set state to Closed
        self.set_state(TcpState::Closed);
        if let Some(waker) = self.send_waker.lock().as_ref() {
            waker.wake_all();
        }
        if let Some(waker) = self.recv_waker.lock().as_ref() {
            waker.wake_all();
        }
    }

    /// Handle control segment (ACK, FIN, RST)
    fn handle_control_segment(&self, src_ip: Ipv4Address, header: TcpHeader) {
        if header.flags() & tcp_flags::RST != 0 {
            self.handle_rst();
            return;
        }

        if header.flags() & tcp_flags::FIN != 0 {
            self.handle_fin(src_ip, header);
        }

        if header.flags() & tcp_flags::ACK != 0 {
            self.update_send_window(header.ack_number);
            self.stop_rtt_measurement(header.ack_number);
            self.remove_acked_segments(header.ack_number);
            self.handle_close_ack(header.ack_number);
        }
    }

    /// Handle ACKs that advance TCP close state.
    fn handle_close_ack(&self, ack_number: u32) {
        let send_seq = self.send_seq.load(Ordering::SeqCst);
        if !is_seq_acknowledged(send_seq, ack_number) {
            return;
        }

        match self.get_state() {
            TcpState::FinWait1 => self.set_state(TcpState::FinWait2),
            TcpState::Closing => self.set_state(TcpState::TimeWait),
            TcpState::LastAck => self.set_state(TcpState::Closed),
            _ => {}
        }
    }

    /// Handle data segment
    fn handle_data_segment(&self, src_ip: Ipv4Address, header: TcpHeader, data: &[u8]) {
        if header.flags() & tcp_flags::RST != 0 {
            self.handle_rst();
            return;
        }

        // Check sequence number
        let expected_seq = self.recv_seq.load(Ordering::SeqCst);
        let mut segment_seq = header.seq_number;
        let mut payload = data;
        let segment_end = segment_seq.wrapping_add(payload.len() as u32);

        // Old segment (duplicate) - send ACK
        if seq_before_or_equal(segment_end, expected_seq) {
            if should_log_tcp_https(header.src_port, header.dst_port) {
                crate::println!(
                    "[tcp] drop duplicate seq={} end={} expected={} len={}",
                    segment_seq,
                    segment_end,
                    expected_seq,
                    payload.len()
                );
            }
            self.send_ack(src_ip, header.src_port, expected_seq);
            return;
        }

        // Partially duplicate segment - trim the bytes we already accepted.
        if seq_before(segment_seq, expected_seq) {
            let skip = expected_seq.wrapping_sub(segment_seq) as usize;
            if skip >= payload.len() {
                if should_log_tcp_https(header.src_port, header.dst_port) {
                    crate::println!(
                        "[tcp] drop fully overlapped seq={} expected={} len={} skip={}",
                        segment_seq,
                        expected_seq,
                        payload.len(),
                        skip
                    );
                }
                self.send_ack(src_ip, header.src_port, expected_seq);
                return;
            }
            if should_log_tcp_https(header.src_port, header.dst_port) {
                crate::println!(
                    "[tcp] trim overlap seq={} expected={} len={} skip={}",
                    segment_seq,
                    expected_seq,
                    payload.len(),
                    skip
                );
            }
            payload = &payload[skip..];
            segment_seq = expected_seq;
        }

        // Out-of-order segment - buffer it
        if seq_after(segment_seq, expected_seq) {
            if !payload.is_empty() {
                let mut out_of_order = self.out_of_order.lock();
                // Check if segment already buffered
                if !out_of_order.contains_key(&segment_seq) {
                    // Check out-of-order buffer limit
                    if out_of_order.len() < 128 {
                        let ooo_seg = OutOfOrderSegment {
                            seq: segment_seq,
                            data: payload.to_vec(),
                            flags: header.flags(),
                        };
                        out_of_order.insert(segment_seq, ooo_seg);
                    }
                }
                if should_log_tcp_https(header.src_port, header.dst_port) {
                    crate::println!(
                        "[tcp] queue ooo seq={} expected={} len={} ooo_count={}",
                        segment_seq,
                        expected_seq,
                        payload.len(),
                        out_of_order.len()
                    );
                }
                drop(out_of_order);

                // Send ACK for expected sequence
                self.send_ack(src_ip, header.src_port, expected_seq);
            }

            // Process ACK if present
            if header.flags() & tcp_flags::ACK != 0 {
                self.update_send_window(header.ack_number);
                self.stop_rtt_measurement(header.ack_number);
                self.remove_acked_segments(header.ack_number);
                self.handle_close_ack(header.ack_number);
            }
            return;
        }

        // In-order segment (segment_seq == expected_seq)
        if !payload.is_empty() {
            let mut recv_buf = self.recv_buffer.lock();

            // Check receive buffer limit - drop data if full (should update window to 0)
            if recv_buf.len() + payload.len() > MAX_RECV_BUFFER_SIZE {
                // Buffer full - send ACK with window=0 to stop sender
                if should_log_tcp_https(header.src_port, header.dst_port) {
                    crate::println!(
                        "[tcp] drop recv full seq={} expected={} len={} recv_len={}",
                        segment_seq,
                        expected_seq,
                        payload.len(),
                        recv_buf.len()
                    );
                }
                self.recv_window.store(0, Ordering::SeqCst);
                drop(recv_buf);
                self.send_ack(src_ip, header.src_port, expected_seq);
                return;
            }

            recv_buf.extend(payload);
            let mut next_seq = expected_seq.wrapping_add(payload.len() as u32);
            let mut reassembled_fin = false;

            // Check if we can reassemble from out-of-order buffer
            let mut out_of_order = self.out_of_order.lock();
            loop {
                // Remove and process next consecutive segment from out-of-order buffer
                if let Some((seq, ooo_seg)) = out_of_order.first_key_value() {
                    let seq = *seq;
                    let seg_end = seq.wrapping_add(ooo_seg.data.len() as u32);
                    if seq_after(seq, next_seq) {
                        // Gap found, stop reassembly.
                        break;
                    }

                    let ooo_seg = out_of_order.remove(&seq).unwrap();
                    if seq_before_or_equal(seg_end, next_seq) {
                        if (ooo_seg.flags & tcp_flags::FIN) != 0 && seg_end == next_seq {
                            reassembled_fin = true;
                        }
                        // Entire buffered segment is already covered.
                        continue;
                    }

                    let skip = next_seq.wrapping_sub(seq) as usize;
                    let new_data = &ooo_seg.data[skip..];

                    // Check buffer limit before adding out-of-order data
                    if recv_buf.len() + new_data.len() <= MAX_RECV_BUFFER_SIZE {
                        recv_buf.extend(new_data);
                        next_seq = next_seq.wrapping_add(new_data.len() as u32);
                        if (ooo_seg.flags & tcp_flags::FIN) != 0 && seg_end == next_seq {
                            reassembled_fin = true;
                        }
                    } else {
                        // Buffer full, stop reassembly
                        break;
                    }
                } else {
                    // No more out-of-order segments
                    break;
                }
            }
            drop(out_of_order);

            // Update receive window based on remaining buffer space
            let available = MAX_RECV_BUFFER_SIZE.saturating_sub(recv_buf.len());
            self.recv_window
                .store(available.min(65535) as u16, Ordering::SeqCst);

            self.recv_seq.store(next_seq, Ordering::SeqCst);
            self.recv_ack.store(next_seq, Ordering::SeqCst);
            if should_log_tcp_https(header.src_port, header.dst_port) {
                crate::println!(
                    "[tcp] accept data seq={} len={} next={} recv_len={}",
                    segment_seq,
                    payload.len(),
                    next_seq,
                    recv_buf.len()
                );
            }
            drop(recv_buf);

            // Wake up any blocking recv() calls
            if let Some(waker) = self.recv_waker.lock().as_ref() {
                waker.wake_one();
            }

            if (header.flags() & tcp_flags::FIN != 0) || reassembled_fin {
                self.finish_in_order_fin(src_ip, header, next_seq.wrapping_add(1));
            } else {
                // Send ACK for the new next_seq (may acknowledge multiple segments)
                self.send_ack(src_ip, header.src_port, next_seq);
            }

            // Update received bytes
            self.bytes_received
                .fetch_add(payload.len() as u64, Ordering::SeqCst);
        }

        // Process ACK if present
        if header.flags() & tcp_flags::ACK != 0 {
            self.update_send_window(header.ack_number);
            self.stop_rtt_measurement(header.ack_number);
            self.remove_acked_segments(header.ack_number);
            self.handle_close_ack(header.ack_number);
        }
    }

    /// Handle FIN segment
    fn handle_fin(&self, src_ip: Ipv4Address, header: TcpHeader) {
        let current_state = self.get_state();
        let ack_seq = header.seq_number.wrapping_add(1);
        self.recv_seq.store(ack_seq, Ordering::SeqCst);
        self.recv_ack.store(ack_seq, Ordering::SeqCst);
        self.send_ack(src_ip, header.src_port, ack_seq);

        let local_fin_acknowledged = (header.flags() & tcp_flags::ACK) != 0
            && is_seq_acknowledged(self.send_seq.load(Ordering::SeqCst), header.ack_number);
        let next_state = state_after_peer_fin(current_state, local_fin_acknowledged);
        if next_state != current_state {
            self.set_state(next_state);
        }
        if let Some(waker) = self.recv_waker.lock().as_ref() {
            waker.wake_all();
        }
    }

    /// Update send window based on acknowledgment
    fn update_send_window(&self, ack_number: u32) {
        let previous_unacked = self.send_unacked.load(Ordering::SeqCst);
        let send_seq = self.send_seq.load(Ordering::SeqCst);
        let mut acknowledged = if seq_after(ack_number, send_seq) {
            send_seq
        } else {
            ack_number
        };
        if seq_before(acknowledged, previous_unacked) {
            acknowledged = previous_unacked;
        }

        if seq_after(acknowledged, previous_unacked) {
            let acked_bytes = acknowledged.wrapping_sub(previous_unacked) as usize;
            let mut send_buf = self.send_buffer.lock();
            let drain_len = acked_bytes.min(send_buf.len());
            drop(send_buf.drain(..drain_len));
        }

        // Track the latest acknowledged sequence number
        self.send_unacked.store(acknowledged, Ordering::SeqCst);

        // Fast Retransmit - duplicate ACK detection
        let last_ack = self.last_ack_seq.load(Ordering::SeqCst);
        if acknowledged == last_ack {
            // Duplicate ACK - increment counter
            let count = self.dup_ack_count.fetch_add(1, Ordering::SeqCst);

            // If we've received 3 duplicate ACKs, trigger fast retransmit
            if count >= 2 {
                self.fast_retransmit();
                self.dup_ack_count.store(0, Ordering::SeqCst);
            }
        } else {
            // New ACK - reset duplicate counter
            self.last_ack_seq.store(acknowledged, Ordering::SeqCst);
            self.dup_ack_count.store(0, Ordering::SeqCst);
        }

        // Wake up any blocking send() calls (buffer may have space now)
        if let Some(waker) = self.send_waker.lock().as_ref() {
            waker.wake_one();
        }
    }

    /// Fast Retransmit - immediately retransmit unacknowledged segments
    fn fast_retransmit(&self) {
        let first_seg = { self.unacked_segments.lock().front().cloned() };
        let Some(first_seg) = first_seg else {
            return;
        };
        let Some(dest_ip) = self.remote_ip.lock().clone() else {
            return;
        };

        let dest_port = self.remote_port.load(Ordering::SeqCst);
        let local_port = self.local_port.load(Ordering::SeqCst);
        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = first_seg.seq;
        header.ack_number = self.recv_ack.load(Ordering::SeqCst);
        header.set_flags(first_seg.flags);

        // An ACK after retransmission cannot identify which transmission it
        // acknowledges, so Karn's algorithm excludes it from RTT sampling.
        self.timing_rtt.store(0, Ordering::SeqCst);
        let _ = self.send_segment(dest_ip, header, &first_seg.data, false, true);

        let rearm_seq = {
            let mut unacked = self.unacked_segments.lock();
            let Some(current_head) = unacked.front_mut() else {
                return;
            };
            if current_head.seq != first_seg.seq {
                return;
            }
            current_head.tx_count = current_head.tx_count.saturating_add(1);
            current_head.last_tx_time = crate::timer::get_time_ns();
            Some(current_head.seq)
        };

        self.retrans_count.store(1, Ordering::SeqCst);
        if let Some(seq) = rearm_seq {
            self.schedule_retrans_timer(seq);
        }
    }

    /// Send SYN packet
    fn send_syn(&self, dest_ip: Ipv4Address, dest_port: u16) {
        let local_port = self.local_port.load(Ordering::SeqCst);

        let initial_seq = 1000;
        self.send_seq.store(initial_seq, Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = initial_seq;
        header.set_flags(tcp_flags::SYN);

        self.set_state(TcpState::SynSent);
        let _ = self.send_segment(dest_ip, header, &[], true, false);
    }

    /// Send SYN-ACK packet
    fn send_syn_ack(&self, dest_ip: Ipv4Address, dest_port: u16, their_seq: u32, ack_seq: u32) {
        let local_port = self.local_port.load(Ordering::SeqCst);

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = their_seq;
        header.ack_number = ack_seq;
        header.set_flags(tcp_flags::SYN | tcp_flags::ACK);

        let _ = self.send_segment(dest_ip, header, &[], false, false);
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

        let _ = self.send_segment(dest_ip, header, &[], false, false);
    }

    fn update_recv_window_after_drain(&self, buffered_len: usize) -> bool {
        let available = MAX_RECV_BUFFER_SIZE.saturating_sub(buffered_len);
        let new_window = available.min(65535) as u16;
        let old_window = self.recv_window.swap(new_window, Ordering::SeqCst);
        new_window > old_window
            && (old_window == 0 || new_window.saturating_sub(old_window) >= WINDOW_UPDATE_THRESHOLD)
    }

    fn send_window_update_ack(&self) {
        if !tcp_receive_side_open(self.get_state()) {
            return;
        }
        let dest_ip = match self.remote_ip.lock().clone() {
            Some(ip) => ip,
            None => return,
        };
        let dest_port = self.remote_port.load(Ordering::SeqCst);
        if dest_port == 0 {
            return;
        }
        let ack_seq = self.recv_ack.load(Ordering::SeqCst);
        self.send_ack(dest_ip, dest_port, ack_seq);
    }

    /// Send FIN packet
    fn send_fin(&self) {
        let dest_ip = self.remote_ip.lock().clone().unwrap();
        let dest_port = self.remote_port.load(Ordering::SeqCst);
        let local_port = self.local_port.load(Ordering::SeqCst);

        let send_seq = self.send_seq.load(Ordering::SeqCst);
        let recv_seq = self.recv_seq.load(Ordering::SeqCst);
        let current_state = self.get_state();

        let mut header = TcpHeader::new(local_port, dest_port);
        header.seq_number = send_seq;
        header.ack_number = recv_seq;
        header.set_flags(tcp_flags::FIN | tcp_flags::ACK);

        let _ = self.send_segment(dest_ip, header, &[], true, false);
        if current_state == TcpState::CloseWait {
            self.set_state(TcpState::LastAck);
        } else {
            self.set_state(TcpState::FinWait1);
        }
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

        let _ = self.send_segment(dest_ip, header, &[], true, false);
    }

    /// Send TCP segment through IP layer
    fn send_segment(
        &self,
        dest_ip: Ipv4Address,
        mut header: TcpHeader,
        data: &[u8],
        update_seq: bool,
        is_retransmit: bool,
    ) -> Result<(), SocketError> {
        self.ensure_local_ip();
        let mut local_ip = self
            .local_ip
            .lock()
            .clone()
            .unwrap_or(Ipv4Address::new(0, 0, 0, 0));
        if dest_ip.0[0] == 127 && local_ip.0 == [0, 0, 0, 0] {
            local_ip = Ipv4Address::new(127, 0, 0, 1);
            *self.local_ip.lock() = Some(local_ip);
        }

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

        if dest_ip.0[0] == 127 {
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

            if let Some(tcp_layer) = self.tcp_layer.upgrade() {
                let _ = tcp_layer.receive_packet(local_ip, dest_ip, &segment);
            }
            return Ok(());
        }

        // Create IP context
        let mut ip_context = LayerContext::new();
        ip_context.set("ip_dst", &dest_ip.0);
        ip_context.set("ip_src", &local_ip.0);
        ip_context.set("ip_protocol", &[6]); // TCP protocol

        // Send through IP layer
        let ip_layer = get_network_manager()
            .get_layer("ip")
            .ok_or(SocketError::NoRoute)?;
        match ip_layer.send(&segment, &ip_context, &[]) {
            Ok(()) | Err(SocketError::WouldBlock) => {}
            Err(err) => return Err(err),
        }

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

        Ok(())
    }

    fn send_payload_segments(
        &self,
        dest_ip: Ipv4Address,
        dest_port: u16,
        data: &[u8],
    ) -> Result<usize, SocketError> {
        let local_port = self.local_port.load(Ordering::SeqCst);
        let mut sent = 0;

        for chunk in data.chunks(TCP_MAX_SEGMENT_DATA) {
            let mut header = TcpHeader::new(local_port, dest_port);
            header.seq_number = self.send_seq.load(Ordering::SeqCst);
            header.ack_number = self.recv_ack.load(Ordering::SeqCst);
            let is_last = sent + chunk.len() == data.len();
            header.set_flags(tcp_flags::ACK | if is_last { tcp_flags::PSH } else { 0 });

            match self.send_segment(dest_ip, header, chunk, true, false) {
                Ok(()) => sent += chunk.len(),
                Err(err) if sent == 0 => return Err(err),
                Err(_) => return Ok(sent),
            }
        }

        Ok(sent)
    }

    fn remove_unsent_buffer_tail(&self, unsent: usize) {
        if unsent == 0 {
            return;
        }
        let mut send_buf = self.send_buffer.lock();
        let retained = send_buf.len().saturating_sub(unsent);
        send_buf.truncate(retained);
    }

    /// Send data through socket
    pub fn send_data(&self, data: &[u8]) -> Result<usize, SocketError> {
        if !tcp_send_side_open(self.get_state()) {
            return Err(SocketError::NotConnected);
        }

        if let Some(len) = self.deliver_loopback_data(data)? {
            return Ok(len);
        }
        if data.is_empty() {
            return Ok(0);
        }

        let dest_ip = self
            .remote_ip
            .lock()
            .clone()
            .ok_or(SocketError::NotConnected)?;
        let dest_port = self.remote_port.load(Ordering::SeqCst);

        let _transmit_guard = self.transmit_lock.lock();

        // Check buffer size limit
        let mut send_buf = self.send_buffer.lock();
        if send_buf.len() + data.len() > MAX_SEND_BUFFER_SIZE {
            return Err(SocketError::WouldBlock);
        }
        send_buf.extend(data);
        drop(send_buf);

        let sent = match self.send_payload_segments(dest_ip, dest_port, data) {
            Ok(sent) => sent,
            Err(err) => {
                self.remove_unsent_buffer_tail(data.len());
                return Err(err);
            }
        };
        self.remove_unsent_buffer_tail(data.len() - sent);
        Ok(sent)
    }

    /// Blocking send - waits for buffer space
    pub fn send_blocking(
        &self,
        data: &[u8],
        task_id: usize,
        trapframe: &mut crate::arch::Trapframe,
    ) -> Result<usize, SocketError> {
        if !tcp_send_side_open(self.get_state()) {
            return Err(SocketError::NotConnected);
        }
        if data.is_empty() {
            return Ok(0);
        }

        let dest_ip = self
            .remote_ip
            .lock()
            .clone()
            .ok_or(SocketError::NotConnected)?;
        let dest_port = self.remote_port.load(Ordering::SeqCst);
        let nonblocking = !self.blocking_mode.load(Ordering::SeqCst);

        loop {
            match self.deliver_loopback_data(data) {
                Ok(Some(len)) => return Ok(len),
                Ok(None) => {}
                Err(SocketError::WouldBlock) if nonblocking => return Err(SocketError::WouldBlock),
                Err(SocketError::WouldBlock) => {
                    let waker = {
                        let mut waker_lock = self.send_waker.lock();
                        waker_lock
                            .get_or_insert_with(|| {
                                Arc::new(crate::sync::Waker::new_interruptible("tcp_send"))
                            })
                            .clone()
                    };
                    match waker.wait_with_timeout_result(
                        task_id,
                        trapframe,
                        self.write_timeout_ns(),
                    ) {
                        WaitResult::Woken => {}
                        WaitResult::TimedOut => return Err(SocketError::WouldBlock),
                        WaitResult::Interrupted => return Err(SocketError::Interrupted),
                    }
                    continue;
                }
                Err(err) => return Err(err),
            }

            {
                let _transmit_guard = self.transmit_lock.lock();
                let mut send_buf = self.send_buffer.lock();
                if send_buf.len() + data.len() <= MAX_SEND_BUFFER_SIZE {
                    send_buf.extend(data);
                    drop(send_buf);

                    let sent = match self.send_payload_segments(dest_ip, dest_port, data) {
                        Ok(sent) => sent,
                        Err(err) => {
                            self.remove_unsent_buffer_tail(data.len());
                            return Err(err);
                        }
                    };
                    self.remove_unsent_buffer_tail(data.len() - sent);
                    return Ok(sent);
                }
            }

            if nonblocking {
                return Err(SocketError::WouldBlock);
            }

            {
                let waker = {
                    let mut waker_lock = self.send_waker.lock();
                    waker_lock
                        .get_or_insert_with(|| {
                            Arc::new(crate::sync::Waker::new_interruptible("tcp_send"))
                        })
                        .clone()
                };
                {
                    let send_buf = self.send_buffer.lock();
                    if send_buf.len() + data.len() <= MAX_SEND_BUFFER_SIZE {
                        continue;
                    }
                }
                match waker.wait_with_timeout_result(task_id, trapframe, self.write_timeout_ns()) {
                    WaitResult::Woken => {}
                    WaitResult::TimedOut => return Err(SocketError::WouldBlock),
                    WaitResult::Interrupted => return Err(SocketError::Interrupted),
                }
            }
        }
    }

    /// Receive data from socket
    pub fn recv_data(&self, buffer: &mut [u8]) -> Result<usize, SocketError> {
        let (len, send_window_update) = {
            let mut recv_buf = self.recv_buffer.lock();
            let len = buffer.len().min(recv_buf.len());

            for i in 0..len {
                buffer[i] = recv_buf.pop_front().unwrap();
            }

            let send_window_update = self.update_recv_window_after_drain(recv_buf.len());
            (len, send_window_update)
        };

        if len > 0 {
            if send_window_update {
                self.send_window_update_ack();
            }
            return Ok(len);
        }

        match self.get_state() {
            state if tcp_receive_side_open(state) => Err(SocketError::WouldBlock),
            state if tcp_receive_side_eof(state) => Ok(0),
            _ => Err(SocketError::NotConnected),
        }
    }

    fn finish_in_order_fin(&self, src_ip: Ipv4Address, header: TcpHeader, fin_ack: u32) {
        self.recv_seq.store(fin_ack, Ordering::SeqCst);
        self.recv_ack.store(fin_ack, Ordering::SeqCst);
        self.send_ack(src_ip, header.src_port, fin_ack);
        let current_state = self.get_state();
        let local_fin_acknowledged = (header.flags() & tcp_flags::ACK) != 0
            && is_seq_acknowledged(self.send_seq.load(Ordering::SeqCst), header.ack_number);
        let next_state = state_after_peer_fin(current_state, local_fin_acknowledged);
        if next_state != current_state {
            self.set_state(next_state);
        }
        if let Some(waker) = self.recv_waker.lock().as_ref() {
            waker.wake_all();
        }
    }

    /// Blocking receive - waits for data to be available
    pub fn recv_blocking(
        &self,
        buffer: &mut [u8],
        task_id: usize,
        trapframe: &mut crate::arch::Trapframe,
    ) -> Result<usize, SocketError> {
        let nonblocking = !self.blocking_mode.load(Ordering::SeqCst);

        loop {
            {
                let mut recv_buf = self.recv_buffer.lock();
                let len = buffer.len().min(recv_buf.len());

                if len > 0 {
                    for i in 0..len {
                        buffer[i] = recv_buf.pop_front().unwrap();
                    }

                    let send_window_update = self.update_recv_window_after_drain(recv_buf.len());
                    drop(recv_buf);
                    if send_window_update {
                        self.send_window_update_ack();
                    }
                    return Ok(len);
                }
            }

            let state = self.get_state();
            if tcp_receive_side_eof(state) {
                return Ok(0);
            }
            if !tcp_receive_side_open(state) {
                return Err(SocketError::NotConnected);
            }

            if nonblocking {
                return Err(SocketError::WouldBlock);
            }

            {
                let waker = {
                    let mut waker_lock = self.recv_waker.lock();
                    waker_lock
                        .get_or_insert_with(|| {
                            Arc::new(crate::sync::Waker::new_interruptible("tcp_recv"))
                        })
                        .clone()
                };
                let state = self.get_state();
                if tcp_receive_side_eof(state) {
                    return Ok(0);
                }
                if !tcp_receive_side_open(state) {
                    return Err(SocketError::NotConnected);
                }
                if !self.recv_buffer.lock().is_empty() {
                    continue;
                }
                match waker.wait_with_timeout_result(task_id, trapframe, self.read_timeout_ns()) {
                    WaitResult::Woken => {}
                    WaitResult::TimedOut => return Err(SocketError::WouldBlock),
                    WaitResult::Interrupted => return Err(SocketError::Interrupted),
                }
            }
        }
    }

    // ===================================================================
    // RTO (Retransmission Timeout) - RFC 6298
    // ===================================================================

    /// Update RTO based on RTT measurement (Jacobson/Karels algorithm)
    /// Uses fixed-point arithmetic for better precision in no_std
    fn update_rto(&self, rtt_ns: u64) {
        // RFC 6298: RTO calculation
        // SRTT = (1 - alpha) * SRTT + alpha * RTT
        // RTTVAR = (1 - beta) * RTTVAR + beta * |SRTT - RTT|
        // RTO = SRTT + max(G, K * RTTVAR)
        // where alpha = 1/8, beta = 1/4, K = 4, G = clock granularity

        const ALPHA_SHIFT: u32 = 3; // alpha = 1/8
        const BETA_SHIFT: u32 = 2; // beta = 1/4
        const K: u64 = 4; // multiplier for RTTVAR

        let srtt = self.srtt_ns.load(Ordering::SeqCst);
        let rttvar = self.rttvar_ns.load(Ordering::SeqCst);

        if srtt == 0 {
            // First RTT measurement
            // SRTT = RTT
            // RTTVAR = RTT / 2
            self.srtt_ns
                .store(rtt_ns.saturating_mul(8), Ordering::SeqCst);
            self.rttvar_ns
                .store(rtt_ns.saturating_mul(2), Ordering::SeqCst);
        } else {
            // Subsequent measurements
            // RTTVAR = (1 - beta) * RTTVAR + beta * |SRTT - RTT|
            // SRTT = (1 - alpha) * SRTT + alpha * RTT
            let srtt_val = srtt >> 3; // Divide by 8
            let diff = if srtt_val > rtt_ns {
                srtt_val - rtt_ns
            } else {
                rtt_ns - srtt_val
            };

            // RTTVAR = (3/4) * RTTVAR + (1/4) * |diff|
            let new_rttvar = ((rttvar.saturating_mul(3)) >> BETA_SHIFT).saturating_add(diff);
            self.rttvar_ns.store(new_rttvar, Ordering::SeqCst);

            // SRTT = (7/8) * SRTT + (1/8) * RTT
            let new_srtt = ((srtt.saturating_mul(7)) >> ALPHA_SHIFT).saturating_add(rtt_ns);
            self.srtt_ns.store(new_srtt, Ordering::SeqCst);
        }

        // RTO = SRTT + max(G, K * RTTVAR)
        let srtt_ns = self.srtt_ns.load(Ordering::SeqCst) >> ALPHA_SHIFT;
        let rttvar_ns = self.rttvar_ns.load(Ordering::SeqCst) >> BETA_SHIFT;
        let mut rto_ns = srtt_ns.saturating_add(K.saturating_mul(rttvar_ns).max(Self::MIN_RTO_NS));

        // Clamp RTO to bounds
        rto_ns = rto_ns.clamp(Self::MIN_RTO_NS, Self::MAX_RTO_NS);

        self.rto_ns.store(rto_ns, Ordering::SeqCst);
    }

    /// Get the current RTO in nanoseconds.
    fn get_rto_ns(&self) -> u64 {
        self.rto_ns.load(Ordering::SeqCst)
    }

    /// Start RTT measurement for a sequence number
    fn start_rtt_measurement(&self, seq: u32) {
        // Only start timing if not already timing
        if self.timing_rtt.load(Ordering::SeqCst) == 0 {
            self.timed_seq.store(seq, Ordering::SeqCst);
            self.last_send_time
                .store(crate::timer::get_time_ns(), Ordering::SeqCst);
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
                let now = crate::timer::get_time_ns();
                if now > send_time {
                    self.update_rto(now - send_time);
                }
                self.timing_rtt.store(0, Ordering::SeqCst);
                // Reset retransmission count on successful ACK
                self.retrans_count.store(0, Ordering::SeqCst);
            }
        }
    }

    /// Exponential backoff for retransmission
    fn backoff_rto(&self) {
        let current_rto = self.rto_ns.load(Ordering::SeqCst);
        self.rto_ns.store(
            backed_off_retransmission_timeout_ns(current_rto),
            Ordering::SeqCst,
        );
        let count = self.retrans_count.load(Ordering::SeqCst);
        self.retrans_count
            .store(count.saturating_add(1), Ordering::SeqCst);
    }

    /// Handle retransmission timeout
    fn handle_retrans_timeout(&self, seq: u32) {
        // Check if socket is still in a valid state for retransmission
        let state = self.get_state();
        match state {
            TcpState::Closed | TcpState::Listen | TcpState::TimeWait => return,
            _ => {}
        }

        let retransmit = {
            let unacked = self.unacked_segments.lock();
            let Some(seg) = unacked.front().cloned() else {
                return;
            };
            if seg.seq != seq {
                return;
            }

            if seg.tx_count >= Self::MAX_SEGMENT_TRANSMISSIONS {
                self.set_state(TcpState::Closed);
                if let Some(waker) = self.send_waker.lock().as_ref() {
                    waker.wake_all();
                }
                if let Some(waker) = self.recv_waker.lock().as_ref() {
                    waker.wake_all();
                }
                return;
            }

            self.backoff_rto();

            let Some(dest_ip) = self.remote_ip.lock().clone() else {
                return;
            };
            let dest_port = self.remote_port.load(Ordering::SeqCst);
            let local_port = self.local_port.load(Ordering::SeqCst);

            let mut header = TcpHeader::new(local_port, dest_port);
            header.seq_number = seg.seq;
            header.ack_number = self.recv_ack.load(Ordering::SeqCst);
            header.set_flags(seg.flags);

            (seg, dest_ip, header)
        };

        // Do not use an ACK for a retransmitted segment as an RTT sample.
        self.timing_rtt.store(0, Ordering::SeqCst);
        let _ = self.send_segment(retransmit.1, retransmit.2, &retransmit.0.data, false, true);

        let rearm_seq = {
            let mut unacked = self.unacked_segments.lock();
            let Some(current_head) = unacked.front_mut() else {
                return;
            };
            if current_head.seq != retransmit.0.seq {
                return;
            }
            current_head.tx_count = current_head.tx_count.saturating_add(1);
            current_head.last_tx_time = crate::timer::get_time_ns();
            Some(current_head.seq)
        };

        if let Some(seq) = rearm_seq {
            self.schedule_retrans_timer(seq);
        }
    }

    /// Schedule the retransmission timer if `seq` is still the unacknowledged head.
    fn schedule_retrans_timer(&self, seq: u32) {
        let previous = {
            let unacked = self.unacked_segments.lock();
            let Some(head) = unacked.front() else {
                return;
            };
            if !retransmission_request_matches_head(seq, Some(head.seq)) {
                return;
            }

            let expires = retransmission_deadline_ns(head.last_tx_time, self.get_rto_ns());
            let timer: Arc<dyn crate::timer::TimerHandler> = Arc::new(RetransTimer {
                socket: self.self_weak.clone(),
                seq,
            });
            let handle =
                crate::timer::add_timer(expires, crate::timer::TimerPrecision::Normal, &timer, 0);
            self.retrans_timer.lock().replace(ActiveRetransTimer {
                handle,
                handler: timer,
            })
        };

        if let Some(previous) = previous {
            crate::timer::cancel_timer(previous.handle);
        }
    }

    /// Cancel retransmission timer
    fn cancel_retrans_timer(&self) {
        let active = {
            let mut timer_lock = self.retrans_timer.lock();
            timer_lock.take()
        };

        if let Some(active) = active {
            crate::timer::cancel_timer(active.handle);
        }
    }

    /// Clear the retained callback only when this timer owns the active slot.
    fn take_active_retrans_timer(&self, handler: &Arc<dyn crate::timer::TimerHandler>) -> bool {
        let active = {
            let mut timer_lock = self.retrans_timer.lock();
            if timer_lock
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(&active.handler, handler))
            {
                timer_lock.take()
            } else {
                None
            }
        };
        active.is_some()
    }

    /// Add segment to unacked list and schedule retransmission
    fn add_unacked_segment(&self, seq: u32, data: Vec<u8>, flags: u8) {
        // Check unacked segment limit to prevent memory exhaustion
        let mut unacked = self.unacked_segments.lock();
        let previous_head = unacked.front().map(|segment| segment.seq);
        if unacked.len() >= MAX_UNACKED_SEGMENTS {
            // Remove oldest segment if limit reached
            let _ = unacked.pop_front();
        }

        let segment = UnackedSegment {
            seq,
            data,
            flags,
            tx_count: 1,
            last_tx_time: crate::timer::get_time_ns(),
        };

        unacked.push_back(segment);
        let next_head = unacked.front().map(|segment| segment.seq);
        drop(unacked);

        if retransmission_head_changed(previous_head, next_head) {
            if let Some(seq) = next_head {
                self.schedule_retrans_timer(seq);
            } else {
                self.cancel_retrans_timer();
            }
        }

        // Start RTT measurement if not already timing
        self.start_rtt_measurement(seq);
    }

    /// Remove acknowledged segments from unacked list
    fn remove_acked_segments(&self, ack_seq: u32) {
        let mut unacked = self.unacked_segments.lock();
        let previous_head = unacked.front().map(|segment| segment.seq);
        // Remove all segments that are fully acknowledged
        unacked.retain(|seg| {
            let seg_end = seg.seq.wrapping_add(seg.data.len() as u32);
            // Keep if not fully acknowledged
            !is_seq_acknowledged(seg_end, ack_seq)
        });

        let next_head = unacked.front().map(|segment| segment.seq);
        drop(unacked);

        if retransmission_head_changed(previous_head, next_head) {
            if let Some(seq) = next_head {
                self.schedule_retrans_timer(seq);
            } else {
                self.cancel_retrans_timer();
            }
        }
    }
}

#[inline]
fn retransmission_head_changed(previous_head: Option<u32>, next_head: Option<u32>) -> bool {
    previous_head != next_head
}

#[inline]
fn retransmission_request_matches_head(requested_seq: u32, head_seq: Option<u32>) -> bool {
    head_seq == Some(requested_seq)
}

#[inline]
fn retransmission_deadline_ns(last_tx_time: u64, rto_ns: u64) -> u64 {
    last_tx_time.saturating_add(rto_ns)
}

#[inline]
fn backed_off_retransmission_timeout_ns(current_rto_ns: u64) -> u64 {
    current_rto_ns.saturating_mul(2).min(TcpSocket::MAX_RTO_NS)
}

/// Check if a sequence number is acknowledged by an ACK number
/// Handles sequence number wraparound
fn is_seq_acknowledged(seq: u32, ack: u32) -> bool {
    // Standard TCP sequence number comparison
    // Returns true if ack acknowledges seq
    seq_before_or_equal(seq, ack)
}

/// Return true if sequence number `a` is before or equal to `b`.
fn seq_before_or_equal(a: u32, b: u32) -> bool {
    a == b || seq_before(a, b)
}

/// Return true if sequence number `a` is before `b`.
fn seq_before(a: u32, b: u32) -> bool {
    a.wrapping_sub(b) > (1u32 << 31)
}

/// Return true if sequence number `a` is after `b`.
fn seq_after(a: u32, b: u32) -> bool {
    seq_before(b, a)
}

fn should_log_tcp_https(src_port: u16, dst_port: u16) -> bool {
    LOG_TCP_HTTPS && (src_port == 443 || dst_port == 443)
}

impl SocketObject for TcpSocket {
    fn socket_type(&self) -> crate::network::socket::SocketType {
        crate::network::socket::SocketType::Stream
    }

    fn socket_domain(&self) -> crate::network::socket::SocketDomain {
        crate::network::socket::SocketDomain::Inet4
    }

    fn socket_protocol(&self) -> crate::network::socket::SocketProtocol {
        crate::network::socket::SocketProtocol::Tcp
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_selectable(&self) -> Option<&dyn crate::object::capability::Selectable> {
        Some(self)
    }

    fn as_control_ops(&self) -> Option<&dyn crate::object::capability::ControlOps> {
        Some(self)
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

impl crate::object::capability::ControlOps for TcpSocket {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            crate::network::socket::socket_ctl::SCTL_SOCKET_SET_NONBLOCK => {
                crate::object::capability::selectable::Selectable::set_nonblocking(self, arg != 0);
                Ok(0)
            }
            crate::network::socket::socket_ctl::SCTL_SOCKET_GET_NONBLOCK => Ok(
                if crate::object::capability::selectable::Selectable::is_nonblocking(self) {
                    1
                } else {
                    0
                },
            ),
            crate::network::socket::socket_ctl::SCTL_SOCKET_SET_READ_TIMEOUT_MS => {
                self.set_read_timeout_ms(arg)
                    .map_err(|_| "Invalid read timeout")?;
                Ok(0)
            }
            crate::network::socket::socket_ctl::SCTL_SOCKET_SET_WRITE_TIMEOUT_MS => {
                self.set_write_timeout_ms(arg)
                    .map_err(|_| "Invalid write timeout")?;
                Ok(0)
            }
            crate::network::socket::socket_ctl::SCTL_SOCKET_GET_READ_TIMEOUT_MS => {
                Ok(self.read_timeout_ms.load(Ordering::SeqCst) as i32)
            }
            crate::network::socket::socket_ctl::SCTL_SOCKET_GET_WRITE_TIMEOUT_MS => {
                Ok(self.write_timeout_ms.load(Ordering::SeqCst) as i32)
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
            (
                crate::network::socket::socket_ctl::SCTL_SOCKET_SET_READ_TIMEOUT_MS,
                "Set read timeout",
            ),
            (
                crate::network::socket::socket_ctl::SCTL_SOCKET_SET_WRITE_TIMEOUT_MS,
                "Set write timeout",
            ),
            (
                crate::network::socket::socket_ctl::SCTL_SOCKET_GET_READ_TIMEOUT_MS,
                "Get read timeout",
            ),
            (
                crate::network::socket::socket_ctl::SCTL_SOCKET_GET_WRITE_TIMEOUT_MS,
                "Get write timeout",
            ),
        ]
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

    fn listen(&self, backlog: usize) -> Result<(), SocketError> {
        if self.local_port.load(Ordering::SeqCst) == 0 {
            return Err(SocketError::InvalidOperation);
        }

        // Set backlog limit (clamp to reasonable range)
        let max_backlog = backlog.max(1).min(128);
        self.max_backlog.store(max_backlog, Ordering::SeqCst);

        {
            let mut waker_lock = self.accept_waker.lock();
            waker_lock.get_or_insert_with(|| {
                Arc::new(crate::sync::Waker::new_interruptible("tcp_accept"))
            });
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

                if addr.0[0] == 127 {
                    let mut local_ip = self.local_ip.lock();
                    if local_ip.as_ref().is_none_or(|ip| ip.0 == [0, 0, 0, 0]) {
                        *local_ip = Some(Ipv4Address::new(127, 0, 0, 1));
                    }
                } else {
                    self.ensure_local_ip();
                }

                if self.try_loopback_connect(addr, port)? {
                    return Ok(());
                }

                // Start 3-way handshake
                self.send_syn(addr, port);

                if !self.blocking_mode.load(Ordering::SeqCst) {
                    return Err(SocketError::WouldBlock);
                }

                let task = crate::task::mytask().ok_or(SocketError::InvalidOperation)?;
                loop {
                    match self.get_state() {
                        TcpState::Established => return Ok(()),
                        TcpState::Closed => return Err(SocketError::ConnectionRefused),
                        TcpState::SynSent => {
                            let waker = {
                                let mut waker_lock = self.send_waker.lock();
                                waker_lock
                                    .get_or_insert_with(|| {
                                        Arc::new(crate::sync::Waker::new_interruptible(
                                            "tcp_connect",
                                        ))
                                    })
                                    .clone()
                            };
                            match self.get_state() {
                                TcpState::Established => return Ok(()),
                                TcpState::Closed => return Err(SocketError::ConnectionRefused),
                                TcpState::SynSent => {}
                                _ => return Err(SocketError::InvalidOperation),
                            }
                            if waker.wait_result(task.get_id(), task.get_trapframe())
                                == WaitResult::Interrupted
                            {
                                return Err(SocketError::Interrupted);
                            }
                        }
                        _ => return Err(SocketError::InvalidOperation),
                    }
                }
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
        if self.loopback_peer.lock().upgrade().is_some() {
            self.set_state(TcpState::Closed);
            if let Some(waker) = self.recv_waker.lock().as_ref() {
                waker.wake_all();
            }
            if let Some(waker) = self.send_waker.lock().as_ref() {
                waker.wake_all();
            }
            return Ok(());
        }

        match how {
            crate::network::socket::ShutdownHow::Write
            | crate::network::socket::ShutdownHow::Both => {
                if tcp_send_side_open(self.get_state()) {
                    self.send_fin();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        tcp_connection_present(self.get_state())
    }

    fn state(&self) -> SocketState {
        match self.get_state() {
            TcpState::Closed => SocketState::Unconnected,
            TcpState::Listen => SocketState::Listening,
            state if tcp_connection_present(state) => SocketState::Connected,
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
        use crate::object::capability::selectable::Selectable;

        if Selectable::is_nonblocking(self) {
            return self.recv_data(buffer).map_err(|err| match err {
                SocketError::WouldBlock => crate::object::capability::StreamError::WouldBlock,
                SocketError::Interrupted => crate::object::capability::StreamError::Interrupted,
                SocketError::NotConnected => crate::object::capability::StreamError::BrokenPipe,
                _ => crate::object::capability::StreamError::Other("tcp recv error".into()),
            });
        }

        let task = match crate::task::mytask() {
            Some(task) => task,
            None => {
                return Err(crate::object::capability::StreamError::Other(
                    "tcp recv: no task".into(),
                ));
            }
        };

        self.recv_blocking(buffer, task.get_id(), task.get_trapframe())
            .map_err(|err| match err {
                SocketError::WouldBlock => crate::object::capability::StreamError::WouldBlock,
                SocketError::Interrupted => crate::object::capability::StreamError::Interrupted,
                SocketError::NotConnected => crate::object::capability::StreamError::BrokenPipe,
                _ => crate::object::capability::StreamError::Other("tcp recv error".into()),
            })
    }

    fn write(&self, data: &[u8]) -> Result<usize, crate::object::capability::StreamError> {
        use crate::object::capability::selectable::Selectable;

        if Selectable::is_nonblocking(self) {
            return self.send_data(data).map_err(|err| match err {
                SocketError::WouldBlock => crate::object::capability::StreamError::WouldBlock,
                SocketError::Interrupted => crate::object::capability::StreamError::Interrupted,
                SocketError::NotConnected => crate::object::capability::StreamError::BrokenPipe,
                _ => crate::object::capability::StreamError::Other("tcp send error".into()),
            });
        }

        let task = match crate::task::mytask() {
            Some(task) => task,
            None => {
                return Err(crate::object::capability::StreamError::Other(
                    "tcp send: no task".into(),
                ));
            }
        };

        self.send_blocking(data, task.get_id(), task.get_trapframe())
            .map_err(|err| match err {
                SocketError::WouldBlock => crate::object::capability::StreamError::WouldBlock,
                SocketError::Interrupted => crate::object::capability::StreamError::Interrupted,
                SocketError::NotConnected => crate::object::capability::StreamError::BrokenPipe,
                _ => crate::object::capability::StreamError::Other("tcp send error".into()),
            })
    }
}

impl crate::object::capability::Selectable for TcpSocket {
    fn current_ready(
        &self,
        interest: crate::object::capability::selectable::ReadyInterest,
    ) -> crate::object::capability::selectable::ReadySet {
        let mut ready = crate::object::capability::selectable::ReadySet::none();

        if interest.read {
            let recv_buf = self.recv_buffer.lock();
            let has_data = !recv_buf.is_empty();
            drop(recv_buf);
            let state = self.get_state();
            ready.read = has_data || tcp_receive_side_eof(state);
        }

        if interest.write {
            let send_buf = self.send_buffer.lock();
            ready.write =
                tcp_send_side_open(self.get_state()) && send_buf.len() < MAX_SEND_BUFFER_SIZE;
        }

        ready
    }

    fn wait_until_ready(
        &self,
        interest: crate::object::capability::selectable::ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        timeout_ns: Option<u64>,
        _min_wait_ns: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        let task_id = {
            use crate::arch::get_cpu;
            let cpu_id = get_cpu().get_cpuid();
            current_task_id(cpu_id).unwrap_or(0)
        };
        let deadline =
            timeout_ns.map(|duration_ns| crate::timer::get_time_ns().saturating_add(duration_ns));

        loop {
            let current = self.current_ready(interest);
            if (interest.read && current.read) || (interest.write && current.write) {
                return crate::object::capability::selectable::SelectWaitOutcome::Ready;
            }

            let remaining = match deadline {
                Some(deadline) => {
                    let now = crate::timer::get_time_ns();
                    if now >= deadline {
                        return crate::object::capability::selectable::SelectWaitOutcome::TimedOut;
                    }
                    Some(deadline - now)
                }
                None => None,
            };

            if matches!(remaining, Some(0)) {
                return crate::object::capability::selectable::SelectWaitOutcome::TimedOut;
            }

            let woke = if interest.read {
                let waker = {
                    let mut waker_lock = self.recv_waker.lock();
                    waker_lock
                        .get_or_insert_with(|| {
                            Arc::new(crate::sync::Waker::new_interruptible("tcp_recv"))
                        })
                        .clone()
                };
                let current = self.current_ready(interest);
                if (interest.read && current.read) || (interest.write && current.write) {
                    return crate::object::capability::selectable::SelectWaitOutcome::Ready;
                }
                waker.wait_with_timeout(task_id, trapframe, remaining)
            } else if interest.write {
                let waker = {
                    let mut waker_lock = self.send_waker.lock();
                    waker_lock
                        .get_or_insert_with(|| {
                            Arc::new(crate::sync::Waker::new_interruptible("tcp_send"))
                        })
                        .clone()
                };
                let current = self.current_ready(interest);
                if (interest.read && current.read) || (interest.write && current.write) {
                    return crate::object::capability::selectable::SelectWaitOutcome::Ready;
                }
                waker.wait_with_timeout(task_id, trapframe, remaining)
            } else {
                true
            };

            if !woke {
                let after = self.current_ready(interest);
                if !((interest.read && after.read) || (interest.write && after.write)) {
                    return crate::object::capability::selectable::SelectWaitOutcome::TimedOut;
                }
            }
        }
    }

    fn set_nonblocking(&self, enabled: bool) {
        self.blocking_mode.store(!enabled, Ordering::SeqCst);
    }

    fn is_nonblocking(&self) -> bool {
        !self.blocking_mode.load(Ordering::SeqCst)
    }
}

impl crate::object::capability::CloneOps for TcpSocket {
    fn custom_clone(&self) -> crate::object::KernelObject {
        crate::object::KernelObject::Socket(TcpSocket::new(self.tcp_layer.clone()))
    }
}

impl Drop for TcpSocket {
    fn drop(&mut self) {
        // Cancel any pending retransmission timers
        self.cancel_retrans_timer();

        // Send FIN if connection is still open
        let state = self.get_state();
        match state {
            TcpState::Established
            | TcpState::SynReceived
            | TcpState::SynSent
            | TcpState::CloseWait => {
                if self.loopback_peer.lock().upgrade().is_none() && self.remote_ip.lock().is_some()
                {
                    self.send_fin();
                }
            }
            _ => {}
        }

        // Unregister this socket from TcpLayer (not all sockets on this port)
        if let Some(layer) = self.tcp_layer.upgrade() {
            let port = self.local_port.load(Ordering::SeqCst);
            if port != 0 {
                layer.unregister_socket(port, &self.self_weak);
            }
        }

        if let Some(waker) = self.accept_waker.lock().as_ref() {
            waker.wake_all();
        }
        if let Some(waker) = self.recv_waker.lock().as_ref() {
            waker.wake_all();
        }
        if let Some(waker) = self.send_waker.lock().as_ref() {
            waker.wake_all();
        }

        crate::network::NetworkManager::get_manager()
            .remove_socket_by_ptr(self as *const Self as usize);
    }
}

/// TCP layer
///
/// Manages TCP port bindings and routes packets to sockets.
pub struct TcpLayer {
    /// Port-to-socket mapping for receiving packets
    port_map: IrqRwSpinLock<BTreeMap<u16, Vec<Weak<TcpSocket>>>>,
    /// Statistics
    stats: IrqRwSpinLock<NetworkLayerStats>,
    self_weak: Weak<TcpLayer>,
}

impl TcpLayer {
    /// Create a new TCP layer
    pub fn new() -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            port_map: IrqRwSpinLock::new(BTreeMap::new()),
            stats: IrqRwSpinLock::new(NetworkLayerStats::default()),
            self_weak: weak.clone(),
        })
    }

    /// Initialize and register the TCP layer with NetworkManager
    ///
    /// Registers with NetworkManager and registers itself with Ipv4Layer
    /// for protocol number 6 (TCP).
    ///
    /// # Panics
    ///
    /// Panics if Ipv4Layer is not registered (must be initialized first).
    pub fn init(network_manager: &crate::network::NetworkManager) {
        let layer = Self::new();
        network_manager.register_layer("tcp", layer.clone());

        // Register with IPv4 layer for TCP packets (protocol 6)
        let ipv4 = network_manager
            .get_layer("ip")
            .expect("Ipv4Layer must be initialized before TcpLayer");
        ipv4.register_protocol(crate::network::ipv4::protocol::TCP as u16, layer);
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

    /// Unregister a specific socket from a port
    ///
    /// Only removes the given socket from the port's socket list.
    /// The port entry itself is removed only when no sockets remain.
    pub fn unregister_socket(&self, port: u16, socket: &Weak<TcpSocket>) {
        let mut map = self.port_map.write();
        if let Some(sockets) = map.get_mut(&port) {
            sockets.retain(|existing| !existing.ptr_eq(socket));
            if sockets.is_empty() {
                map.remove(&port);
            }
        }
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

        let src_port = unsafe { core::ptr::addr_of!(header.src_port).read_unaligned() };
        let dst_port = unsafe { core::ptr::addr_of!(header.dst_port).read_unaligned() };

        if let Some(socket) = self.find_socket(dst_port, src_ip, src_port) {
            socket.process_segment(src_ip, header, data);
        } else if should_log_tcp_https(src_port, dst_port) {
            let seq = header.seq_number;
            let ack = header.ack_number;
            let flags = header.flags();
            crate::println!(
                "[tcp] no socket {}.{}.{}.{}:{} -> local:{} flags=0x{:02x} seq={} ack={} len={}",
                src_ip.0[0],
                src_ip.0[1],
                src_ip.0[2],
                src_ip.0[3],
                src_port,
                dst_port,
                flags,
                seq,
                ack,
                data.len()
            );
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

    fn receive(&self, packet: &[u8], context: Option<&LayerContext>) -> Result<(), SocketError> {
        let mut src_ip = Ipv4Address::new(0, 0, 0, 0);
        let mut dst_ip = Ipv4Address::new(0, 0, 0, 0);
        if let Some(ctx) = context {
            if let Some(raw) = ctx.get("ip_src") {
                if raw.len() >= 4 {
                    src_ip = Ipv4Address::new(raw[0], raw[1], raw[2], raw[3]);
                }
            }
            if let Some(raw) = ctx.get("ip_dst") {
                if raw.len() >= 4 {
                    dst_ip = Ipv4Address::new(raw[0], raw[1], raw[2], raw[3]);
                }
            }
        }
        self.receive_packet(src_ip, dst_ip, packet)
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
    fn tcp_payload_segments_fit_the_ethernet_mtu() {
        assert_eq!(TCP_MAX_SEGMENT_DATA, 1460);
        assert_eq!(
            TCP_MAX_SEGMENT_DATA + TCP_HEADER_SIZE + IPV4_HEADER_SIZE,
            crate::network::ethernet::ETHERNET_MTU
        );
    }

    #[test_case]
    fn player_api_request_is_split_at_the_tcp_mss() {
        let chunk_lengths: Vec<usize> = [0u8; 2117]
            .chunks(TCP_MAX_SEGMENT_DATA)
            .map(<[u8]>::len)
            .collect();
        assert_eq!(chunk_lengths, [1460, 657]);
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
    fn local_write_half_close_keeps_the_receive_side_open() {
        for state in [
            TcpState::Established,
            TcpState::FinWait1,
            TcpState::FinWait2,
        ] {
            assert!(tcp_receive_side_open(state));
        }
        assert!(!tcp_send_side_open(TcpState::FinWait1));
        assert!(!tcp_send_side_open(TcpState::FinWait2));
        assert!(tcp_connection_present(TcpState::FinWait1));
        assert!(tcp_connection_present(TcpState::FinWait2));
    }

    #[test_case]
    fn peer_write_half_close_keeps_the_send_side_open() {
        assert!(tcp_receive_side_eof(TcpState::CloseWait));
        assert!(tcp_send_side_open(TcpState::CloseWait));
        assert!(tcp_connection_present(TcpState::CloseWait));
    }

    #[test_case]
    fn peer_fin_advances_active_close_states() {
        assert_eq!(
            state_after_peer_fin(TcpState::Established, false),
            TcpState::CloseWait
        );
        assert_eq!(
            state_after_peer_fin(TcpState::FinWait1, false),
            TcpState::Closing
        );
        assert_eq!(
            state_after_peer_fin(TcpState::FinWait1, true),
            TcpState::TimeWait
        );
        assert_eq!(
            state_after_peer_fin(TcpState::FinWait2, true),
            TcpState::TimeWait
        );
    }

    #[test_case]
    fn fin_wait_receive_without_data_would_block_instead_of_disconnect() {
        let tcp_layer = TcpLayer::new();
        let socket = TcpSocket::new(Arc::downgrade(&tcp_layer));
        let mut buffer = [0u8; 1];

        socket.set_state(TcpState::FinWait1);
        assert_eq!(socket.recv_data(&mut buffer), Err(SocketError::WouldBlock));
        socket.set_state(TcpState::FinWait2);
        assert_eq!(socket.recv_data(&mut buffer), Err(SocketError::WouldBlock));
    }

    #[test_case]
    fn acknowledging_local_fin_advances_to_fin_wait_2() {
        let tcp_layer = TcpLayer::new();
        let socket = TcpSocket::new(Arc::downgrade(&tcp_layer));

        socket.set_state(TcpState::FinWait1);
        socket.send_seq.store(1001, Ordering::SeqCst);
        socket.handle_close_ack(1001);

        assert_eq!(socket.get_state(), TcpState::FinWait2);
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

    #[test_case]
    fn retransmission_timer_retargets_only_when_the_unacked_head_changes() {
        assert!(retransmission_head_changed(None, Some(10)));
        assert!(!retransmission_head_changed(Some(10), Some(10)));
        assert!(retransmission_head_changed(Some(10), Some(20)));
        assert!(retransmission_head_changed(Some(20), None));
    }

    #[test_case]
    fn retransmission_deadline_uses_the_head_transmit_time_and_saturates() {
        assert_eq!(retransmission_deadline_ns(100, 2), 102);
        assert_eq!(retransmission_deadline_ns(u64::MAX - 1, 1), u64::MAX);
    }

    #[test_case]
    fn rto_nanosecond_bounds_preserve_the_legacy_durations() {
        assert_eq!(TcpSocket::INITIAL_RTO_NS, crate::timer::ms_to_ns(1_000));
        assert_eq!(TcpSocket::MIN_RTO_NS, crate::timer::ms_to_ns(10));
        assert_eq!(TcpSocket::MAX_RTO_NS, crate::timer::ms_to_ns(120_000));
    }

    #[test_case]
    fn retransmission_timeout_backoff_doubles_once_per_timeout() {
        let mut rto_ns = TcpSocket::INITIAL_RTO_NS;
        for expected_ms in [
            2_000, 4_000, 8_000, 16_000, 32_000, 64_000, 120_000, 120_000,
        ] {
            rto_ns = backed_off_retransmission_timeout_ns(rto_ns);
            assert_eq!(rto_ns, crate::timer::ms_to_ns(expected_ms));
        }
    }

    #[test_case]
    fn stale_retransmission_requests_do_not_match_a_new_head() {
        assert!(retransmission_request_matches_head(10, Some(10)));
        assert!(!retransmission_request_matches_head(10, Some(20)));
        assert!(!retransmission_request_matches_head(10, None));
    }
}
