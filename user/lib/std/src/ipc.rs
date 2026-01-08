//! IPC (Inter-Process Communication) Module
//!
//! This module provides user-space interfaces for IPC mechanisms including
//! pipes, shared memory, and event channels.

use crate::syscall::{Syscall, syscall1, syscall2};

/// Handle to a shared memory object
pub type SharedMemoryHandle = u32;

/// Permissions for shared memory
pub mod permissions {
    /// Read permission
    pub const READ: usize = 0x1;
    /// Write permission
    pub const WRITE: usize = 0x2;
    /// Execute permission
    pub const EXECUTE: usize = 0x4;
    /// Read and write permissions
    pub const READ_WRITE: usize = READ | WRITE;
}

/// Create a shared memory region
///
/// Creates a new shared memory object that can be mapped into the address space
/// of multiple processes for efficient zero-copy data sharing.
///
/// # Arguments
///
/// * `size` - Size of the shared memory region in bytes
/// * `permissions` - Access permissions using constants from `permissions` module
///
/// # Returns
///
/// Returns a handle to the shared memory object on success, or `None` on failure.
///
/// # Examples
///
/// ```no_run
/// use scarlet_std::ipc::{shared_memory_create, permissions};
///
/// // Create a 4KB shared memory region with read-write permissions
/// let handle = shared_memory_create(4096, permissions::READ_WRITE).unwrap();
/// ```
pub fn shared_memory_create(size: usize, permissions: usize) -> Option<SharedMemoryHandle> {
    let result = syscall2(Syscall::SharedMemoryCreate, size, permissions);
    if result == usize::MAX {
        None
    } else {
        Some(result as u32)
    }
}

/// Send a kernel object handle through a socket
///
/// Transfers a kernel object (such as a SharedMemoryObject) to another task
/// through a connected socket. This provides Unix-like SCM_RIGHTS functionality
/// for passing file descriptors / handles between processes.
///
/// Uses dup() semantics: the handle is duplicated, not moved. The original handle
/// remains valid in the sender's task after the send operation completes.
///
/// # Arguments
///
/// * `socket_handle` - Handle to the connected socket
/// * `object_handle` - Handle to the kernel object to send (remains valid after send)
///
/// # Returns
///
/// Returns `true` on success, `false` on failure.
///
/// # Examples
///
/// ```no_run
/// use scarlet_std::ipc::{socket_send_handle, shared_memory_create, permissions};
///
/// // Create a shared memory object
/// let shmem = shared_memory_create(4096, permissions::READ_WRITE).unwrap();
///
/// // Send it through a connected socket (shmem remains valid after this)
/// let socket = /* ... get connected socket handle ... */;
/// if socket_send_handle(socket, shmem) {
///     println!("Successfully sent shared memory handle!");
///     // shmem can still be used here
/// }
/// ```
pub fn socket_send_handle(socket_handle: u32, object_handle: u32) -> bool {
    let result = syscall2(
        Syscall::SocketSendHandle,
        socket_handle as usize,
        object_handle as usize,
    );
    result == 0
}

/// Receive a kernel object handle from a socket
///
/// Receives a kernel object that was sent by a peer task through a connected socket.
/// This provides Unix-like SCM_RIGHTS functionality for receiving file descriptors
/// / handles from other processes.
///
/// # Arguments
///
/// * `socket_handle` - Handle to the connected socket
///
/// # Returns
///
/// Returns a handle to the received kernel object on success, or `None` if no
/// handle is available or on error.
///
/// # Examples
///
/// ```no_run
/// use scarlet_std::ipc::socket_recv_handle;
///
/// let socket = /* ... get connected socket handle ... */;
/// if let Some(received_handle) = socket_recv_handle(socket) {
///     println!("Received handle: {}", received_handle);
///     // Can now use the received handle (e.g., map shared memory)
/// }
/// ```
pub fn socket_recv_handle(socket_handle: u32) -> Option<u32> {
    let result = syscall1(Syscall::SocketRecvHandle, socket_handle as usize);
    if result == usize::MAX {
        None
    } else {
        Some(result as u32)
    }
}
