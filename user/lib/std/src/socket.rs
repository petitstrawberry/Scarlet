//! Socket operations for Scarlet Native
//!
//! This module provides high-level wrappers for Scarlet Native socket system calls.
//! Unlike POSIX sockets, these are handle-based and designed for local IPC.
//!
//! # Examples
//!
//! ```no_run
//! use std::socket::{Socket, ShutdownHow};
//!
//! // Server side
//! let server = Socket::new().unwrap();
//! server.bind("/tmp/server.sock").unwrap();
//! server.listen(5).unwrap();
//! let client = server.accept().unwrap();
//!
//! // Client side
//! let client = Socket::new().unwrap();
//! client.connect("/tmp/server.sock").unwrap();
//!
//! // Socket pair for IPC
//! let (sock1, sock2) = Socket::pair().unwrap();
//! ```

use crate::handle::Handle;
use crate::handle::RawHandle;
use crate::syscall::{Syscall, syscall0, syscall1};

pub use crate::handle::capability::ShutdownHow;

/// Socket handle wrapper
///
/// Represents a Scarlet Native local socket for inter-process communication.
/// Sockets are automatically closed when dropped.
#[derive(Debug)]
pub struct Socket {
    handle: Handle,
}

/// Socket error type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketError {
    /// System call failed
    SyscallFailed,
    /// Invalid handle
    InvalidHandle,
    /// Invalid path
    InvalidPath,
    /// Already bound or connected
    AlreadyBound,
    /// Not listening
    NotListening,
    /// Connection refused
    ConnectionRefused,
    /// Would block (no pending connections)
    WouldBlock,
}

pub type Result<T> = core::result::Result<T, SocketError>;

impl Socket {
    /// Create a new socket
    ///
    /// # Returns
    ///
    /// A new unconnected socket, or an error if the system call fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let socket = Socket::new().unwrap();
    /// ```
    pub fn new() -> Result<Self> {
        let handle = syscall0(Syscall::SocketCreate);
        if handle == usize::MAX {
            return Err(SocketError::SyscallFailed);
        }

        let handle =
            unsafe { Handle::from_raw(handle as i32) }.map_err(|_| SocketError::SyscallFailed)?;
        Ok(Socket { handle })
    }

    /// Create a `Socket` from an existing [`Handle`].
    ///
    /// This performs a type check using the handle's cached kernel object info.
    /// If the handle does not represent a socket, this returns [`SocketError::InvalidHandle`]
    /// and does **not** consume the handle.
    pub fn from_handle(handle: Handle) -> Result<Self> {
        handle.as_socket().map_err(|_| SocketError::InvalidHandle)?;
        Ok(Self { handle })
    }

    /// Create a socket from a raw handle
    ///
    /// # Safety
    /// The caller must ensure the handle is valid and represents a socket.
    pub unsafe fn from_raw(raw: RawHandle) -> Self {
        let handle = unsafe { Handle::from_raw(raw) }.expect("invalid raw handle");
        Self { handle }
    }

    /// Get the underlying handle (for advanced usage)
    pub fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Get the raw handle value
    pub fn as_raw(&self) -> RawHandle {
        self.handle.as_raw()
    }

    /// Convert the Socket into a Handle
    pub fn into_handle(self) -> Handle {
        self.handle
    }

    /// Send a kernel object handle through this connected socket.
    pub fn send_handle(&self, object: &Handle) -> Result<()> {
        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        sock.send_handle(object.as_raw())
            .map_err(|_| SocketError::SyscallFailed)
    }

    /// Receive a kernel object handle from this connected socket.
    pub fn recv_handle(&self) -> Result<Handle> {
        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        let raw = sock.recv_handle().map_err(|_| SocketError::WouldBlock)?;
        unsafe { Handle::from_raw(raw) }.map_err(|_| SocketError::SyscallFailed)
    }

    /// Bind socket to a path
    ///
    /// # Arguments
    ///
    /// * `path` - Socket path (e.g., "/tmp/server.sock")
    ///
    /// # Returns
    ///
    /// Ok on success, or an error if the socket is already bound or the path is invalid.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let socket = Socket::new().unwrap();
    /// socket.bind("/tmp/server.sock").unwrap();
    /// ```
    pub fn bind(&self, path: &str) -> Result<()> {
        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        sock.bind(path).map_err(|_| SocketError::AlreadyBound)
    }

    /// Start listening for connections
    ///
    /// # Arguments
    ///
    /// * `backlog` - Maximum number of pending connections
    ///
    /// # Returns
    ///
    /// Ok on success, or an error if the socket is not bound.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let socket = Socket::new().unwrap();
    /// socket.bind("/tmp/server.sock").unwrap();
    /// socket.listen(5).unwrap();
    /// ```
    pub fn listen(&self, backlog: usize) -> Result<()> {
        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        sock.listen(backlog).map_err(|_| SocketError::NotListening)
    }

    /// Connect to a named socket
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the listening socket
    ///
    /// # Returns
    ///
    /// Ok on success, or an error if the target socket is not found or not listening.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let socket = Socket::new().unwrap();
    /// socket.connect("/tmp/server.sock").unwrap();
    /// ```
    pub fn connect(&self, path: &str) -> Result<()> {
        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        sock.connect(path)
            .map_err(|_| SocketError::ConnectionRefused)
    }

    /// Accept an incoming connection
    ///
    /// # Returns
    ///
    /// A new socket representing the accepted connection, or an error if no
    /// connections are pending or the socket is not listening.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let server = Socket::new().unwrap();
    /// server.bind("/tmp/server.sock").unwrap();
    /// server.listen(5).unwrap();
    /// let client = server.accept().unwrap();
    /// ```
    pub fn accept(&self) -> Result<Socket> {
        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        let raw = sock.accept().map_err(|_| SocketError::WouldBlock)?;
        let handle = unsafe { Handle::from_raw(raw) }.map_err(|_| SocketError::SyscallFailed)?;
        Ok(Socket { handle })
    }

    /// Create a connected socket pair
    ///
    /// # Returns
    ///
    /// A tuple of two connected sockets, or an error if creation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let (sock1, sock2) = Socket::pair().unwrap();
    /// // sock1 and sock2 can now communicate bidirectionally
    /// ```
    pub fn pair() -> Result<(Socket, Socket)> {
        let mut handles = [0usize; 2];
        let result = syscall1(Syscall::Socketpair, handles.as_mut_ptr() as usize);
        if result == usize::MAX {
            return Err(SocketError::SyscallFailed);
        }
        let handle0 = unsafe { Handle::from_raw(handles[0] as i32) }
            .map_err(|_| SocketError::SyscallFailed)?;
        let handle1 = unsafe { Handle::from_raw(handles[1] as i32) }
            .map_err(|_| SocketError::SyscallFailed)?;
        Ok((Socket { handle: handle0 }, Socket { handle: handle1 }))
    }

    /// Shutdown socket
    ///
    /// # Arguments
    ///
    /// * `how` - Which direction to shutdown (Read, Write, or Both)
    ///
    /// # Returns
    ///
    /// Ok on success, or an error if the socket is not connected.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let socket = Socket::new().unwrap();
    /// socket.connect("/tmp/server.sock").unwrap();
    /// socket.shutdown(ShutdownHow::Both).unwrap();
    /// ```
    pub fn shutdown(&self, how: ShutdownHow) -> Result<()> {
        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        sock.shutdown(how).map_err(|_| SocketError::SyscallFailed)
    }

    /// Get StreamOps capability for this socket
    ///
    /// # Returns
    ///
    /// StreamOps capability for reading and writing data through this socket.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let socket = Socket::new().unwrap();
    /// let stream = socket.as_stream().unwrap();
    /// stream.write(b"Hello").unwrap();
    /// ```
    pub fn as_stream(&self) -> Result<crate::handle::capability::StreamOps<'_>> {
        self.handle
            .as_stream()
            .map_err(|_| SocketError::InvalidHandle)
    }
}
