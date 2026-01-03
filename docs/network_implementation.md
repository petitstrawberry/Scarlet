# Network Functionality Implementation

## Overview

This document describes the design and implementation of network functionality for the Scarlet kernel. Following Scarlet's OS-agnostic philosophy (similar to TTY devices), the core provides abstract socket infrastructure while ABI modules provide concrete implementations.

## Goals

1. **OS-Agnostic Design**: Core provides abstractions only, like TTY devices
2. **Pattern Consistency**: Follow existing patterns (VfsManager, DeviceManager, TTY)
3. **ABI Flexibility**: Support different OS compatibility layers through factory pattern
4. **Bidirectional Communication**: Enable full-duplex communication between endpoints
5. **Protocol Stack Support**: Extensible design for TCP/IP and other network protocols
6. **Local IPC**: Support for local inter-process communication (Unix domain socket equivalent)

## Design Philosophy

**Scarlet is not Unix.** Like TTY devices, the network implementation follows these principles:

- **Core = Abstraction**: Scarlet core provides OS-neutral socket infrastructure
- **ABI = Implementation**: ABI modules (Linux, xv6, etc.) provide concrete socket implementations
- **Neutral Terminology**: Use "Local" not "Unix", SCTL_SOCKET_* not ioctl numbers
- **Extensibility**: Factory pattern + protocol stack support for diverse use cases

## Architecture

### High-Level Design

The network functionality is structured around three main components:

1. **SocketObject**: Abstract trait for network endpoints (defined in core)
2. **NetworkManager**: Global manager for socket lifecycle (registration, lookup, connection tracking)
3. **Socket Implementations**: Provided by ABI modules or protocol stacks

This mirrors both VFS and TTY designs:
- Like VFS: `SocketObject` ≈ `FileObject`, `NetworkManager` ≈ `VfsManager`
- Like TTY: Core defines `SocketObject` trait, ABIs implement it (like `CharDevice` + `TtyControl`)

### Component Structure

```
kernel/src/network/
├── mod.rs                    # NetworkManager and factory registration
├── socket.rs                 # SocketObject/SocketControl traits, SCTL_SOCKET_* opcodes
└── protocol_stack.rs         # Protocol stack abstraction for TCP/IP, UDP, etc.
```

**Note**: No concrete socket implementations in core. ABI modules provide them.

## Core Types and Traits

### SocketControl Trait

Like `TtyControl` for TTY devices, `SocketControl` provides OS-neutral socket operations:

```rust
/// Socket control operations trait (OS-agnostic)
pub trait SocketControl {
    /// Bind socket to an address
    fn bind(&self, address: &SocketAddress) -> Result<(), SocketError>;
    
    /// Connect to a remote address
    fn connect(&self, address: &SocketAddress) -> Result<(), SocketError>;
    
    /// Listen for incoming connections (for stream sockets)
    fn listen(&self, backlog: usize) -> Result<(), SocketError>;
    
    /// Accept an incoming connection (for listening sockets)
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
```

### SocketObject Trait

Combines `StreamIpcOps` (data transfer) + `SocketControl` (connection management) + `CloneOps`:

```rust
/// Complete socket interface (similar to TtyDeviceEndpoint)
pub trait SocketObject: StreamIpcOps + SocketControl + CloneOps + Send + Sync {
    /// Get socket type (Stream, Datagram, etc.)
    fn socket_type(&self) -> SocketType;
    
    /// Get socket domain (Local, Inet, Inet6, etc.)
    fn socket_domain(&self) -> SocketDomain;
    
    /// Get socket protocol
    fn socket_protocol(&self) -> SocketProtocol;
    
    /// Send data to a specific address (for datagram sockets)
    fn sendto(&self, data: &[u8], address: &SocketAddress, flags: u32) 
        -> Result<usize, SocketError>;
    
    /// Receive data with source address (for datagram sockets)
    fn recvfrom(&self, buffer: &mut [u8], flags: u32) 
        -> Result<(usize, SocketAddress), SocketError>;
    
    /// Optional capability: select/poll support
    fn as_selectable(&self) -> Option<&dyn Selectable>;
}
```

### Scarlet-Private Control Opcodes

Like TTY devices use `SCTL_TTY_*`, sockets use `SCTL_SOCKET_*` (magic 'SS' = 0x5353):

```rust
pub mod socket_ctl {
    pub const SCTL_SOCKET_BIND: u32 = 0x5353_0001;
    pub const SCTL_SOCKET_CONNECT: u32 = 0x5353_0002;
    pub const SCTL_SOCKET_LISTEN: u32 = 0x5353_0003;
    pub const SCTL_SOCKET_GETSOCKNAME: u32 = 0x5353_0004;
    pub const SCTL_SOCKET_GETPEERNAME: u32 = 0x5353_0005;
    pub const SCTL_SOCKET_SHUTDOWN: u32 = 0x5353_0006;
    pub const SCTL_SOCKET_SET_NONBLOCK: u32 = 0x5353_0007;
    pub const SCTL_SOCKET_GET_STATE: u32 = 0x5353_0008;
    pub const SCTL_SOCKET_GET_TYPE: u32 = 0x5353_0009;
    pub const SCTL_SOCKET_IS_CONNECTED: u32 = 0x5353_000A;
}
```

**ABI modules translate OS-specific syscalls/ioctls to these opcodes.**

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

/// Socket domain (address family) - OS-agnostic terminology
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketDomain {
    /// Local inter-process communication (NOT tied to Unix)
    Local,
    /// IPv4 Internet protocols
    Inet,
    /// IPv6 Internet protocols
    Inet6,
    /// Packet-level communication
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
```

Note: `LocalSocketAddress` is the OS-agnostic version of Unix domain socket addresses.

### NetworkManager

The NetworkManager follows the DeviceManager and VfsManager patterns with a factory-based design:

```rust
/// Network Manager - Global socket and connection manager
pub struct NetworkManager {
    /// Socket factories per domain (registered by ABI modules)
    socket_factories: RwLock<BTreeMap<SocketDomain, SocketFactory>>,

    /// Protocol stacks for network protocols (TCP/IP, UDP, etc.)
    protocol_stacks: ProtocolStackManager,
    
    /// Named sockets namespace (path/name -> socket)
    /// Used by ABI modules for Local IPC
    named_sockets: RwLock<BTreeMap<String, Weak<dyn SocketObject>>>,
    
    /// Active socket connections by ID
    connections: RwLock<BTreeMap<SocketId, Arc<dyn SocketObject>>>,
    
    /// Next socket ID counter
    next_socket_id: AtomicUsize,
}

impl NetworkManager {
    /// Register a socket factory for a specific domain
    /// (Called by ABI modules)
    pub fn register_socket_factory(&self, domain: SocketDomain, factory: SocketFactory);
    
    /// Register a protocol stack
    /// (Called by network drivers or ABI modules)
    pub fn register_protocol_stack(&self, stack: Arc<dyn ProtocolStack>);
    
    /// Create a new socket
    /// Priority: socket factories first, then protocol stacks
    pub fn create_socket(
        &self,
        domain: SocketDomain,
        socket_type: SocketType,
        protocol: SocketProtocol,
    ) -> Result<KernelObject, SocketError>;
    
    /// Register a named socket (for Local IPC with path-based addressing)
    /// (Used by ABI modules)
    pub fn register_named_socket(
        &self,
        name: &str,
        socket: Arc<dyn SocketObject>,
    ) -> Result<(), SocketError>;
    
    /// Lookup a named socket by path
    /// (Used by ABI modules for Local IPC connection establishment)
    pub fn lookup_named_socket(&self, name: &str) 
        -> Result<Arc<dyn SocketObject>, SocketError>;
    
    /// Process an incoming network packet
    /// (Routes packet to appropriate protocol stack)
    pub fn process_packet(&self, packet: &DevicePacket) -> Result<(), SocketError>;
}
```

## Implementation Details

### ABI Module Responsibilities

ABI modules (Linux, xv6, etc.) must:

1. **Implement SocketObject** for their specific socket types
2. **Register socket factories** for domains they support
3. **Translate syscalls** to SCTL_SOCKET_* opcodes
4. **Handle OS-specific semantics** (e.g., Unix domain socket permissions, credentials)

**Example: Linux ABI Local Socket**
```rust
// In abi/linux module
struct LinuxLocalSocket {
    // Internal state...
}

impl SocketObject for LinuxLocalSocket {
    fn socket_type(&self) -> SocketType { SocketType::Stream }
    fn socket_domain(&self) -> SocketDomain { SocketDomain::Local }
    // Implement other required methods...
}

fn linux_create_local_socket(typ: SocketType, proto: SocketProtocol) 
    -> Result<Arc<dyn SocketObject>, SocketError> {
    Ok(Arc::new(LinuxLocalSocket::new(typ, proto)?))
}

// During Linux ABI initialization
NetworkManager::get_manager().register_socket_factory(
    SocketDomain::Local,
    linux_create_local_socket
);
```

### Protocol Stack Abstraction

For TCP/IP, UDP, and other network protocols:

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
    
    /// Process incoming packet from network device
    fn process_incoming_packet(&self, packet: &DevicePacket) -> Result<(), SocketError>;
    
    /// Send packet through network device
    fn send_packet(&self, packet: DevicePacket) -> Result<(), SocketError>;
    
    /// Get protocol stack statistics
    fn statistics(&self) -> ProtocolStackStats;
    
    /// Check if protocol stack supports a socket type/protocol
    fn supports(&self, socket_type: SocketType, protocol: SocketProtocol) -> bool;
}
```

**Example: TCP/IP Stack**
```rust
struct TcpIpStack {
    // TCP/IP state, routing tables, etc.
}

impl ProtocolStack for TcpIpStack {
    fn domain(&self) -> SocketDomain { SocketDomain::Inet }
    
    fn create_socket(&self, typ: SocketType, proto: SocketProtocol) 
        -> Result<Arc<dyn SocketObject>, SocketError> {
        match (typ, proto) {
            (SocketType::Stream, SocketProtocol::Tcp) => {
                Ok(Arc::new(TcpSocket::new(self)))
            }
            (SocketType::Datagram, SocketProtocol::Udp) => {
                Ok(Arc::new(UdpSocket::new(self)))
            }
            _ => Err(SocketError::NotSupported),
        }
    }
    // ...
}

// Register protocol stack
let tcp_ip = Arc::new(TcpIpStack::new());
NetworkManager::get_manager().register_protocol_stack(tcp_ip);
```

### Integration with KernelObject

Socket variant added to KernelObject enum:

```rust
#[cfg(feature = "network")]
pub enum KernelObject {
    File(Arc<dyn FileObject>),
    Pipe(Arc<dyn PipeObject>),
    EventChannel(Arc<EventChannelObject>),
    EventSubscription(Arc<EventSubscriptionObject>),
    Socket(Arc<dyn SocketObject>),  // New variant
}
```

Capability methods:
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

## ABI Module Integration

### System Call Translation

ABI modules are responsible for:
1. Implementing OS-specific system calls (e.g., Linux `socket()`, `bind()`, etc.)
2. Translating to SCTL_SOCKET_* control opcodes
3. Converting OS-specific address structures to `SocketAddress`
4. Handling OS-specific semantics (permissions, credentials, etc.)

**Example: Linux ABI socket syscall**
```rust
// In abi/linux/syscall/net.rs
pub fn sys_socket(domain: i32, type_: i32, protocol: i32) -> Result<usize, SyscallError> {
    // Translate Linux domain values to SocketDomain
    let socket_domain = match domain {
        libc::AF_UNIX => SocketDomain::Local,
        libc::AF_INET => SocketDomain::Inet,
        libc::AF_INET6 => SocketDomain::Inet6,
        _ => return Err(SyscallError::InvalidArgument),
    };
    
    // Translate socket type
    let socket_type = translate_socket_type(type_)?;
    let protocol = translate_protocol(protocol)?;
    
    // Create socket using NetworkManager
    let socket_obj = NetworkManager::get_manager()
        .create_socket(socket_domain, socket_type, protocol)?;
    
    // Add to process handle table
    let handle = current_task().handle_table().insert(socket_obj)?;
    Ok(handle)
}

pub fn sys_bind(sockfd: usize, addr: usize, addrlen: usize) -> Result<usize, SyscallError> {
    // Get socket from handle
    let socket = current_task().handle_table()
        .get(sockfd)?
        .as_socket()
        .ok_or(SyscallError::InvalidHandle)?;
    
    // Translate Linux sockaddr to SocketAddress
    let socket_addr = translate_sockaddr_from_user(addr, addrlen)?;
    
    // Call bind through SocketControl
    socket.bind(&socket_addr)?;
    Ok(0)
}
```

### Example: Local IPC (Unix Domain Socket Equivalent)

**Linux ABI Implementation:**
```rust
// Linux ABI provides Unix domain socket emulation using Local sockets
impl LinuxLocalSocket {
    fn new(socket_type: SocketType) -> Result<Self, SocketError> {
        Ok(Self {
            state: SocketState::Unconnected,
            buffer: VecDeque::new(),
            // ... other fields
        })
    }
}

impl SocketObject for LinuxLocalSocket {
    fn socket_domain(&self) -> SocketDomain { SocketDomain::Local }
    
    fn bind(&self, address: &SocketAddress) -> Result<(), SocketError> {
        match address {
            SocketAddress::Local(local_addr) => {
                // Register with NetworkManager named socket registry
                NetworkManager::get_manager().register_named_socket(
                    local_addr.path(),
                    Arc::new(self.clone())
                )?;
                Ok(())
            }
            _ => Err(SocketError::InvalidAddress),
        }
    }
    
    fn connect(&self, address: &SocketAddress) -> Result<(), SocketError> {
        match address {
            SocketAddress::Local(local_addr) => {
                // Lookup listening socket in NetworkManager
                let listening = NetworkManager::get_manager()
                    .lookup_named_socket(local_addr.path())?;
                    
                // Establish connection (implementation specific)
                // ...
                Ok(())
            }
            _ => Err(SocketError::InvalidAddress),
        }
    }
    // ... other SocketObject methods
}
```

## Integration with Existing Systems

### VFS Integration

Local IPC sockets can optionally integrate with VFS:

1. ABI module creates socket node in filesystem (e.g., `/tmp/socket.sock`)
2. Socket registered with NetworkManager's named socket registry
3. Applications connect through filesystem path
4. VFS handles path resolution, NetworkManager handles socket lookup

### IPC Module Integration

SocketObject extends StreamIpcOps, providing:
- `read()`/`write()` for stream sockets (via StreamOps)
- Integration with existing IPC infrastructure
- Support for select/poll operations via Selectable trait
- Bidirectional communication like pipes

### Device Integration

Network devices provide the physical/virtual layer:
- NetworkDevice trait already exists in `kernel/src/device/network/`
- Protocol stacks bridge SocketObject and NetworkDevice
- Packet flow: Application → Socket → ProtocolStack → NetworkDevice → Hardware

## Usage Examples (for ABI Implementers)

### Example 1: Local IPC Stream Socket (Server)

**In Linux ABI implementation:**
```rust
// Create socket
let socket = NetworkManager::get_manager()
    .create_socket(SocketDomain::Unix, SocketType::Stream, SocketProtocol::Default)?;

// Bind to path
let addr = SocketAddress::Unix(UnixSocketAddress::from_path("/tmp/server.sock")?);
socket.as_socket().unwrap().bind(&addr)?;

// Listen for connections
socket.as_socket().unwrap().listen(5)?;

// Socket created by Linux ABI's factory function
let socket = NetworkManager::get_manager()
    .create_socket(SocketDomain::Local, SocketType::Stream, SocketProtocol::Default)?;

// Bind to path
let addr = SocketAddress::Local(LocalSocketAddress::from_path("/tmp/server.sock")?);
socket.as_socket().unwrap().bind(&addr)?;

// Listen for connections
socket.as_socket().unwrap().listen(5)?;

// Accept connection
let client_socket = socket.as_socket().unwrap().accept()?;

// Read/write data using StreamOps
let mut buffer = vec![0u8; 1024];
let n = client_socket.read(&mut buffer)?;  // StreamOps from SocketObject
```

### Example 2: Local IPC Stream Socket (Client)

**In Linux ABI implementation:**
```rust
// Create socket
let socket = NetworkManager::get_manager()
    .create_socket(SocketDomain::Local, SocketType::Stream, SocketProtocol::Default)?;

// Connect to server
let addr = SocketAddress::Local(LocalSocketAddress::from_path("/tmp/server.sock")?);
socket.as_socket().unwrap().connect(&addr)?;

// Write data using StreamOps
let data = b"Hello, server!";
socket.write(data)?;  // StreamOps from SocketObject
```

### Example 3: TCP Socket (Using Protocol Stack)

**In network stack or ABI implementation:**
```rust
// First, register TCP/IP protocol stack (during initialization)
let tcp_ip_stack = Arc::new(TcpIpStack::new());
NetworkManager::get_manager().register_protocol_stack(tcp_ip_stack);

// Later, create TCP socket
let socket = NetworkManager::get_manager()
    .create_socket(SocketDomain::Inet, SocketType::Stream, SocketProtocol::Tcp)?;

// Bind to local address
let local_addr = SocketAddress::Inet(Inet4SocketAddress::new([0, 0, 0, 0], 8080));
socket.as_socket().unwrap().bind(&local_addr)?;

// Connect to remote host
let remote_addr = SocketAddress::Inet(Inet4SocketAddress::new([192, 168, 1, 1], 80));
socket.as_socket().unwrap().connect(&remote_addr)?;

// Use StreamOps for data transfer
socket.write(b"GET / HTTP/1.1\r\n\r\n")?;
```

## Testing Strategy

### Core Infrastructure Tests

Tests in `kernel/src/network/`:

1. **SocketAddress**: Test address creation, validation, domain detection
2. **NetworkManager**: Test factory registration, socket creation priority
3. **ProtocolStackManager**: Test stack registration, packet routing

### ABI Module Tests

Tests in ABI modules (e.g., `kernel/src/abi/linux/`):

1. **Local Socket Implementation**: Test bind, connect, listen, accept
2. **Data Transfer**: Test bidirectional communication
3. **Error Handling**: Test error conditions (connection refused, etc.)
4. **State Management**: Test socket state transitions
5. **Syscall Translation**: Test conversion from Linux syscalls to SCTL_SOCKET_*

### Integration Tests

1. **IPC Integration**: Test StreamOps compatibility
2. **VFS Integration**: Test filesystem path-based socket access (if implemented)
3. **Multi-socket**: Test multiple simultaneous connections
4. **Protocol Stack**: Test TCP/IP integration when available

## Implementation Status

### Phase 1: Core Infrastructure (COMPLETED ✓)
- ✓ Design documentation (English + Japanese)
- ✓ SocketObject and SocketControl traits
- ✓ NetworkManager with factory pattern
- ✓ ProtocolStack trait and ProtocolStackManager
- ✓ KernelObject::Socket variant
- ✓ SCTL_SOCKET_* control opcodes
- ✓ SocketDomain, SocketType, SocketAddress types
- ✓ Build integration and tests

### Phase 2: ABI Module Implementation (For ABI maintainers)
- [ ] Linux ABI: Local socket (Unix domain socket equivalent)
- [ ] Linux ABI: System call translation (socket, bind, connect, etc.)
- [ ] Linux ABI: Address structure conversion
- [ ] xv6 ABI: Socket support (if needed)
- [ ] Unit tests for socket implementations

### Phase 3: Protocol Stack Implementation (Future)
- [ ] TCP/IP protocol stack
- [ ] UDP socket support
- [ ] IPv4/IPv6 address handling
- [ ] Packet routing and processing
- [ ] Network device integration

### Phase 4: Advanced Features (Future)
- [ ] VFS integration for named sockets
- [ ] Credential passing (SCM_CREDENTIALS)
- [ ] File descriptor passing (SCM_RIGHTS)
- [ ] Ancillary data support
- [ ] Raw socket support

## Security Considerations

**For ABI Implementers:**

1. **Address Validation**: Validate all socket addresses and lengths from user space
2. **Buffer Boundaries**: Prevent buffer overflows when copying data
3. **Permission Checks**: Implement OS-specific permission checks (e.g., filesystem permissions for Local sockets)
4. **Resource Limits**: Enforce limits on socket count, buffer sizes, connection backlogs
5. **Credential Verification**: Validate user credentials for privileged operations

**Core Infrastructure:**

- NetworkManager uses interior mutability with proper locking
- Socket factories and protocol stacks registered during initialization only
- No user-controlled function pointers

## Performance Considerations

**Core Design:**

1. **Lock-free Lookups**: Use RwLock for read-heavy operations
2. **Minimal Allocations**: Reuse socket IDs, weak references for named sockets
3. **Zero-copy Potential**: StreamOps interface allows buffer sharing
4. **Two-tier Creation**: Try factory first (fast path) before protocol stack

**For ABI Implementers:**

1. **Efficient Buffers**: Use ring buffers or similar for socket data
2. **Async Support**: Implement non-blocking mode via SCTL_SOCKET_SET_NONBLOCK
3. **Connection Pooling**: Reuse socket structures when possible
4. **Select/Poll**: Implement Selectable trait for efficient I/O multiplexing

## Compatibility Notes

### Linux ABI
- Translate Linux AF_* domains to SocketDomain
- Map Linux SOCK_* types to SocketType
- Convert Linux sockaddr structures to SocketAddress
- Implement Linux-specific socket options as needed
- Use SCTL_SOCKET_* opcodes internally

### xv6 ABI
- Simplified socket interface focusing on core functionality
- May not support all socket options
- Focus on Local IPC and basic networking

### Scarlet Native ABI (Future)
- Direct use of SocketObject API
- No translation overhead
- Access to all Scarlet-specific features

## Future Enhancements

1. **Network Protocols**
   - TCP/IP stack with congestion control
   - UDP socket implementation
   - ICMP and raw socket support
   - IPv6 full support
   
2. **Advanced IPC**
   - Socket pairs (socketpair syscall)
   - Abstract namespace for Local sockets
   - Multicast/broadcast support
   
3. **Performance**
   - Sendfile system call
   - Splice/tee operations
   - Zero-copy networking optimizations
   - DPDK-style direct device access
   
4. **Security**
   - SELinux/AppArmor socket labeling
   - Network namespace support
   - Fine-grained capability system

## References

- POSIX Socket API Specification
- Linux socket(7) and unix(7) man pages
- Stevens, W. Richard. "Unix Network Programming"
- Scarlet VFS v2 design (`kernel/src/fs/vfs_v2/`)
- Scarlet IPC design (`kernel/src/ipc/`)
- Scarlet Device Manager (`kernel/src/device/manager.rs`)

## Revision History

- 2026-01-03: Initial design document
