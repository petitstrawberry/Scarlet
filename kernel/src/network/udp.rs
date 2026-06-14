//! UDP protocol layer
//!
//! This module provides UDP datagram handling for the network stack.
//! It implements the NetworkLayer trait and provides UDP socket functionality.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use spin::{Mutex, RwLock};

use crate::early_println;
use crate::network::Ipv4Address;
use crate::network::protocol_stack::get_network_manager;
use crate::network::protocol_stack::{LayerContext, NetworkLayer, NetworkLayerStats, SocketConfig};
use crate::network::socket::{
    Inet4SocketAddress, SocketAddress, SocketControl, SocketError, SocketObject, SocketProtocol,
    SocketState, SocketType,
};
use crate::object::capability::{ControlOps, selectable::Selectable};
use crate::sched::scheduler::current_task_id;

/// Helper function to get local IP address bytes from the default interface
fn get_local_ip_bytes() -> [u8; 4] {
    let manager = get_network_manager();
    if let Some(default_iface) = manager.get_default_interface() {
        if let Some(ip_layer) = manager.get_layer("ip") {
            if let Some(ipv4_layer) = ip_layer
                .as_any()
                .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
            {
                if let Some(addr) = ipv4_layer.get_primary_ip(default_iface.name()) {
                    return addr.as_bytes();
                }
            }
        }
    }
    [0u8; 4]
}

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
        sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += 17u32; // UDP protocol number
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += self.length as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);

        // UDP header (except checksum field)
        sum += self.src_port as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += self.dst_port as u32;
        sum = (sum & 0xFFFF) + (sum >> 16);
        sum += self.length as u32;
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

        // Final carry
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
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
    /// Weak self reference for registration
    self_weak: Weak<UdpSocket>,
    /// Receive waker for blocking I/O
    recv_waker: Mutex<Option<alloc::sync::Arc<crate::sync::Waker>>>,
    /// Send waker for blocking I/O
    send_waker: Mutex<Option<alloc::sync::Arc<crate::sync::Waker>>>,
    /// Blocking mode (default: true)
    blocking_mode: spin::Mutex<bool>,
}

impl UdpSocket {
    /// Create a new UDP socket
    pub fn new(udp_layer: Arc<UdpLayer>) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            local_addr: RwLock::new(None),
            remote_addr: RwLock::new(None),
            send_buffer: Mutex::new(Vec::new()),
            recv_buffer: Mutex::new(Vec::new()),
            state: RwLock::new(SocketState::Unconnected),
            udp_layer,
            self_weak: weak.clone(),
            recv_waker: Mutex::new(None),
            send_waker: Mutex::new(None),
            blocking_mode: spin::Mutex::new(true),
        })
    }

    /// Deliver received datagram to this socket
    pub fn deliver_datagram(&self, data: Vec<u8>) {
        self.recv_buffer.lock().push(data);
        // Wake up any waiting reader
        if let Some(waker) = self.recv_waker.lock().as_ref() {
            waker.wake_one();
        }
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        if let Some(SocketAddress::Inet(inet)) = self.local_addr.read().clone() {
            self.udp_layer.unregister_socket(inet.port, &self.self_weak);
        }

        *self.state.write() = SocketState::Closed;

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

impl SocketObject for UdpSocket {
    fn socket_type(&self) -> SocketType {
        SocketType::Datagram
    }

    fn socket_domain(&self) -> crate::network::socket::SocketDomain {
        crate::network::socket::SocketDomain::Inet4
    }

    fn socket_protocol(&self) -> SocketProtocol {
        SocketProtocol::Udp
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
        _flags: u32,
    ) -> Result<usize, SocketError> {
        match address {
            SocketAddress::Inet(inet) => {
                let addr = inet.addr;
                let port = inet.port;
                // Queue the datagram for sending
                let mut buffer = self.send_buffer.lock();
                let datagram = data.to_vec();
                buffer.push(datagram.clone());

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
        // Blocking mode: wait for data
        if !self.is_nonblocking() {
            let interest = crate::object::capability::selectable::ReadyInterest {
                read: true,
                write: false,
                except: false,
            };
            let task = crate::task::mytask();
            if let Some(t) = task {
                let trapframe = t.get_trapframe();
                Selectable::wait_until_ready(self, interest, trapframe, None, 0);
            }
        }

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
            SocketAddress::Inet(inet) => {
                let addr = inet.addr;
                let port = if inet.port == 0 {
                    self.udp_layer.allocate_port()
                } else {
                    inet.port
                };
                let mut config = SocketConfig::new();
                config.set("udp_local_port", &port.to_be_bytes());
                config.set("ip_local", &addr);

                // Configure UDP layer
                self.udp_layer
                    .configure_socket(self.self_weak.clone(), &config)?;

                *self.local_addr.write() =
                    Some(SocketAddress::Inet(Inet4SocketAddress::new(addr, port)));
                *self.state.write() = SocketState::Bound;
                Ok(())
            }
            _ => Err(SocketError::InvalidAddress),
        }
    }

    fn connect(&self, address: &SocketAddress) -> Result<(), SocketError> {
        match address {
            SocketAddress::Inet(_) => {
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
        use crate::object::capability::selectable::Selectable;

        if !Selectable::is_nonblocking(self) {
            // Blocking mode: wait for data
            let task = crate::task::mytask();
            if let Some(t) = task {
                let trapframe = t.get_trapframe();
                let interest = crate::object::capability::selectable::ReadyInterest {
                    read: true,
                    write: false,
                    except: false,
                };
                Selectable::wait_until_ready(self, interest, trapframe, None, 0);
            }
        }

        let (len, _) = self.recvfrom(buffer, 0).map_err(|e| match e {
            SocketError::WouldBlock => crate::object::capability::StreamError::WouldBlock,
            _ => crate::object::capability::StreamError::Other("udp recv error".into()),
        })?;
        Ok(len)
    }

    fn write(&self, data: &[u8]) -> Result<usize, crate::object::capability::StreamError> {
        let remote_addr = self
            .remote_addr
            .read()
            .clone()
            .unwrap_or(SocketAddress::Unspecified);
        self.sendto(data, &remote_addr, 0)
            .map_err(|err| match err {
                SocketError::WouldBlock => crate::object::capability::StreamError::WouldBlock,
                SocketError::InvalidAddress => {
                    crate::object::capability::StreamError::InvalidArgument
                }
                SocketError::NotConnected => crate::object::capability::StreamError::BrokenPipe,
                SocketError::NotSupported => crate::object::capability::StreamError::NotSupported,
                _ => crate::object::capability::StreamError::Other("udp send error".into()),
            })?;
        Ok(data.len())
    }
}

impl crate::object::capability::CloneOps for UdpSocket {
    fn custom_clone(&self) -> crate::object::KernelObject {
        crate::object::KernelObject::Socket(UdpSocket::new(self.udp_layer.clone()))
    }
}

impl crate::object::capability::Selectable for UdpSocket {
    fn current_ready(
        &self,
        interest: crate::object::capability::selectable::ReadyInterest,
    ) -> crate::object::capability::selectable::ReadySet {
        let mut ready = crate::object::capability::selectable::ReadySet::none();

        if interest.read {
            let recv_buf = self.recv_buffer.lock();
            ready.read = !recv_buf.is_empty();
        }

        if interest.write {
            // UDP writes are always ready (no congestion control)
            ready.write = true;
        }

        ready
    }

    fn wait_until_ready(
        &self,
        interest: crate::object::capability::selectable::ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        let current = self.current_ready(interest);
        if (interest.read && current.read) || (interest.write && current.write) {
            return crate::object::capability::selectable::SelectWaitOutcome::Ready;
        }

        let task_id = {
            use crate::arch::get_cpu;
            let cpu_id = get_cpu().get_cpuid();
            current_task_id(cpu_id).unwrap_or(0)
        };

        let woke = if interest.read {
            let waker = {
                let mut waker_lock = self.recv_waker.lock();
                waker_lock
                    .get_or_insert_with(|| {
                        alloc::sync::Arc::new(crate::sync::Waker::new_interruptible("udp_recv"))
                    })
                    .clone()
            };
            waker.wait_with_timeout(task_id, trapframe, timeout_ticks)
        } else {
            true
        };

        if timeout_ticks.is_some() && !woke {
            let after = self.current_ready(interest);
            if (interest.read && !after.read) && (interest.write && !after.write) {
                return crate::object::capability::selectable::SelectWaitOutcome::TimedOut;
            }
        }

        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }

    fn set_nonblocking(&self, enabled: bool) {
        *self.blocking_mode.lock() = !enabled;
    }

    fn is_nonblocking(&self) -> bool {
        !*self.blocking_mode.lock()
    }
}

impl ControlOps for UdpSocket {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            crate::network::socket::socket_ctl::SCTL_SOCKET_SET_NONBLOCK => {
                self.set_nonblocking(arg != 0);
                Ok(0)
            }
            crate::network::socket::socket_ctl::SCTL_SOCKET_GET_NONBLOCK => {
                Ok(if self.is_nonblocking() { 1 } else { 0 })
            }
            _ => Err("Unknown control command"),
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
    self_weak: Weak<UdpLayer>,
}

impl UdpLayer {
    /// Create a new UDP layer
    pub fn new() -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            port_map: RwLock::new(BTreeMap::new()),
            next_ephemeral_port: Mutex::new(49152),
            stats: RwLock::new(NetworkLayerStats::default()),
            self_weak: weak.clone(),
        })
    }

    /// Initialize and register the UDP layer with NetworkManager
    ///
    /// Registers with NetworkManager and registers itself with Ipv4Layer
    /// for protocol number 17 (UDP).
    ///
    /// # Panics
    ///
    /// Panics if Ipv4Layer is not registered (must be initialized first).
    pub fn init(network_manager: &crate::network::NetworkManager) {
        let layer = Self::new();
        network_manager.register_layer("udp", layer.clone());

        // Register with IPv4 layer for UDP packets (protocol 17)
        let ipv4 = network_manager
            .get_layer("ip")
            .expect("Ipv4Layer must be initialized before UdpLayer");
        ipv4.register_protocol(crate::network::ipv4::protocol::UDP as u16, layer);
    }

    /// Create a new UDP socket
    pub fn create_socket(&self) -> Arc<UdpSocket> {
        let layer = self
            .self_weak
            .upgrade()
            .expect("udp layer is not initialized");
        UdpSocket::new(layer)
    }

    /// Allocate an ephemeral port
    pub fn allocate_port(&self) -> u16 {
        const EPHEMERAL_START: u16 = 49152;
        const EPHEMERAL_END: u16 = 65535;
        const EPHEMERAL_COUNT: usize = (EPHEMERAL_END - EPHEMERAL_START + 1) as usize;

        let mut next_port = self.next_ephemeral_port.lock();
        for _ in 0..EPHEMERAL_COUNT {
            let port = *next_port;
            *next_port = if port == EPHEMERAL_END {
                EPHEMERAL_START
            } else {
                port + 1
            };

            if !self.port_map.read().contains_key(&port) {
                return port;
            }
        }

        EPHEMERAL_START
    }

    /// Register a socket for a specific port
    ///
    /// # Arguments
    ///
    /// * `port` - UDP local port to register.
    /// * `socket` - Weak reference to the socket that owns the port.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the port was registered, or [`SocketError::AddressInUse`] if another live
    /// socket already owns the port.
    pub fn register_port(
        &self,
        port: u16,
        socket: alloc::sync::Weak<UdpSocket>,
    ) -> Result<(), SocketError> {
        let mut map = self.port_map.write();
        if let Some(existing) = map.get(&port) {
            if existing.upgrade().is_some() && !existing.ptr_eq(&socket) {
                return Err(SocketError::AddressInUse);
            }
        }
        map.insert(port, socket);
        Ok(())
    }

    /// Unregister a specific socket from a port
    ///
    /// Only removes the port entry if the registered socket matches.
    pub fn unregister_socket(&self, port: u16, socket: &alloc::sync::Weak<UdpSocket>) {
        let mut map = self.port_map.write();
        if let Some(existing) = map.get(&port) {
            if existing.ptr_eq(socket) {
                map.remove(&port);
            }
        }
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
        socket: Weak<UdpSocket>,
        config: &SocketConfig,
    ) -> Result<(), SocketError> {
        let port = config
            .get_u16("udp_local_port")
            .ok_or(SocketError::InvalidAddress)?;

        // Register the port
        self.register_port(port, socket)?;

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
        let (src_ip_bytes, src_port) = match socket.local_addr.read().clone() {
            Some(SocketAddress::Inet(inet)) => {
                if inet.addr == [0, 0, 0, 0] {
                    let ip = get_local_ip_bytes();
                    (ip, inet.port)
                } else {
                    (inet.addr, inet.port)
                }
            }
            _ => {
                // Get local IP from IPv4 layer if not bound
                let ip = get_local_ip_bytes();
                // Allocate ephemeral port for unbound socket
                let port = self.allocate_port();
                self.register_port(port, socket.self_weak.clone())?;
                *socket.local_addr.write() =
                    Some(SocketAddress::Inet(Inet4SocketAddress::new(ip, port)));
                (ip, port)
            }
        };

        let total_length = (8 + data.len()) as u16;

        if dest_ip[0] == 127 {
            let data_len = data.len() as u64;
            {
                let mut stats = self.stats.write();
                stats.packets_sent += 1;
                stats.bytes_sent += (8 + data_len) as u64;
            }
            self.receive_datagram(
                Ipv4Address::new(
                    src_ip_bytes[0],
                    src_ip_bytes[1],
                    src_ip_bytes[2],
                    src_ip_bytes[3],
                ),
                src_port,
                dest_port,
                data,
            );
            return Ok(());
        }

        let mut header = UdpHeader::new(src_port, dest_port, total_length);

        header.checksum = header.calculate_checksum(src_ip_bytes, dest_ip, &data);

        let mut udp_packet = Vec::with_capacity(8 + data.len());
        udp_packet.extend_from_slice(&header.to_bytes());
        udp_packet.extend_from_slice(&data);

        let mut ip_context = LayerContext::new();
        ip_context.set("ip_src", &src_ip_bytes);
        ip_context.set("ip_dst", &dest_ip);
        ip_context.set("ip_protocol", &[17]);

        early_println!(
            "[UDP] Send: {} bytes (src port: {}, dst: {}.{}.{}.{})",
            udp_packet.len(),
            src_port,
            dest_ip[0],
            dest_ip[1],
            dest_ip[2],
            dest_ip[3]
        );

        if let Some(ip_layer) = get_network_manager().get_layer("ip") {
            match ip_layer.send(&udp_packet, &ip_context, &[]) {
                Ok(()) | Err(SocketError::WouldBlock) => {}
                Err(err) => return Err(err),
            }
        }

        let mut stats = self.stats.write();
        stats.packets_sent += 1;
        stats.bytes_sent += udp_packet.len() as u64;

        Ok(())
    }

    /// Receive a UDP datagram
    pub fn receive_datagram(
        &self,
        src_ip: Ipv4Address,
        src_port: u16,
        dst_port: u16,
        data: Vec<u8>,
    ) {
        let mut stats = self.stats.write();
        stats.packets_received += 1;
        stats.bytes_received += (8 + data.len()) as u64;

        if let Some(socket) = self.find_socket(dst_port) {
            socket.deliver_datagram(data);
            let mut remote_lock = socket.remote_addr.write();
            if remote_lock.is_none() {
                *remote_lock = Some(SocketAddress::Inet(Inet4SocketAddress::new(
                    src_ip.0, src_port,
                )));
            }
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
        "UDP"
    }

    fn stats(&self) -> NetworkLayerStats {
        self.stats.read().clone()
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl UdpLayer {
    /// Receive a UDP datagram
    pub fn receive_packet(
        &self,
        src_ip: Ipv4Address,
        _dst_ip: Ipv4Address,
        packet: &[u8],
    ) -> Result<(), SocketError> {
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
        self.receive_datagram(src_ip, header.src_port, header.dst_port, data.to_vec());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_udp_header_creation() {
        let header = UdpHeader::new(1234, 5678, 100);

        let src_port = unsafe { core::ptr::addr_of!(header.src_port).read_unaligned() };
        let dst_port = unsafe { core::ptr::addr_of!(header.dst_port).read_unaligned() };
        let length = unsafe { core::ptr::addr_of!(header.length).read_unaligned() };
        assert_eq!(src_port, 1234);
        assert_eq!(dst_port, 5678);
        assert_eq!(length, 100);
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

        let src_port = unsafe { core::ptr::addr_of!(header.src_port).read_unaligned() };
        let dst_port = unsafe { core::ptr::addr_of!(header.dst_port).read_unaligned() };
        let length = unsafe { core::ptr::addr_of!(header.length).read_unaligned() };
        let checksum = unsafe { core::ptr::addr_of!(header.checksum).read_unaligned() };
        assert_eq!(src_port, 1234);
        assert_eq!(dst_port, 5678);
        assert_eq!(length, 100);
        assert_eq!(checksum, 0xABCD);
    }

    #[test_case]
    fn test_udp_checksum() {
        let src_ip = [192, 168, 1, 100];
        let dst_ip = [192, 168, 1, 1];
        let data = b"test";

        let mut header = UdpHeader::new(1234, 5678, (8 + data.len()) as u16);
        header.checksum = header.calculate_checksum(src_ip, dst_ip, data);

        // Just verify that checksum calculation runs without panicking
        let checksum = unsafe { core::ptr::addr_of!(header.checksum).read_unaligned() };
        assert_ne!(checksum, 0);
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

    #[test_case]
    fn test_udp_bind_zero_allocates_ephemeral_port() {
        let udp_layer = UdpLayer::new();
        let socket = UdpSocket::new(udp_layer.clone());

        socket
            .bind(&SocketAddress::Inet(Inet4SocketAddress::new(
                [0, 0, 0, 0],
                0,
            )))
            .unwrap();

        let local = socket.getsockname().unwrap();
        let SocketAddress::Inet(inet) = local else {
            panic!("UDP socket should have an IPv4 local address");
        };

        assert!(inet.port >= 49152);
        assert!(udp_layer.find_socket(inet.port).is_some());
    }

    #[test_case]
    fn test_udp_register_port_rejects_live_duplicate() {
        let udp_layer = UdpLayer::new();
        let socket1 = UdpSocket::new(udp_layer.clone());
        let socket2 = UdpSocket::new(udp_layer.clone());

        udp_layer
            .register_port(53000, socket1.self_weak.clone())
            .unwrap();

        assert_eq!(
            udp_layer.register_port(53000, socket2.self_weak.clone()),
            Err(SocketError::AddressInUse)
        );
    }
}
