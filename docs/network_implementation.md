# Network Functionality Implementation

## Overview

This document describes the design and implementation of network functionality for the Scarlet kernel. The design provides Unix domain socket-like capabilities with support for bidirectional communication and protocol stacks (TCP/IP), while following Scarlet's existing architectural patterns.

## Goals

1. **Pattern Consistency**: Follow existing patterns established by VfsManager and DeviceManager
2. **Abstraction**: Support different OS compatibility layers through well-defined abstractions
3. **Bidirectional Communication**: Enable full-duplex communication between endpoints
4. **Protocol Stack Support**: Design for future TCP/IP and other protocol implementations
5. **Unix Domain Socket**: Provide local IPC through filesystem-like paths

## Architecture

### High-Level Design

The network functionality is structured around three main components:

1. **SocketObject**: A KernelObject type representing network endpoints
2. **NetworkManager**: A global manager handling socket lifecycle and connections
3. **Socket Types**: Different socket implementations (UnixDomain, TCP, UDP, etc.)

This mirrors the VFS design where:
- `SocketObject` ≈ `FileObject` (represents an endpoint)
- `NetworkManager` ≈ `VfsManager` (manages resources)
- Socket types ≈ Filesystem types (different implementations)

### Component Structure

```
kernel/src/network/
├── mod.rs                    # Module definition and NetworkManager
├── socket.rs                 # SocketObject trait and common types
├── unix_domain.rs            # Unix domain socket implementation
├── protocol_stack.rs         # Protocol stack abstraction
└── syscall.rs               # Socket-related system calls
```

## Core Types and Traits

### SocketObject Trait

```rust
/// Socket operations trait extending StreamIpcOps
pub trait SocketObject: StreamIpcOps + CloneOps {
    /// Get socket type (Stream, Datagram, etc.)
    fn socket_type(&self) -> SocketType;
    
    /// Get socket domain (Unix, IPv4, IPv6, etc.)
    fn socket_domain(&self) -> SocketDomain;
    
    /// Get socket protocol
    fn socket_protocol(&self) -> SocketProtocol;
    
    /// Bind socket to an address
    fn bind(&self, address: &SocketAddress) -> Result<(), SocketError>;
    
    /// Connect to a remote address
    fn connect(&self, address: &SocketAddress) -> Result<(), SocketError>;
    
    /// Listen for incoming connections (for stream sockets)
    fn listen(&self, backlog: usize) -> Result<(), SocketError>;
    
    /// Accept an incoming connection (for listening sockets)
    fn accept(&self) -> Result<Arc<dyn SocketObject>, SocketError>;
    
    /// Send data to a specific address (for datagram sockets)
    fn sendto(&self, data: &[u8], address: &SocketAddress, flags: u32) 
        -> Result<usize, SocketError>;
    
    /// Receive data with source address (for datagram sockets)
    fn recvfrom(&self, buffer: &mut [u8], flags: u32) 
        -> Result<(usize, SocketAddress), SocketError>;
    
    /// Get socket peer address
    fn getpeername(&self) -> Result<SocketAddress, SocketError>;
    
    /// Get socket local address
    fn getsockname(&self) -> Result<SocketAddress, SocketError>;
    
    /// Set socket options
    fn setsockopt(&self, level: i32, optname: i32, optval: &[u8]) 
        -> Result<(), SocketError>;
    
    /// Get socket options
    fn getsockopt(&self, level: i32, optname: i32, optval: &mut [u8]) 
        -> Result<usize, SocketError>;
    
    /// Shutdown socket for reading, writing, or both
    fn shutdown(&self, how: ShutdownHow) -> Result<(), SocketError>;
    
    /// Check if socket is connected
    fn is_connected(&self) -> bool;
    
    /// Get socket state
    fn state(&self) -> SocketState;
}
```

### Socket Types and Enums

```rust
/// Socket type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    /// Stream socket (connection-oriented, reliable)
    Stream,
    /// Datagram socket (connectionless, unreliable)
    Datagram,
    /// Raw socket (direct protocol access)
    Raw,
    /// Sequenced packet socket
    SeqPacket,
}

/// Socket domain (address family)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketDomain {
    /// Unix domain sockets (local IPC)
    Unix,
    /// IPv4 Internet protocols
    Inet,
    /// IPv6 Internet protocols
    Inet6,
    /// Netlink sockets (kernel-user communication)
    Netlink,
    /// Packet sockets (low-level packet interface)
    Packet,
}

/// Socket protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketProtocol {
    /// Default protocol for socket type/domain
    Default,
    /// TCP protocol
    Tcp,
    /// UDP protocol
    Udp,
    /// ICMP protocol
    Icmp,
    /// Raw protocol with specific number
    Raw(u16),
}

/// Socket address abstraction
#[derive(Debug, Clone)]
pub enum SocketAddress {
    /// Unix domain socket address (file path)
    Unix(UnixSocketAddress),
    /// IPv4 address with port
    Inet(Inet4SocketAddress),
    /// IPv6 address with port
    Inet6(Inet6SocketAddress),
    /// Unspecified/any address
    Unspecified,
}

/// Socket shutdown directions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownHow {
    /// Shutdown reading
    Read,
    /// Shutdown writing
    Write,
    /// Shutdown both reading and writing
    Both,
}

/// Socket state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    /// Socket is unbound and unconnected
    Unconnected,
    /// Socket is bound to an address
    Bound,
    /// Socket is listening for connections
    Listening,
    /// Socket is connecting (for non-blocking sockets)
    Connecting,
    /// Socket is connected
    Connected,
    /// Socket is disconnecting
    Disconnecting,
    /// Socket is closed
    Closed,
}
```

### NetworkManager

The NetworkManager follows the DeviceManager and VfsManager patterns:

```rust
/// Network Manager - Global socket and connection manager
pub struct NetworkManager {
    /// Unix domain socket namespace (path -> listening socket)
    unix_sockets: RwLock<BTreeMap<String, Weak<dyn SocketObject>>>,
    
    /// Active socket connections
    connections: RwLock<BTreeMap<SocketId, Arc<dyn SocketObject>>>,
    
    /// Protocol stack registry
    protocol_stacks: RwLock<BTreeMap<SocketDomain, Arc<dyn ProtocolStack>>>,
    
    /// Next socket ID
    next_socket_id: AtomicUsize,
}

impl NetworkManager {
    /// Create a new socket
    pub fn create_socket(
        &self,
        domain: SocketDomain,
        socket_type: SocketType,
        protocol: SocketProtocol,
    ) -> Result<KernelObject, SocketError>;
    
    /// Register a Unix domain socket at a path
    pub fn register_unix_socket(
        &self,
        path: &str,
        socket: Arc<dyn SocketObject>,
    ) -> Result<(), SocketError>;
    
    /// Lookup a Unix domain socket by path
    pub fn lookup_unix_socket(&self, path: &str) 
        -> Result<Arc<dyn SocketObject>, SocketError>;
    
    /// Register a protocol stack
    pub fn register_protocol_stack(
        &self,
        domain: SocketDomain,
        stack: Arc<dyn ProtocolStack>,
    ) -> Result<(), SocketError>;
    
    /// Get a protocol stack for a domain
    pub fn get_protocol_stack(&self, domain: SocketDomain) 
        -> Option<Arc<dyn ProtocolStack>>;
}
```

## Implementation Details

### Unix Domain Sockets

Unix domain sockets provide local IPC through filesystem-like paths:

**Key Features:**
- Stream-oriented (SOCK_STREAM) and datagram (SOCK_DGRAM) support
- Bidirectional communication
- Connection-oriented streams
- Connectionless datagrams
- Credential passing capability (future)
- File descriptor passing capability (future)

**Internal Structure:**
```rust
/// Unix domain stream socket
struct UnixStreamSocket {
    /// Socket state
    state: Arc<Mutex<UnixSocketState>>,
    /// Local address (path)
    local_addr: RwLock<Option<String>>,
    /// Peer address
    peer_addr: RwLock<Option<String>>,
    /// Connection backlog for listening sockets
    backlog: Mutex<VecDeque<Arc<UnixStreamSocket>>>,
    /// Data buffer
    buffer: Mutex<VecDeque<u8>>,
    /// Socket options
    options: RwLock<SocketOptions>,
}
```

### Protocol Stack Abstraction

To support TCP/IP and other protocol stacks:

```rust
/// Protocol stack trait for network protocols
pub trait ProtocolStack: Send + Sync {
    /// Get protocol stack domain
    fn domain(&self) -> SocketDomain;
    
    /// Create a socket for this protocol stack
    fn create_socket(
        &self,
        socket_type: SocketType,
        protocol: SocketProtocol,
    ) -> Result<Arc<dyn SocketObject>, SocketError>;
    
    /// Process incoming packet
    fn process_packet(&self, packet: &[u8]) -> Result<(), SocketError>;
    
    /// Get protocol stack statistics
    fn statistics(&self) -> ProtocolStackStats;
}
```

### Integration with KernelObject

Add Socket variant to KernelObject enum:

```rust
pub enum KernelObject {
    File(Arc<dyn FileObject>),
    Pipe(Arc<dyn PipeObject>),
    EventChannel(Arc<EventChannelObject>),
    EventSubscription(Arc<EventSubscriptionObject>),
    Socket(Arc<dyn SocketObject>),  // New variant
}
```

Implement capability methods:
```rust
impl KernelObject {
    /// Try to get SocketObject capability
    pub fn as_socket(&self) -> Option<&dyn SocketObject> {
        match self {
            KernelObject::Socket(socket) => Some(socket.as_ref()),
            _ => None,
        }
    }
}
```

## System Call Interface

### Socket Creation and Management

```rust
/// Create a new socket
/// sys_socket(domain: i32, type: i32, protocol: i32) -> Result<Handle, Error>
pub fn sys_socket(domain: i32, type_: i32, protocol: i32) -> Result<usize, SyscallError>;

/// Bind socket to an address
/// sys_bind(sockfd: Handle, addr: *const u8, addrlen: usize) -> Result<(), Error>
pub fn sys_bind(sockfd: usize, addr: usize, addrlen: usize) -> Result<usize, SyscallError>;

/// Connect to a remote address
/// sys_connect(sockfd: Handle, addr: *const u8, addrlen: usize) -> Result<(), Error>
pub fn sys_connect(sockfd: usize, addr: usize, addrlen: usize) -> Result<usize, SyscallError>;

/// Listen for connections
/// sys_listen(sockfd: Handle, backlog: i32) -> Result<(), Error>
pub fn sys_listen(sockfd: usize, backlog: i32) -> Result<usize, SyscallError>;

/// Accept a connection
/// sys_accept(sockfd: Handle, addr: *mut u8, addrlen: *mut usize) -> Result<Handle, Error>
pub fn sys_accept(sockfd: usize, addr: usize, addrlen: usize) -> Result<usize, SyscallError>;
```

### Data Transfer

```rust
/// Send data through socket
/// sys_sendto(sockfd: Handle, buf: *const u8, len: usize, flags: i32, 
///            dest_addr: *const u8, addrlen: usize) -> Result<usize, Error>
pub fn sys_sendto(
    sockfd: usize, buf: usize, len: usize, flags: i32,
    dest_addr: usize, addrlen: usize
) -> Result<usize, SyscallError>;

/// Receive data from socket
/// sys_recvfrom(sockfd: Handle, buf: *mut u8, len: usize, flags: i32,
///              src_addr: *mut u8, addrlen: *mut usize) -> Result<usize, Error>
pub fn sys_recvfrom(
    sockfd: usize, buf: usize, len: usize, flags: i32,
    src_addr: usize, addrlen: usize
) -> Result<usize, SyscallError>;
```

### Socket Options and Control

```rust
/// Get socket name (local address)
/// sys_getsockname(sockfd: Handle, addr: *mut u8, addrlen: *mut usize) 
///     -> Result<(), Error>
pub fn sys_getsockname(sockfd: usize, addr: usize, addrlen: usize) 
    -> Result<usize, SyscallError>;

/// Get peer name (remote address)
/// sys_getpeername(sockfd: Handle, addr: *mut u8, addrlen: *mut usize) 
///     -> Result<(), Error>
pub fn sys_getpeername(sockfd: usize, addr: usize, addrlen: usize) 
    -> Result<usize, SyscallError>;

/// Set socket option
/// sys_setsockopt(sockfd: Handle, level: i32, optname: i32, 
///                optval: *const u8, optlen: usize) -> Result<(), Error>
pub fn sys_setsockopt(
    sockfd: usize, level: i32, optname: i32, optval: usize, optlen: usize
) -> Result<usize, SyscallError>;

/// Get socket option
/// sys_getsockopt(sockfd: Handle, level: i32, optname: i32,
///                optval: *mut u8, optlen: *mut usize) -> Result<(), Error>
pub fn sys_getsockopt(
    sockfd: usize, level: i32, optname: i32, optval: usize, optlen: usize
) -> Result<usize, SyscallError>;

/// Shutdown socket
/// sys_shutdown(sockfd: Handle, how: i32) -> Result<(), Error>
pub fn sys_shutdown(sockfd: usize, how: i32) -> Result<usize, SyscallError>;
```

## Integration with Existing Systems

### VFS Integration

Unix domain sockets can be accessed through the VFS:

1. Create socket node in filesystem (e.g., `/tmp/socket.sock`)
2. Register with NetworkManager
3. Applications can connect through filesystem path
4. VFS handles path resolution, NetworkManager handles socket operations

### IPC Module Integration

SocketObject extends StreamIpcOps, providing:
- `read()`/`write()` for stream sockets
- Integration with existing IPC infrastructure
- Support for select/poll operations via Selectable trait

### Device Integration

Network devices provide the physical/virtual layer:
- NetworkDevice trait already exists in `kernel/src/device/network/`
- Protocol stacks bridge SocketObject and NetworkDevice
- Packets flow: Application → Socket → ProtocolStack → NetworkDevice

## Usage Examples

### Unix Domain Stream Socket (Server)

```rust
// Create socket
let socket = NetworkManager::get_manager()
    .create_socket(SocketDomain::Unix, SocketType::Stream, SocketProtocol::Default)?;

// Bind to path
let addr = SocketAddress::Unix(UnixSocketAddress::from_path("/tmp/server.sock")?);
socket.as_socket().unwrap().bind(&addr)?;

// Listen for connections
socket.as_socket().unwrap().listen(5)?;

// Accept connection
let client_socket = socket.as_socket().unwrap().accept()?;

// Read/write data using StreamOps
let mut buffer = vec![0u8; 1024];
let n = client_socket.as_stream().unwrap().read(&mut buffer)?;
```

### Unix Domain Stream Socket (Client)

```rust
// Create socket
let socket = NetworkManager::get_manager()
    .create_socket(SocketDomain::Unix, SocketType::Stream, SocketProtocol::Default)?;

// Connect to server
let addr = SocketAddress::Unix(UnixSocketAddress::from_path("/tmp/server.sock")?);
socket.as_socket().unwrap().connect(&addr)?;

// Write data using StreamOps
let data = b"Hello, server!";
socket.as_stream().unwrap().write(data)?;
```

### Unix Domain Datagram Socket

```rust
// Create socket
let socket = NetworkManager::get_manager()
    .create_socket(SocketDomain::Unix, SocketType::Datagram, SocketProtocol::Default)?;

// Bind to address
let local_addr = SocketAddress::Unix(UnixSocketAddress::from_path("/tmp/client.sock")?);
socket.as_socket().unwrap().bind(&local_addr)?;

// Send datagram
let remote_addr = SocketAddress::Unix(UnixSocketAddress::from_path("/tmp/server.sock")?);
let data = b"Hello!";
socket.as_socket().unwrap().sendto(data, &remote_addr, 0)?;

// Receive datagram
let mut buffer = vec![0u8; 1024];
let (n, sender_addr) = socket.as_socket().unwrap().recvfrom(&mut buffer, 0)?;
```

## Testing Strategy

### Unit Tests

Each component should have comprehensive unit tests:

1. **Socket Creation**: Test socket creation with various parameters
2. **Unix Domain Sockets**: Test bind, connect, listen, accept operations
3. **Data Transfer**: Test bidirectional data transfer
4. **Error Handling**: Test error conditions (connection refused, etc.)
5. **State Management**: Test socket state transitions

### Integration Tests

Test interaction between components:

1. **VFS Integration**: Test socket access through filesystem paths
2. **IPC Integration**: Test StreamOps implementation
3. **Multi-socket**: Test multiple simultaneous connections
4. **Concurrent Access**: Test thread-safe operations

### System Tests

End-to-end testing in realistic scenarios:

1. **Echo Server**: Implement and test echo server/client
2. **File Transfer**: Test large data transfers
3. **Connection Management**: Test multiple clients
4. **Protocol Stack**: Test TCP/IP stack when implemented

## Implementation Phases

### Phase 1: Foundation (Current)
- ✓ Design documentation
- [ ] SocketObject trait definition
- [ ] NetworkManager skeleton
- [ ] KernelObject::Socket variant
- [ ] Basic error types

### Phase 2: Unix Domain Sockets
- [ ] UnixStreamSocket implementation
- [ ] UnixDatagramSocket implementation
- [ ] Unix socket address handling
- [ ] Bind/connect/listen/accept operations
- [ ] Data transfer (read/write)
- [ ] Unit tests

### Phase 3: System Call Interface
- [ ] Socket system call implementation
- [ ] Bind/connect/listen/accept syscalls
- [ ] Send/receive syscalls
- [ ] Socket option syscalls
- [ ] Integration with handle table

### Phase 4: VFS Integration
- [ ] Socket filesystem nodes
- [ ] Path-based socket access
- [ ] Permission checking
- [ ] Socket cleanup on close

### Phase 5: Protocol Stack (Future)
- [ ] ProtocolStack trait implementation
- [ ] TCP/IP stack integration
- [ ] UDP socket implementation
- [ ] Raw socket support

## Security Considerations

1. **Address Validation**: Validate all socket addresses and lengths
2. **Buffer Boundaries**: Prevent buffer overflows in data transfer
3. **Permission Checks**: Enforce Unix socket file permissions
4. **Resource Limits**: Implement limits on socket count and buffer sizes
5. **Connection Limits**: Limit backlog size and concurrent connections

## Performance Considerations

1. **Zero-copy**: Use shared memory for large data transfers when possible
2. **Buffer Management**: Implement efficient ring buffers
3. **Lock Contention**: Minimize lock scope in hot paths
4. **Async Operations**: Support non-blocking and async operations
5. **Connection Pooling**: Reuse socket structures efficiently

## Compatibility Notes

### Linux ABI Compatibility
- Socket system calls match Linux semantics
- Socket option values compatible with Linux
- Address structure layouts match Linux sockaddr

### xv6 ABI Compatibility
- Simplified socket interface for xv6
- May not support all socket options
- Focus on core functionality

## Future Enhancements

1. **Advanced Features**
   - Credential passing (SCM_CREDENTIALS)
   - File descriptor passing (SCM_RIGHTS)
   - Ancillary data support
   
2. **Network Protocols**
   - TCP/IP stack implementation
   - UDP socket support
   - Raw socket support
   - IPv6 support
   
3. **Performance**
   - Sendfile system call
   - Splice/tee operations
   - Zero-copy networking
   
4. **Advanced IPC**
   - UNIX socket pairs (socketpair)
   - Abstract namespace sockets
   - Multicast support

## References

- POSIX Socket API Specification
- Linux socket(7) and unix(7) man pages
- Stevens, W. Richard. "Unix Network Programming"
- Scarlet VFS v2 design (`kernel/src/fs/vfs_v2/`)
- Scarlet IPC design (`kernel/src/ipc/`)
- Scarlet Device Manager (`kernel/src/device/manager.rs`)

## Revision History

- 2026-01-03: Initial design document
