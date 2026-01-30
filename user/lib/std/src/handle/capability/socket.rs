//! SocketObject capability for Scarlet Native API
//!
//! This module provides type-safe socket operations for KernelObjects that
//! implement the Scarlet Native local socket interface.

use crate::handle::{Handle, RawHandle};
use crate::syscall::{Syscall, syscall1, syscall2, syscall3, syscall4, syscall5};

/// Result type for socket operations
pub type SocketObjectResult<T> = Result<T, SocketObjectError>;

/// Errors that can occur during socket operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketObjectError {
    /// Other system error
    SystemError(i32),
}

impl SocketObjectError {
    fn from_syscall_result(result: usize) -> Result<usize, Self> {
        if result == usize::MAX {
            Err(SocketObjectError::SystemError(-1))
        } else {
            Ok(result)
        }
    }
}

/// Socket shutdown direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownHow {
    /// Shutdown read operations
    Read = 0,
    /// Shutdown write operations
    Write = 1,
    /// Shutdown both read and write operations
    Both = 2,
}

/// Socket object capability for socket-specific operations
pub struct SocketObject<'a> {
    handle: &'a Handle,
}

impl<'a> SocketObject<'a> {
    /// Create a SocketObject capability from a Handle reference.
    ///
    /// This is crate-internal to prevent bypassing `Handle::as_socket` validation.
    pub(crate) fn from_handle(handle: &'a Handle) -> Self {
        Self { handle }
    }

    /// Get the raw handle value
    pub fn as_raw(&self) -> RawHandle {
        self.handle.as_raw()
    }

    /// Bind socket to a path
    pub fn bind(&self, path: &str) -> SocketObjectResult<()> {
        let result = syscall3(
            Syscall::SocketBind,
            self.handle.as_raw() as usize,
            path.as_ptr() as usize,
            path.len(),
        );
        SocketObjectError::from_syscall_result(result).map(|_| ())
    }

    /// Start listening for connections
    pub fn listen(&self, backlog: usize) -> SocketObjectResult<()> {
        let result = syscall2(
            Syscall::SocketListen,
            self.handle.as_raw() as usize,
            backlog,
        );
        SocketObjectError::from_syscall_result(result).map(|_| ())
    }

    /// Connect to a named socket
    pub fn connect(&self, path: &str) -> SocketObjectResult<()> {
        let result = syscall3(
            Syscall::SocketConnect,
            self.handle.as_raw() as usize,
            path.as_ptr() as usize,
            path.len(),
        );
        SocketObjectError::from_syscall_result(result).map(|_| ())
    }

    /// Accept an incoming connection
    pub fn accept(&self) -> SocketObjectResult<RawHandle> {
        let result = syscall1(Syscall::SocketAccept, self.handle.as_raw() as usize);
        SocketObjectError::from_syscall_result(result).map(|h| h as RawHandle)
    }

    /// Shutdown socket
    pub fn shutdown(&self, how: ShutdownHow) -> SocketObjectResult<()> {
        let result = syscall2(
            Syscall::SocketShutdown,
            self.handle.as_raw() as usize,
            how as usize,
        );
        SocketObjectError::from_syscall_result(result).map(|_| ())
    }

    /// Send a kernel object handle through a socket
    pub fn send_handle(&self, object_handle: RawHandle) -> SocketObjectResult<()> {
        let result = syscall2(
            Syscall::SocketSendHandle,
            self.handle.as_raw() as usize,
            object_handle as usize,
        );
        SocketObjectError::from_syscall_result(result).map(|_| ())
    }

    /// Receive a kernel object handle from a socket
    pub fn recv_handle(&self) -> SocketObjectResult<RawHandle> {
        let result = syscall1(Syscall::SocketRecvHandle, self.handle.as_raw() as usize);
        SocketObjectError::from_syscall_result(result).map(|h| h as RawHandle)
    }

    /// Send a kernel object handle and data atomically through a socket
    ///
    /// This method ensures that both the handle and data are available before
    /// waking the peer, preventing race conditions in Wayland protocol.
    ///
    /// # Arguments
    ///
    /// * `object_handle` - The raw handle of the kernel object to send
    /// * `data` - The data to send with the handle
    pub fn send_handle_and_data(&self, object_handle: RawHandle, data: &[u8]) -> SocketObjectResult<()> {
        let result = syscall4(
            Syscall::SocketSendHandleAndData,
            self.handle.as_raw() as usize,
            object_handle as usize,
            data.as_ptr() as usize,
            data.len(),
        );
        SocketObjectError::from_syscall_result(result).map(|_| ())
    }

    /// Receive a kernel object handle and data atomically through a socket
    ///
    /// Returns both a handle and data in a single atomic operation.
    ///
    /// # Arguments
    ///
    /// * `handle_out` - Pointer to store the received handle
    /// * `data_out` - Buffer to store the received data
    ///
    /// # Returns
    ///
    /// * `usize` - Number of bytes received on success
    /// * `SocketObjectError` - Error on failure
    pub fn recv_handle_and_data(
        &self,
        handle_out: &mut RawHandle,
        data_out: &mut [u8],
    ) -> SocketObjectResult<usize> {
        let result = syscall5(
            Syscall::SocketRecvHandleAndData,
            self.handle.as_raw() as usize,
            handle_out as *mut RawHandle as usize,
            data_out.as_mut_ptr() as usize,
            data_out.len(),
            0, // reserved
        );
        SocketObjectError::from_syscall_result(result)
    }
}
