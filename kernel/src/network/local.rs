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

use crate::sync::IrqRwSpinLock;
use alloc::{
    collections::VecDeque,
    format,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;

use crate::sched::scheduler::current_task_id;

use super::{
    LocalSocketAddress, NetworkManager, ShutdownHow, SocketAddress, SocketControl, SocketDomain,
    SocketError, SocketObject, SocketProtocol, SocketState, SocketType,
};
use crate::ipc::StreamIpcOps;
use crate::object::KernelObject;
use crate::object::capability::{
    ControlOps, ReadyInterest, ReadySet, SelectWaitOutcome, Selectable, StreamError, StreamOps,
};
use crate::sync::Waker;

const LOCALSOCKET_LOG: bool = false;

macro_rules! localsocket_log {
    ($($arg:tt)*) => {
        if LOCALSOCKET_LOG {
            crate::println!($($arg)*);
        }
    };
}

/// Maximum buffer size per socket (64 KB)
const MAX_BUFFER_SIZE: usize = 65536;

/// Maximum number of handles that can be queued for transfer
/// This prevents unbounded memory growth from DoS attacks
const MAX_HANDLE_QUEUE_SIZE: usize = 64;

fn local_socket_registry_name(addr: &LocalSocketAddress) -> String {
    if addr.is_abstract() {
        let mut name = String::new();
        name.push('\0');
        name.push_str(addr.path());
        name
    } else {
        addr.path().to_string()
    }
}

fn local_socket_address_from_registry_name(name: &str) -> LocalSocketAddress {
    if let Some(abstract_name) = name.strip_prefix('\0') {
        LocalSocketAddress::from_abstract(abstract_name)
            .unwrap_or_else(|_| LocalSocketAddress::unnamed())
    } else {
        LocalSocketAddress::from_path(name).unwrap_or_else(|_| LocalSocketAddress::unnamed())
    }
}

/// Shared buffer structure for socket data
struct SocketBuffer {
    /// Data buffer
    data: IrqRwSpinLock<VecDeque<u8>>,
    /// Flag indicating this buffer has been closed (peer shutdown)
    closed: IrqRwSpinLock<bool>,
}

impl SocketBuffer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            data: IrqRwSpinLock::new(VecDeque::with_capacity(MAX_BUFFER_SIZE)),
            closed: IrqRwSpinLock::new(false),
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
    self_weak: IrqRwSpinLock<Weak<LocalSocket>>,

    /// Socket protocol
    protocol: SocketProtocol,

    /// Current socket state
    state: IrqRwSpinLock<SocketState>,

    /// Local address (if bound)
    local_addr: IrqRwSpinLock<Option<String>>,

    /// Peer address (if connected)
    peer_addr: IrqRwSpinLock<Option<String>>,

    /// Read buffer: data received from peer (shared with peer for writing)
    read_buffer: IrqRwSpinLock<Arc<SocketBuffer>>,

    /// Write buffer reference: shared with peer socket for writing
    /// When we write, we push to peer's read_buffer
    peer_read_buffer: IrqRwSpinLock<Option<Arc<SocketBuffer>>>,

    /// Peer socket reference (for waking read waiters)
    peer_socket: IrqRwSpinLock<Option<Weak<LocalSocket>>>,

    /// Backlog queue for listening sockets
    /// Contains pending connections waiting to be accepted
    backlog: IrqRwSpinLock<Vec<Arc<LocalSocket>>>,

    /// Maximum backlog size (set by listen())
    max_backlog: IrqRwSpinLock<usize>,

    /// Waker for blocking accept() operations
    accept_waker: Waker,

    /// Waker for blocking read() operations
    read_waker: Waker,

    /// Waker for blocking recv_handle() operations
    handle_waker: Waker,

    /// Queue of handles (KernelObjects) received from peer
    /// This allows passing file descriptors / kernel objects between tasks
    handle_queue: IrqRwSpinLock<VecDeque<KernelObject>>,

    /// Nonblocking I/O flag
    nonblocking: IrqRwSpinLock<bool>,
}

impl LocalSocket {
    pub(crate) fn init_self_weak(this: &Arc<Self>) {
        *this.self_weak.write() = Arc::downgrade(this);
    }

    /// Safely downcast a SocketObject to LocalSocket using Any trait
    ///
    /// Returns None if the socket is not a LocalSocket.
    /// This is completely safe and does not use any unsafe code.
    pub fn from_socket_object(socket: &dyn SocketObject) -> Option<&Self> {
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
            state: IrqRwSpinLock::new(SocketState::Unconnected),
            local_addr: IrqRwSpinLock::new(None),
            peer_addr: IrqRwSpinLock::new(None),
            read_buffer: IrqRwSpinLock::new(SocketBuffer::new()),
            peer_read_buffer: IrqRwSpinLock::new(None),
            peer_socket: IrqRwSpinLock::new(None),
            backlog: IrqRwSpinLock::new(Vec::new()),
            max_backlog: IrqRwSpinLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
            read_waker: Waker::new_interruptible("socket_read"),
            handle_waker: Waker::new_interruptible("socket_handle"),
            handle_queue: IrqRwSpinLock::new(VecDeque::new()),
            self_weak: IrqRwSpinLock::new(Weak::new()),
            nonblocking: IrqRwSpinLock::new(false),
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

    /// Send a handle and data together atomically for Wayland protocol
    ///
    /// This method ensures that both the handle and data are available before
    /// waking the peer, preventing race conditions where recvmsg might get
    /// the handle but not the data (or vice versa).
    ///
    /// This is needed for Wayland protocol messages with file descriptors,
    /// where the client expects both the FD and message data in a single recvmsg call.
    pub fn send_handle_and_data(
        &self,
        object: KernelObject,
        data: &[u8],
    ) -> Result<(), crate::ipc::IpcError> {
        use crate::ipc::IpcError;

        localsocket_log!(
            "[LocalSocket] send_handle_and_data: self={:p}, data_len={}",
            self as *const _,
            data.len()
        );

        // Verify socket is connected
        if *self.state.read() != SocketState::Connected {
            localsocket_log!("[LocalSocket] send_handle_and_data: not connected");
            return Err(IpcError::InvalidState);
        }

        // Get peer socket reference
        let peer_weak = self.peer_socket.read();
        let peer_weak_ref = peer_weak.as_ref().ok_or(IpcError::PeerClosed)?;
        let peer = peer_weak_ref.upgrade().ok_or(IpcError::PeerClosed)?;

        localsocket_log!(
            "[LocalSocket] send_handle_and_data: peer={:p}",
            peer.as_ref() as *const _
        );

        // Check if peer's handle queue is full to prevent DoS attacks
        let mut peer_handle_queue = peer.handle_queue.write();
        if peer_handle_queue.len() >= MAX_HANDLE_QUEUE_SIZE {
            localsocket_log!("[LocalSocket] send_handle_and_data: handle queue full");
            return Err(IpcError::ChannelFull);
        }

        // Get peer's data buffer through peer_read_buffer
        let peer_buffer_option = peer.peer_read_buffer.read();
        let peer_sock_buffer = peer_buffer_option.as_ref().ok_or(IpcError::PeerClosed)?;

        // Check if peer's data buffer has space
        let mut peer_buffer = peer_sock_buffer.data.write();
        if peer_buffer.len() + data.len() > MAX_BUFFER_SIZE {
            localsocket_log!(
                "[LocalSocket] send_handle_and_data: buffer full, current_len={}, adding_len={}",
                peer_buffer.len(),
                data.len()
            );
            drop(peer_buffer);
            drop(peer_buffer_option);
            drop(peer_handle_queue);
            return Err(IpcError::ChannelFull);
        }

        localsocket_log!(
            "[LocalSocket] send_handle_and_data: before send - handle_queue_len={}, buffer_len={}",
            peer_handle_queue.len(),
            peer_buffer.len()
        );

        // Add handle to peer's receive queue
        peer_handle_queue.push_back(object);
        let queue_len = peer_handle_queue.len();
        drop(peer_handle_queue);

        // Add data to peer's buffer
        peer_buffer.extend(data.iter().copied());
        let buffer_len = peer_buffer.len();
        drop(peer_buffer);
        drop(peer_buffer_option);

        localsocket_log!(
            "[LocalSocket] send_handle_and_data: after send - handle_queue_len={}, buffer_len={}",
            queue_len,
            buffer_len
        );

        // Wake the peer after BOTH handle and data are available
        peer.handle_waker.wake_one();
        peer.read_waker.wake_one();

        Ok(())
    }

    /// Receive a handle and data together atomically for Wayland protocol
    ///
    /// Returns both a handle and data in a single atomic operation.
    /// This is the counterpart to send_handle_and_data().
    ///
    /// # Arguments
    ///
    /// * `max_data_len` - Maximum amount of data to read
    ///
    /// # Returns
    ///
    /// * `(KernelObject, Vec<u8>)` - Handle and data on success
    /// * `IpcError` - Error if no handle/data available or other error
    pub fn recv_handle_and_data(
        &self,
        max_data_len: usize,
    ) -> Result<(KernelObject, Vec<u8>), crate::ipc::IpcError> {
        use crate::ipc::IpcError;

        localsocket_log!(
            "[LocalSocket] recv_handle_and_data: self={:p}, max_data_len={}",
            self as *const _,
            max_data_len
        );

        // Verify socket is connected
        if *self.state.read() != SocketState::Connected {
            localsocket_log!("[LocalSocket] recv_handle_and_data: not connected");
            return Err(IpcError::InvalidState);
        }

        // Try to get a handle from the queue
        let mut queue = self.handle_queue.write();
        localsocket_log!(
            "[LocalSocket] recv_handle_and_data: handle_queue_len={}",
            queue.len()
        );

        let handle = match queue.pop_front() {
            Some(h) => h,
            None => {
                localsocket_log!(
                    "[LocalSocket] recv_handle_and_data: handle queue empty - returning ChannelEmpty"
                );
                return Err(IpcError::ChannelEmpty);
            }
        };
        drop(queue);

        // Read data from read buffer
        let read_buffer = self.read_buffer.read();
        let mut buffer_data = read_buffer.data.write();
        localsocket_log!(
            "[LocalSocket] recv_handle_and_data: buffer_len={}, max_data_len={}",
            buffer_data.len(),
            max_data_len
        );

        // Read up to max_data_len bytes
        let actual_len = buffer_data.len().min(max_data_len);
        let mut data = Vec::with_capacity(actual_len);
        for _ in 0..actual_len {
            data.push(buffer_data.pop_front().unwrap());
        }
        drop(buffer_data);
        drop(read_buffer);

        localsocket_log!(
            "[LocalSocket] recv_handle_and_data: returning handle and {} bytes of data",
            data.len()
        );

        Ok((handle, data))
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
            state: IrqRwSpinLock::new(SocketState::Connected),
            local_addr: IrqRwSpinLock::new(Some(local_addr.clone())),
            peer_addr: IrqRwSpinLock::new(Some(peer_addr.clone())),
            read_buffer: IrqRwSpinLock::new(local_read_buffer.clone()),
            peer_read_buffer: IrqRwSpinLock::new(Some(peer_read_buffer.clone())),
            peer_socket: IrqRwSpinLock::new(None),
            backlog: IrqRwSpinLock::new(Vec::new()),
            max_backlog: IrqRwSpinLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
            read_waker: Waker::new_interruptible("socket_read"),
            handle_waker: Waker::new_interruptible("socket_handle"),
            handle_queue: IrqRwSpinLock::new(VecDeque::new()),
            self_weak: IrqRwSpinLock::new(Weak::new()),
            nonblocking: IrqRwSpinLock::new(false),
        });

        // Create peer socket (client side)
        // It reads from peer_read_buffer, writes to local_read_buffer
        let peer_socket = Arc::new(Self {
            socket_type: SocketType::Stream,
            protocol: SocketProtocol::Default,
            state: IrqRwSpinLock::new(SocketState::Connected),
            local_addr: IrqRwSpinLock::new(Some(peer_addr)),
            peer_addr: IrqRwSpinLock::new(Some(local_addr)),
            read_buffer: IrqRwSpinLock::new(peer_read_buffer.clone()),
            peer_read_buffer: IrqRwSpinLock::new(Some(local_read_buffer.clone())),
            peer_socket: IrqRwSpinLock::new(None),
            backlog: IrqRwSpinLock::new(Vec::new()),
            max_backlog: IrqRwSpinLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
            read_waker: Waker::new_interruptible("socket_read"),
            handle_waker: Waker::new_interruptible("socket_handle"),
            handle_queue: IrqRwSpinLock::new(VecDeque::new()),
            self_weak: IrqRwSpinLock::new(Weak::new()),
            nonblocking: IrqRwSpinLock::new(false),
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

        loop {
            {
                let read_buf_arc = self.read_buffer.read();
                let mut read_data = read_buf_arc.data.write();

                if !read_data.is_empty() {
                    let bytes_to_read = buffer.len().min(read_data.len());
                    for i in 0..bytes_to_read {
                        buffer[i] = read_data.pop_front().unwrap();
                    }

                    return Ok(bytes_to_read);
                }
            } // Release locks before checking nonblocking/EOF

            {
                let read_buf_arc = self.read_buffer.read();

                // Check EOF before honoring non-blocking mode. A disconnected peer is
                // a completed read condition, not WouldBlock.
                let my_state = *self.state.read();
                if my_state == SocketState::Closed {
                    return Ok(0);
                }

                if my_state == SocketState::Connected {
                    let peer_closed = match self.peer_socket.read().as_ref() {
                        Some(peer_weak) => match peer_weak.upgrade() {
                            Some(peer) => *peer.state.read() == SocketState::Closed,
                            None => true,
                        },
                        None => true,
                    };
                    if peer_closed {
                        return Ok(0);
                    }
                }

                // Check if this read buffer has been closed by peer's shutdown/drop.
                if *read_buf_arc.closed.read() {
                    return Ok(0);
                }
            }

            // Check nonblocking mode before blocking.
            if *self.nonblocking.read() {
                return Err(StreamError::WouldBlock);
            }

            {
                let read_buf_arc = self.read_buffer.read();

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
        if *self.state.read() == SocketState::Closed {
            return Err(StreamError::Closed);
        }

        let peer_buffer = self.peer_read_buffer.read();
        match peer_buffer.as_ref() {
            Some(peer_sock_buffer) => {
                if *peer_sock_buffer.closed.read() {
                    return Err(StreamError::Closed);
                }

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

impl Drop for LocalSocket {
    fn drop(&mut self) {
        let state = *self.state.read();

        if matches!(state, SocketState::Bound | SocketState::Listening)
            && let Some(path) = self.local_addr.read().as_ref()
            && !path.is_empty()
        {
            NetworkManager::get_manager().unregister_named_socket(path);
        }

        if let Some(peer_buf) = self.peer_read_buffer.read().as_ref() {
            *peer_buf.closed.write() = true;
        }

        if let Some(peer_weak) = self.peer_socket.read().as_ref()
            && let Some(peer) = peer_weak.upgrade()
        {
            *peer.peer_read_buffer.write() = None;
            *peer.peer_socket.write() = None;
            peer.read_waker.wake_all();
            peer.handle_waker.wake_all();
        }

        self.accept_waker.wake_all();
        self.read_waker.wake_all();
        self.handle_waker.wake_all();
        *self.state.write() = SocketState::Closed;

        NetworkManager::get_manager().remove_socket_by_ptr(self as *const Self as usize);
    }
}

impl SocketControl for LocalSocket {
    fn bind(&self, address: &SocketAddress) -> Result<(), SocketError> {
        // Check socket is unconnected
        let mut state = self.state.write();
        if *state != SocketState::Unconnected {
            return Err(SocketError::AlreadyConnected);
        }

        // Extract registry name from address.
        let name = match address {
            SocketAddress::Local(addr) => local_socket_registry_name(addr),
            _ => return Err(SocketError::InvalidAddress),
        };

        // Update state
        // Note: NetworkManager registration is done by the syscall layer
        // to ensure the same Arc<Self> is registered that's in the handle table
        *self.local_addr.write() = Some(name);
        *state = SocketState::Bound;

        Ok(())
    }

    fn listen(&self, backlog: usize) -> Result<(), SocketError> {
        let mut state = self.state.write();
        if *state != SocketState::Bound {
            return Err(SocketError::InvalidOperation);
        }

        // Some Linux applications pass backlog=0 and still expect at least one
        // pending connection to be accepted. Keep the internal queue usable.
        *self.max_backlog.write() = backlog.max(1);
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

        // Extract registry name from address.
        let name = match address {
            SocketAddress::Local(addr) => local_socket_registry_name(addr),
            _ => return Err(SocketError::InvalidAddress),
        };

        // Lookup listening socket in NetworkManager
        let manager = NetworkManager::get_manager();
        let server_socket = match manager.lookup_named_socket(&name) {
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
            state: IrqRwSpinLock::new(SocketState::Connected),
            local_addr: IrqRwSpinLock::new(Some(name.clone())),
            peer_addr: IrqRwSpinLock::new(Some(local_addr.clone())),
            read_buffer: IrqRwSpinLock::new(server_read_buffer.clone()),
            peer_read_buffer: IrqRwSpinLock::new(Some(client_read_buffer.clone())),
            peer_socket: IrqRwSpinLock::new(None), // Will be set below
            backlog: IrqRwSpinLock::new(Vec::new()),
            max_backlog: IrqRwSpinLock::new(0),
            accept_waker: Waker::new_interruptible("socket_accept"),
            read_waker: Waker::new_interruptible("socket_read"),
            handle_waker: Waker::new_interruptible("socket_handle"),
            handle_queue: IrqRwSpinLock::new(VecDeque::new()),
            self_weak: IrqRwSpinLock::new(Weak::new()),
            nonblocking: IrqRwSpinLock::new(false),
        });

        Self::init_self_weak(&server_conn);

        // Update self (client socket) to use the shared buffers
        *self.read_buffer.write() = client_read_buffer.clone();
        *self.peer_read_buffer.write() = Some(server_read_buffer.clone());
        *self.local_addr.write() = Some(local_addr);
        *self.peer_addr.write() = Some(name.clone());
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
        let server_local = match Self::from_socket_object(server_socket.as_ref()) {
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
            ShutdownHow::Read => {
                *self.read_buffer.read().closed.write() = true;
                self.read_waker.wake_all();
                self.handle_waker.wake_all();
                Ok(())
            }
            ShutdownHow::Write => {
                // Mark peer's read buffer as closed so they detect EOF, while
                // keeping our read side open for the peer's response.
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
            ShutdownHow::Both => {
                *state = SocketState::Closed;
                *self.read_buffer.read().closed.write() = true;

                if let Some(peer_buf) = self.peer_read_buffer.read().as_ref() {
                    *peer_buf.closed.write() = true;
                }

                if let Some(peer_weak) = self.peer_socket.read().as_ref()
                    && let Some(peer) = peer_weak.upgrade()
                {
                    peer.read_waker.wake_one();
                    peer.handle_waker.wake_all();
                }
                self.read_waker.wake_all();
                self.handle_waker.wake_all();

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
                local_socket_address_from_registry_name(path),
            )),
            None => Err(SocketError::NotConnected),
        }
    }

    fn getsockname(&self) -> Result<SocketAddress, SocketError> {
        let local = self.local_addr.read();
        match local.as_ref() {
            Some(path) => Ok(SocketAddress::Local(
                local_socket_address_from_registry_name(path),
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
        timeout_ticks: Option<u64>,
        min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        let current = self.current_ready(interest);
        if (interest.read && current.read) || (interest.write && current.write) {
            return SelectWaitOutcome::Ready;
        }

        let state = *self.state.read();

        let task_id = {
            use crate::arch::get_cpu;
            let cpu_id = get_cpu().get_cpuid();
            current_task_id(cpu_id).unwrap_or(0)
        };

        let woke = match state {
            SocketState::Listening if interest.read => {
                if min_wait_ticks > 0 {
                    self.accept_waker.wait_with_min_timeout(
                        task_id,
                        trapframe,
                        timeout_ticks,
                        min_wait_ticks,
                    )
                } else {
                    self.accept_waker
                        .wait_with_timeout(task_id, trapframe, timeout_ticks)
                }
            }
            SocketState::Connected if interest.read => {
                if min_wait_ticks > 0 {
                    self.read_waker.wait_with_min_timeout(
                        task_id,
                        trapframe,
                        timeout_ticks,
                        min_wait_ticks,
                    )
                } else {
                    self.read_waker
                        .wait_with_timeout(task_id, trapframe, timeout_ticks)
                }
            }
            SocketState::Connected if interest.write => {
                let write_closed = match self.peer_read_buffer.read().as_ref() {
                    Some(peer_buffer) => *peer_buffer.closed.read(),
                    None => true,
                };
                return if write_closed {
                    SelectWaitOutcome::TimedOut
                } else {
                    SelectWaitOutcome::Ready
                };
            }
            _ => true,
        };

        if timeout_ticks.is_some() && !woke {
            let after = self.current_ready(interest);
            if (interest.read && !after.read) && (interest.write && !after.write) {
                return SelectWaitOutcome::TimedOut;
            }
        }

        SelectWaitOutcome::Ready
    }

    fn set_nonblocking(&self, enabled: bool) {
        *self.nonblocking.write() = enabled;
    }

    fn is_nonblocking(&self) -> bool {
        *self.nonblocking.read()
    }
}

impl ControlOps for LocalSocket {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            crate::network::socket::socket_ctl::SCTL_SOCKET_SET_NONBLOCK => {
                let enabled = arg != 0;
                self.set_nonblocking(enabled);
                Ok(0)
            }
            crate::network::socket::socket_ctl::SCTL_SOCKET_GET_NONBLOCK => {
                let is_nonblocking = self.is_nonblocking();
                Ok(if is_nonblocking { 1 } else { 0 })
            }
            _ => {
                localsocket_log!("[LocalSocket::control] Unknown command");
                Err("Unknown control command")
            }
        }
    }

    fn supported_control_commands(&self) -> alloc::vec::Vec<(u32, &'static str)> {
        alloc::vec![
            (
                crate::network::socket::socket_ctl::SCTL_SOCKET_SET_NONBLOCK,
                "Set non-blocking mode",
            ),
            (
                crate::network::socket::socket_ctl::SCTL_SOCKET_GET_NONBLOCK,
                "Get non-blocking mode",
            ),
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
    fn test_peer_observes_close_when_socket_is_dropped() {
        let (sock1, sock2) =
            LocalSocket::create_connected_pair("server".to_string(), "client".to_string());

        drop(sock1);

        let mut buffer = [0u8; 8];
        let read = sock2.read(&mut buffer).unwrap();
        assert_eq!(read, 0, "peer read should observe EOF");
        assert!(
            sock2.write(b"closed").is_err(),
            "peer write should fail after remote drop"
        );
    }

    #[test_case]
    fn test_shutdown_write_rejects_later_writes() {
        let (sock1, sock2) =
            LocalSocket::create_connected_pair("server".to_string(), "client".to_string());

        sock1.shutdown(ShutdownHow::Write).unwrap();

        let mut buffer = [0u8; 8];
        let read = sock2.read(&mut buffer).unwrap();
        assert_eq!(read, 0, "peer read should observe EOF after SHUT_WR");
        assert!(
            sock1.write(b"after-shutdown").is_err(),
            "write should fail after SHUT_WR"
        );
        assert!(
            !sock1
                .current_ready(ReadyInterest {
                    read: false,
                    write: true,
                    except: false,
                })
                .write,
            "socket should not report writable after SHUT_WR"
        );
    }

    #[test_case]
    fn test_nonblocking_peer_drop_returns_eof() {
        let (sock1, sock2) =
            LocalSocket::create_connected_pair("server".to_string(), "client".to_string());

        sock2.set_nonblocking(true);
        drop(sock1);

        let mut buffer = [0u8; 8];
        let read = sock2.read(&mut buffer).unwrap();
        assert_eq!(read, 0, "non-blocking peer read should observe EOF");
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
