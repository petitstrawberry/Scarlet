# Socket VFS Integration

## Overview

This document describes the integration of Socket objects into Scarlet's Virtual File System (VFS), enabling socket files to be created and accessed through filesystem paths, similar to Unix domain sockets.

## Motivation

Unix-like systems support binding sockets to filesystem paths, allowing processes to communicate through named socket files. This integration brings similar functionality to Scarlet, enabling:

- Inter-process communication through filesystem-based socket paths
- Consistent interface for both network and local sockets
- Seamless integration with VFS operations (open, read, write, close)
- Support for socket files across different filesystem types (TmpFS, ext2, etc.)

## Architecture

### Core Components

#### 1. SocketFileInfo Structure

Similar to `DeviceFileInfo` used for device files, `SocketFileInfo` identifies socket files in the filesystem:

```rust
pub struct SocketFileInfo {
    pub socket_id: usize,
}
```

The `socket_id` uniquely identifies a socket object registered in the NetworkManager.

#### 2. Updated FileType Enum

The `FileType::Socket` variant now includes socket information:

```rust
pub enum FileType {
    RegularFile,
    Directory,
    CharDevice(DeviceFileInfo),
    BlockDevice(DeviceFileInfo),
    Pipe,
    SymbolicLink(String),
    Socket(SocketFileInfo),  // ← Updated to include SocketFileInfo
    Unknown,
}
```

#### 3. NetworkManager Integration

NetworkManager now provides socket registry functionality:

```rust
impl NetworkManager {
    /// Register a socket with a specific ID for VFS integration
    pub fn register_socket_with_id(
        &self,
        socket_id: SocketId,
        socket: Arc<dyn SocketObject>,
    ) -> Result<(), SocketError>

    /// Get a socket by its ID
    pub fn get_socket(&self, socket_id: SocketId) -> Option<Arc<dyn SocketObject>>
}
```

### Data Flow

```
┌─────────────────────────────────────────────────────────────┐
│                    Application Layer                        │
│  vfs.create_file("/socket.sock", Socket(info))            │
│  vfs.open("/socket.sock", flags)                           │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    VFS Layer                                │
│  - Path resolution                                          │
│  - VfsEntry caching                                         │
│  - Mount point traversal                                    │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│              Filesystem Driver (TmpFS/ext2)                 │
│  create(): Creates TmpNode with Socket FileType            │
│  open():   Returns TmpFileObject with socket_ref           │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                  NetworkManager                             │
│  socket_id → Arc<dyn SocketObject> mapping                 │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│              Socket Implementation                          │
│  LocalSocket, TcpSocket, UdpSocket, etc.                   │
│  - Implements SocketObject trait                            │
│  - Handles actual I/O operations                            │
└─────────────────────────────────────────────────────────────┘
```

## Implementation Details

### TmpFS Implementation

TmpFS has been updated to support socket files:

#### Socket File Creation

```rust
match file_type {
    FileType::Socket(_) => {
        Arc::new(TmpNode::new_device(name.clone().to_string(), file_type, file_id))
    }
    // ... other file types
}
```

Socket files are created using the same infrastructure as device files, storing the FileType which includes the socket_id.

#### Socket File Opening

```rust
impl TmpFileObject {
    pub fn new_socket(node: Arc<TmpNode>, info: SocketFileInfo) -> Self {
        // Get socket from NetworkManager
        match NetworkManager::get_manager().get_socket(info.socket_id) {
            Some(socket) => Self {
                node,
                position: RwLock::new(0),
                device_guard: None,
                socket_ref: Some(socket),
            },
            None => panic!("Failed to get socket {}", info.socket_id),
        }
    }
}
```

When a socket file is opened, TmpFileObject retrieves the socket object from NetworkManager and stores a reference.

#### I/O Operations

Read and write operations are delegated to the underlying socket:

```rust
fn read_socket(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
    if let Some(ref socket) = self.socket_ref {
        socket.read(buffer)
    } else {
        Err(StreamError::NotSupported)
    }
}

fn write_socket(&self, buffer: &[u8]) -> Result<usize, StreamError> {
    if let Some(ref socket) = self.socket_ref {
        socket.write(buffer)
    } else {
        Err(StreamError::NotSupported)
    }
}
```

### ext2 Implementation

The ext2 filesystem driver has been updated to recognize and handle socket file types:

```rust
// Reading from disk
EXT2_S_IFSOCK => Ok(FileType::Socket(SocketFileInfo { socket_id: 0 }))

// Writing to disk  
FileType::Socket(_) => EXT2_S_IFSOCK | 0o666

// Creating socket files
FileType::Socket(_) => {
    Arc::new(Ext2Node::new(new_inode_number, file_type.clone(), file_id))
}
```

Note: When reading socket files from disk, the socket_id is initially set to 0. The actual binding of socket IDs to filesystem paths is managed at runtime by the application or ABI layer.

## Usage Patterns

### Creating a Socket File

```rust
use crate::fs::{SocketFileInfo, FileType};
use crate::network::{NetworkManager, LocalSocket, SocketType, SocketProtocol};

// 1. Create a socket object
let socket = Arc::new(LocalSocket::new(
    SocketType::Stream,
    SocketProtocol::Default,
));

// 2. Register socket with NetworkManager
let socket_id = 1001;
NetworkManager::get_manager()
    .register_socket_with_id(socket_id, socket.clone())
    .unwrap();

// 3. Create socket file in VFS
let socket_file_type = FileType::Socket(SocketFileInfo { socket_id });
vfs.create_file("/tmp/my_socket.sock", socket_file_type).unwrap();
```

### Opening and Using a Socket File

```rust
// Open the socket file
let socket_file = vfs.open("/tmp/my_socket.sock", 0x02).unwrap();

// Use it like a regular file
if let KernelObject::File(file_obj) = socket_file {
    // Write data to socket
    file_obj.write(b"Hello, socket!").unwrap();
    
    // Read data from socket
    let mut buffer = [0u8; 1024];
    let bytes_read = file_obj.read(&mut buffer).unwrap();
}
```

### Unix Domain Socket Pattern

```rust
// Server side
let server_socket = create_server_socket();
let server_socket_id = register_socket(server_socket);

// Bind to filesystem path
vfs.create_file(
    "/tmp/server.sock",
    FileType::Socket(SocketFileInfo { socket_id: server_socket_id })
).unwrap();

server_socket.listen(5).unwrap();

// Client side
let socket_file = vfs.open("/tmp/server.sock", 0x02).unwrap();
// socket_file now contains the server socket reference
// Client can connect through this file handle
```

## Design Decisions

### Socket ID Management

Socket IDs are managed by the NetworkManager and must be unique across the system. The current implementation uses a simple counter starting from 1, but this can be extended to support:

- Process-scoped socket IDs
- Namespace-specific socket registries
- Persistent socket IDs for socket files on disk

### FileObject Wrapper

Socket files use the same FileObject interface as regular files, with operations delegated to the underlying SocketObject. This provides:

- **Consistency**: Socket files work with all VFS operations (read, write, seek, etc.)
- **Flexibility**: Socket-specific operations (bind, listen, accept) are available through ioctl-like control operations
- **Simplicity**: No new abstractions needed at the VFS layer

### Filesystem Independence

The socket file integration works with any filesystem that implements:
- `create()` operation with Socket FileType support
- `open()` operation that can create FileObjects for sockets

Currently implemented for:
- TmpFS (in-memory filesystem)
- ext2 (on-disk filesystem)

## Testing

A comprehensive test has been added in `kernel/src/fs/vfs_v2/drivers/tmpfs/tests.rs`:

```rust
#[test_case]
fn test_socket_file_creation() {
    // Tests:
    // 1. Socket registration with NetworkManager
    // 2. Socket file creation in VFS
    // 3. Socket file type verification
    // 4. Socket file opening
}
```

## Future Enhancements

### Socket File Permissions

Currently, socket files inherit basic file permissions. Future enhancements could add:
- Socket-specific permission checks
- Connection acceptance based on file permissions
- Owner/group-based access control

### Automatic Cleanup

Socket files should be automatically removed when:
- The socket is closed
- The process exits
- The socket object is deallocated

This requires integration with the resource cleanup system.

### Socket File Metadata

Socket files could expose additional metadata through extended attributes:
- Socket type (Stream, Datagram, etc.)
- Socket domain (Local, Inet, Inet6)
- Connection state
- Number of pending connections (for listening sockets)

### Cross-Filesystem Socket Support

Enable sockets to work across different filesystem types:
- Bind a socket on one filesystem (e.g., ext2)
- Access the same socket from another filesystem (e.g., overlayfs)
- Handle socket file persistence across remounts

## Comparison with Other Systems

### Linux

Linux supports Unix domain sockets through special inode types in the filesystem. Key differences:

- **Linux**: Socket files are identified by inode type and don't persist across reboots (socket endpoints are lost)
- **Scarlet**: Socket files store socket_id, allowing flexible persistence models

### Plan 9

Plan 9 treats everything as a file, including network connections. Scarlet's approach is similar but:

- **Plan 9**: Network connections appear as files in /net directory
- **Scarlet**: Sockets can be created anywhere in the VFS hierarchy

## Security Considerations

### Socket ID Collision

Care must be taken to prevent socket ID collisions:
- Socket IDs should be unique per system or namespace
- Reusing socket IDs after socket closure should be handled carefully
- Consider cryptographically strong random socket IDs for security-sensitive applications

### Access Control

Socket files inherit filesystem permissions, but additional checks should be performed:
- Verify caller has permission to access the socket
- Enforce socket-specific access controls (e.g., connection limits)
- Audit socket access for security monitoring

## Conclusion

The Socket VFS integration provides a clean and consistent interface for socket files in Scarlet's filesystem. By following the established patterns for device files and leveraging the existing VFS infrastructure, this implementation enables powerful inter-process communication capabilities while maintaining simplicity and flexibility.

The integration supports multiple filesystem types, allows for future enhancements, and provides a foundation for advanced networking features in Scarlet.
