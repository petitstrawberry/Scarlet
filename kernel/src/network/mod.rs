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
    string::String,
    sync::{Arc, Weak},
};
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::{Once, RwLock};

pub mod socket;
pub mod protocol_stack;

// Re-export commonly used types
pub use socket::{
    Inet4SocketAddress, Inet6SocketAddress, LocalSocketAddress, ShutdownHow, SocketAddress,
    SocketControl, SocketDomain, SocketError, SocketObject, SocketProtocol, SocketState,
    SocketType, UnixSocketAddress, // Keep for backwards compatibility
};
pub use protocol_stack::{ProtocolStack, ProtocolStackManager, ProtocolStackStats};

use crate::object::KernelObject;

/// Unique socket identifier
pub type SocketId = usize;

/// Socket factory function type
/// 
/// ABI modules register socket factories for their specific implementations
pub type SocketFactory = fn(SocketType, SocketProtocol) -> Result<Arc<dyn SocketObject>, SocketError>;

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
pub struct NetworkManager {
    /// Socket factories per domain (registered by ABI modules)
    socket_factories: RwLock<BTreeMap<SocketDomain, SocketFactory>>,

    /// Protocol stacks for network protocols (TCP/IP, UDP, etc.)
    protocol_stacks: protocol_stack::ProtocolStackManager,

    /// Named sockets namespace (path/name -> socket)
    /// Used by ABI modules for Unix domain socket-like functionality
    named_sockets: RwLock<BTreeMap<String, Weak<dyn SocketObject>>>,

    /// Active socket connections by ID
    connections: RwLock<BTreeMap<SocketId, Arc<dyn SocketObject>>>,

    /// Next socket ID counter
    next_socket_id: AtomicUsize,
}

impl NetworkManager {
    /// Create a new NetworkManager instance
    const fn new() -> Self {
        Self {
            socket_factories: RwLock::new(BTreeMap::new()),
            protocol_stacks: protocol_stack::ProtocolStackManager::new(),
            named_sockets: RwLock::new(BTreeMap::new()),
            connections: RwLock::new(BTreeMap::new()),
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
            self.connections
                .write()
                .insert(socket_id, socket.clone());

            return Ok(KernelObject::Socket(socket));
        }
        drop(factories);

        // Then try protocol stacks (for TCP/IP, etc.)
        if let Some(stack) = self.protocol_stacks.get_stack(domain) {
            let socket = stack.create_socket(socket_type, protocol)?;
            
            // Register the socket
            let socket_id = self.next_socket_id.fetch_add(1, Ordering::SeqCst);
            self.connections
                .write()
                .insert(socket_id, socket.clone());

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

    /// Get a registered protocol stack
    ///
    /// # Arguments
    ///
    /// * `domain` - Socket domain
    ///
    /// # Returns
    ///
    /// The protocol stack for this domain, or None if not registered
    pub fn get_protocol_stack(&self, domain: SocketDomain) -> Option<Arc<dyn protocol_stack::ProtocolStack>> {
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
    pub fn process_packet(&self, packet: &crate::device::network::DevicePacket) -> Result<(), SocketError> {
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
            Some(weak_socket) => weak_socket
                .upgrade()
                .ok_or(SocketError::ConnectionRefused),
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

    /// Remove a socket from the connections registry
    ///
    /// # Arguments
    ///
    /// * `socket_id` - The unique socket identifier to remove
    pub fn remove_socket(&self, socket_id: SocketId) {
        self.connections.write().remove(&socket_id);
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
            Err(SocketError::NotSupported) => {},
            _ => panic!("Expected NotSupported error"),
        }
    }
}
