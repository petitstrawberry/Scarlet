//! SocketObject capability for Scarlet Native API
//!
//! This module provides type-safe socket operations for KernelObjects that
//! implement the Scarlet Native local socket interface.

use crate::handle::{Handle, RawHandle};
use crate::syscall::{Syscall, syscall1, syscall2, syscall3};

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
    /// This capability does not own the handle; dropping it will not close anything.
    pub fn from_handle(handle: &'a Handle) -> Self {
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
}
