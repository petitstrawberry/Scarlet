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
use crate::object::capability::{
    CloneOps, ControlOps, ReadyInterest, ReadySet, SelectWaitOutcome, Selectable, StreamError,
    StreamOps,
};
use crate::sync::Waker;

/// Maximum buffer size per socket (64 KB)
const MAX_BUFFER_SIZE: usize = 65536;

/// Maximum number of handles that can be queued for transfer
/// This prevents unbounded memory growth from DoS attacks
const MAX_HANDLE_QUEUE_SIZE: usize = 64;

/// Shared buffer structure for socket data
struct SocketBuffer {
    /// Data buffer
    data: RwLock<VecDeque<u8>>,
    /// Flag indicating this buffer has been closed (peer shutdown)
    closed: RwLock<bool>,
}

impl SocketBuffer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            data: RwLock::new(VecDeque::with_capacity(MAX_BUFFER_SIZE)),
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

    /// Weak self reference (initialized when wrapped in Arc)
    ///
    /// This is used to establish peer relationships in methods that only
    /// have `&self` (e.g., connect()), where we still need an `Arc<Self>`.
    self_weak: RwLock<Weak<LocalSocket>>,

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

    /// Waker for blocking recv_handle() operations
    handle_waker: Waker,

    /// Queue of handles (KernelObjects) received from peer
    /// This allows passing file descriptors / kernel objects between tasks
    handle_queue: RwLock<VecDeque<KernelObject>>,

    /// Nonblocking I/O flag
    nonblocking: RwLock<bool>,
}

impl LocalSocket {
    pub(crate) fn init_self_weak(this: &Arc<Self>) {
        *this.self_weak.write() = Arc::downgrade(this);
    }

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
            handle_waker: Waker::new_interruptible("socket_handle"),
            handle_queue: RwLock::new(VecDeque::new()),
            self_weak: RwLock::new(Weak::new()),
            nonblocking: RwLock::new(false),
        }
    }

    /// Send a KernelObject handle through this socket
    ///
    /// This is LocalSocket-only (SCM_RIGHTS equivalent) and uses dup() semantics.
    pub fn send_handle(&self, object: KernelObject) -> Result<(), crate::ipc::IpcError> {
        use crate::ipc::IpcError;

        // Verify socket is connected
        if *self.state.read() != SocketState::Connected {
            return Err(IpcError::InvalidState);
        }

        // Get peer socket reference
        let peer_weak = self.peer_socket.read();
        let peer_weak_ref = peer_weak.as_ref().ok_or(IpcError::PeerClosed)?;
        let peer = peer_weak_ref.upgrade().ok_or(IpcError::PeerClosed)?;

        // Check if peer's handle queue is full to prevent DoS attacks
        let mut peer_queue = peer.handle_queue.write();
        if peer_queue.len() >= MAX_HANDLE_QUEUE_SIZE {
            return Err(IpcError::ChannelFull);
        }

        // Add handle to peer's receive queue
        peer_queue.push_back(object);
        drop(peer_queue);

        // Wake one task potentially blocked on recv_handle
        peer.handle_waker.wake_one();

        Ok(())
    }

    /// Receive a KernelObject handle from this socket (non-blocking)
    pub fn recv_handle(&self) -> Result<KernelObject, crate::ipc::IpcError> {
        use crate::ipc::IpcError;

        // Verify socket is connected
        if *self.state.read() != SocketState::Connected {
            return Err(IpcError::InvalidState);
        }

        // Try to get a handle from the queue
        let mut queue = self.handle_queue.write();
        queue.pop_front().ok_or(IpcError::ChannelEmpty)
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
            peer_socket: RwLock::new(None),
            backlog: RwLock::new(Vec::new()),
            max_backlog: RwLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
            read_waker: Waker::new_interruptible("socket_read"),
            handle_waker: Waker::new_interruptible("socket_handle"),
            handle_queue: RwLock::new(VecDeque::new()),
            self_weak: RwLock::new(Weak::new()),
            nonblocking: RwLock::new(false),
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
            peer_socket: RwLock::new(None),
            backlog: RwLock::new(Vec::new()),
            max_backlog: RwLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
            read_waker: Waker::new_interruptible("socket_read"),
            handle_waker: Waker::new_interruptible("socket_handle"),
            handle_queue: RwLock::new(VecDeque::new()),
            self_weak: RwLock::new(Weak::new()),
            nonblocking: RwLock::new(false),
        });

        Self::init_self_weak(&local_socket);
        Self::init_self_weak(&peer_socket);

        // Set peer references
        *local_socket.peer_socket.write() = Some(Arc::downgrade(&peer_socket));
        *peer_socket.peer_socket.write() = Some(Arc::downgrade(&local_socket));

        (local_socket, peer_socket)
    }

    /// Blocking handle receive operation
    ///
    /// This method blocks the calling task until a handle is available in the
    /// handle queue, or the peer is closed.
    pub fn recv_handle_blocking(
        &self,
        task_id: usize,
        trapframe: &mut crate::arch::Trapframe,
    ) -> Result<KernelObject, crate::ipc::IpcError> {
        use crate::ipc::IpcError;

        loop {
            // Verify socket is connected
            {
                let state = self.state.read();
                if *state != SocketState::Connected {
                    return Err(IpcError::InvalidState);
                }
            }

            // Fast path: handle already queued
            {
                let mut queue = self.handle_queue.write();
                if let Some(obj) = queue.pop_front() {
                    return Ok(obj);
                }
            }

            // If peer has shut down (or been dropped), don't block forever.
            // We reuse the same conditions as read_blocking() uses for EOF.
            {
                let peer_weak_opt = self.peer_socket.read();
                if let Some(peer_weak) = peer_weak_opt.as_ref() {
                    if let Some(peer) = peer_weak.upgrade() {
                        let peer_state = peer.state.read();
                        if *peer_state == SocketState::Closed {
                            return Err(IpcError::PeerClosed);
                        }
                    } else {
                        return Err(IpcError::PeerClosed);
                    }
                }
            }

            // If peer performed shutdown(), our read buffer is marked closed.
            {
                let read_buf = self.read_buffer.read();
                let closed = read_buf.closed.read();
                if *closed {
                    return Err(IpcError::PeerClosed);
                }
            }

            // No handle available, block the task
            self.handle_waker.wait(task_id, trapframe);
        }
    }
}

impl StreamOps for LocalSocket {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        use crate::task::mytask;

        // Debug: count read attempts
        static READ_ATTEMPT_COUNTER: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let attempt = READ_ATTEMPT_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        loop {
            {
                let read_buf_arc = self.read_buffer.read();
                let mut read_data = read_buf_arc.data.write();
                let is_nonblocking = *self.nonblocking.read();
                let has_data = !read_data.is_empty();

                // // Log every 100 attempts or first 5 attempts
                // if attempt < 5 || attempt % 100 == 0 {
                //     crate::println!(
                //         "[LocalSocket::read] self={:p} attempt={} nonblocking={} has_data={} data_len={}",
                //         self as *const _,
                //         attempt,
                //         is_nonblocking,
                //         has_data,
                //         read_data.len()
                //     );
                // }

                if !read_data.is_empty() {
                    let bytes_to_read = buffer.len().min(read_data.len());
                    for i in 0..bytes_to_read {
                        buffer[i] = read_data.pop_front().unwrap();
                    }

                    // if attempt < 5 || attempt % 100 == 0 {
                    //     crate::println!(
                    //         "[LocalSocket::read] attempt={} returning {} bytes",
                    //         attempt,
                    //         bytes_to_read
                    //     );
                    // }
                    return Ok(bytes_to_read);
                }
            } // Release locks before checking nonblocking/EOF

            // Check nonblocking mode before blocking
            if *self.nonblocking.read() {
                // // Nonblocking mode: return WouldBlock error immediately
                // if attempt < 5 || attempt % 100 == 0 {
                //     crate::println!(
                //         "[LocalSocket::read] attempt={} returning WouldBlock",
                //         attempt
                //     );
                // }
                return Err(StreamError::WouldBlock);
            }

            {
                let read_buf_arc = self.read_buffer.read();

                // Check if socket is closed (peer shutdown)
                // Return 0 to indicate EOF (not an error)
                let my_state = *self.state.read();
                if my_state == SocketState::Closed {
                    return Ok(0);
                }

                // Check if peer is closed (they called shutdown)
                if let Some(peer_weak) = self.peer_socket.read().as_ref() {
                    if let Some(peer) = peer_weak.upgrade() {
                        let peer_state = *peer.state.read();
                        if peer_state == SocketState::Closed {
                            return Ok(0); // Peer closed, return EOF
                        }
                    } else {
                        return Ok(0); // Peer dropped, treat as EOF
                    }
                }

                // Check if this read buffer has been closed by peer's shutdown()
                if *read_buf_arc.closed.read() {
                    return Ok(0);
                }

                // Register this task as waiting to read
                if let Some(task) = mytask() {
                    drop(read_buf_arc);

                    // Block the task
                    self.read_waker.wait(task.get_id(), task.get_trapframe());
                } else {
                    return Err(StreamError::WouldBlock);
                }
            } // Release lock
            // When woken, loop back to check for data or shutdown
        }
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

                drop(peer_data); // Release data lock

                // Wake tasks waiting on read/select/poll.
                if let Some(peer_weak) = self.peer_socket.read().as_ref() {
                    if let Some(peer) = peer_weak.upgrade() {
                        peer.read_waker.wake_one();
                    }
                }

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
        let state = self.state.read();
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
            handle_waker: Waker::new_interruptible("socket_handle"),
            handle_queue: RwLock::new(VecDeque::new()),
            self_weak: RwLock::new(Weak::new()),
            nonblocking: RwLock::new(false),
        });

        Self::init_self_weak(&server_conn);

        // Update self (client socket) to use the shared buffers
        *self.read_buffer.write() = client_read_buffer.clone();
        *self.peer_read_buffer.write() = Some(server_read_buffer.clone());
        *self.local_addr.write() = Some(local_addr);
        *self.peer_addr.write() = Some(path.to_string());
        *self.state.write() = SocketState::Connected;

        // Set peer_socket references - IMPORTANT for shutdown()
        // Client (self) points to server_conn
        *self.peer_socket.write() = Some(Arc::downgrade(&server_conn));

        // Server_conn needs to point back to client for handle transfer.
        // We keep a Weak<Self> initialized at creation time, so upgrade it here.
        let client_arc = self
            .self_weak
            .read()
            .upgrade()
            .ok_or(SocketError::InvalidOperation)?;
        *server_conn.peer_socket.write() = Some(Arc::downgrade(&client_arc));

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
                        // Also wake any tasks waiting for handle transfer
                        peer.handle_waker.wake_all();
                    } else {
                        // crate::println!("[LocalSocket] shutdown: peer already dropped");
                    }
                } else {
                    // No direct peer reference - wake via waker
                    // crate::println!(
                    //     "[LocalSocket] shutdown: no peer_socket, waking via read_waker"
                    // );
                    self.read_waker.wake_all(); // Wake any waiting readers
                    self.handle_waker.wake_all(); // Wake any waiting handle receivers
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

    fn as_selectable(&self) -> Option<&dyn Selectable> {
        Some(self)
    }

    fn as_control_ops(&self) -> Option<&dyn crate::object::capability::ControlOps> {
        Some(self)
    }
}

impl Selectable for LocalSocket {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut ready = ReadySet::none();

        let state = *self.state.read();

        match state {
            SocketState::Listening => {
                // Listening sockets: readable when backlog has connections
                if interest.read {
                    let backlog = self.backlog.read();
                    ready.read = !backlog.is_empty();
                }
                // Listening sockets are always writable (not applicable)
                if interest.write {
                    ready.write = false;
                }
            }
            SocketState::Connected => {
                // Connected sockets: readable when data available
                if interest.read {
                    let read_buffer = self.read_buffer.read();
                    let data = read_buffer.data.read();
                    let closed = *read_buffer.closed.read();
                    ready.read = !data.is_empty() || closed;
                }
                // Connected sockets: writable when peer buffer not full
                if interest.write {
                    if let Some(peer_buffer) = self.peer_read_buffer.read().as_ref() {
                        let data = peer_buffer.data.read();
                        let closed = *peer_buffer.closed.read();
                        ready.write = data.len() < MAX_BUFFER_SIZE && !closed;
                    } else {
                        ready.write = false;
                    }
                }
            }
            _ => {
                // Unconnected/Bound/other: not ready
                ready.read = false;
                ready.write = false;
            }
        }

        ready
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
    ) -> SelectWaitOutcome {
        // Check if already ready
        let current = self.current_ready(interest);
        if (interest.read && current.read) || (interest.write && current.write) {
            return SelectWaitOutcome::Ready;
        }

        let state = *self.state.read();

        // Get current task ID
        let task_id = {
            use crate::arch::get_cpu;
            use crate::sched::scheduler::get_scheduler;
            let cpu_id = get_cpu().get_cpuid();
            get_scheduler().get_current_task_id(cpu_id).unwrap_or(0)
        };

        // Wait based on state and interest
        // Note: timeout is not yet implemented - always blocks until ready
        match state {
            SocketState::Listening if interest.read => {
                // Wait for incoming connections
                self.accept_waker.wait(task_id, trapframe);
            }
            SocketState::Connected if interest.read => {
                // Wait for data to arrive
                self.read_waker.wait(task_id, trapframe);
            }
            SocketState::Connected if interest.write => {
                // For write readiness, treat as immediately ready (optimistic)
                // Most sockets are writable most of the time
                return SelectWaitOutcome::Ready;
            }
            _ => {
                // Other states: immediately return as not ready
                return SelectWaitOutcome::Ready;
            }
        }

        // After waking, consider it ready
        // TODO: properly check timeout and return TimedOut if needed
        SelectWaitOutcome::Ready
    }

    fn set_nonblocking(&self, enabled: bool) {
        // crate::println!(
        //     "[LocalSocket::set_nonblocking] self={:p} enabled={}",
        //     self as *const _,
        //     enabled
        // );
        *self.nonblocking.write() = enabled;
        let verify = *self.nonblocking.read();
        // crate::println!(
        //     "[LocalSocket::set_nonblocking] self={:p} after write, read back={}",
        //     self as *const _,
        //     verify
        // );
    }

    fn is_nonblocking(&self) -> bool {
        let value = *self.nonblocking.read();
        // crate::println!(
        //     "[LocalSocket::is_nonblocking] self={:p} returning={}",
        //     self as *const _,
        //     value
        // );
        value
    }
}

/// Control command IDs for socket operations
const SOCKET_CMD_SET_NONBLOCKING: u32 = 1;
const SOCKET_CMD_GET_NONBLOCKING: u32 = 2;

impl ControlOps for LocalSocket {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        // crate::println!("[LocalSocket::control] command={} arg={}", command, arg);
        match command {
            SOCKET_CMD_SET_NONBLOCKING => {
                let enabled = arg != 0;
                // crate::println!("[LocalSocket::control] Setting nonblocking={}", enabled);
                self.set_nonblocking(enabled);
                let verify = self.is_nonblocking();
                // crate::println!("[LocalSocket::control] Verified nonblocking={}", verify);
                Ok(0)
            }
            SOCKET_CMD_GET_NONBLOCKING => {
                let is_nonblocking = self.is_nonblocking();
                // crate::println!(
                // "[LocalSocket::control] Getting nonblocking={}",
                // is_nonblocking
                // );
                Ok(if is_nonblocking { 1 } else { 0 })
            }
            _ => {
                crate::println!("[LocalSocket::control] Unknown command");
                Err("Unknown control command")
            }
        }
    }

    fn supported_control_commands(&self) -> alloc::vec::Vec<(u32, &'static str)> {
        alloc::vec![
            (SOCKET_CMD_SET_NONBLOCKING, "Set non-blocking mode"),
            (SOCKET_CMD_GET_NONBLOCKING, "Get non-blocking mode"),
        ]
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

    #[test_case]
    fn test_handle_transfer_send_recv() {
        use crate::ipc::SharedMemory;
        use alloc::sync::Arc;

        // Create a connected socket pair
        let (sock1, sock2) =
            LocalSocket::create_connected_pair("server".to_string(), "client".to_string());

        // Create a shared memory object to transfer
        let shmem = match SharedMemory::new(4096, 0x3) {
            // READ | WRITE
            Ok(shmem) => shmem,
            Err(_) => {
                crate::println!("SharedMemory::new failed, skipping test");
                return;
            }
        };
        let shmem_obj = KernelObject::from_shared_memory_object(Arc::new(shmem));

        // Send handle from sock1 to sock2
        let result = sock1.send_handle(shmem_obj);
        assert!(result.is_ok(), "send_handle should succeed");

        // Receive handle at sock2
        let received = sock2.recv_handle();
        assert!(received.is_ok(), "recv_handle should succeed");

        // Verify it's a SharedMemory object
        let received_obj = received.unwrap();
        assert!(
            received_obj.as_shared_memory().is_some(),
            "Received object should be SharedMemory"
        );
    }

    #[test_case]
    fn test_handle_transfer_multiple_handles() {
        use crate::ipc::SharedMemory;
        use alloc::sync::Arc;

        // Create a connected socket pair
        let (sock1, sock2) =
            LocalSocket::create_connected_pair("server".to_string(), "client".to_string());

        // Send multiple handles
        for i in 0..3 {
            if let Ok(shmem) = SharedMemory::new(4096 * (i + 1), 0x3) {
                let shmem_obj = KernelObject::from_shared_memory_object(Arc::new(shmem));
                assert!(sock1.send_handle(shmem_obj).is_ok());
            }
        }

        // Receive all handles
        for _ in 0..3 {
            let received = sock2.recv_handle();
            assert!(received.is_ok(), "recv_handle should succeed");
            assert!(
                received.unwrap().as_shared_memory().is_some(),
                "Received object should be SharedMemory"
            );
        }

        // Queue should be empty now
        let result = sock2.recv_handle();
        assert!(
            result.is_err(),
            "recv_handle should fail when queue is empty"
        );
    }

    #[test_case]
    fn test_handle_transfer_on_disconnected_socket() {
        use crate::ipc::SharedMemory;
        use alloc::sync::Arc;

        // Create an unconnected socket
        let sock = LocalSocket::new(SocketType::Stream, SocketProtocol::Default);

        // Try to send handle on disconnected socket
        if let Ok(shmem) = SharedMemory::new(4096, 0x3) {
            // READ | WRITE
            let shmem_obj = KernelObject::from_shared_memory_object(Arc::new(shmem));
            let result = sock.send_handle(shmem_obj);
            assert!(
                result.is_err(),
                "send_handle should fail on disconnected socket"
            );
        }

        // Try to receive handle on disconnected socket
        let result = sock.recv_handle();
        assert!(
            result.is_err(),
            "recv_handle should fail on disconnected socket"
        );
    }

    #[test_case]
    fn test_handle_transfer_empty_queue() {
        // Create a connected socket pair
        let (_, sock2) =
            LocalSocket::create_connected_pair("server".to_string(), "client".to_string());

        // Try to receive from empty queue
        let result = sock2.recv_handle();
        assert!(
            result.is_err(),
            "recv_handle should fail when queue is empty"
        );
    }
}
