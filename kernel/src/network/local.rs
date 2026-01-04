//! Local Socket Implementation
//!
//! This module implements local (Unix-like) domain sockets for inter-process
//! communication through named socket paths in the filesystem namespace.
//!
//! # Design
//!
//! - **Named Sockets**: Sockets can be bound to filesystem paths
//! - **Connection Oriented**: Uses stream sockets for reliable, ordered data transfer
//! - **NetworkManager Integration**: Uses global NetworkManager for socket registry
//! - **Direct Buffer Management**: Uses VecDeque for efficient data queuing
//!
//! # Socket States
//!
//! 1. **Unconnected**: Initial state after creation
//! 2. **Bound**: Socket bound to a local address
//! 3. **Listening**: Server socket accepting connections
//! 4. **Connected**: Client socket or accepted connection

use alloc::{
    collections::VecDeque,
    format,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use spin::RwLock;

use super::{
    LocalSocketAddress, NetworkManager, ShutdownHow, SocketAddress, SocketControl, SocketDomain,
    SocketError, SocketObject, SocketProtocol, SocketState, SocketType,
};
use crate::ipc::StreamIpcOps;
use crate::object::KernelObject;
use crate::object::capability::{CloneOps, StreamError, StreamOps};
use crate::sync::Waker;

/// Maximum buffer size per socket (64 KB)
const MAX_BUFFER_SIZE: usize = 65536;

/// Local Socket Implementation
///
/// This socket type provides local (Unix-like) domain socket functionality.
/// It uses VecDeque buffers internally for data transfer and integrates with
/// the NetworkManager for socket registry.
pub struct LocalSocket {
    /// Socket type (Stream, Datagram, etc.)
    socket_type: SocketType,

    /// Socket protocol
    protocol: SocketProtocol,

    /// Current socket state
    state: RwLock<SocketState>,

    /// Local address (if bound)
    local_addr: RwLock<Option<String>>,

    /// Peer address (if connected)
    peer_addr: RwLock<Option<String>>,

    /// Read buffer: data received from peer (shared with peer for writing)
    read_buffer: Arc<RwLock<VecDeque<u8>>>,

    /// Write buffer reference: shared with peer socket for writing
    /// When we write, we push to peer's read_buffer
    peer_read_buffer: RwLock<Option<Arc<RwLock<VecDeque<u8>>>>>,

    /// Backlog queue for listening sockets
    /// Contains pending connections waiting to be accepted
    backlog: RwLock<Vec<Arc<LocalSocket>>>,

    /// Maximum backlog size (set by listen())
    max_backlog: RwLock<usize>,

    /// Waker for blocking accept() operations
    accept_waker: Waker,
}

impl LocalSocket {
    /// Create a new local socket
    ///
    /// # Arguments
    ///
    /// * `socket_type` - Socket type (Stream, Datagram, etc.)
    /// * `protocol` - Socket protocol
    ///
    /// # Returns
    ///
    /// A new socket in the Unconnected state
    pub fn new(socket_type: SocketType, protocol: SocketProtocol) -> Self {
        Self {
            socket_type,
            protocol,
            state: RwLock::new(SocketState::Unconnected),
            local_addr: RwLock::new(None),
            peer_addr: RwLock::new(None),
            read_buffer: Arc::new(RwLock::new(VecDeque::with_capacity(MAX_BUFFER_SIZE))),
            peer_read_buffer: RwLock::new(None),
            backlog: RwLock::new(Vec::new()),
            max_backlog: RwLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
        }
    }

    /// Accept a connection with blocking behavior
    ///
    /// This method blocks the calling task until a connection is available in the backlog.
    /// It uses the waker mechanism to properly suspend and wake the task.
    ///
    /// # Arguments
    ///
    /// * `task_id` - ID of the task calling accept
    /// * `trapframe` - Trapframe for scheduler context switching
    ///
    /// # Returns
    ///
    /// Arc to the accepted socket connection
    pub fn accept_blocking(
        &self,
        task_id: usize,
        trapframe: &mut crate::arch::Trapframe,
    ) -> Result<Arc<dyn SocketObject>, SocketError> {
        let state = self.state.read();
        if *state != SocketState::Listening {
            return Err(SocketError::NotListening);
        }
        drop(state);

        // Try to get a pending connection from backlog
        loop {
            {
                let mut backlog = self.backlog.write();
                if let Some(client_socket) = backlog.pop() {
                    return Ok(client_socket);
                }
            } // Release backlog lock

            // No connection available, block the task
            self.accept_waker.wait(task_id, trapframe);

            // When we reach here, task has been woken up
            // Check again if there's a connection
        }
    }

    /// Create a connected socket pair (for internal use)
    ///
    /// This creates two connected sockets, useful for accept() implementation.
    ///
    /// # Arguments
    ///
    /// * `local_addr` - Local address for the first socket
    /// * `peer_addr` - Peer address for the second socket
    ///
    /// # Returns
    ///
    /// A tuple of (local_socket, peer_socket) that are connected
    pub fn create_connected_pair(local_addr: String, peer_addr: String) -> (Arc<Self>, Arc<Self>) {
        // Create shared buffers for bidirectional communication
        let local_read_buffer = Arc::new(RwLock::new(VecDeque::with_capacity(MAX_BUFFER_SIZE)));
        let peer_read_buffer = Arc::new(RwLock::new(VecDeque::with_capacity(MAX_BUFFER_SIZE)));

        // Create local socket (server side)
        // It reads from local_read_buffer, writes to peer_read_buffer
        let local_socket = Arc::new(Self {
            socket_type: SocketType::Stream,
            protocol: SocketProtocol::Default,
            state: RwLock::new(SocketState::Connected),
            local_addr: RwLock::new(Some(local_addr.clone())),
            peer_addr: RwLock::new(Some(peer_addr.clone())),
            read_buffer: local_read_buffer.clone(),
            peer_read_buffer: RwLock::new(Some(peer_read_buffer.clone())),
            backlog: RwLock::new(Vec::new()),
            max_backlog: RwLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
        });

        // Create peer socket (client side)
        // It reads from peer_read_buffer, writes to local_read_buffer
        let peer_socket = Arc::new(Self {
            socket_type: SocketType::Stream,
            protocol: SocketProtocol::Default,
            state: RwLock::new(SocketState::Connected),
            local_addr: RwLock::new(Some(peer_addr)),
            peer_addr: RwLock::new(Some(local_addr)),
            read_buffer: peer_read_buffer.clone(),
            peer_read_buffer: RwLock::new(Some(local_read_buffer.clone())),
            backlog: RwLock::new(Vec::new()),
            max_backlog: RwLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
        });

        (local_socket, peer_socket)
    }
}

impl StreamOps for LocalSocket {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut read_buf = self.read_buffer.write();

        if read_buf.is_empty() {
            // Check if socket is still connected
            if *self.state.read() != SocketState::Connected {
                return Err(StreamError::Closed);
            }
            // Would block if buffer is empty and socket is connected
            return Err(StreamError::WouldBlock);
        }

        let bytes_to_read = buffer.len().min(read_buf.len());
        for i in 0..bytes_to_read {
            buffer[i] = read_buf.pop_front().unwrap();
        }

        Ok(bytes_to_read)
    }

    fn write(&self, data: &[u8]) -> Result<usize, StreamError> {
        let peer_buffer = self.peer_read_buffer.read();
        match peer_buffer.as_ref() {
            Some(buf) => {
                let mut peer_buf = buf.write();

                // Check if buffer has space
                if peer_buf.len() + data.len() > MAX_BUFFER_SIZE {
                    return Err(StreamError::WouldBlock);
                }

                // Write data to peer's read buffer
                peer_buf.extend(data.iter().copied());
                Ok(data.len())
            }
            None => Err(StreamError::Closed),
        }
    }
}

impl StreamIpcOps for LocalSocket {
    fn is_connected(&self) -> bool {
        *self.state.read() == SocketState::Connected
    }

    fn peer_count(&self) -> usize {
        if StreamIpcOps::is_connected(self) {
            1
        } else {
            0
        }
    }

    fn description(&self) -> String {
        let local = self.local_addr.read();
        let peer = self.peer_addr.read();
        format!("LocalSocket[{:?} -> {:?}]", local.as_ref(), peer.as_ref())
    }
}

impl SocketControl for LocalSocket {
    fn bind(&self, address: &SocketAddress) -> Result<(), SocketError> {
        // Check socket is unconnected
        let mut state = self.state.write();
        if *state != SocketState::Unconnected {
            return Err(SocketError::AlreadyConnected);
        }

        // Extract path from address
        let path = match address {
            SocketAddress::Local(addr) => addr.path(),
            _ => return Err(SocketError::InvalidAddress),
        };

        // Register with NetworkManager
        let manager = NetworkManager::get_manager();
        // Create a socket object from self reference
        let socket_obj: Arc<dyn SocketObject> = Arc::new(LocalSocket {
            socket_type: self.socket_type,
            protocol: self.protocol,
            state: RwLock::new(*state),
            local_addr: RwLock::new(Some(path.to_string())),
            peer_addr: RwLock::new(None),
            read_buffer: Arc::new(RwLock::new(VecDeque::with_capacity(MAX_BUFFER_SIZE))),
            peer_read_buffer: RwLock::new(None),
            backlog: RwLock::new(Vec::new()),
            max_backlog: RwLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
        });
        manager.register_named_socket(path, socket_obj)?;

        // Update state
        *self.local_addr.write() = Some(path.to_string());
        *state = SocketState::Bound;

        Ok(())
    }

    fn listen(&self, backlog: usize) -> Result<(), SocketError> {
        let mut state = self.state.write();
        if *state != SocketState::Bound {
            return Err(SocketError::InvalidOperation);
        }

        *self.max_backlog.write() = backlog;
        *state = SocketState::Listening;

        Ok(())
    }

    fn accept(&self) -> Result<Arc<dyn SocketObject>, SocketError> {
        let state = self.state.read();
        if *state != SocketState::Listening {
            return Err(SocketError::NotListening);
        }
        drop(state);

        // Try to get a pending connection from backlog
        let mut backlog = self.backlog.write();
        if let Some(client_socket) = backlog.pop() {
            Ok(client_socket)
        } else {
            Err(SocketError::WouldBlock)
        }
    }

    fn connect(&self, address: &SocketAddress) -> Result<(), SocketError> {
        let mut state = self.state.write();
        if *state != SocketState::Unconnected {
            return Err(SocketError::AlreadyConnected);
        }

        // Extract path from address
        let path = match address {
            SocketAddress::Local(addr) => addr.path(),
            _ => return Err(SocketError::InvalidAddress),
        };

        // Lookup listening socket in NetworkManager
        let manager = NetworkManager::get_manager();
        let server_socket = match manager.lookup_named_socket(path) {
            Ok(socket) => socket,
            Err(e) => return Err(e),
        };

        // Check server is listening
        if server_socket.state() != SocketState::Listening {
            return Err(SocketError::ConnectionRefused);
        }

        // Create connected socket pair
        let local_addr = format!("anon-{}", self as *const _ as usize);
        let (server_conn, client_conn) = Self::create_connected_pair(path.to_string(), local_addr);

        // Add client connection to server's backlog
        // Safety: We know server_socket is a LocalSocket since we created it
        let server_local = unsafe { &*(Arc::as_ptr(&server_socket) as *const LocalSocket) };
        let mut server_backlog = server_local.backlog.write();
        let max_backlog = *server_local.max_backlog.read();

        if server_backlog.len() >= max_backlog {
            return Err(SocketError::ConnectionRefused);
        }
        server_backlog.push(server_conn);
        drop(server_backlog); // Release lock before waking

        // Wake up any task waiting in accept()
        server_local.accept_waker.wake_one();

        // Update self to become the client connection
        *state = SocketState::Connected;
        *self.local_addr.write() = client_conn.local_addr.read().clone();
        *self.peer_addr.write() = client_conn.peer_addr.read().clone();
        *self.peer_read_buffer.write() = client_conn.peer_read_buffer.read().clone();

        Ok(())
    }

    fn shutdown(&self, how: ShutdownHow) -> Result<(), SocketError> {
        let mut state = self.state.write();
        if *state != SocketState::Connected {
            return Err(SocketError::NotConnected);
        }

        match how {
            ShutdownHow::Read | ShutdownHow::Write | ShutdownHow::Both => {
                *state = SocketState::Closed;
                Ok(())
            }
        }
    }

    fn is_connected(&self) -> bool {
        *self.state.read() == SocketState::Connected
    }

    fn state(&self) -> SocketState {
        *self.state.read()
    }

    fn getpeername(&self) -> Result<SocketAddress, SocketError> {
        let peer = self.peer_addr.read();
        match peer.as_ref() {
            Some(path) => Ok(SocketAddress::Local(
                LocalSocketAddress::from_path(path)
                    .unwrap_or_else(|_| LocalSocketAddress::unnamed()),
            )),
            None => Err(SocketError::NotConnected),
        }
    }

    fn getsockname(&self) -> Result<SocketAddress, SocketError> {
        let local = self.local_addr.read();
        match local.as_ref() {
            Some(path) => Ok(SocketAddress::Local(
                LocalSocketAddress::from_path(path)
                    .unwrap_or_else(|_| LocalSocketAddress::unnamed()),
            )),
            None => Err(SocketError::InvalidOperation),
        }
    }
}

impl SocketObject for LocalSocket {
    fn socket_type(&self) -> SocketType {
        self.socket_type
    }

    fn socket_domain(&self) -> SocketDomain {
        SocketDomain::Local
    }

    fn socket_protocol(&self) -> SocketProtocol {
        self.protocol
    }
}

impl CloneOps for LocalSocket {
    fn custom_clone(&self) -> KernelObject {
        KernelObject::Socket(Arc::new(LocalSocket {
            socket_type: self.socket_type,
            protocol: self.protocol,
            state: RwLock::new(*self.state.read()),
            local_addr: RwLock::new(self.local_addr.read().clone()),
            peer_addr: RwLock::new(self.peer_addr.read().clone()),
            read_buffer: self.read_buffer.clone(),
            peer_read_buffer: RwLock::new(self.peer_read_buffer.read().clone()),
            backlog: RwLock::new(Vec::new()),
            max_backlog: RwLock::new(*self.max_backlog.read()),
            accept_waker: Waker::new_interruptible("socket_accept_cloned"),
        }))
    }
}

/// Socket factory function for local sockets
///
/// This function is registered with the NetworkManager to create
/// local domain sockets.
pub fn local_socket_factory(
    socket_type: SocketType,
    protocol: SocketProtocol,
) -> Result<Arc<dyn SocketObject>, SocketError> {
    Ok(Arc::new(LocalSocket::new(socket_type, protocol)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_socket_creation() {
        let socket = LocalSocket::new(SocketType::Stream, SocketProtocol::Default);
        assert_eq!(socket.state(), SocketState::Unconnected);
        assert_eq!(socket.socket_domain(), SocketDomain::Local);
    }

    #[test_case]
    fn test_socket_factory() {
        let socket = local_socket_factory(SocketType::Stream, SocketProtocol::Default).unwrap();
        assert_eq!(socket.socket_domain(), SocketDomain::Local);
        assert_eq!(socket.socket_type(), SocketType::Stream);
    }

    #[test_case]
    fn test_connected_pair() {
        let (sock1, sock2) =
            LocalSocket::create_connected_pair("server".to_string(), "client".to_string());
        assert_eq!(sock1.state(), SocketState::Connected);
        assert_eq!(sock2.state(), SocketState::Connected);
    }

    #[test_case]
    fn test_read_write() {
        let (sock1, sock2) =
            LocalSocket::create_connected_pair("server".to_string(), "client".to_string());

        // Write from sock1 to sock2
        let data = b"Hello, World!";
        let written = sock1.write(data).unwrap();
        assert_eq!(written, data.len());

        // Read from sock2
        let mut buffer = [0u8; 32];
        let read = sock2.read(&mut buffer).unwrap();
        assert_eq!(read, data.len());
        assert_eq!(&buffer[..read], data);
    }

    #[test_case]
    fn test_bidirectional_communication() {
        let (sock1, sock2) =
            LocalSocket::create_connected_pair("server".to_string(), "client".to_string());

        // sock1 -> sock2
        sock1.write(b"ping").unwrap();
        let mut buf = [0u8; 4];
        sock2.read(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");

        // sock2 -> sock1
        sock2.write(b"pong").unwrap();
        let mut buf = [0u8; 4];
        sock1.read(&mut buf).unwrap();
        assert_eq!(&buf, b"pong");
    }
}
