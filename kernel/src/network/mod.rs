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

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::{Once, RwLock};

pub mod arp;
pub mod ethernet;
pub mod icmp;
pub mod ipv4;
pub mod local;
pub mod protocol_stack;
pub mod socket;
pub mod syscall;
pub mod tcp;
pub mod udp;
pub mod virtio_integration;

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

use crate::object::KernelObject;

/// Unique socket identifier
pub type SocketId = usize;

/// Socket factory function type
///
/// ABI modules register socket factories for their specific implementations
pub type SocketFactory =
    fn(SocketType, SocketProtocol) -> Result<Arc<dyn SocketObject>, SocketError>;

/// Network Manager - Global socket and connection manager
///
/// The NetworkManager follows the DeviceManager and VfsManager patterns,
/// providing centralized management of socket resources.
///
/// # Design
///
/// The NetworkManager provides infrastructure for socket management but does not
/// implement specific socket types. ABI modules (Linux, xv6, etc.) register their
/// own socket implementations through factory functions.
///
/// For network protocols (TCP/IP, UDP, etc.), protocol stacks can be registered
/// and will be used to create sockets for those domains.
///
/// # VFS Pattern: Shared Protocol Layers
///
/// Following VfsManager's pattern where filesystem instances are shared:
/// - **NetworkLayer instances** are shared, singleton-like protocol implementations
/// - **SocketObject** is the per-socket handle that references these shared layers
/// - Like FileSystem (ext2, tmpfs) vs FileObject (file handle)
///
/// This enables:
/// - Protocol layers shared across all sockets (like filesystems across files)
/// - Per-task NetworkManager for network namespace isolation (future)
/// - Efficient routing table and protocol state management
pub struct NetworkManager {
    /// Socket factories per domain (registered by ABI modules)
    socket_factories: RwLock<BTreeMap<SocketDomain, SocketFactory>>,

    /// Protocol stacks for network protocols (TCP/IP, UDP, etc.)
    protocol_stacks: protocol_stack::ProtocolStackManager,

    /// Protocol layers registry (shared instances like VFS filesystems)
    /// Maps layer name -> shared layer instance
    /// Examples: "ethernet" -> EthernetLayer, "ip" -> IpLayer, "tcp" -> TcpLayer
    protocol_layers: RwLock<BTreeMap<String, Arc<dyn protocol_stack::NetworkLayer>>>,

    /// Named sockets namespace (path/name -> socket)
    /// Used by ABI modules for Unix domain socket-like functionality
    named_sockets: RwLock<BTreeMap<String, Weak<dyn SocketObject>>>,

    /// Active socket connections by ID
    connections: RwLock<BTreeMap<SocketId, Arc<dyn SocketObject>>>,

    /// Reverse mapping: socket pointer address -> socket ID for O(1) lookups
    /// This is maintained alongside connections for efficient get_socket_id()
    socket_to_id: RwLock<BTreeMap<usize, SocketId>>,

    /// Next socket ID counter
    next_socket_id: AtomicUsize,
}

impl NetworkManager {
    /// Create a new NetworkManager instance
    const fn new() -> Self {
        Self {
            socket_factories: RwLock::new(BTreeMap::new()),
            protocol_stacks: protocol_stack::ProtocolStackManager::new(),
            protocol_layers: RwLock::new(BTreeMap::new()),
            named_sockets: RwLock::new(BTreeMap::new()),
            connections: RwLock::new(BTreeMap::new()),
            socket_to_id: RwLock::new(BTreeMap::new()),
            next_socket_id: AtomicUsize::new(1), // Start from 1, reserve 0 for invalid
        }
    }

    /// Get the global NetworkManager instance
    pub fn get_manager() -> &'static NetworkManager {
        GLOBAL_NETWORK_MANAGER
            .get()
            .expect("Network manager not initialized")
    }

    /// Initialize the global NetworkManager
    pub fn init() -> &'static NetworkManager {
        GLOBAL_NETWORK_MANAGER.call_once(|| NetworkManager::new())
    }

    /// Register a socket factory for a specific domain
    ///
    /// ABI modules call this to register their socket implementations.
    ///
    /// # Arguments
    ///
    /// * `domain` - Socket domain (Unix, Inet, Inet6, etc.)
    /// * `factory` - Factory function to create sockets of this domain
    ///
    /// # Example
    ///
    /// ```
    /// // In Linux ABI module
    /// NetworkManager::get_manager().register_socket_factory(
    ///     SocketDomain::Unix,
    ///     linux_create_unix_socket
    /// );
    /// ```
    pub fn register_socket_factory(&self, domain: SocketDomain, factory: SocketFactory) {
        self.socket_factories.write().insert(domain, factory);
    }

    /// Create a new socket using registered factory or protocol stack
    ///
    /// # Arguments
    ///
    /// * `domain` - Socket domain (Local, Inet, Inet6, etc.)
    /// * `socket_type` - Socket type (Stream, Datagram, etc.)
    /// * `protocol` - Socket protocol (Default, Tcp, Udp, etc.)
    ///
    /// # Returns
    ///
    /// A KernelObject containing the newly created socket
    ///
    /// # Errors
    ///
    /// Returns an error if no factory is registered for the domain or if
    /// the factory fails to create the socket.
    ///
    /// # Socket Creation Priority
    ///
    /// 1. First tries registered socket factories (for ABI-specific implementations)
    /// 2. Then tries registered protocol stacks (for TCP/IP, UDP, etc.)
    /// 3. Returns NotSupported if neither is available
    pub fn create_socket(
        &self,
        domain: SocketDomain,
        socket_type: SocketType,
        protocol: SocketProtocol,
    ) -> Result<KernelObject, SocketError> {
        // First try socket factories (ABI-specific implementations)
        let factories = self.socket_factories.read();
        if let Some(factory) = factories.get(&domain) {
            let socket = factory(socket_type, protocol)?;

            // Register the socket
            let socket_id = self.next_socket_id.fetch_add(1, Ordering::SeqCst);
            self.connections.write().insert(socket_id, socket.clone());

            return Ok(KernelObject::Socket(socket));
        }
        drop(factories);

        // Then try protocol stacks (for TCP/IP, etc.)
        if let Some(stack) = self.protocol_stacks.get_stack(domain) {
            let socket = stack.create_socket(socket_type, protocol)?;

            // Register the socket
            let socket_id = self.next_socket_id.fetch_add(1, Ordering::SeqCst);
            self.connections.write().insert(socket_id, socket.clone());

            return Ok(KernelObject::Socket(socket));
        }

        Err(SocketError::NotSupported)
    }

    /// Register a protocol stack
    ///
    /// Protocol stacks handle network protocols like TCP/IP, UDP, etc.
    ///
    /// # Arguments
    ///
    /// * `stack` - Protocol stack implementation
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // In a network driver or ABI module
    /// let tcp_ip_stack = Arc::new(TcpIpStack::new());
    /// NetworkManager::get_manager().register_protocol_stack(tcp_ip_stack);
    /// ```
    pub fn register_protocol_stack(&self, stack: Arc<dyn protocol_stack::ProtocolStack>) {
        self.protocol_stacks.register_stack(stack);
    }

    /// Register a protocol layer (VFS pattern: like mounting a filesystem)
    ///
    /// Protocol layers are shared, singleton-like instances that implement
    /// network protocol logic. Like filesystems in VFS, they are registered
    /// once and shared across all sockets.
    ///
    /// # Arguments
    ///
    /// * `name` - Layer name (e.g., "ethernet", "ip", "tcp", "udp")
    /// * `layer` - Shared protocol layer implementation
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Register shared protocol layer instances
    /// let ethernet = Arc::new(EthernetLayer::new());
    /// let ip = Arc::new(IpLayer::new());
    /// let tcp = Arc::new(TcpLayer::new());
    ///
    /// NetworkManager::get_manager().register_layer("ethernet", ethernet);
    /// NetworkManager::get_manager().register_layer("ip", ip.clone());
    /// NetworkManager::get_manager().register_layer("tcp", tcp);
    ///
    /// // Connect layers
    /// ip.register_protocol(6, tcp); // TCP protocol number
    /// ```
    ///
    /// # Design Note
    ///
    /// This follows the VFS pattern where FileSystem instances are registered
    /// and shared across all FileObject handles. Similarly, NetworkLayer instances
    /// are shared across all SocketObject handles.
    pub fn register_layer(&self, name: &str, layer: Arc<dyn protocol_stack::NetworkLayer>) {
        self.protocol_layers.write().insert(name.to_string(), layer);
    }

    /// Get a registered protocol layer (VFS pattern: like getting a filesystem)
    ///
    /// Returns a reference to a shared protocol layer instance. SocketObject
    /// implementations hold references to these shared layers, similar to how
    /// FileObject references a filesystem.
    ///
    /// # Arguments
    ///
    /// * `name` - Layer name (e.g., "ethernet", "ip", "tcp")
    ///
    /// # Returns
    ///
    /// The shared protocol layer, or None if not registered
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // When creating a socket, get references to shared layers
    /// let tcp = NetworkManager::get_manager()
    ///     .get_layer("tcp")
    ///     .ok_or(SocketError::NotSupported)?;
    /// let ip = NetworkManager::get_manager()
    ///     .get_layer("ip")
    ///     .ok_or(SocketError::NotSupported)?;
    ///
    /// // Socket holds references to shared layers
    /// let socket = TcpSocket {
    ///     tcp_layer: tcp,
    ///     ip_layer: ip,
    ///     // per-socket state...
    /// };
    /// ```
    pub fn get_layer(&self, name: &str) -> Option<Arc<dyn protocol_stack::NetworkLayer>> {
        self.protocol_layers.read().get(name).cloned()
    }

    /// List all registered protocol layers
    ///
    /// Returns names of all registered protocol layers for debugging/introspection.
    pub fn list_layers(&self) -> Vec<String> {
        self.protocol_layers.read().keys().cloned().collect()
    }

    /// Get a registered protocol stack
    ///
    /// # Arguments
    ///
    /// * `domain` - Socket domain
    ///
    /// # Returns
    ///
    /// The protocol stack for this domain, or None if not registered
    pub fn get_protocol_stack(
        &self,
        domain: SocketDomain,
    ) -> Option<Arc<dyn protocol_stack::ProtocolStack>> {
        self.protocol_stacks.get_stack(domain)
    }

    /// Process an incoming network packet
    ///
    /// Routes the packet to the appropriate protocol stack.
    ///
    /// # Arguments
    ///
    /// * `packet` - Raw packet from network device
    ///
    /// # Errors
    ///
    /// Returns an error if no protocol stack can handle the packet
    pub fn process_packet(
        &self,
        packet: &crate::device::network::DevicePacket,
    ) -> Result<(), SocketError> {
        self.protocol_stacks.process_packet(packet)
    }

    /// Register a named socket (for Unix domain socket-like functionality)
    ///
    /// ABI modules can use this for path-based socket naming.
    ///
    /// # Arguments
    ///
    /// * `name` - Name/path for the socket
    /// * `socket` - The socket to register
    ///
    /// # Errors
    ///
    /// Returns an error if a socket is already registered with this name.
    pub fn register_named_socket(
        &self,
        name: &str,
        socket: Arc<dyn SocketObject>,
    ) -> Result<(), SocketError> {
        let mut sockets = self.named_sockets.write();

        // Check if name is already in use
        if let Some(weak_socket) = sockets.get(name) {
            if weak_socket.upgrade().is_some() {
                return Err(SocketError::AddressInUse);
            }
        }

        // Register the socket
        sockets.insert(name.into(), Arc::downgrade(&socket));
        Ok(())
    }

    /// Lookup a named socket
    ///
    /// # Arguments
    ///
    /// * `name` - Name/path of the socket
    ///
    /// # Returns
    ///
    /// The socket registered with this name
    ///
    /// # Errors
    ///
    /// Returns an error if no socket is registered with this name or
    /// if the socket has been closed.
    pub fn lookup_named_socket(&self, name: &str) -> Result<Arc<dyn SocketObject>, SocketError> {
        let sockets = self.named_sockets.read();

        match sockets.get(name) {
            Some(weak_socket) => weak_socket.upgrade().ok_or(SocketError::ConnectionRefused),
            None => Err(SocketError::ConnectionRefused),
        }
    }

    /// Unregister a named socket
    ///
    /// # Arguments
    ///
    /// * `name` - Name/path of the socket to unregister
    pub fn unregister_named_socket(&self, name: &str) {
        self.named_sockets.write().remove(name);
    }

    /// Get a socket by its ID
    ///
    /// # Arguments
    ///
    /// * `socket_id` - The unique socket identifier
    ///
    /// # Returns
    ///
    /// The socket with this ID, or None if not found
    pub fn get_socket(&self, socket_id: SocketId) -> Option<Arc<dyn SocketObject>> {
        self.connections.read().get(&socket_id).cloned()
    }

    /// Register a socket with a specific ID
    ///
    /// This method allows external systems (like VFS) to register socket objects
    /// with specific IDs for filesystem integration.
    ///
    /// # Arguments
    ///
    /// * `socket_id` - The unique socket identifier to use
    /// * `socket` - The socket object to register
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Socket successfully registered
    /// * `Err(SocketError::AddressInUse)` - Socket ID already in use
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // When creating a socket file in VFS
    /// let socket = create_local_socket()?;
    /// let socket_id = 1001; // Use a unique ID
    /// NetworkManager::get_manager().register_socket_with_id(socket_id, socket)?;
    /// ```
    pub fn register_socket_with_id(
        &self,
        socket_id: SocketId,
        socket: Arc<dyn SocketObject>,
    ) -> Result<(), SocketError> {
        let mut connections = self.connections.write();

        // Check if ID is already in use
        if connections.contains_key(&socket_id) {
            return Err(SocketError::AddressInUse);
        }

        // Get the socket pointer address for reverse mapping
        let socket_ptr = Arc::as_ptr(&socket) as *const () as usize;

        // Insert into both mappings
        connections.insert(socket_id, socket);
        drop(connections);

        // Update reverse mapping
        self.socket_to_id.write().insert(socket_ptr, socket_id);

        Ok(())
    }

    /// Remove a socket from the connections registry
    ///
    /// # Arguments
    ///
    /// * `socket_id` - The unique socket identifier to remove
    pub fn remove_socket(&self, socket_id: SocketId) {
        let mut connections = self.connections.write();

        // Get the socket to find its pointer address before removal
        if let Some(socket) = connections.get(&socket_id) {
            let socket_ptr = Arc::as_ptr(socket) as *const () as usize;
            drop(connections);

            // Remove from both mappings
            self.connections.write().remove(&socket_id);
            self.socket_to_id.write().remove(&socket_ptr);
        }
    }

    /// Allocate a new socket ID and register the socket
    ///
    /// This is a convenience method that allocates a unique socket ID
    /// and registers the socket in one operation.
    ///
    /// # Arguments
    ///
    /// * `socket` - The socket object to register
    ///
    /// # Returns
    ///
    /// The allocated socket ID
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let socket = Arc::new(LocalSocket::new(SocketType::Stream, SocketProtocol::Default));
    /// let socket_id = NetworkManager::get_manager().allocate_socket_id(socket)?;
    /// ```
    pub fn allocate_socket_id(
        &self,
        socket: Arc<dyn SocketObject>,
    ) -> Result<SocketId, SocketError> {
        // Keep ownership of the socket and clone the Arc on each registration attempt.
        let socket = socket;

        // Use the current value of `next_socket_id` as our starting point.
        let start_id = self.next_socket_id.load(Ordering::SeqCst);
        let mut current_id = start_id;

        loop {
            // Try to register the current candidate ID.
            match self.register_socket_with_id(current_id, Arc::clone(&socket)) {
                Ok(()) => {
                    // On success, advance `next_socket_id` so that it is at least
                    // one past the allocated ID. This uses a CAS loop to avoid
                    // regressing the counter in the presence of concurrent allocators.
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
                                    // Another allocator already moved the counter forward.
                                    break;
                                }
                                observed = actual;
                            }
                        }
                    }

                    return Ok(current_id);
                }
                Err(e) => {
                    // Move to the next candidate ID, wrapping on overflow.
                    let next_id = current_id.wrapping_add(1);

                    // If we've wrapped around and tried the entire ID space, give up.
                    if next_id == start_id {
                        return Err(e);
                    }

                    current_id = next_id;
                }
            }
        }
    }

    /// Get the socket ID for a given socket object
    ///
    /// Searches the socket registry to find the ID for a socket object.
    /// This is now an O(1) operation using a reverse mapping.
    ///
    /// # Arguments
    ///
    /// * `socket` - The socket object to find
    ///
    /// # Returns
    ///
    /// The socket ID if found, None otherwise
    pub fn get_socket_id(&self, socket: &Arc<dyn SocketObject>) -> Option<SocketId> {
        let socket_ptr = Arc::as_ptr(socket) as *const () as usize;
        self.socket_to_id.read().get(&socket_ptr).copied()
    }

    /// Get the count of active connections
    pub fn connection_count(&self) -> usize {
        self.connections.read().len()
    }

    /// Get the count of registered named sockets
    pub fn named_socket_count(&self) -> usize {
        let sockets = self.named_sockets.read();
        // Count only sockets that are still alive
        sockets.values().filter(|s| s.upgrade().is_some()).count()
    }
}

/// Global network manager instance
static GLOBAL_NETWORK_MANAGER: Once<NetworkManager> = Once::new();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::StreamIpcOps;
    use crate::object::capability::CloneOps;

    // Mock socket implementation for testing
    struct MockSocket {
        socket_type: SocketType,
        domain: SocketDomain,
        protocol: SocketProtocol,
        state: RwLock<SocketState>,
        local_addr: RwLock<Option<SocketAddress>>,
        peer_addr: RwLock<Option<SocketAddress>>,
    }

    impl MockSocket {
        fn new(socket_type: SocketType, domain: SocketDomain, protocol: SocketProtocol) -> Self {
            Self {
                socket_type,
                domain,
                protocol,
                state: RwLock::new(SocketState::Unconnected),
                local_addr: RwLock::new(None),
                peer_addr: RwLock::new(None),
            }
        }
    }

    impl crate::object::capability::StreamOps for MockSocket {
        fn read(
            &self,
            _buffer: &mut [u8],
        ) -> Result<usize, crate::object::capability::StreamError> {
            Ok(0)
        }

        fn write(&self, data: &[u8]) -> Result<usize, crate::object::capability::StreamError> {
            Ok(data.len())
        }
    }

    impl StreamIpcOps for MockSocket {
        fn is_connected(&self) -> bool {
            *self.state.read() == SocketState::Connected
        }

        fn peer_count(&self) -> usize {
            if StreamIpcOps::is_connected(self) {
                1
            } else {
                0
            }
        }

        fn description(&self) -> String {
            alloc::format!("MockSocket({:?}, {:?})", self.domain, self.socket_type)
        }
    }

    impl SocketControl for MockSocket {
        fn bind(&self, address: &SocketAddress) -> Result<(), SocketError> {
            *self.local_addr.write() = Some(address.clone());
            Ok(())
        }

        fn connect(&self, address: &SocketAddress) -> Result<(), SocketError> {
            *self.peer_addr.write() = Some(address.clone());
            *self.state.write() = SocketState::Connected;
            Ok(())
        }

        fn listen(&self, _backlog: usize) -> Result<(), SocketError> {
            *self.state.write() = SocketState::Listening;
            Ok(())
        }

        fn accept(&self) -> Result<Arc<dyn SocketObject>, SocketError> {
            Err(SocketError::NotSupported)
        }

        fn getpeername(&self) -> Result<SocketAddress, SocketError> {
            self.peer_addr
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

        fn shutdown(&self, _how: ShutdownHow) -> Result<(), SocketError> {
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

    impl CloneOps for MockSocket {
        fn custom_clone(&self) -> KernelObject {
            KernelObject::Socket(Arc::new(MockSocket::new(
                self.socket_type,
                self.domain,
                self.protocol,
            )))
        }
    }

    impl SocketObject for MockSocket {
        fn socket_type(&self) -> SocketType {
            self.socket_type
        }

        fn socket_domain(&self) -> SocketDomain {
            self.domain
        }

        fn socket_protocol(&self) -> SocketProtocol {
            self.protocol
        }

        fn as_any(&self) -> &dyn core::any::Any {
            self
        }

        fn sendto(
            &self,
            data: &[u8],
            _address: &SocketAddress,
            _flags: u32,
        ) -> Result<usize, SocketError> {
            Ok(data.len())
        }

        fn recvfrom(
            &self,
            _buffer: &mut [u8],
            _flags: u32,
        ) -> Result<(usize, SocketAddress), SocketError> {
            Err(SocketError::WouldBlock)
        }

        fn as_selectable(&self) -> Option<&dyn crate::object::capability::selectable::Selectable> {
            None
        }
    }

    // Mock socket factory
    fn mock_socket_factory(
        socket_type: SocketType,
        protocol: SocketProtocol,
    ) -> Result<Arc<dyn SocketObject>, SocketError> {
        Ok(Arc::new(MockSocket::new(
            socket_type,
            SocketDomain::Local,
            protocol,
        )))
    }

    #[test_case]
    fn test_network_manager_creation() {
        let manager = NetworkManager::new();
        assert_eq!(manager.connection_count(), 0);
        assert_eq!(manager.named_socket_count(), 0);
    }

    #[test_case]
    fn test_no_factory_registered() {
        let manager = NetworkManager::new();
        let result = manager.create_socket(
            SocketDomain::Local,
            SocketType::Stream,
            SocketProtocol::Default,
        );
        assert!(result.is_err());
        match result {
            Err(SocketError::NotSupported) => {}
            _ => panic!("Expected NotSupported error"),
        }
    }

    #[test_case]
    fn test_register_and_create_socket() {
        let manager = NetworkManager::new();

        // Register factory
        manager.register_socket_factory(SocketDomain::Local, mock_socket_factory);

        // Create socket
        let result = manager.create_socket(
            SocketDomain::Local,
            SocketType::Stream,
            SocketProtocol::Default,
        );

        assert!(result.is_ok());
        let socket_obj = result.unwrap();
        assert!(matches!(socket_obj, KernelObject::Socket(_)));

        // Verify socket was registered
        assert_eq!(manager.connection_count(), 1);
    }

    #[test_case]
    fn test_named_socket_registration() {
        let manager = NetworkManager::new();

        // Create a mock socket
        let socket = Arc::new(MockSocket::new(
            SocketType::Stream,
            SocketDomain::Local,
            SocketProtocol::Default,
        ));

        // Register with name
        let result = manager.register_named_socket("/tmp/test.sock", socket.clone());
        assert!(result.is_ok());

        // Lookup socket
        let lookup_result = manager.lookup_named_socket("/tmp/test.sock");
        assert!(lookup_result.is_ok());

        let found_socket = lookup_result.unwrap();
        assert_eq!(found_socket.socket_domain(), SocketDomain::Local);
        assert_eq!(found_socket.socket_type(), SocketType::Stream);
    }

    #[test_case]
    fn test_named_socket_duplicate_registration() {
        let manager = NetworkManager::new();

        let socket1 = Arc::new(MockSocket::new(
            SocketType::Stream,
            SocketDomain::Local,
            SocketProtocol::Default,
        ));

        let socket2 = Arc::new(MockSocket::new(
            SocketType::Stream,
            SocketDomain::Local,
            SocketProtocol::Default,
        ));

        // First registration should succeed
        // Keep socket1 alive by cloning the Arc
        let _socket1_ref = socket1.clone();
        assert!(
            manager
                .register_named_socket("/tmp/test.sock", socket1)
                .is_ok()
        );

        // Second registration should fail (socket1 is still alive)
        let result = manager.register_named_socket("/tmp/test.sock", socket2);
        assert!(result.is_err());
        match result {
            Err(SocketError::AddressInUse) => {}
            _ => panic!("Expected AddressInUse error"),
        }
    }

    #[test_case]
    fn test_named_socket_unregister() {
        let manager = NetworkManager::new();

        let socket = Arc::new(MockSocket::new(
            SocketType::Stream,
            SocketDomain::Local,
            SocketProtocol::Default,
        ));

        // Register and verify - use socket.clone() to keep original alive
        manager
            .register_named_socket("/tmp/test.sock", socket.clone())
            .unwrap();
        let result = manager.lookup_named_socket("/tmp/test.sock");
        assert!(
            result.is_ok(),
            "Expected lookup to succeed, but got error: {:?}",
            result.err()
        );

        // Unregister
        manager.unregister_named_socket("/tmp/test.sock");

        // Lookup should fail
        let result = manager.lookup_named_socket("/tmp/test.sock");
        assert!(result.is_err());
    }

    #[test_case]
    fn test_named_socket_weak_reference() {
        let manager = NetworkManager::new();

        let socket = Arc::new(MockSocket::new(
            SocketType::Stream,
            SocketDomain::Local,
            SocketProtocol::Default,
        ));

        {
            // Register socket - manager stores a Weak reference
            // We keep 'socket' alive outside this scope for the assertion to succeed
            manager
                .register_named_socket("/tmp/test.sock", socket.clone())
                .unwrap();

            // Socket is alive ('socket' variable holds strong ref), lookup succeeds
            let result = manager.lookup_named_socket("/tmp/test.sock");
            assert!(
                result.is_ok(),
                "Expected lookup to succeed, but got error: {:?}",
                result.err()
            );
        }

        // Now drop the socket - all strong references gone
        drop(socket);

        // Socket is dead (all Arc dropped), weak reference can't upgrade, lookup fails
        let result = manager.lookup_named_socket("/tmp/test.sock");
        assert!(result.is_err());
        match result {
            Err(SocketError::ConnectionRefused) => {}
            _ => panic!("Expected ConnectionRefused error"),
        }
    }

    #[test_case]
    fn test_protocol_layer_registration() {
        use crate::network::protocol_stack::NetworkLayer;
        use core::sync::atomic::AtomicU64;

        // Simple mock layer
        struct MockLayer {
            name: &'static str,
            packets_sent: AtomicU64,
        }

        impl MockLayer {
            fn new(name: &'static str) -> Self {
                Self {
                    name,
                    packets_sent: AtomicU64::new(0),
                }
            }
        }

        impl NetworkLayer for MockLayer {
            fn register_protocol(&self, _proto_num: u16, _handler: Arc<dyn NetworkLayer>) {}

            fn send(
                &self,
                _packet: &[u8],
                _context: &LayerContext,
                _next_layers: &[Arc<dyn NetworkLayer>],
            ) -> Result<(), SocketError> {
                self.packets_sent.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }

            fn receive(&self, _packet: &[u8]) -> Result<(), SocketError> {
                Ok(())
            }

            fn name(&self) -> &'static str {
                self.name
            }

            fn as_any(&self) -> &dyn core::any::Any {
                self
            }
        }

        let manager = NetworkManager::new();

        // Register layers
        let tcp = Arc::new(MockLayer::new("tcp"));
        let ip = Arc::new(MockLayer::new("ip"));
        let eth = Arc::new(MockLayer::new("ethernet"));

        manager.register_layer("tcp", tcp.clone());
        manager.register_layer("ip", ip.clone());
        manager.register_layer("ethernet", eth.clone());

        // Verify layers registered
        let layers = manager.list_layers();
        assert_eq!(layers.len(), 3);
        assert!(layers.contains(&"tcp".to_string()));
        assert!(layers.contains(&"ip".to_string()));
        assert!(layers.contains(&"ethernet".to_string()));

        // Retrieve layer
        let retrieved_tcp = manager.get_layer("tcp");
        assert!(retrieved_tcp.is_some());
        assert_eq!(retrieved_tcp.unwrap().name(), "tcp");

        // Non-existent layer
        assert!(manager.get_layer("nonexistent").is_none());
    }

    #[test_case]
    fn test_multiple_socket_creation() {
        let manager = NetworkManager::new();
        manager.register_socket_factory(SocketDomain::Local, mock_socket_factory);

        // Create multiple sockets
        for _ in 0..5 {
            let result = manager.create_socket(
                SocketDomain::Local,
                SocketType::Stream,
                SocketProtocol::Default,
            );
            assert!(result.is_ok());
        }

        // Verify all registered
        assert_eq!(manager.connection_count(), 5);
    }
}
