# Scarlet Network Architecture

## Executive Summary

This document presents the network architecture design and implementation status for Scarlet. The design balances simplicity, flexibility, and practical implementation concerns, providing OS-agnostic socket infrastructure with protocol-agnostic layer composition.

## Core Design Principles

### 1. Socket as Top-Level Handle Only

**Decision**: SocketObject holds ONLY a reference to the top-level protocol layer (e.g., TCP), not the entire chain.

**Rationale**:
- Simplifies socket implementation - doesn't need to know about IP, Ethernet, etc.
- Follows natural layering - application only cares about transport protocol
- Reduces redundancy - no need to store references to layers the socket never directly uses

```rust
struct TcpSocket {
    tcp_layer: Arc<dyn NetworkLayer>,  // ONLY the top layer
    local_port: u16,
    remote_port: u16,
    // ... socket state
}
```

### 2. Layer-to-Layer Routing with Hints

**Decision**: When sending, caller provides hints in LayerContext; each layer routes to next layer based on context.

**Send Flow**:
```
Socket → TCP Layer (adds TCP info to context)
           ↓
         IP Layer (adds IP info, routes based on destination IP)
           ↓
         Ethernet Layer (performs ARP, adds MAC addresses)
           ↓
         Device Driver
```

**Key**: Each layer:
1. Reads hints from context (e.g., IP layer reads destination IP)
2. Adds its own protocol information to context
3. Decides which lower layer(s) to use
4. Calls `send()` on selected lower layer(s)

If hints are insufficient for routing, layer returns `SocketError::NoRoute`.

### 3. Reception Configuration via SocketConfig

**Problem**: How does IP layer know "send packets for 192.168.1.100:80 to this socket"?

**Solution**: At socket creation/bind time, SocketConfig flows FROM socket DOWN through layers:

```rust
// User calls bind(192.168.1.100:5000)
let mut config = SocketConfig::new();
config.set("tcp_local_port", &5000u16.to_be_bytes());
config.set("ip_local", &[192, 168, 1, 100]);

// Socket passes config to TCP layer
tcp_socket.configure(config);

// TCP layer:
// 1. Extracts port (5000)
// 2. Registers itself with TCP protocol handler: "deliver port 5000 packets to me"
// 3. Passes config down to IP layer

// IP layer:
// 1. Extracts IP address (192.168.1.100)
// 2. Registers itself: "deliver packets for 192.168.1.100 to me"
// 3. May pass config further down if needed
```

**Receive Flow** (opposite direction):
```
Device Driver receives frame
  ↓
Ethernet Layer extracts EtherType (0x0800 = IPv4), calls registered IP handler
  ↓
IP Layer extracts protocol (6 = TCP), calls registered TCP handler  
  ↓
TCP Layer extracts destination port (5000), delivers to registered socket
  ↓
Socket delivers data to application
```

## Complete API Design

### NetworkLayer Trait

```rust
pub trait NetworkLayer: Send + Sync {
    /// Send packet to lower layers with routing hints
    ///
    /// # Arguments
    /// * `packet` - Data to send (layer adds its header)
    /// * `context` - Routing hints (destination addresses, QoS, etc.)
    /// * `next_layers` - Possible lower layers to route to
    ///
    /// # Returns
    /// * `Ok(())` - Packet sent successfully
    /// * `Err(SocketError::NoRoute)` - Insufficient routing information
    /// * `Err(SocketError::ProtocolNotSupported)` - No suitable lower layer
    ///
    /// # Example
    /// ```rust,ignore
    /// // TCP layer sending to IP layer
    /// let mut ctx = LayerContext::new();
    /// ctx.set("ip_dst", &[192, 168, 1, 1]);
    /// ctx.set("ip_src", &[192, 168, 1, 100]);
    /// tcp_layer.send(&data, &ctx, &[ip_layer.clone()])?;
    /// ```
    fn send(
        &self,
        packet: &[u8],
        context: &LayerContext,
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError>;
    
    /// Receive packet from lower layer
    ///
    /// Layer parses packet, extracts protocol info, and routes to registered
    /// upper layer handler.
    ///
    /// # Arguments
    /// * `packet` - Complete packet with this layer's header
    ///
    /// # Returns  
    /// * `Ok(())` - Packet processed successfully
    /// * `Err(SocketError::ProtocolNotSupported)` - Unknown upper layer protocol
    /// * `Err(SocketError::InvalidPacket)` - Malformed packet
    fn receive(&self, packet: &[u8]) -> Result<(), SocketError>;
    
    /// Register upper layer protocol handler
    ///
    /// # Arguments
    /// * `proto_num` - Protocol number (e.g., TCP=6, UDP=17 for IP layer)
    /// * `handler` - Layer to route packets to
    ///
    /// # Example
    /// ```rust,ignore
    /// // IP layer registers TCP handler
    /// ip_layer.register_protocol(6, tcp_layer.clone());
    /// ip_layer.register_protocol(17, udp_layer.clone());
    /// ```
    fn register_protocol(&self, proto_num: u16, handler: Arc<dyn NetworkLayer>);
    
    /// Configure layer for receiving packets
    ///
    /// Called when socket is created/bound. Layer extracts relevant config,
    /// registers itself for packet delivery, and passes config to lower layers.
    ///
    /// # Arguments
    /// * `config` - Configuration from socket (addresses, ports, etc.)
    /// * `next_layers` - Lower layers to configure
    ///
    /// # Example
    /// ```rust,ignore
    /// // TCP socket binding
    /// let mut config = SocketConfig::new();
    /// config.set("tcp_local_port", &5000u16.to_be_bytes());
    /// config.set("ip_local", &[192, 168, 1, 100]);
    ///
    /// tcp_layer.configure(&config, &[ip_layer.clone()])?;
    /// // TCP registers for port 5000, passes config to IP
    /// // IP registers for 192.168.1.100, may pass config to Ethernet
    /// ```
    fn configure(
        &self,
        config: &SocketConfig,
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError>;
    
    /// Get layer name for debugging/management
    fn name(&self) -> &str;
    
    /// Get layer statistics
    fn stats(&self) -> NetworkLayerStats;
}
```

### LayerContext - Protocol-Agnostic Routing Hints

```rust
/// Context for passing routing hints between layers
///
/// **Design**: Completely protocol-agnostic key-value store.
/// No hardcoded fields like `destination_ip` or `source_port`.
///
/// Each layer adds/reads what it needs using string keys:
/// - TCP: "tcp_src_port", "tcp_dst_port", "tcp_seq", "tcp_ack", "tcp_flags"
/// - IP: "ip_src", "ip_dst", "ip_protocol", "ip_ttl"
/// - Ethernet: "eth_dst_mac", "eth_src_mac", "eth_type"
/// - Custom: Any application-specific metadata
#[derive(Debug, Clone, Default)]
pub struct LayerContext {
    info: BTreeMap<String, Vec<u8>>,
}

impl LayerContext {
    pub fn new() -> Self {
        Self {
            info: BTreeMap::new(),
        }
    }
    
    /// Set arbitrary data in context
    pub fn set(&mut self, key: &str, value: &[u8]) {
        self.info.insert(key.into(), value.to_vec());
    }
    
    /// Get data from context
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.info.get(key).map(|v| v.as_slice())
    }
    
    /// Helper: Get u16 value (ports, etc.)
    pub fn get_u16(&self, key: &str) -> Option<u16> {
        self.get(key).and_then(|bytes| {
            if bytes.len() == 2 {
                Some(u16::from_be_bytes([bytes[0], bytes[1]]))
            } else {
                None
            }
        })
    }
    
    /// Helper: Get IPv4 address
    pub fn get_ipv4(&self, key: &str) -> Option<[u8; 4]> {
        self.get(key).and_then(|bytes| {
            if bytes.len() == 4 {
                Some([bytes[0], bytes[1], bytes[2], bytes[3]])
            } else {
                None
            }
        })
    }
}
```

### SocketConfig - Reception Configuration

```rust
/// Configuration passed to layers when socket is created/bound
///
/// **Purpose**: Tell layers how to route incoming packets to this socket.
///
/// **Flow**: Socket → TCP → IP → Ethernet (each layer extracts what it needs)
#[derive(Debug, Clone, Default)]
pub struct SocketConfig {
    params: BTreeMap<String, Vec<u8>>,
}

impl SocketConfig {
    pub fn new() -> Self {
        Self {
            params: BTreeMap::new(),
        }
    }
    
    /// Set configuration parameter
    pub fn set(&mut self, key: &str, value: &[u8]) {
        self.params.insert(key.into(), value.to_vec());
    }
    
    /// Get configuration parameter
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.params.get(key).map(|v| v.as_slice())
    }
    
    /// Helper: Get u16 value (ports, etc.)
    pub fn get_u16(&self, key: &str) -> Option<u16> {
        self.get(key).and_then(|bytes| {
            if bytes.len() == 2 {
                Some(u16::from_be_bytes([bytes[0], bytes[1]]))
            } else {
                None
            }
        })
    }
    
    /// Helper: Get IPv4 address
    pub fn get_ipv4(&self, key: &str) -> Option<[u8; 4]> {
        self.get(key).and_then(|bytes| {
            if bytes.len() == 4 {
                Some([bytes[0], bytes[1], bytes[2], bytes[3]])
            } else {
                None
            }
        })
    }
}
```

## Complete Example: TCP Socket Send/Receive

### Setup Phase (Registration)

```rust
// 1. Create and register shared protocol layers
let eth = Arc::new(EthernetLayer::new(device));
let ip = Arc::new(IpLayer::new());
let tcp = Arc::new(TcpLayer::new());
let udp = Arc::new(UdpLayer::new());

// 2. Register in NetworkManager (global shared instances)
NetworkManager::get_manager().register_layer("ethernet", eth.clone());
NetworkManager::get_manager().register_layer("ip", ip.clone());
NetworkManager::get_manager().register_layer("tcp", tcp.clone());
NetworkManager::get_manager().register_layer("udp", udp.clone());

// 3. Wire up protocol stack (upper layer registration)
ip.register_protocol(6, tcp.clone());   // TCP
ip.register_protocol(17, udp.clone());  // UDP
```

### Socket Creation and Bind

```rust
// User calls bind(192.168.1.100:5000)
let tcp_layer = NetworkManager::get_manager().get_layer("tcp")?;

// Create socket (holds only TCP reference)
let mut socket = TcpSocket {
    tcp_layer: tcp_layer.clone(),
    local_port: 0,  // Will be set by configure
    remote_port: 0,
    local_ip: [0, 0, 0, 0],  // Will be set by configure
    remote_ip: [0, 0, 0, 0],
    send_buffer: Vec::new(),
    recv_buffer: Vec::new(),
};

// Configure for reception
let mut config = SocketConfig::new();
config.set("tcp_local_port", &5000u16.to_be_bytes());
config.set("ip_local", &[192, 168, 1, 100]);

// Get IP layer reference for configuration
let ip_layer = NetworkManager::get_manager().get_layer("ip")?;
let eth_layer = NetworkManager::get_manager().get_layer("ethernet")?;

// Configure layers (from top to bottom)
socket.local_port = 5000;
socket.local_ip = [192, 168, 1, 100];

// TCP layer registers: "port 5000 packets → this socket"
tcp_layer.configure(&config, &[ip_layer.clone()])?;
// IP layer registers: "192.168.1.100 packets → TCP layer"
// (TCP already registered as protocol 6, so packets route correctly)
```

### Sending Data

```rust
// User calls send("Hello")
impl TcpSocket {
    fn send(&mut self, data: &[u8]) -> Result<usize, SocketError> {
        // Build context with routing hints
        let mut ctx = LayerContext::new();
        ctx.set("tcp_src_port", &self.local_port.to_be_bytes());
        ctx.set("tcp_dst_port", &self.remote_port.to_be_bytes());
        ctx.set("ip_src", &self.local_ip);
        ctx.set("ip_dst", &self.remote_ip);
        
        // Get IP layer for routing
        let ip_layer = NetworkManager::get_manager().get_layer("ip")?;
        
        // Send to TCP layer (TCP doesn't need to know about IP/Ethernet)
        self.tcp_layer.send(data, &ctx, &[ip_layer])?;
        
        Ok(data.len())
    }
}

// TCP Layer implementation
impl NetworkLayer for TcpLayer {
    fn send(
        &self,
        packet: &[u8],
        context: &LayerContext,
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // 1. Build TCP header from context
        let src_port = context.get_u16("tcp_src_port").ok_or(SocketError::NoRoute)?;
        let dst_port = context.get_u16("tcp_dst_port").ok_or(SocketError::NoRoute)?;
        
        let mut tcp_packet = Vec::new();
        tcp_packet.extend_from_slice(&src_port.to_be_bytes());
        tcp_packet.extend_from_slice(&dst_port.to_be_bytes());
        // ... build complete TCP header ...
        tcp_packet.extend_from_slice(packet);  // Add payload
        
        // 2. Add TCP protocol number for IP layer
        let mut new_ctx = context.clone();
        new_ctx.set("ip_protocol", &[6]);  // TCP
        
        // 3. Route to IP layer (already have reference from socket)
        if next_layers.is_empty() {
            return Err(SocketError::NoRoute);
        }
        next_layers[0].send(&tcp_packet, &new_ctx, &[])?;  // IP will get Ethernet from NetworkManager
        
        Ok(())
    }
}

// IP Layer implementation
impl NetworkLayer for IpLayer {
    fn send(
        &self,
        packet: &[u8],
        context: &LayerContext,
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // 1. Extract IP addresses from context
        let src_ip = context.get_ipv4("ip_src").ok_or(SocketError::NoRoute)?;
        let dst_ip = context.get_ipv4("ip_dst").ok_or(SocketError::NoRoute)?;
        let protocol = context.get("ip_protocol").and_then(|b| b.get(0).copied()).unwrap_or(0);
        
        // 2. Build IP header
        let mut ip_packet = Vec::new();
        ip_packet.push(0x45);  // Version 4, IHL 5
        ip_packet.push(0);     // TOS
        // ... complete IP header ...
        ip_packet.extend_from_slice(&src_ip);
        ip_packet.extend_from_slice(&dst_ip);
        ip_packet.extend_from_slice(packet);  // Add TCP packet
        
        // 3. Route to Ethernet layer
        let mut new_ctx = context.clone();
        
        // Get Ethernet layer from NetworkManager
        let eth_layer = NetworkManager::get_manager().get_layer("ethernet")
            .map_err(|_| SocketError::NoRoute)?;
        
        eth_layer.send(&ip_packet, &new_ctx, &[])?;
        
        Ok(())
    }
}
```

### Receiving Data

```rust
// Ethernet device receives frame
device.on_receive(|frame| {
    let eth_layer = NetworkManager::get_manager().get_layer("ethernet")?;
    eth_layer.receive(frame)?;
    Ok(())
});

// Ethernet Layer
impl NetworkLayer for EthernetLayer {
    fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
        // Parse Ethernet header
        let ether_type = u16::from_be_bytes([packet[12], packet[13]]);
        
        // Route based on EtherType
        match ether_type {
            0x0800 => {  // IPv4
                let handlers = self.protocol_handlers.read();
                if let Some(handler) = handlers.get(&0x0800) {
                    handler.receive(&packet[14..])?;  // Skip Ethernet header
                }
            }
            _ => return Err(SocketError::ProtocolNotSupported),
        }
        
        Ok(())
    }
}

// IP Layer
impl NetworkLayer for IpLayer {
    fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
        // Parse IP header
        let protocol = packet[9];
        let dst_ip = [packet[16], packet[17], packet[18], packet[19]];
        
        // Check if destined for us
        if !self.is_local_address(&dst_ip) {
            return Ok(());  // Not for us, drop
        }
        
        // Route based on protocol
        let handlers = self.protocol_handlers.read();
        if let Some(handler) = handlers.get(&(protocol as u16)) {
            handler.receive(&packet[20..])?;  // Skip IP header (20 bytes)
        } else {
            return Err(SocketError::ProtocolNotSupported);
        }
        
        Ok(())
    }
}

// TCP Layer
impl NetworkLayer for TcpLayer {
    fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
        // Parse TCP header
        let dst_port = u16::from_be_bytes([packet[2], packet[3]]);
        
        // Find socket listening on this port
        let sockets = self.port_sockets.read();
        if let Some(socket) = sockets.get(&dst_port) {
            // Deliver data to socket (skip 20-byte TCP header)
            socket.deliver_data(&packet[20..])?;
        } else {
            // No socket listening, drop or send RST
        }
        
        Ok(())
    }
}
```

## Benefits of This Design

### 1. Simplicity
- Socket only knows about its transport layer (TCP/UDP)
- No complex pipeline management in socket
- Clear separation of concerns

### 2. Flexibility
- Easy to swap protocol implementations (replace TCP, use IP over InfiniBand)
- Layers don't need to know about each other's internals
- Protocol-agnostic context allows any future protocol

### 3. Balanced Send/Receive
- **Send**: Context flows down with hints, each layer routes
- **Receive**: Packets flow up, each layer routes based on protocol numbers
- Configuration flows down once at bind/connect time

### 4. Practical
- Works with real networking constraints (need destination for routing)
- Doesn't require omniscient sockets or brittle pipelines
- Natural error handling (NoRoute when hints insufficient)

### 5. Testable
- Easy to mock individual layers
- Can test routing logic independently
- Clear interfaces for each layer

## Implementation Notes

### NetworkManager Responsibilities

```rust
pub struct NetworkManager {
    /// Shared protocol layer instances (like mounted filesystems)
    layers: RwLock<BTreeMap<String, Arc<dyn NetworkLayer>>>,
    /// Socket factories for creating sockets
    socket_factories: RwLock<BTreeMap<SocketDomain, SocketFactory>>,
}

impl NetworkManager {
    /// Register shared protocol layer instance
    pub fn register_layer(&self, name: &str, layer: Arc<dyn NetworkLayer>) {
        self.layers.write().insert(name.into(), layer);
    }
    
    /// Get shared protocol layer by name
    pub fn get_layer(&self, name: &str) -> Result<Arc<dyn NetworkLayer>, SocketError> {
        self.layers.read()
            .get(name)
            .cloned()
            .ok_or(SocketError::ProtocolNotSupported)
    }
    
    /// List all registered layers
    pub fn list_layers(&self) -> Vec<String> {
        self.layers.read().keys().cloned().collect()
    }
}
```

### Future: Per-Task Network Isolation

```rust
// Like per-task VfsManager for containerization
task.network_manager = Some(Arc::new(NetworkManager::new()));

// Copy/share specific layers
// Share Ethernet (driver access needed), but isolate IP and above
task.network_manager.register_layer(
    "ethernet",
    global_network_manager.get_layer("ethernet")?  // Shared
);

// Create isolated IP layer for this task
let isolated_ip = Arc::new(IpLayer::new());
task.network_manager.register_layer("ip", isolated_ip);

// Task has its own IP space, but shares physical network
```

## Summary

This design achieves the goals discussed:

✅ Socket holds only top-level layer reference (TCP/UDP)
✅ Layers route based on hints in context (NoRoute if insufficient)
✅ Configuration flows down at bind/connect time for reception
✅ Protocol-agnostic LayerContext (no hardcoded IP addresses)
✅ Balanced send (hints down) and receive (route up) flows
✅ Practical, testable, and flexible
✅ Supports both monolithic (ProtocolStack) and layered (NetworkLayer) approaches

The architecture is ready for implementation and addresses all concerns raised in the discussion.

## Implementation Status

### Phase 1: Core Infrastructure ✅ COMPLETED
- ✅ Protocol-agnostic NetworkLayer trait with composable layer design
- ✅ NetworkManager with global registry (VFS-like pattern)
- ✅ LayerContext for protocol-agnostic routing hints
- ✅ SocketConfig for reception configuration flow
- ✅ Comprehensive test suite with realistic TCP/IP/Ethernet mock layers
- ✅ Circular reference prevention through one-way registration pattern
- ✅ Documentation with safety guidelines and usage examples

**Location**: `kernel/src/network/protocol_stack.rs`

**Tests**: 424 tests passing including:
- NetworkManager creation and layer registration
- Protocol registration and routing
- Realistic TCP/IP/Ethernet stack send/receive simulation
- Two-socket communication scenarios
- Error handling and edge cases

### Phase 2: NetworkManager and Socket Infrastructure ✅ COMPLETED
- ✅ SocketObject and SocketControl traits
- ✅ Socket syscalls (socket, bind, connect, listen, accept, sendto, recvfrom)
- ✅ SocketDomain, SocketType, SocketAddress types
- ✅ NetworkManager with factory pattern and named socket registry
- ✅ LocalSocket implementation (VecDeque-based IPC sockets)
- ✅ Socket factory registration and creation
- ✅ Named socket registration, lookup, and lifecycle management
- ✅ KernelObject::Socket variant

**Location**: `kernel/src/network/{mod.rs, socket.rs, local.rs, syscall.rs}`

**Tests**: 441 tests passing including:
- Socket factory registration and creation
- Named socket registration, lookup, and lifecycle
- Protocol layer registration and management
- Weak reference lifecycle validation
- Duplicate registration handling
- Multiple socket creation
- Bidirectional LocalSocket communication
- Socket state transitions

### Phase 3: ABI Module Implementation (PLANNED)
- [ ] Linux ABI: Local socket (Unix domain socket equivalent)
- [ ] Linux ABI: System call translation (socket, bind, connect, etc.)
- [ ] Linux ABI: Address structure conversion
- [ ] xv6 ABI: Socket support (if needed)
- [ ] Unit tests for socket implementations

### Phase 4: Protocol Stack Implementation (FUTURE)
- [ ] TCP/IP protocol stack
- [ ] UDP socket support
- [ ] IPv4/IPv6 address handling
- [ ] Packet routing and processing
- [ ] Network device integration

### Phase 5: Advanced Features (FUTURE)
- [ ] VFS integration for named sockets
- [ ] Credential passing (SCM_CREDENTIALS)
- [ ] File descriptor passing (SCM_RIGHTS)
- [ ] Ancillary data support
- [ ] Raw socket support
- [ ] Per-task network namespace isolation

## References

- [Pull Request #270](https://github.com/petitstrawberry/Scarlet/pull/270): Implement OS-agnostic network socket infrastructure
- POSIX Socket API Specification
- Linux socket(7) and unix(7) man pages
- Stevens, W. Richard. "Unix Network Programming"
- Scarlet VFS v2 design (`kernel/src/fs/vfs_v2/`)
- Scarlet IPC design (`kernel/src/ipc/`)
- Scarlet Device Manager (`kernel/src/device/manager.rs`)

## Revision History

- 2026-01-03: Initial design document
- 2026-01-03: Updated with Phase 1 completion status and test results
- 2026-01-03: Phase 2 completed - NetworkManager with LocalSocket implementation and comprehensive socket infrastructure (441 tests passing)
