//! Socket abstraction and common types
//!
//! This module provides the core socket abstraction for Scarlet's network functionality.
//! Following Scarlet's philosophy, it defines OS-agnostic socket operations that can be
//! used for both internal IPC and external network communication.
//!
//! # Design Philosophy
//!
//! Like TTY devices, Scarlet Sockets use neutral, OS-agnostic abstractions:
//! - Scarlet-private control opcodes (SCTL_SOCKET_*) instead of OS-specific ioctls
//! - ABI modules translate their specific syscalls to these neutral operations
//! - Works for both internal (process-to-process) and external (network) communication
//!
//! # Control Opcodes
//!
//! Socket control operations use the SCTL_SOCKET_* namespace (magic 'SS' = 0x53, 0x53)

use alloc::{string::String, sync::Arc};

use crate::ipc::StreamIpcOps;
use crate::object::capability::Selectable;

/// Scarlet-private, OS-agnostic control opcodes for Socket operations.
/// These are stable only within Scarlet and must be mapped by ABI adapters.
pub mod socket_ctl {
    /// Magic 'SS' (0x53, 0x53) followed by sequential IDs to avoid collisions.
    ///
    /// Bind socket to an address (arg = address structure pointer)
    pub const SCTL_SOCKET_BIND: u32 = 0x5353_0001;
    /// Connect to remote address (arg = address structure pointer)
    pub const SCTL_SOCKET_CONNECT: u32 = 0x5353_0002;
    /// Start listening for connections (arg = backlog size)
    pub const SCTL_SOCKET_LISTEN: u32 = 0x5353_0003;
    /// Get local address (arg = buffer pointer for address)
    pub const SCTL_SOCKET_GETSOCKNAME: u32 = 0x5353_0004;
    /// Get peer address (arg = buffer pointer for address)
    pub const SCTL_SOCKET_GETPEERNAME: u32 = 0x5353_0005;
    /// Shutdown socket (arg: 0=read, 1=write, 2=both)
    pub const SCTL_SOCKET_SHUTDOWN: u32 = 0x5353_0006;
    /// Set socket to non-blocking mode (arg: 0=blocking, 1=non-blocking)
    pub const SCTL_SOCKET_SET_NONBLOCK: u32 = 0x5353_0007;
    /// Get socket state (returns SocketState value)
    pub const SCTL_SOCKET_GET_STATE: u32 = 0x5353_0008;
    /// Get socket non-blocking mode (returns 0 or 1)
    pub const SCTL_SOCKET_GET_NONBLOCK: u32 = 0x5353_000B;
    /// Set socket read timeout in milliseconds (arg: 0=disabled)
    pub const SCTL_SOCKET_SET_READ_TIMEOUT_MS: u32 = 0x5353_000C;
    /// Set socket write timeout in milliseconds (arg: 0=disabled)
    pub const SCTL_SOCKET_SET_WRITE_TIMEOUT_MS: u32 = 0x5353_000D;
    /// Get socket read timeout in milliseconds (returns 0 if disabled)
    pub const SCTL_SOCKET_GET_READ_TIMEOUT_MS: u32 = 0x5353_000E;
    /// Get socket write timeout in milliseconds (returns 0 if disabled)
    pub const SCTL_SOCKET_GET_WRITE_TIMEOUT_MS: u32 = 0x5353_000F;
    /// Get socket type (returns SocketType value)
    pub const SCTL_SOCKET_GET_TYPE: u32 = 0x5353_0009;
    /// Check if connected (returns 0 or 1)
    pub const SCTL_SOCKET_IS_CONNECTED: u32 = 0x5353_000A;
}

/// Socket type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SocketType {
    /// Stream socket (connection-oriented, reliable, byte stream)
    Stream = 1,
    /// Datagram socket (connectionless, unreliable, message-oriented)
    Datagram = 2,
    /// Raw socket (direct protocol access)
    Raw = 3,
    /// Sequenced packet socket (connection-oriented, reliable, message-oriented)
    SeqPacket = 4,
}

/// Socket domain (address family) - neutral to any specific OS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u32)]
pub enum SocketDomain {
    /// Local inter-process communication
    /// (Unix domain socket equivalent, but OS-agnostic)
    Local = 1,
    /// IPv4 Internet protocols
    Inet4 = 2,
    /// IPv6 Internet protocols
    Inet6 = 3,
    /// Packet-level communication
    Packet = 4,
}

/// Socket protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SocketProtocol {
    /// Default protocol for socket type/domain combination
    Default = 0,
    /// TCP protocol
    Tcp = 6,
    /// UDP protocol  
    Udp = 17,
    /// ICMP protocol
    Icmp = 1,
    /// Raw protocol with specific number
    Raw(u16) = 255,
}

/// Socket address abstraction - OS-agnostic
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketAddress {
    /// Local IPC address (path or abstract name)
    Local(LocalSocketAddress),
    /// IPv4 address with port
    Inet(Inet4SocketAddress),
    /// IPv6 address with port
    Inet6(Inet6SocketAddress),
    /// Unspecified/any address
    Unspecified,
}

impl SocketAddress {
    /// Check if this is an unspecified address
    pub fn is_unspecified(&self) -> bool {
        matches!(self, SocketAddress::Unspecified)
    }

    /// Get the domain of this address
    pub fn domain(&self) -> SocketDomain {
        match self {
            SocketAddress::Local(_) => SocketDomain::Local,
            SocketAddress::Inet(_) => SocketDomain::Inet4,
            SocketAddress::Inet6(_) => SocketDomain::Inet6,
            SocketAddress::Unspecified => SocketDomain::Local, // Default
        }
    }
}

/// Local socket address for inter-process communication
/// (OS-agnostic, not tied to Unix specifically)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSocketAddress {
    /// Socket path or name (may be empty for unnamed sockets)
    path: String,
    /// Whether this is an abstract name (not filesystem-based)
    abstract_name: bool,
}

impl LocalSocketAddress {
    /// Create a local socket address from a path
    pub fn from_path(path: impl Into<String>) -> Result<Self, SocketError> {
        let path = path.into();
        if path.len() > 108 {
            // Common socket path length limit
            return Err(SocketError::InvalidAddress);
        }
        Ok(Self {
            path,
            abstract_name: false,
        })
    }

    /// Create an abstract local socket address (not filesystem-based)
    pub fn from_abstract(name: impl Into<String>) -> Result<Self, SocketError> {
        let name = name.into();
        if name.len() > 107 {
            return Err(SocketError::InvalidAddress);
        }
        Ok(Self {
            path: name,
            abstract_name: true,
        })
    }

    /// Create an unnamed/anonymous local socket address
    pub fn unnamed() -> Self {
        Self {
            path: String::new(),
            abstract_name: false,
        }
    }

    /// Get the socket path or name
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Check if this is an unnamed socket
    pub fn is_unnamed(&self) -> bool {
        self.path.is_empty()
    }

    /// Check if this is an abstract name
    pub fn is_abstract(&self) -> bool {
        self.abstract_name
    }
}

// Keep Unix-named type for backwards compatibility with documentation
pub type UnixSocketAddress = LocalSocketAddress;

/// IPv4 socket address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inet4SocketAddress {
    /// IPv4 address
    pub addr: [u8; 4],
    /// Port number
    pub port: u16,
}

impl Inet4SocketAddress {
    /// Create a new IPv4 socket address
    pub fn new(addr: [u8; 4], port: u16) -> Self {
        Self { addr, port }
    }
}

/// IPv6 socket address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inet6SocketAddress {
    /// IPv6 address
    pub addr: [u8; 16],
    /// Port number
    pub port: u16,
    /// Flow information
    pub flowinfo: u32,
    /// Scope ID
    pub scope_id: u32,
}

impl Inet6SocketAddress {
    /// Create a new IPv6 socket address
    pub fn new(addr: [u8; 16], port: u16) -> Self {
        Self {
            addr,
            port,
            flowinfo: 0,
            scope_id: 0,
        }
    }
}

/// Socket shutdown directions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ShutdownHow {
    /// Shutdown reading
    Read = 0,
    /// Shutdown writing
    Write = 1,
    /// Shutdown both reading and writing
    Both = 2,
}

/// Socket state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SocketState {
    /// Socket is unbound and unconnected
    Unconnected = 0,
    /// Socket is bound to an address
    Bound = 1,
    /// Socket is listening for connections
    Listening = 2,
    /// Socket is connecting (for non-blocking sockets)
    Connecting = 3,
    /// Socket is connected
    Connected = 4,
    /// Socket is disconnecting
    Disconnecting = 5,
    /// Socket is closed
    Closed = 6,
}

/// Socket errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketError {
    /// Invalid socket address
    InvalidAddress,
    /// Address already in use
    AddressInUse,
    /// Address not available
    AddressNotAvailable,
    /// Connection refused
    ConnectionRefused,
    /// Connection reset by peer
    ConnectionReset,
    /// Connection aborted
    ConnectionAborted,
    /// Not connected
    NotConnected,
    /// Already connected
    AlreadyConnected,
    /// Invalid operation for socket state
    InvalidOperation,
    /// Socket is not listening
    NotListening,
    /// No pending connections
    NoConnections,
    /// Operation would block
    WouldBlock,
    /// Operation was interrupted by asynchronous task event delivery
    Interrupted,
    /// Invalid argument
    InvalidArgument,
    /// Not supported
    NotSupported,
    /// No route to destination
    NoRoute,
    /// Protocol not supported
    ProtocolNotSupported,
    /// Invalid packet format
    InvalidPacket,
    /// Custom error message
    Other(String),
}

/// Socket control operations trait
///
/// Provides OS-agnostic socket control, similar to TtyControl for TTY devices.
/// ABI modules translate their specific socket operations to these neutral controls.
pub trait SocketControl {
    /// Bind socket to an address
    fn bind(&self, address: &SocketAddress) -> Result<(), SocketError>;

    /// Connect to a remote address
    fn connect(&self, address: &SocketAddress) -> Result<(), SocketError>;

    /// Listen for incoming connections (for stream sockets)
    fn listen(&self, backlog: usize) -> Result<(), SocketError>;

    /// Accept an incoming connection (for listening sockets)
    /// Returns a new socket for the accepted connection
    fn accept(&self) -> Result<Arc<dyn SocketObject>, SocketError>;

    /// Get socket peer address
    fn getpeername(&self) -> Result<SocketAddress, SocketError>;

    /// Get socket local address  
    fn getsockname(&self) -> Result<SocketAddress, SocketError>;

    /// Shutdown socket for reading, writing, or both
    fn shutdown(&self, how: ShutdownHow) -> Result<(), SocketError>;

    /// Check if socket is connected
    fn is_connected(&self) -> bool;

    /// Get socket state
    fn state(&self) -> SocketState;
}

/// Socket operations trait
///
/// Combines StreamIpcOps (for data transfer), SocketControl (for connection management),
/// and CloneOps (for handle duplication). This is the main trait that socket implementations
/// must satisfy.
///
/// Similar to how TtyDeviceEndpoint combines CharDevice + TtyControl.
pub trait SocketObject: StreamIpcOps + SocketControl + Send + Sync {
    /// Get socket type (Stream, Datagram, etc.)
    fn socket_type(&self) -> SocketType;

    /// Get socket domain (Local, Inet, Inet6, etc.)
    fn socket_domain(&self) -> SocketDomain;

    /// Get socket protocol
    fn socket_protocol(&self) -> SocketProtocol;

    /// Cast to Any for safe downcasting.
    ///
    /// # Returns
    ///
    /// Borrowed [`core::any::Any`] view of this socket object.
    fn as_any(&self) -> &dyn core::any::Any;

    /// Send data to a specific address (for datagram sockets)
    /// For stream sockets, address is ignored and data is sent to connected peer
    fn sendto(
        &self,
        data: &[u8],
        address: &SocketAddress,
        flags: u32,
    ) -> Result<usize, SocketError> {
        let _ = (address, flags);
        // Default implementation for stream sockets - ignore address
        self.write(data).map_err(|_| SocketError::NotSupported)
    }

    /// Receive data with source address (for datagram sockets)
    /// For stream sockets, returns Unspecified address
    fn recvfrom(
        &self,
        buffer: &mut [u8],
        flags: u32,
    ) -> Result<(usize, SocketAddress), SocketError> {
        let _ = flags;
        // Default implementation for stream sockets
        let n = self.read(buffer).map_err(|_| SocketError::NotSupported)?;
        Ok((n, SocketAddress::Unspecified))
    }

    /// Optional capability: expose select/pselect readiness/wait interface
    fn as_selectable(&self) -> Option<&dyn Selectable> {
        None
    }

    /// Optional capability: expose control operations interface
    fn as_control_ops(&self) -> Option<&dyn crate::object::capability::ControlOps> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_socket_type() {
        assert_eq!(SocketType::Stream, SocketType::Stream);
        assert_ne!(SocketType::Stream, SocketType::Datagram);
    }

    #[test_case]
    fn test_socket_domain() {
        assert_eq!(SocketDomain::Local, SocketDomain::Local);
        assert_ne!(SocketDomain::Local, SocketDomain::Inet4);
    }

    #[test_case]
    fn test_local_socket_address() {
        let addr = LocalSocketAddress::from_path("/tmp/test.sock").unwrap();
        assert_eq!(addr.path(), "/tmp/test.sock");
        assert!(!addr.is_unnamed());
        assert!(!addr.is_abstract());

        let unnamed = LocalSocketAddress::unnamed();
        assert!(unnamed.is_unnamed());
        assert_eq!(unnamed.path(), "");

        let abstract_addr = LocalSocketAddress::from_abstract("test").unwrap();
        assert!(abstract_addr.is_abstract());
        assert_eq!(abstract_addr.path(), "test");
    }

    #[test_case]
    fn test_local_socket_address_too_long() {
        let long_path = "a".repeat(109);
        assert!(LocalSocketAddress::from_path(long_path).is_err());
    }

    #[test_case]
    fn test_inet4_socket_address() {
        let addr = Inet4SocketAddress::new([127, 0, 0, 1], 8080);
        assert_eq!(addr.addr, [127, 0, 0, 1]);
        assert_eq!(addr.port, 8080);
    }

    #[test_case]
    fn test_socket_address_domain() {
        let local_addr = SocketAddress::Local(LocalSocketAddress::unnamed());
        assert_eq!(local_addr.domain(), SocketDomain::Local);

        let inet_addr = SocketAddress::Inet(Inet4SocketAddress::new([127, 0, 0, 1], 8080));
        assert_eq!(inet_addr.domain(), SocketDomain::Inet4);
    }

    #[test_case]
    fn test_socket_state() {
        assert_eq!(SocketState::Unconnected, SocketState::Unconnected);
        assert_ne!(SocketState::Unconnected, SocketState::Connected);
    }

    #[test_case]
    fn test_shutdown_how() {
        assert_eq!(ShutdownHow::Read, ShutdownHow::Read);
        assert_ne!(ShutdownHow::Read, ShutdownHow::Write);
    }
}
