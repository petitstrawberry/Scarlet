//! Network functionality for Scarlet
//!
//! This module provides network capabilities including sockets and protocol stacks.
//! It follows the existing patterns established by VfsManager and DeviceManager.
//!
//! # Architecture
//!
//! - **SocketObject**: KernelObject type representing network endpoints
//! - **NetworkManager**: Global manager handling socket lifecycle and connections
//! - **Socket Implementations**: Provided by ABI modules (Linux, xv6, etc.)
//!
//! # Design Philosophy
//!
//! Scarlet's core provides only abstract socket infrastructure. Specific socket
//! implementations (Unix domain sockets, TCP/IP, etc.) are delegated to ABI modules.
//! This maintains Scarlet's OS-agnostic nature while allowing each ABI to provide
//! the socket semantics expected by its applications.
//!
//! # Usage
//!
//! ABI modules implement SocketObject trait and register socket implementations
//! with the NetworkManager.

use crate::sync::{IrqRwSpinLock, Once};
use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicUsize, Ordering};
pub mod arp;
pub mod config;
pub mod ethernet;
pub mod ethernet_interface;
pub mod icmp;
pub mod ipv4;
pub mod local;
pub mod protocol_stack;
pub mod socket;
pub mod syscall;
pub mod tcp;
pub mod udp;
#[cfg(all(test, target_arch = "riscv64"))]
pub mod virtio_net_tests;

// Re-export commonly used types
pub use protocol_stack::{
    LayerContext, NetworkLayer, NetworkLayerStats, ProtocolStack, ProtocolStackManager,
    ProtocolStackStats, SocketConfig,
};
pub use socket::{
    Inet4SocketAddress,
    Inet6SocketAddress,
    LocalSocketAddress,
    ShutdownHow,
    SocketAddress,
    SocketControl,
    SocketDomain,
    SocketError,
    SocketObject,
    SocketProtocol,
    SocketState,
    SocketType,
    UnixSocketAddress, // Keep for backwards compatibility
};

use crate::device::network::{DevicePacket, MacAddress};
use crate::early_println;
use crate::network::arp::ArpCacheEntry;
use crate::network::ipv4::Ipv4Address;
use crate::object::KernelObject;

const LOG_IPV4_PACKET_TRACE: bool = false;

/// Unique socket identifier
pub type SocketId = usize;

/// Socket factory function type
///
/// ABI modules register socket factories for their specific implementations
pub type SocketFactory =
    fn(SocketType, SocketProtocol) -> Result<Arc<dyn SocketObject>, SocketError>;

/// Interface statistics
#[derive(Debug, Clone, Default)]
pub struct InterfaceStats {
    pub tx_packets: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub drops: u64,
    pub errors: u64,
}

/// Network configuration
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub default_gateway: Option<Ipv4Address>,
    pub gateway_mac: Option<MacAddress>,
    pub subnet_mask: Ipv4Address,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            default_gateway: None,
            gateway_mac: None,
            subnet_mask: Ipv4Address::new(0, 0, 0, 0),
        }
    }
}

/// Network interface trait
pub trait NetworkInterface: Send + Sync {
    /// Get the stable interface name.
    ///
    /// # Returns
    ///
    /// The interface name used by routing and configuration APIs.
    fn name(&self) -> &str;

    /// Get the interface MAC address.
    ///
    /// # Returns
    ///
    /// The current link-layer address.
    fn mac_address(&self) -> MacAddress;

    /// Get the configured primary IPv4 address.
    ///
    /// # Returns
    ///
    /// The primary address, or `None` while the interface is unconfigured.
    fn ip_address(&self) -> Option<Ipv4Address>;

    /// Set the interface's primary IPv4 address cache.
    ///
    /// # Arguments
    ///
    /// * `ip` - Primary IPv4 address to cache on the interface.
    ///
    /// # Returns
    ///
    /// This method does not return a value.
    fn set_ip_address(&self, ip: Ipv4Address);

    /// Clear the interface's primary IPv4 address cache.
    ///
    /// # Returns
    ///
    /// This method does not return a value.
    fn clear_ip_address(&self);

    /// Send a link-layer packet.
    ///
    /// # Arguments
    ///
    /// * `packet` - Complete device packet to transmit.
    ///
    /// # Returns
    ///
    /// `Ok(())` after submission, or a driver error.
    fn send(&self, packet: DevicePacket) -> Result<(), &'static str>;

    /// Poll the interface for received packets.
    ///
    /// # Returns
    ///
    /// All currently available packets, or a driver error.
    fn poll(&self) -> Result<Vec<DevicePacket>, &'static str>;

    /// Read interface statistics.
    ///
    /// # Returns
    ///
    /// A snapshot of packet, byte, drop, and error counters.
    fn stats(&self) -> InterfaceStats;
}

/// Network Manager - Global socket and connection manager
pub struct NetworkManager {
    /// Socket factories per domain (registered by ABI modules)
    socket_factories: IrqRwSpinLock<BTreeMap<SocketDomain, SocketFactory>>,

    /// Protocol stacks for network protocols (TCP/IP, UDP, etc.)
    protocol_stacks: protocol_stack::ProtocolStackManager,

    /// Protocol layers registry (shared instances like VFS filesystems)
    protocol_layers: IrqRwSpinLock<BTreeMap<String, Arc<dyn NetworkLayer>>>,

    /// Named sockets namespace (path/name -> socket)
    named_sockets: IrqRwSpinLock<BTreeMap<String, Weak<dyn SocketObject>>>,

    /// Active socket connections by ID
    connections: IrqRwSpinLock<BTreeMap<SocketId, Weak<dyn SocketObject>>>,

    /// Reverse mapping: socket pointer address -> socket ID for O(1) lookups
    socket_to_id: IrqRwSpinLock<BTreeMap<usize, SocketId>>,

    /// Next socket ID counter
    next_socket_id: AtomicUsize,

    /// Registered network interfaces
    interfaces: IrqRwSpinLock<BTreeMap<String, Arc<dyn NetworkInterface>>>,

    /// Default interface name
    default_interface: IrqRwSpinLock<Option<String>>,

    /// ARP cache
    arp_cache: IrqRwSpinLock<BTreeMap<u32, ArpCacheEntry>>,

    /// Network configuration
    network_config: IrqRwSpinLock<NetworkConfig>,
}

impl NetworkManager {
    /// Create a new NetworkManager instance
    fn new() -> Self {
        Self {
            socket_factories: IrqRwSpinLock::new(BTreeMap::new()),
            protocol_stacks: protocol_stack::ProtocolStackManager::new(),
            protocol_layers: IrqRwSpinLock::new(BTreeMap::new()),
            named_sockets: IrqRwSpinLock::new(BTreeMap::new()),
            connections: IrqRwSpinLock::new(BTreeMap::new()),
            socket_to_id: IrqRwSpinLock::new(BTreeMap::new()),
            next_socket_id: AtomicUsize::new(1),
            interfaces: IrqRwSpinLock::new(BTreeMap::new()),
            default_interface: IrqRwSpinLock::new(None),
            arp_cache: IrqRwSpinLock::new(BTreeMap::new()),
            network_config: IrqRwSpinLock::new(NetworkConfig::default()),
        }
    }

    /// Get the global NetworkManager instance
    pub fn get_manager() -> &'static NetworkManager {
        GLOBAL_NETWORK_MANAGER.call_once(|| NetworkManager::new())
    }

    /// Initialize the global NetworkManager
    ///
    /// Initializes all protocol layers in dependency order:
    /// 1. Ethernet (no dependencies)
    /// 2. IPv4, ARP (depend on Ethernet)
    /// 3. ICMP, UDP, TCP (depend on IPv4)
    pub fn init() -> &'static NetworkManager {
        let manager = GLOBAL_NETWORK_MANAGER.call_once(|| NetworkManager::new());

        // Initialize protocol layers in dependency order
        // Layer 1: Ethernet (no dependencies)
        crate::network::ethernet::EthernetLayer::init(manager);

        // Layer 2: IPv4 and ARP (depend on Ethernet)
        crate::network::ipv4::Ipv4Layer::init(manager);
        crate::network::arp::ArpLayer::init(manager);

        // Layer 3: ICMP, UDP, TCP (depend on IPv4)
        crate::network::icmp::IcmpLayer::init(manager);
        crate::network::udp::UdpLayer::init(manager);
        crate::network::tcp::TcpLayer::init(manager);

        manager
    }

    // ===================================================================
    // Interface Management
    // ===================================================================

    /// Register an interface with the network and protocol managers.
    ///
    /// # Arguments
    ///
    /// * `name` - Stable interface name.
    /// * `interface` - Interface implementation to register.
    ///
    /// # Returns
    ///
    /// `Ok(())` after registration, or an error if registration fails.
    pub fn register_interface(
        &self,
        name: &str,
        interface: Arc<dyn NetworkInterface>,
    ) -> Result<(), &'static str> {
        let mut default = self.default_interface.write();
        if default.is_none() {
            *default = Some(String::from(name));
        }

        let interface_clone = interface.clone();
        self.interfaces
            .write()
            .insert(String::from(name), interface);

        self.configure_protocol_layers_with_interface(interface_clone);

        Ok(())
    }

    pub fn get_interface(&self, name: &str) -> Option<Arc<dyn NetworkInterface>> {
        self.interfaces.read().get(name).cloned()
    }

    /// Remove a registered network interface.
    ///
    /// If the removed interface was the default, the lexicographically first
    /// remaining interface becomes the new default.
    ///
    /// # Arguments
    ///
    /// * `name` - Interface name to remove.
    ///
    /// # Returns
    ///
    /// The removed interface, or `None` when `name` was not registered.
    pub fn unregister_interface(&self, name: &str) -> Option<Arc<dyn NetworkInterface>> {
        let mut default = self.default_interface.write();
        let mut interfaces = self.interfaces.write();
        let removed = interfaces.remove(name);
        if default.as_deref() == Some(name) {
            *default = interfaces.keys().next().cloned();
        }
        drop(interfaces);
        drop(default);

        if removed.is_some()
            && let Some(ip_layer) = self.get_layer("ip")
            && let Some(ipv4) = ip_layer
                .as_any()
                .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
        {
            ipv4.remove_interface(name);
        }
        removed
    }

    pub fn get_default_interface(&self) -> Option<Arc<dyn NetworkInterface>> {
        self.default_interface
            .read()
            .as_ref()
            .and_then(|name| self.get_interface(name))
    }

    /// Select the interface preferred by otherwise-unbound sockets.
    ///
    /// Unknown interface names are ignored.
    ///
    /// # Arguments
    ///
    /// * `name` - Registered interface name to select.
    ///
    /// # Returns
    ///
    /// This method does not return a value.
    pub fn set_default_interface(&self, name: &str) {
        if self.get_interface(name).is_none() {
            return;
        }

        *self.default_interface.write() = Some(String::from(name));
        if let Some(ethernet_layer) = self.get_layer("ethernet")
            && let Some(ethernet) = ethernet_layer
                .as_any()
                .downcast_ref::<crate::network::ethernet::EthernetLayer>()
        {
            ethernet.set_default_interface(name);
        }
    }

    /// Get the name of the default network interface.
    ///
    /// # Returns
    ///
    /// The default interface name, or `None` if no interface is registered.
    pub fn default_interface_name(&self) -> Option<String> {
        self.default_interface.read().clone()
    }

    pub fn list_interfaces(&self) -> Vec<String> {
        self.interfaces.read().keys().cloned().collect()
    }

    // ===================================================================
    // Protocol Layer Configuration
    // ===================================================================

    fn configure_protocol_layers_with_interface(&self, interface: Arc<dyn NetworkInterface>) {
        let local_ip = interface.ip_address();
        let interface_name = interface.name();

        // Configure IP layer with local IP address
        if let Some(ip_layer) = self.get_layer("ip") {
            if let Some(ip) = ip_layer
                .as_any()
                .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
            {
                if let Some(local_ip_addr) = local_ip {
                    ip.add_address(
                        interface_name,
                        crate::network::ipv4::Ipv4AddressInfo {
                            address: local_ip_addr,
                            netmask: crate::network::ipv4::Ipv4Address::new(255, 255, 255, 0),
                            broadcast: None,
                            is_primary: true,
                        },
                    );
                }
            }
        }
    }

    // ===================================================================
    // ARP Cache Management
    // ===================================================================

    pub fn arp_lookup(&self, ip: &Ipv4Address) -> Option<MacAddress> {
        let ip_u32 = u32::from_be_bytes(ip.as_bytes());
        let cache = self.arp_cache.read();
        cache.get(&ip_u32).and_then(|entry| {
            if entry.is_expired() {
                None
            } else {
                Some(MacAddress::new(entry.mac_address))
            }
        })
    }

    pub fn arp_cache_add(&self, ip: Ipv4Address, mac: MacAddress) {
        let ip_u32 = u32::from_be_bytes(ip.as_bytes());
        let mut cache = self.arp_cache.write();
        cache.insert(ip_u32, ArpCacheEntry::new(ip, *mac.as_bytes()));
    }

    pub fn send_arp_request(&self, target_ip: Ipv4Address) -> Result<(), &'static str> {
        let interface = self.get_default_interface().ok_or("No default interface")?;

        let local_ip = interface.ip_address().ok_or("Interface has no IP")?;
        let local_mac = interface.mac_address();

        let arp_request =
            crate::network::arp::ArpPacket::request(local_ip.as_bytes(), target_ip.as_bytes());

        let eth_header = crate::network::ethernet::EthernetHeader::new(
            [0xFF; 6],
            *local_mac.as_bytes(),
            crate::network::ethernet::ether_type::ARP,
        );

        let mut packet_data = Vec::new();
        packet_data.extend_from_slice(&eth_header.to_bytes());
        packet_data.extend_from_slice(&arp_request.to_bytes());

        let packet = DevicePacket::with_data(packet_data);
        interface.send(packet)
    }

    pub fn resolve_mac(&self, ip: Ipv4Address) -> Result<MacAddress, &'static str> {
        if let Some(mac) = self.arp_lookup(&ip) {
            return Ok(mac);
        }
        self.send_arp_request(ip)?;
        Err("MAC not in cache, ARP request sent")
    }

    // ===================================================================
    // Network Configuration
    // ===================================================================

    pub fn get_config(&self) -> NetworkConfig {
        self.network_config.read().clone()
    }

    pub fn set_config(&self, config: NetworkConfig) {
        *self.network_config.write() = config;
    }

    pub fn set_default_gateway(&self, gateway: Ipv4Address) {
        self.network_config.write().default_gateway = Some(gateway);
        self.network_config.write().gateway_mac = None;

        // Add default route to Ipv4Layer's routing table
        if let Some(default_iface) = self.get_default_interface() {
            if let Some(ip_layer) = self.get_layer("ip") {
                if let Some(ipv4) = ip_layer
                    .as_any()
                    .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
                {
                    ipv4.set_default_gateway(gateway, default_iface.name());
                }
            }
        }
    }

    /// Set or clear the default gateway for one interface.
    ///
    /// # Arguments
    ///
    /// * `interface` - Interface that owns the default route.
    /// * `gateway` - Gateway to install, or `None` to remove the route.
    /// * `metric` - Route metric; lower values are preferred.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the interface and IPv4 layer exist, otherwise an error.
    pub fn set_default_gateway_for_interface(
        &self,
        interface: &str,
        gateway: Option<Ipv4Address>,
        metric: u32,
    ) -> Result<(), &'static str> {
        if self.get_interface(interface).is_none() {
            return Err("Network interface not found");
        }

        let ip_layer = self.get_layer("ip").ok_or("IPv4 layer not initialized")?;
        let ipv4 = ip_layer
            .as_any()
            .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
            .ok_or("IPv4 layer type mismatch")?;

        if let Some(gateway) = gateway {
            ipv4.set_default_gateway_for_interface(gateway, interface, metric);
        } else {
            ipv4.clear_default_gateway_for_interface(interface);
        }

        if self.default_interface_name().as_deref() == Some(interface) {
            let mut config = self.network_config.write();
            config.default_gateway = gateway;
            config.gateway_mac = None;
        }
        Ok(())
    }

    pub fn get_default_gateway(&self) -> Option<Ipv4Address> {
        self.network_config.read().default_gateway
    }

    pub fn handle_received_packet(&self, interface_name: &str, packet: &DevicePacket) {
        if packet.len < 14 {
            return;
        }

        let eth_type = u16::from_be_bytes([packet.data[12], packet.data[13]]);
        // early_println!(
        //     "[net] recv frame len={} eth_type=0x{:04X}",
        //     packet.len,
        //     eth_type
        // );
        match eth_type {
            0x0806 => self.handle_arp_packet(interface_name, packet),
            0x0800 => self.handle_ipv4_packet(interface_name, packet),
            _ => {}
        }
    }

    fn handle_arp_packet(&self, interface_name: &str, packet: &DevicePacket) {
        if packet.len < 14 + 28 {
            return;
        }

        let arp_data = &packet.data[14..];
        if let Some(arp_layer) = self.get_layer("arp") {
            if let Some(arp) = arp_layer
                .as_any()
                .downcast_ref::<crate::network::arp::ArpLayer>()
            {
                let _ = arp.receive_packet_on_interface(arp_data, Some(interface_name));
            }
        }
    }

    fn handle_ipv4_packet(&self, interface_name: &str, packet: &DevicePacket) {
        if packet.len < 14 + 20 {
            return;
        }

        let ip_bytes = &packet.data[14..packet.len];
        let header = match crate::network::ipv4::Ipv4Header::from_bytes(ip_bytes) {
            Some(h) => h,
            None => return,
        };

        if LOG_IPV4_PACKET_TRACE {
            early_println!(
                "[IPv4] Recv frame: ip_len={} src={}.{}.{}.{} dst={}.{}.{}.{} proto={}",
                ip_bytes.len(),
                header.source_ip[0],
                header.source_ip[1],
                header.source_ip[2],
                header.source_ip[3],
                header.dest_ip[0],
                header.dest_ip[1],
                header.dest_ip[2],
                header.dest_ip[3],
                header.protocol
            );
        }

        let header_len = header.header_length();
        let total_length = usize::from(header.total_length);
        if ip_bytes.len() < header_len {
            return;
        }
        if total_length < header_len || total_length > ip_bytes.len() {
            return;
        }

        let payload = &ip_bytes[header_len..total_length];
        let protocol = header.protocol;

        if let Some(ip_layer) = self.get_layer("ip") {
            if let Some(ip) = ip_layer
                .as_any()
                .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
            {
                if let Some(handler) = ip.get_protocol_handler(protocol) {
                    let src_ip = crate::network::ipv4::Ipv4Address::from_bytes(header.source_ip);
                    let dst_ip = crate::network::ipv4::Ipv4Address::from_bytes(header.dest_ip);
                    let _ = match protocol {
                        crate::network::ipv4::protocol::ICMP => handler
                            .as_any()
                            .downcast_ref::<crate::network::icmp::IcmpLayer>()
                            .map(|icmp| icmp.receive_packet(payload, src_ip, dst_ip)),
                        crate::network::ipv4::protocol::TCP => handler
                            .as_any()
                            .downcast_ref::<crate::network::tcp::TcpLayer>()
                            .map(|tcp| tcp.receive_packet(src_ip, dst_ip, payload)),
                        crate::network::ipv4::protocol::UDP => handler
                            .as_any()
                            .downcast_ref::<crate::network::udp::UdpLayer>()
                            .map(|udp| {
                                udp.receive_packet_on_interface(
                                    src_ip,
                                    dst_ip,
                                    payload,
                                    Some(interface_name),
                                )
                            }),
                        _ => Some(handler.receive(payload, None)),
                    };
                }
            }
        }
    }

    // ===================================================================
    // Socket Management (existing behavior)
    // ===================================================================

    pub fn register_socket_factory(&self, domain: SocketDomain, factory: SocketFactory) {
        self.socket_factories.write().insert(domain, factory);
    }

    pub fn create_socket(
        &self,
        domain: SocketDomain,
        socket_type: SocketType,
        protocol: SocketProtocol,
    ) -> Result<KernelObject, SocketError> {
        let factories = self.socket_factories.read();
        if let Some(factory) = factories.get(&domain) {
            let socket = factory(socket_type, protocol)?;
            let socket_id = self.next_socket_id.fetch_add(1, Ordering::SeqCst);
            self.register_socket_with_id(socket_id, Arc::clone(&socket))?;
            return Ok(KernelObject::Socket(socket));
        }
        drop(factories);

        if let Some(stack) = self.protocol_stacks.get_stack(domain) {
            let socket = stack.create_socket(socket_type, protocol)?;
            let socket_id = self.next_socket_id.fetch_add(1, Ordering::SeqCst);
            self.register_socket_with_id(socket_id, Arc::clone(&socket))?;
            return Ok(KernelObject::Socket(socket));
        }

        Err(SocketError::NotSupported)
    }

    pub fn register_protocol_stack(&self, stack: Arc<dyn protocol_stack::ProtocolStack>) {
        self.protocol_stacks.register_stack(stack);
    }

    pub fn register_layer(&self, name: &str, layer: Arc<dyn NetworkLayer>) {
        self.protocol_layers.write().insert(name.to_string(), layer);
    }

    pub fn unregister_layer(&self, name: &str) -> Option<Arc<dyn NetworkLayer>> {
        self.protocol_layers.write().remove(name)
    }

    pub fn get_layer(&self, name: &str) -> Option<Arc<dyn NetworkLayer>> {
        self.protocol_layers.read().get(name).cloned()
    }

    pub fn list_layers(&self) -> Vec<String> {
        self.protocol_layers.read().keys().cloned().collect()
    }

    pub fn layer_count(&self) -> usize {
        self.protocol_layers.read().len()
    }

    pub fn has_layer(&self, name: &str) -> bool {
        self.protocol_layers.read().contains_key(name)
    }

    pub fn register_named_socket(
        &self,
        name: &str,
        socket: Arc<dyn SocketObject>,
    ) -> Result<(), SocketError> {
        let mut sockets = self.named_sockets.write();
        if let Some(weak_socket) = sockets.get(name) {
            if weak_socket.upgrade().is_some() {
                return Err(SocketError::AddressInUse);
            }
        }
        sockets.insert(name.into(), Arc::downgrade(&socket));
        Ok(())
    }

    pub fn lookup_named_socket(&self, name: &str) -> Result<Arc<dyn SocketObject>, SocketError> {
        let sockets = self.named_sockets.read();
        match sockets.get(name) {
            Some(weak_socket) => weak_socket.upgrade().ok_or(SocketError::ConnectionRefused),
            None => Err(SocketError::ConnectionRefused),
        }
    }

    /// Unregister a named socket when the caller still owns the registration.
    ///
    /// # Arguments
    ///
    /// * `name` - Registry name to remove.
    /// * `socket` - Socket requesting removal. A registration owned by another
    ///   socket is left intact.
    ///
    /// # Returns
    ///
    /// This function returns no value. A missing or differently owned
    /// registration is left unchanged.
    pub fn unregister_named_socket(&self, name: &str, socket: &dyn SocketObject) {
        let socket_ptr = socket as *const dyn SocketObject;
        let mut sockets = self.named_sockets.write();
        let owns_registration = sockets
            .get(name)
            .is_some_and(|registered| core::ptr::addr_eq(registered.as_ptr(), socket_ptr));
        if owns_registration {
            sockets.remove(name);
        }
    }

    pub fn get_socket(&self, socket_id: SocketId) -> Option<Arc<dyn SocketObject>> {
        self.connections
            .read()
            .get(&socket_id)
            .and_then(|socket| socket.upgrade())
    }

    pub fn register_socket_with_id(
        &self,
        socket_id: SocketId,
        socket: Arc<dyn SocketObject>,
    ) -> Result<(), SocketError> {
        let mut connections = self.connections.write();
        if let Some(existing) = connections.get(&socket_id) {
            if existing.upgrade().is_some() {
                return Err(SocketError::AddressInUse);
            }
            connections.remove(&socket_id);
        }
        let socket_ptr = Arc::as_ptr(&socket) as *const () as usize;
        connections.insert(socket_id, Arc::downgrade(&socket));
        drop(connections);
        self.socket_to_id.write().insert(socket_ptr, socket_id);
        Ok(())
    }

    pub fn remove_socket(&self, socket_id: SocketId) {
        let socket = self.connections.write().remove(&socket_id);
        if let Some(socket) = socket {
            let socket_ptr = socket.as_ptr() as *const () as usize;
            self.socket_to_id.write().remove(&socket_ptr);
        }
    }

    pub(crate) fn remove_socket_by_ptr(&self, socket_ptr: usize) {
        let socket_id = self.socket_to_id.write().remove(&socket_ptr);
        if let Some(socket_id) = socket_id {
            self.connections.write().remove(&socket_id);
        }
    }

    pub fn allocate_socket_id(
        &self,
        socket: Arc<dyn SocketObject>,
    ) -> Result<SocketId, SocketError> {
        let socket = socket;
        let start_id = self.next_socket_id.load(Ordering::SeqCst);
        let mut current_id = start_id;
        loop {
            match self.register_socket_with_id(current_id, Arc::clone(&socket)) {
                Ok(()) => {
                    let mut observed = self.next_socket_id.load(Ordering::SeqCst);
                    loop {
                        if observed > current_id {
                            break;
                        }
                        match self.next_socket_id.compare_exchange(
                            observed,
                            current_id.wrapping_add(1),
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        ) {
                            Ok(_) => break,
                            Err(actual) => {
                                if actual > current_id {
                                    break;
                                }
                                observed = actual;
                            }
                        }
                    }
                    return Ok(current_id);
                }
                Err(e) => {
                    let next_id = current_id.wrapping_add(1);
                    if next_id == start_id {
                        return Err(e);
                    }
                    current_id = next_id;
                }
            }
        }
    }

    pub fn get_socket_id(&self, socket: &Arc<dyn SocketObject>) -> Option<SocketId> {
        let socket_ptr = Arc::as_ptr(socket) as *const () as usize;
        self.socket_to_id.read().get(&socket_ptr).copied()
    }

    pub fn connection_count(&self) -> usize {
        self.connections
            .read()
            .values()
            .filter(|socket| socket.upgrade().is_some())
            .count()
    }

    pub fn named_socket_count(&self) -> usize {
        let sockets = self.named_sockets.read();
        sockets.values().filter(|s| s.upgrade().is_some()).count()
    }
}

/// Global network manager instance
static GLOBAL_NETWORK_MANAGER: Once<NetworkManager> = Once::new();

/// Get the global network manager
pub fn get_network_manager() -> &'static NetworkManager {
    NetworkManager::get_manager()
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use super::*;
    use crate::network::local::LocalSocket;

    #[test_case]
    fn unregister_named_socket_preserves_a_different_live_owner() {
        let manager = NetworkManager::new();
        let owner: Arc<dyn SocketObject> = Arc::new(LocalSocket::new(
            SocketType::Stream,
            SocketProtocol::Default,
        ));
        let duplicate: Arc<dyn SocketObject> = Arc::new(LocalSocket::new(
            SocketType::Stream,
            SocketProtocol::Default,
        ));
        let name = "/tmp/named-socket-owner-test";

        manager
            .register_named_socket(name, Arc::clone(&owner))
            .unwrap();
        assert_eq!(
            manager.register_named_socket(name, Arc::clone(&duplicate)),
            Err(SocketError::AddressInUse)
        );

        manager.unregister_named_socket(name, duplicate.as_ref());
        let registered = manager.lookup_named_socket(name).unwrap();
        assert!(Arc::ptr_eq(&registered, &owner));

        manager.unregister_named_socket(name, owner.as_ref());
        assert!(matches!(
            manager.lookup_named_socket(name),
            Err(SocketError::ConnectionRefused)
        ));
    }
}
