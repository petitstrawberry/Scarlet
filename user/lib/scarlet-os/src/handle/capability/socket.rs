//! SocketObject capability for Scarlet Native API
//!
//! This module provides type-safe socket operations for KernelObjects that
//! implement the Scarlet Native local socket interface.

use crate::handle::{Handle, RawHandle};
use scarlet_sys::{Syscall, syscall1, syscall2, syscall3, syscall4, syscall5};

/// Scarlet Native socket domains
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SocketDomain {
    Local = 1,
    Inet4 = 2,
    Inet6 = 3,
}

/// Scarlet Native socket types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SocketType {
    Stream = 1,
    Datagram = 2,
    Raw = 3,
    SeqPacket = 4,
}

/// Scarlet Native socket protocols
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SocketProtocol {
    Default = 0,
    Icmp = 1,
    Tcp = 6,
    Udp = 17,
    Raw = 255,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inet4SocketAddress {
    pub addr: [u8; 4],
    pub port: u16,
}

impl Inet4SocketAddress {
    pub fn new(addr: [u8; 4], port: u16) -> Self {
        Self { addr, port }
    }
}

/// Socket address abstraction - OS-agnostic
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketAddress {
    /// IPv4 address with port
    Inet(Inet4SocketAddress),
    /// Unspecified/any address
    Unspecified,
}

impl SocketAddress {
    /// Check if this is an unspecified address
    pub fn is_unspecified(&self) -> bool {
        matches!(self, SocketAddress::Unspecified)
    }
}

/// Result type for socket operations
pub type SocketObjectResult<T> = Result<T, SocketObjectError>;

/// Errors that can occur during socket operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketObjectError {
    /// The requested operation cannot consume the first queued segment yet
    WouldBlock,
    /// The supplied receive buffer cannot hold the complete record
    ReceiveBufferTooSmall {
        /// Exact number of bytes required for the queued record
        required_len: usize,
    },
    /// The record exceeds the maximum supported local-socket record size
    MessageTooLarge,
    /// Other system error
    SystemError(i32),
}

impl SocketObjectError {
    fn from_syscall_result(result: usize) -> Result<usize, Self> {
        if result == usize::MAX {
            Err(SocketObjectError::SystemError(-1))
        } else if result > isize::MAX as usize {
            let errno = -(result as isize) as i32;
            match errno {
                11 => Err(SocketObjectError::WouldBlock),
                90 => Err(SocketObjectError::MessageTooLarge),
                _ => Err(SocketObjectError::SystemError(errno)),
            }
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

    /// Bind socket to an abstract local name.
    ///
    /// Abstract local names are not represented as filesystem entries. This is
    /// the Scarlet-native equivalent of Linux `sockaddr_un` names whose
    /// `sun_path` starts with a NUL byte.
    ///
    /// # Arguments
    ///
    /// * `name` - Abstract socket name without the leading NUL byte
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an error if the socket cannot be bound.
    pub fn bind_abstract(&self, name: &str) -> SocketObjectResult<()> {
        let mut address = [0u8; 108];
        if name.len() > address.len() - 1 {
            return Err(SocketObjectError::SystemError(-1));
        }
        address[1..1 + name.len()].copy_from_slice(name.as_bytes());
        let result = syscall3(
            Syscall::SocketBind,
            self.handle.as_raw() as usize,
            address.as_ptr() as usize,
            name.len() + 1,
        );
        SocketObjectError::from_syscall_result(result).map(|_| ())
    }

    /// Bind socket to an IPv4 address
    pub fn bind_inet(&self, addr: &Inet4SocketAddress) -> SocketObjectResult<()> {
        let result = syscall3(
            Syscall::SocketBind,
            self.handle.as_raw() as usize,
            addr as *const Inet4SocketAddress as usize,
            core::mem::size_of::<Inet4SocketAddress>(),
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

    /// Connect to an abstract local socket.
    ///
    /// # Arguments
    ///
    /// * `name` - Abstract socket name without the leading NUL byte
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an error if the target socket is not listening.
    pub fn connect_abstract(&self, name: &str) -> SocketObjectResult<()> {
        let mut address = [0u8; 108];
        if name.len() > address.len() - 1 {
            return Err(SocketObjectError::SystemError(-1));
        }
        address[1..1 + name.len()].copy_from_slice(name.as_bytes());
        let result = syscall3(
            Syscall::SocketConnect,
            self.handle.as_raw() as usize,
            address.as_ptr() as usize,
            name.len() + 1,
        );
        SocketObjectError::from_syscall_result(result).map(|_| ())
    }

    /// Connect to an IPv4 address
    pub fn connect_inet(&self, addr: &Inet4SocketAddress) -> SocketObjectResult<()> {
        let result = syscall3(
            Syscall::SocketConnect,
            self.handle.as_raw() as usize,
            addr as *const Inet4SocketAddress as usize,
            core::mem::size_of::<Inet4SocketAddress>(),
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
    /// The handle and bytes form one boundary-preserving record ordered with
    /// normal byte writes and handle-only transfers on the socket.
    ///
    /// # Arguments
    ///
    /// * `object_handle` - The raw handle of the kernel object to send
    /// * `data` - The data to send with the handle
    ///
    /// # Returns
    ///
    /// `Ok(())` when the complete record is queued, or a socket error when the
    /// peer queue cannot accept it.
    pub fn send_handle_and_data(
        &self,
        object_handle: RawHandle,
        data: &[u8],
    ) -> SocketObjectResult<()> {
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
    /// Returns one complete handle-and-data record. If `data_out` is too small,
    /// the error contains the required size and the record remains queued.
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
        let mut required_len = 0usize;
        let result = syscall5(
            Syscall::SocketRecvHandleAndData,
            self.handle.as_raw() as usize,
            handle_out as *mut RawHandle as usize,
            data_out.as_mut_ptr() as usize,
            data_out.len(),
            &mut required_len as *mut usize as usize,
        );
        match SocketObjectError::from_syscall_result(result) {
            Err(SocketObjectError::MessageTooLarge) => {
                Err(SocketObjectError::ReceiveBufferTooSmall { required_len })
            }
            other => other,
        }
    }
}
