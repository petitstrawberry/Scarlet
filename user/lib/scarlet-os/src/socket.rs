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
use scarlet_sys::{
    SCTL_SOCKET_GET_NONBLOCK, SCTL_SOCKET_SET_NONBLOCK, Syscall, syscall1, syscall3,
};

pub use crate::handle::capability::ShutdownHow;
pub use crate::handle::capability::socket::Inet4SocketAddress;
pub use crate::handle::capability::socket::SocketAddress;
pub use crate::handle::capability::socket::SocketDomain;
pub use crate::handle::capability::socket::SocketProtocol;
pub use crate::handle::capability::socket::SocketType;

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
    /// Invalid address
    InvalidAddress,
    /// Already bound or connected
    AlreadyBound,
    /// Not listening
    NotListening,
    /// Connection refused
    ConnectionRefused,
    /// Operation would block
    WouldBlock,
    /// The supplied destination cannot hold the next complete socket record
    ReceiveBufferTooSmall {
        /// Exact number of bytes required for the queued record
        required_len: usize,
    },
    /// The outgoing record exceeds the local socket record limit
    MessageTooLarge,
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
        Self::new_with_domain(
            SocketDomain::Local,
            SocketType::Stream,
            SocketProtocol::Default,
        )
    }

    /// Create a new socket with specified domain, type, and protocol
    ///
    /// # Arguments
    ///
    /// * `domain` - Socket domain (e.g., Local, Inet)
    /// * `socket_type` - Socket type (e.g., Stream, Datagram)
    /// * `protocol` - Socket protocol (e.g., Default, Tcp, Udp)
    ///
    /// # Returns
    ///
    /// A new unconnected socket with the specified configuration, or an error if creation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::socket::{Socket, SocketDomain, SocketType, SocketProtocol};
    /// let socket = Socket::new_with_domain(
    ///     SocketDomain::Inet4,
    ///     SocketType::Stream,
    ///     SocketProtocol::Tcp
    /// ).unwrap();
    /// ```
    pub fn new_with_domain(
        domain: SocketDomain,
        socket_type: SocketType,
        protocol: SocketProtocol,
    ) -> Result<Self> {
        let raw_handle = syscall3(
            Syscall::SocketCreate,
            domain as usize,
            socket_type as usize,
            protocol as usize,
        );
        if raw_handle == usize::MAX {
            return Err(SocketError::SyscallFailed);
        }

        let handle = unsafe { Handle::from_raw(raw_handle as i32) }
            .map_err(|_| SocketError::SyscallFailed)?;
        Ok(Socket { handle })
    }

    /// Bind socket to an IPv4 address
    ///
    /// # Arguments
    ///
    /// * `addr` - IPv4 socket address to bind to
    ///
    /// # Returns
    ///
    /// Ok on success, or an error if the socket is already bound or the address is invalid.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::socket::{Socket, Inet4SocketAddress};
    /// let socket = Socket::new().unwrap();
    /// let addr = Inet4SocketAddress::new([0, 0, 0, 0], 8080);
    /// socket.bind_inet(addr).unwrap();
    /// ```
    pub fn bind_inet(&self, addr: Inet4SocketAddress) -> Result<()> {
        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        sock.bind_inet(&addr).map_err(|_| SocketError::AlreadyBound)
    }

    /// Bind outgoing IPv4 datagrams to a network interface.
    ///
    /// # Arguments
    ///
    /// * `interface` - Registered network interface name.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the datagram socket is bound to the interface, or an error
    /// if the interface does not exist or the socket does not support this
    /// operation.
    pub fn bind_interface(&self, interface: &str) -> Result<()> {
        if interface.is_empty() {
            return Err(SocketError::InvalidAddress);
        }
        let result = syscall3(
            Syscall::SocketBindInterface,
            self.handle.as_raw() as usize,
            interface.as_ptr() as usize,
            interface.len(),
        );
        if result == usize::MAX {
            Err(SocketError::InvalidAddress)
        } else {
            Ok(())
        }
    }

    /// Connect socket to an IPv4 address
    ///
    /// # Arguments
    ///
    /// * `addr` - IPv4 socket address to connect to
    ///
    /// # Returns
    ///
    /// Ok on success, or an error if connection fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::socket::{Socket, Inet4SocketAddress};
    /// let socket = Socket::new().unwrap();
    /// let addr = Inet4SocketAddress::new([10, 0, 2, 15], 8080);
    /// socket.connect_inet(addr).unwrap();
    /// ```
    pub fn connect_inet(&self, addr: Inet4SocketAddress) -> Result<()> {
        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        sock.connect_inet(&addr)
            .map_err(|_| SocketError::ConnectionRefused)
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

    /// Send one ordered handle-and-data record through this connected socket.
    ///
    /// The record boundary and its order relative to normal writes and
    /// handle-only transfers are preserved.
    ///
    /// # Arguments
    ///
    /// * `object` - The kernel object handle to send
    /// * `data` - The data to send with the handle
    ///
    /// # Returns
    ///
    /// `Ok(())` when the record is queued, or a socket error otherwise.
    pub fn send_handle_and_data(&self, object: &Handle, data: &[u8]) -> Result<()> {
        use crate::handle::capability::socket::SocketObjectError;

        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        sock.send_handle_and_data(object.as_raw(), data)
            .map_err(|error| match error {
                SocketObjectError::WouldBlock => SocketError::WouldBlock,
                SocketObjectError::MessageTooLarge => SocketError::MessageTooLarge,
                _ => SocketError::SyscallFailed,
            })
    }

    /// Receive a kernel object handle and data atomically through this connected socket.
    ///
    /// Returns one complete handle-and-data record. A short destination leaves
    /// the record queued and reports its required size.
    ///
    /// # Arguments
    ///
    /// * `data_out` - Buffer to store the received data
    ///
    /// # Returns
    ///
    /// * `(Handle, usize)` - The received handle and number of bytes on success
    /// * `SocketError` - Error on failure
    pub fn recv_handle_and_data(&self, data_out: &mut [u8]) -> Result<(Handle, usize)> {
        use crate::handle::capability::socket::SocketObjectError;

        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        let mut raw_handle = 0;
        let bytes_read = sock
            .recv_handle_and_data(&mut raw_handle, data_out)
            .map_err(|error| match error {
                SocketObjectError::WouldBlock => SocketError::WouldBlock,
                SocketObjectError::ReceiveBufferTooSmall { required_len } => {
                    SocketError::ReceiveBufferTooSmall { required_len }
                }
                _ => SocketError::SyscallFailed,
            })?;
        let handle =
            unsafe { Handle::from_raw(raw_handle) }.map_err(|_| SocketError::SyscallFailed)?;
        Ok((handle, bytes_read))
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

    /// Bind socket to an abstract local name.
    ///
    /// Abstract local sockets do not create filesystem entries. This is useful
    /// for interoperating with Linux programs that use `sockaddr_un` abstract
    /// namespace sockets.
    ///
    /// # Arguments
    ///
    /// * `name` - Abstract socket name without the leading NUL byte
    ///
    /// # Returns
    ///
    /// Ok on success, or an error if the socket is already bound or the name is invalid.
    pub fn bind_abstract(&self, name: &str) -> Result<()> {
        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        sock.bind_abstract(name)
            .map_err(|_| SocketError::AlreadyBound)
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

    /// Connect to an abstract local socket.
    ///
    /// # Arguments
    ///
    /// * `name` - Abstract socket name without the leading NUL byte
    ///
    /// # Returns
    ///
    /// Ok on success, or an error if the target socket is not found or not listening.
    pub fn connect_abstract(&self, name: &str) -> Result<()> {
        let sock = self
            .handle
            .as_socket()
            .map_err(|_| SocketError::InvalidHandle)?;
        sock.connect_abstract(name)
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
        let handle0 = match unsafe { Handle::from_raw(handles[0] as i32) } {
            Ok(handle) => handle,
            Err(_) => {
                // from_raw consumed and closed handles[0]. The other endpoint
                // has not been adopted, so close it explicitly to avoid a leak.
                let _ = syscall1(Syscall::HandleClose, handles[1]);
                return Err(SocketError::SyscallFailed);
            }
        };
        let handle1 = match unsafe { Handle::from_raw(handles[1] as i32) } {
            Ok(handle) => handle,
            Err(_) => {
                // from_raw consumed and closed handles[1]; handle0 is closed by
                // its Drop while returning this error.
                return Err(SocketError::SyscallFailed);
            }
        };
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

    /// Set non-blocking mode for this socket
    ///
    /// When enabled, read operations will return immediately with WouldBlock error
    /// if no data is available, instead of blocking.
    ///
    /// # Arguments
    ///
    /// * `enabled` - true to enable non-blocking mode, false to disable
    ///
    /// # Returns
    ///
    /// Ok on success, or an error if the system call fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let socket = Socket::new().unwrap();
    /// socket.set_nonblocking(true).unwrap();
    /// ```
    pub fn set_nonblocking(&self, enabled: bool) -> Result<()> {
        let result = syscall3(
            Syscall::HandleControl,
            self.handle.as_raw() as usize,
            SCTL_SOCKET_SET_NONBLOCK as usize,
            if enabled { 1 } else { 0 },
        );
        if result == usize::MAX {
            return Err(SocketError::SyscallFailed);
        }
        Ok(())
    }

    /// Get the non-blocking mode of this socket
    ///
    /// # Returns
    ///
    /// Ok(true) if non-blocking mode is enabled, Ok(false) otherwise
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let socket = Socket::new().unwrap();
    /// socket.set_nonblocking(true).unwrap();
    /// assert!(socket.is_nonblocking().unwrap());
    /// ```
    pub fn is_nonblocking(&self) -> Result<bool> {
        let result = syscall3(
            Syscall::HandleControl,
            self.handle.as_raw() as usize,
            SCTL_SOCKET_GET_NONBLOCK as usize,
            0,
        );
        if result == usize::MAX {
            return Err(SocketError::SyscallFailed);
        }
        Ok(result != 0)
    }
}

/// Datagram operations trait for UDP and Local datagram sockets
///
/// This trait provides operations for connectionless datagram sockets,
/// allowing send/receive with explicit addresses.
pub trait DatagramOps {
    /// Receive a datagram with sender address
    ///
    /// # Arguments
    /// * `buf` - Buffer to store received data
    ///
    /// # Returns
    /// * `(usize, SocketAddress)` - Number of bytes received and sender address
    fn recvfrom(&self, buf: &mut [u8]) -> Result<(usize, SocketAddress)>;

    /// Send a datagram to specified address
    ///
    /// # Arguments
    /// * `buf` - Data to send
    /// * `addr` - Destination address
    ///
    /// # Returns
    /// * `usize` - Number of bytes sent
    fn sendto(&self, buf: &[u8], addr: &SocketAddress) -> Result<usize>;
}

impl DatagramOps for Socket {
    fn recvfrom(&self, buf: &mut [u8]) -> Result<(usize, SocketAddress)> {
        use scarlet_sys::{Syscall, syscall4};

        // Allocate space for address (8 bytes: 2 for family, 4 for IP, 2 for port)
        let mut addr_buf = [0u8; 8];

        let result = syscall4(
            Syscall::SocketRecvFrom,
            self.handle.as_raw() as usize,
            buf.as_mut_ptr() as usize,
            buf.len(),
            addr_buf.as_mut_ptr() as usize,
        );

        // Handle negative errno values first
        if result > (isize::MAX as usize) {
            let errno = -(result as isize) as i32;
            if errno == 11 {
                return Err(SocketError::WouldBlock);
            }
            return Err(SocketError::SyscallFailed);
        }

        if result == usize::MAX {
            return Err(SocketError::SyscallFailed);
        }

        // Success - parse address
        let addr = match addr_buf[0] {
            2 => {
                // AF_INET
                let ip = [addr_buf[2], addr_buf[3], addr_buf[4], addr_buf[5]];
                let port = u16::from_be_bytes([addr_buf[6], addr_buf[7]]);
                SocketAddress::Inet(Inet4SocketAddress::new(ip, port))
            }
            _ => return Err(SocketError::InvalidAddress),
        };
        Ok((result, addr))
    }

    fn sendto(&self, buf: &[u8], addr: &SocketAddress) -> Result<usize> {
        use scarlet_sys::{Syscall, syscall4};

        // Serialize address
        let mut addr_buf = [0u8; 8];
        match addr {
            SocketAddress::Inet(inet) => {
                addr_buf[0] = 2; // AF_INET
                addr_buf[2..6].copy_from_slice(&inet.addr);
                addr_buf[6..8].copy_from_slice(&inet.port.to_be_bytes());
            }
            _ => return Err(SocketError::InvalidAddress),
        }

        let result = syscall4(
            Syscall::SocketSendTo,
            self.handle.as_raw() as usize,
            buf.as_ptr() as usize,
            buf.len(),
            addr_buf.as_ptr() as usize,
        );

        if result == usize::MAX {
            return Err(SocketError::SyscallFailed);
        }

        Ok(result)
    }
}

// Handle's Drop implementation will automatically close the socket
