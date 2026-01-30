# Scarlet Native Socket API - User Library

This document describes the user-space socket API provided by `scarlet_std`.

## Overview

The Scarlet Native socket API provides handle-based IPC sockets for inter-process communication. Unlike POSIX sockets, these are:

- **Handle-based**: Use handle IDs instead of file descriptors
- **Path-based**: Identify sockets by filesystem-like paths
- **Scarlet-native**: Optimized for local IPC, not network sockets

## API Reference

### Socket Creation

```rust
use scarlet_std::socket::Socket;

// Create a new socket
let socket = Socket::new().unwrap();
```

### Server Example

```rust
use scarlet_std::socket::Socket;

// Create and bind a server socket
let server = Socket::new().unwrap();
server.bind("/tmp/server.sock").unwrap();
server.listen(5).unwrap();

// Accept incoming connections
loop {
    let client = server.accept().unwrap();
    // Handle client connection...
}
```

### Client Example

```rust
use scarlet_std::socket::Socket;

// Create and connect a client socket
let client = Socket::new().unwrap();
client.connect("/tmp/server.sock").unwrap();

// Use stream operations (via handle)...
```

### Socket Pair for IPC

For simple bidirectional communication, use socket pairs:

```rust
use scarlet_std::socket::Socket;

// Create a connected socket pair
let (sock1, sock2) = Socket::pair().unwrap();

// sock1 and sock2 are now connected bidirectionally
// Use stream operations to communicate
```

## Socket Operations

### `Socket::new() -> Result<Socket>`

Creates a new unconnected socket.

### `socket.bind(path: &str) -> Result<()>`

Binds the socket to a path in the socket namespace.

**Arguments:**
- `path`: Socket path (e.g., "/tmp/server.sock")

**Errors:**
- `AlreadyBound`: Socket is already bound
- `InvalidPath`: Path is invalid or too long

### `socket.listen(backlog: usize) -> Result<()>`

Marks the socket as passive (listening for connections).

**Arguments:**
- `backlog`: Maximum number of pending connections

**Errors:**
- `NotListening`: Socket is not bound

### `socket.connect(path: &str) -> Result<()>`

Connects the socket to a listening socket at the specified path.

**Arguments:**
- `path`: Path to the listening socket

**Errors:**
- `ConnectionRefused`: Target socket not found or not listening

### `socket.accept() -> Result<Socket>`

Accepts an incoming connection from the backlog queue.

**Returns:** A new `Socket` representing the accepted connection

**Errors:**
- `WouldBlock`: No pending connections
- `NotListening`: Socket is not in listening state

### `Socket::pair() -> Result<(Socket, Socket)>`

Creates a connected socket pair for IPC.

**Returns:** A tuple of two connected sockets

### `socket.shutdown(how: ShutdownHow) -> Result<()>`

Shuts down part or all of a socket connection.

**Arguments:**
- `how`: Shutdown direction
  - `ShutdownHow::Read`: Shutdown read operations
  - `ShutdownHow::Write`: Shutdown write operations
  - `ShutdownHow::Both`: Shutdown both directions

## Integration with Stream Operations

Sockets implement the `StreamOps` capability and can be used with `stream_read()` and `stream_write()` system calls:

```rust
use scarlet_std::socket::Socket;
use scarlet_std::handle::capability::stream::{stream_read, stream_write};

let socket = Socket::new().unwrap();
socket.connect("/tmp/server.sock").unwrap();

// Write data
let data = b"Hello, Server!";
stream_write(socket.as_raw_handle(), data).unwrap();

// Read response
let mut buffer = [0u8; 1024];
let n = stream_read(socket.as_raw_handle(), &mut buffer).unwrap();
```

## Error Handling

All socket operations return `Result<T, SocketError>`:

```rust
pub enum SocketError {
    SyscallFailed,      // System call failed
    InvalidHandle,      // Invalid handle
    InvalidPath,        // Invalid path
    AlreadyBound,       // Already bound or connected
    NotListening,       // Not listening
    ConnectionRefused,  // Connection refused
    WouldBlock,         // Would block (no pending connections)
}
```

## System Call Numbers

The following system calls are used internally:

- `SocketCreate (900)`: Create a socket
- `SocketBind (901)`: Bind to path
- `SocketListen (902)`: Start listening
- `SocketConnect (903)`: Connect to socket
- `SocketAccept (904)`: Accept connection
- `Socketpair (905)`: Create socket pair
- `SocketShutdown (906)`: Shutdown socket
- `HandleClose (102)`: Close socket (on drop)

## Complete Example

```rust
use scarlet_std::socket::{Socket, ShutdownHow};
use scarlet_std::handle::capability::stream::{stream_read, stream_write};

fn server_example() {
    // Create server socket
    let server = Socket::new().unwrap();
    server.bind("/tmp/echo.sock").unwrap();
    server.listen(5).unwrap();

    println!("Server listening on /tmp/echo.sock");

    // Accept and handle one client
    let client = server.accept().unwrap();
    println!("Client connected");

    // Echo loop
    let mut buffer = [0u8; 1024];
    loop {
        match stream_read(client.as_raw_handle(), &mut buffer) {
            Ok(n) if n > 0 => {
                stream_write(client.as_raw_handle(), &buffer[..n]).unwrap();
            }
            _ => break,
        }
    }

    client.shutdown(ShutdownHow::Both).unwrap();
}

fn client_example() {
    // Create client socket
    let client = Socket::new().unwrap();
    client.connect("/tmp/echo.sock").unwrap();

    // Send message
    let message = b"Hello, Echo Server!";
    stream_write(client.as_raw_handle(), message).unwrap();

    // Receive echo
    let mut buffer = [0u8; 1024];
    let n = stream_read(client.as_raw_handle(), &mut buffer).unwrap();
    println!("Received: {}", core::str::from_utf8(&buffer[..n]).unwrap());

    client.shutdown(ShutdownHow::Both).unwrap();
}

fn socketpair_example() {
    // Create socket pair for IPC
    let (parent, child) = Socket::pair().unwrap();

    // Parent sends to child
    let message = b"Hello from parent";
    stream_write(parent.as_raw_handle(), message).unwrap();

    // Child reads from parent
    let mut buffer = [0u8; 1024];
    let n = stream_read(child.as_raw_handle(), &mut buffer).unwrap();
    println!("Child received: {}", core::str::from_utf8(&buffer[..n]).unwrap());
}
```

## See Also

- [Kernel Implementation](../../../kernel/src/network/syscall.rs): System call implementations
- [Socket Architecture](../../../docs/network_architecture_final.md): Overall design
- [Handle System](handle/): Handle-based object system
