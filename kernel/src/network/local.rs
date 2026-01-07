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
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;
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

/// Shared buffer structure that tracks reading task
struct SocketBuffer {
    /// Data buffer
    data: RwLock<VecDeque<u8>>,
    /// Task ID waiting to read (if any)
    reading_task: RwLock<Option<usize>>,
    /// Flag indicating this buffer has been closed (peer shutdown)
    closed: RwLock<bool>,
}

impl SocketBuffer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            data: RwLock::new(VecDeque::with_capacity(MAX_BUFFER_SIZE)),
            reading_task: RwLock::new(None),
            closed: RwLock::new(false),
        })
    }
}

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
    read_buffer: RwLock<Arc<SocketBuffer>>,

    /// Write buffer reference: shared with peer socket for writing
    /// When we write, we push to peer's read_buffer
    peer_read_buffer: RwLock<Option<Arc<SocketBuffer>>>,

    /// Peer socket reference (for waking read waiters)
    peer_socket: RwLock<Option<Weak<LocalSocket>>>,

    /// Backlog queue for listening sockets
    /// Contains pending connections waiting to be accepted
    backlog: RwLock<Vec<Arc<LocalSocket>>>,

    /// Maximum backlog size (set by listen())
    max_backlog: RwLock<usize>,

    /// Waker for blocking accept() operations
    accept_waker: Waker,

    /// Waker for blocking read() operations
    read_waker: Waker,

    /// Queue of handles (KernelObjects) received from peer
    /// This allows passing file descriptors / kernel objects between tasks
    handle_queue: RwLock<VecDeque<KernelObject>>,
}

impl LocalSocket {
    /// Safely downcast a SocketObject to LocalSocket using Any trait
    ///
    /// Returns None if the socket is not a LocalSocket.
    /// This is completely safe and does not use any unsafe code.
    pub fn from_socket_object(socket: &Arc<dyn SocketObject>) -> Option<&Self> {
        // Use SocketObject's as_any() to get &dyn Any
        socket.as_any().downcast_ref::<LocalSocket>()
    }

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
            read_buffer: RwLock::new(SocketBuffer::new()),
            peer_read_buffer: RwLock::new(None),
            peer_socket: RwLock::new(None),
            backlog: RwLock::new(Vec::new()),
            max_backlog: RwLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
            read_waker: Waker::new_interruptible("socket_read"),
            handle_queue: RwLock::new(VecDeque::new()),
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
        let local_read_buffer = SocketBuffer::new();
        let peer_read_buffer = SocketBuffer::new();

        // Create local socket (server side)
        // It reads from local_read_buffer, writes to peer_read_buffer
        let local_socket = Arc::new(Self {
            socket_type: SocketType::Stream,
            protocol: SocketProtocol::Default,
            state: RwLock::new(SocketState::Connected),
            local_addr: RwLock::new(Some(local_addr.clone())),
            peer_addr: RwLock::new(Some(peer_addr.clone())),
            read_buffer: RwLock::new(local_read_buffer.clone()),
            peer_read_buffer: RwLock::new(Some(peer_read_buffer.clone())),
            peer_socket: RwLock::new(None), // Set later
            backlog: RwLock::new(Vec::new()),
            max_backlog: RwLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
            read_waker: Waker::new_interruptible("socket_read"),
            handle_queue: RwLock::new(VecDeque::new()),
        });

        // Create peer socket (client side)
        // It reads from peer_read_buffer, writes to local_read_buffer
        let peer_socket = Arc::new(Self {
            socket_type: SocketType::Stream,
            protocol: SocketProtocol::Default,
            state: RwLock::new(SocketState::Connected),
            local_addr: RwLock::new(Some(peer_addr)),
            peer_addr: RwLock::new(Some(local_addr)),
            read_buffer: RwLock::new(peer_read_buffer.clone()),
            peer_read_buffer: RwLock::new(Some(local_read_buffer.clone())),
            peer_socket: RwLock::new(None), // Set later
            backlog: RwLock::new(Vec::new()),
            max_backlog: RwLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
            read_waker: Waker::new_interruptible("socket_read"),
            handle_queue: RwLock::new(VecDeque::new()),
        });

        // Set peer references
        *local_socket.peer_socket.write() = Some(Arc::downgrade(&peer_socket));
        *peer_socket.peer_socket.write() = Some(Arc::downgrade(&local_socket));

        (local_socket, peer_socket)
    }

    /// Blocking read operation
    ///
    /// This method blocks the calling task until data is available.
    pub fn read_blocking(
        &self,
        buffer: &mut [u8],
        task_id: usize,
        trapframe: &mut crate::arch::Trapframe,
    ) -> Result<usize, StreamError> {
        loop {
            {
                let read_buf_arc = self.read_buffer.read();
                let mut read_data = read_buf_arc.data.write();

                if !read_data.is_empty() {
                    let bytes_to_read = buffer.len().min(read_data.len());
                    for i in 0..bytes_to_read {
                        buffer[i] = read_data.pop_front().unwrap();
                    }
                    // Clear reading_task since we're done reading
                    *read_buf_arc.reading_task.write() = None;
                    return Ok(bytes_to_read);
                }

                // Check if socket is closed (peer shutdown)
                // Return 0 to indicate EOF (not an error)
                let my_state = *self.state.read();
                if my_state == SocketState::Closed {
                    // crate::println!("[LocalSocket] read_blocking: self is Closed, returning EOF");
                    return Ok(0);
                }

                // Check if peer is closed (they called shutdown)
                if let Some(peer_weak) = self.peer_socket.read().as_ref() {
                    if let Some(peer) = peer_weak.upgrade() {
                        let peer_state = *peer.state.read();
                        if peer_state == SocketState::Closed {
                            // crate::println!(
                            //     "[LocalSocket] read_blocking: peer is Closed, returning EOF"
                            // );
                            return Ok(0); // Peer closed, return EOF
                        }
                    } else {
                        // crate::println!("[LocalSocket] read_blocking: peer dropped, returning EOF");
                        return Ok(0); // Peer dropped, treat as EOF
                    }
                }

                // Check if this read buffer has been closed by peer's shutdown()
                if *read_buf_arc.closed.read() {
                    // crate::println!(
                    //     "[LocalSocket] read_blocking: read_buffer marked closed, returning EOF"
                    // );
                    return Ok(0);
                }

                // Register this task as waiting to read
                *read_buf_arc.reading_task.write() = Some(task_id);
            } // Release lock

            // No data available, block the task
            self.read_waker.wait(task_id, trapframe);
            // crate::println!("[LocalSocket] read_blocking: woken up, checking again...");
            // When woken, loop back to check for data or shutdown
        }
    }
}

impl StreamOps for LocalSocket {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let read_buf_arc = self.read_buffer.read();
        let mut read_data = read_buf_arc.data.write();

        if read_data.is_empty() {
            // Check if socket is still connected
            if *self.state.read() != SocketState::Connected {
                return Err(StreamError::Closed);
            }
            // Would block if buffer is empty and socket is connected
            return Err(StreamError::WouldBlock);
        }

        let bytes_to_read = buffer.len().min(read_data.len());
        for i in 0..bytes_to_read {
            buffer[i] = read_data.pop_front().unwrap();
        }

        Ok(bytes_to_read)
    }

    fn write(&self, data: &[u8]) -> Result<usize, StreamError> {
        let peer_buffer = self.peer_read_buffer.read();
        match peer_buffer.as_ref() {
            Some(peer_sock_buffer) => {
                let mut peer_data = peer_sock_buffer.data.write();

                // Check if buffer has space
                if peer_data.len() + data.len() > MAX_BUFFER_SIZE {
                    return Err(StreamError::WouldBlock);
                }

                // Write data to peer's read buffer
                peer_data.extend(data.iter().copied());
                let bytes_written = data.len();

                // Check if there's a task waiting to read
                // Keep reading_task lock held while we wake to prevent race condition
                let reading_task_guard = peer_sock_buffer.reading_task.read();
                let reading_task = *reading_task_guard;

                drop(peer_data); // Release data lock

                // Wake up the task if one is waiting (still holding reading_task lock)
                if let Some(task_id) = reading_task {
                    use crate::sched::scheduler::get_scheduler;
                    get_scheduler().wake_task(task_id);
                }

                drop(reading_task_guard); // Release reading_task lock
                drop(peer_buffer); // Release peer_buffer lock

                Ok(bytes_written)
            }
            None => {
                // crate::println!("[LocalSocket] write: peer buffer is None (closed)");
                Err(StreamError::Closed)
            }
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

    fn send_handle(&self, object: KernelObject) -> Result<(), crate::ipc::IpcError> {
        use crate::ipc::IpcError;

        // Verify socket is connected
        if *self.state.read() != SocketState::Connected {
            return Err(IpcError::InvalidState);
        }

        // Get peer socket reference
        let peer_weak = self.peer_socket.read();
        let peer_weak_ref = peer_weak.as_ref().ok_or(IpcError::PeerClosed)?;
        let peer = peer_weak_ref.upgrade().ok_or(IpcError::PeerClosed)?;

        // Add handle to peer's receive queue
        peer.handle_queue.write().push_back(object);

        Ok(())
    }

    fn recv_handle(&self) -> Result<KernelObject, crate::ipc::IpcError> {
        use crate::ipc::IpcError;

        // Verify socket is connected
        if *self.state.read() != SocketState::Connected {
            return Err(IpcError::InvalidState);
        }

        // Try to get a handle from the queue
        let mut queue = self.handle_queue.write();
        queue.pop_front().ok_or(IpcError::ChannelEmpty)
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

        // Update state
        // Note: NetworkManager registration is done by the syscall layer
        // to ensure the same Arc<Self> is registered that's in the handle table
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
        // Validate current state
        let mut state = self.state.write();
        if *state != SocketState::Unconnected {
            return Err(SocketError::AlreadyConnected);
        }
        drop(state);

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

        // We need to create a proper Arc to self to be able to store a Weak reference in the peer
        // Since we're in &self, we don't have access to the Arc. We'll need to store the
        // connection information and let the server-side socket refer back through handle table.

        // Instead, we'll use a different approach: create shared buffers and update both sockets
        let local_addr = format!("anon-{}", self as *const _ as usize);

        // Create shared buffers for bidirectional communication
        let client_read_buffer = SocketBuffer::new();
        let server_read_buffer = SocketBuffer::new();

        // Create server-side connection socket that will be added to backlog
        let server_conn = Arc::new(Self {
            socket_type: SocketType::Stream,
            protocol: SocketProtocol::Default,
            state: RwLock::new(SocketState::Connected),
            local_addr: RwLock::new(Some(path.to_string())),
            peer_addr: RwLock::new(Some(local_addr.clone())),
            read_buffer: RwLock::new(server_read_buffer.clone()),
            peer_read_buffer: RwLock::new(Some(client_read_buffer.clone())),
            peer_socket: RwLock::new(None), // Will be set below
            backlog: RwLock::new(Vec::new()),
            max_backlog: RwLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
            read_waker: Waker::new_interruptible("socket_read"),
            handle_queue: RwLock::new(VecDeque::new()),
        });

        // Update self (client socket) to use the shared buffers
        *self.read_buffer.write() = client_read_buffer.clone();
        *self.peer_read_buffer.write() = Some(server_read_buffer.clone());
        *self.local_addr.write() = Some(local_addr);
        *self.peer_addr.write() = Some(path.to_string());
        *self.state.write() = SocketState::Connected;

        // Set peer_socket references - IMPORTANT for shutdown()
        // Client (self) points to server_conn
        *self.peer_socket.write() = Some(Arc::downgrade(&server_conn));

        // Server_conn needs to point back to client, but we don't have Arc<Self>
        // We'll handle this in a moment by creating a temporary strong reference

        // Add server connection to server's backlog
        let server_local = match Self::from_socket_object(&server_socket) {
            Some(socket) => socket,
            None => return Err(SocketError::InvalidOperation), // Not a LocalSocket
        };
        let mut server_backlog = server_local.backlog.write();
        let max_backlog = *server_local.max_backlog.read();

        if server_backlog.len() >= max_backlog {
            // Rollback state change - restore original empty buffer
            *self.read_buffer.write() = SocketBuffer::new();
            *self.state.write() = SocketState::Unconnected;
            *self.local_addr.write() = None;
            *self.peer_addr.write() = None;
            *self.peer_read_buffer.write() = None;
            *self.peer_socket.write() = None;
            return Err(SocketError::ConnectionRefused);
        }
        server_backlog.push(server_conn);
        drop(server_backlog); // Release lock before waking

        // Wake up any task waiting in accept()
        server_local.accept_waker.wake_one();

        Ok(())
    }

    fn shutdown(&self, how: ShutdownHow) -> Result<(), SocketError> {
        let mut state = self.state.write();
        if *state != SocketState::Connected {
            return Err(SocketError::NotConnected);
        }

        // crate::println!("[LocalSocket] shutdown({:?}) called", how);

        match how {
            ShutdownHow::Read | ShutdownHow::Write | ShutdownHow::Both => {
                *state = SocketState::Closed;

                // Mark peer's read buffer as closed so they detect EOF
                if let Some(peer_buf) = self.peer_read_buffer.read().as_ref() {
                    // crate::println!("[LocalSocket] shutdown: marking peer_read_buffer as closed");
                    *peer_buf.closed.write() = true;
                }

                // Wake up peer's read_waker so it can detect the shutdown
                if let Some(peer_weak) = self.peer_socket.read().as_ref() {
                    if let Some(peer) = peer_weak.upgrade() {
                        // crate::println!("[LocalSocket] shutdown: waking peer's read_waker");
                        peer.read_waker.wake_one();
                    } else {
                        // crate::println!("[LocalSocket] shutdown: peer already dropped");
                    }
                } else {
                    // No direct peer reference - wake via waker
                    // crate::println!(
                    //     "[LocalSocket] shutdown: no peer_socket, waking via read_waker"
                    // );
                    self.read_waker.wake_all(); // Wake any waiting readers
                }

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

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl CloneOps for LocalSocket {
    fn custom_clone(&self) -> KernelObject {
        // Socket cloning creates a new unconnected socket with the same type/protocol
        // This is similar to dup() behavior for sockets in UNIX - creates a new independent socket
        // rather than sharing the connection state
        KernelObject::Socket(Arc::new(LocalSocket::new(self.socket_type, self.protocol)))
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
