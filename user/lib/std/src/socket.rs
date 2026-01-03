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

use crate::syscall::{Syscall, syscall0, syscall1, syscall2, syscall3};

/// Socket handle wrapper
///
/// Represents a Scarlet Native local socket for inter-process communication.
/// Sockets are automatically closed when dropped.
#[derive(Debug)]
pub struct Socket {
    handle: u32,
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
        Ok(Socket {
            handle: handle as u32,
        })
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
        let result = syscall3(
            Syscall::SocketBind,
            self.handle as usize,
            path.as_ptr() as usize,
            path.len(),
        );
        if result == usize::MAX {
            return Err(SocketError::AlreadyBound);
        }
        Ok(())
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
        let result = syscall2(Syscall::SocketListen, self.handle as usize, backlog);
        if result == usize::MAX {
            return Err(SocketError::NotListening);
        }
        Ok(())
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
        let result = syscall3(
            Syscall::SocketConnect,
            self.handle as usize,
            path.as_ptr() as usize,
            path.len(),
        );
        if result == usize::MAX {
            return Err(SocketError::ConnectionRefused);
        }
        Ok(())
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
        let handle = syscall1(Syscall::SocketAccept, self.handle as usize);
        if handle == usize::MAX {
            return Err(SocketError::WouldBlock);
        }
        Ok(Socket {
            handle: handle as u32,
        })
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
        Ok((
            Socket {
                handle: handles[0] as u32,
            },
            Socket {
                handle: handles[1] as u32,
            },
        ))
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
        let result = syscall2(Syscall::SocketShutdown, self.handle as usize, how as usize);
        if result == usize::MAX {
            return Err(SocketError::SyscallFailed);
        }
        Ok(())
    }

    /// Get the raw handle ID
    ///
    /// # Returns
    ///
    /// The underlying handle ID for this socket.
    pub fn as_raw_handle(&self) -> u32 {
        self.handle
    }

    /// Create a socket from a raw handle
    ///
    /// # Safety
    ///
    /// The caller must ensure the handle is valid and represents a socket.
    ///
    /// # Arguments
    ///
    /// * `handle` - Raw handle ID
    ///
    /// # Returns
    ///
    /// A Socket wrapping the given handle.
    pub unsafe fn from_raw_handle(handle: u32) -> Self {
        Socket { handle }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        // Close the socket handle
        // HandleClose = 102
        let _ = syscall1(Syscall::HandleClose, self.handle as usize);
    }
}
