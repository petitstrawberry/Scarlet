//! Socket System Calls for Scarlet Native
//!
//! This module implements socket system calls specifically for Scarlet Native.
//! Unlike POSIX sockets, these are designed around Scarlet's handle-based architecture.
//!
//! # Design Principles
//!
//! 1. **Handle-based**: Returns handle IDs instead of file descriptors
//! 2. **Scarlet-native**: Uses LocalSocket for IPC, not POSIX Unix domain sockets
//! 3. **Path-based naming**: Sockets are identified by filesystem-like paths
//! 4. **Simple and direct**: Minimal abstraction for kernel IPC
//!
//! # System Call Interface
//!
//! - `sys_socket_create()` - Create a new socket (returns handle ID)
//! - `sys_socket_bind()` - Bind socket to a path
//! - `sys_socket_listen()` - Start listening for connections
//! - `sys_socket_connect()` - Connect to a named socket
//! - `sys_socket_accept()` - Accept an incoming connection (returns new handle)
//! - `sys_socketpair()` - Create a connected socket pair (for IPC)
//! - `sys_socket_shutdown()` - Shutdown socket (read, write, or both)
//!
//! # Usage Example
//!
//! ```rust,ignore
//! // Server side
//! let server_handle = sys_socket_create();
//! sys_socket_bind(server_handle, "/tmp/server.sock");
//! sys_socket_listen(server_handle, 5);
//! let client_handle = sys_socket_accept(server_handle);
//!
//! // Client side
//! let client_handle = sys_socket_create();
//! sys_socket_connect(client_handle, "/tmp/server.sock");
//!
//! // IPC pair (simpler)
//! let [handle1, handle2] = sys_socketpair();
//! ```

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::arch::Trapframe;
use crate::network::{
    Inet4SocketAddress, Ipv4Address, LocalSocketAddress, NetworkManager, ShutdownHow,
    SocketAddress, SocketDomain, SocketObject, SocketProtocol, SocketType, local::LocalSocket,
    tcpip_stack::create_tcp_ip_stack,
};
use crate::object::KernelObject;
use crate::object::handle::{AccessMode, HandleMetadata, HandleType};
use crate::task::mytask;

#[repr(C)]
#[derive(Clone, Copy)]
struct NetworkSetIpv4Request {
    iface_ptr: usize,
    iface_len: usize,
    addr: [u8; 4],
}

fn read_user_string(ptr: usize, len: usize) -> Option<String> {
    let task = mytask()?;
    if len == 0 {
        return None;
    }
    let addr = task.vm_manager.translate_vaddr(ptr)? as *const u8;
    if len > 256 {
        return None;
    }
    let mut bytes = Vec::with_capacity(len);
    unsafe {
        for i in 0..len {
            bytes.push(*addr.add(i));
        }
    }
    String::from_utf8(bytes).ok()
}

fn read_user_ipv4(ptr: usize) -> Option<Ipv4Address> {
    let task = mytask()?;
    let addr = task.vm_manager.translate_vaddr(ptr)? as *const u8;
    unsafe {
        let bytes = [*addr, *addr.add(1), *addr.add(2), *addr.add(3)];
        Some(Ipv4Address::from_bytes(bytes))
    }
}

pub fn sys_network_set_ipv4(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(task);

    let req_ptr = tf.get_arg(0);
    let req_addr = match task.vm_manager.translate_vaddr(req_ptr) {
        Some(addr) => addr as *const NetworkSetIpv4Request,
        None => return usize::MAX,
    };

    let req = unsafe { *req_addr };
    let iface = match read_user_string(req.iface_ptr, req.iface_len) {
        Some(name) => name,
        None => return usize::MAX,
    };
    let ip = Ipv4Address::from_bytes(req.addr);

    if crate::network::get_network_manager()
        .get_interface(&iface)
        .is_none()
    {
        return usize::MAX;
    }

    match crate::network::device_integration::set_interface_ip(&iface, ip) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
}

pub fn sys_network_set_gateway(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(task);

    let addr_ptr = tf.get_arg(0);
    let gateway = match read_user_ipv4(addr_ptr) {
        Some(addr) => addr,
        None => return usize::MAX,
    };
    crate::network::get_network_manager().set_default_gateway(gateway);
    0
}

pub fn sys_network_set_dns(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(task);

    let addr_ptr = tf.get_arg(0);
    let dns = match read_user_ipv4(addr_ptr) {
        Some(addr) => addr,
        None => return usize::MAX,
    };
    let manager = crate::network::get_network_manager();
    let mut config = manager.get_config();
    config.dns_server = Some(dns);
    manager.set_config(config);
    0
}

pub fn sys_network_set_netmask(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(task);

    let addr_ptr = tf.get_arg(0);
    let mask = match read_user_ipv4(addr_ptr) {
        Some(addr) => addr,
        None => return usize::MAX,
    };
    let manager = crate::network::get_network_manager();
    let mut config = manager.get_config();
    config.subnet_mask = mask;
    manager.set_config(config);
    0
}

pub fn sys_network_list_interfaces(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(task);

    let buf_ptr = tf.get_arg(0);
    let buf_len = tf.get_arg(1);
    if buf_ptr == 0 || buf_len == 0 {
        return usize::MAX;
    }

    let buf_addr = match task.vm_manager.translate_vaddr(buf_ptr) {
        Some(addr) => addr as *mut u8,
        None => return usize::MAX,
    };

    let interfaces = crate::network::get_network_manager().list_interfaces();
    let mut output = String::new();
    for (idx, name) in interfaces.iter().enumerate() {
        if idx > 0 {
            output.push('\n');
        }
        output.push_str(name);
    }

    let bytes = output.as_bytes();
    let copy_len = bytes.len().min(buf_len);
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf_addr, copy_len);
    }
    copy_len
}

/// System call: Create a new socket
///
/// Creates a Scarlet Native local socket for IPC.
///
/// # Arguments (via trapframe)
///
/// - `a0`: Socket domain (SocketDomain)
/// - `a1`: Socket type (SocketType)
/// - `a2`: Socket protocol (SocketProtocol)
///
/// # Returns
///
/// Handle ID of the newly created socket (> 0), or error code (usize::MAX for -1).
///
/// # Errors
///
/// Returns usize::MAX (-1) if:
/// - Failed to allocate handle
/// - Internal error creating socket
pub fn sys_socket_create(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    tf.increment_pc_next(task);

    let domain = tf.get_arg(0) as u32;
    let socket_type = tf.get_arg(1) as u32;
    let protocol = tf.get_arg(2) as u32;

    let domain = match domain {
        0 | 1 => SocketDomain::Local,
        2 => SocketDomain::Inet,
        3 => SocketDomain::Inet6,
        _ => return usize::MAX,
    };

    let socket_type = match socket_type {
        0 | 1 => SocketType::Stream,
        2 => SocketType::Datagram,
        3 => SocketType::Raw,
        4 => SocketType::SeqPacket,
        _ => return usize::MAX,
    };

    let protocol = match protocol {
        0 => SocketProtocol::Default,
        1 => SocketProtocol::Icmp,
        6 => SocketProtocol::Tcp,
        17 => SocketProtocol::Udp,
        value => SocketProtocol::Raw(value as u16),
    };

    let protocol = match (socket_type, protocol) {
        (SocketType::Stream, SocketProtocol::Default) => SocketProtocol::Tcp,
        (SocketType::Datagram, SocketProtocol::Default) => SocketProtocol::Udp,
        (SocketType::Raw, SocketProtocol::Default) => SocketProtocol::Raw(0),
        _ => protocol,
    };

    if matches!(domain, SocketDomain::Inet | SocketDomain::Inet6) {
        let _ = create_tcp_ip_stack(domain);
    }

    let socket = match domain {
        SocketDomain::Local => {
            let socket = Arc::new(LocalSocket::new(socket_type, protocol));
            LocalSocket::init_self_weak(&socket);
            socket as Arc<dyn SocketObject>
        }
        SocketDomain::Inet | SocketDomain::Inet6 => {
            let manager = NetworkManager::get_manager();
            let socket = match protocol {
                SocketProtocol::Tcp => manager.get_layer("tcp").map(|layer| {
                    let tcp = layer
                        .as_any()
                        .downcast_ref::<crate::network::tcp::TcpLayer>()
                        .expect("tcp layer type mismatch");
                    tcp.create_socket() as Arc<dyn SocketObject>
                }),
                SocketProtocol::Udp => manager.get_layer("udp").map(|layer| {
                    let udp = layer
                        .as_any()
                        .downcast_ref::<crate::network::udp::UdpLayer>()
                        .expect("udp layer type mismatch");
                    udp.create_socket() as Arc<dyn SocketObject>
                }),
                SocketProtocol::Icmp => manager.get_layer("icmp").map(|layer| {
                    let icmp = layer
                        .as_any()
                        .downcast_ref::<crate::network::icmp::IcmpLayer>()
                        .expect("icmp layer type mismatch");
                    icmp.create_socket() as Arc<dyn SocketObject>
                }),
                _ => None,
            };

            match socket {
                Some(socket) => socket,
                None => return usize::MAX,
            }
        }
        SocketDomain::Packet => return usize::MAX,
    };

    // Register socket with NetworkManager to get a socket ID for VFS integration
    let socket_id = match NetworkManager::get_manager().allocate_socket_id(socket.clone()) {
        Ok(id) => id,
        Err(_) => return usize::MAX,
    };

    // Wrap in KernelObject
    let kernel_obj = KernelObject::Socket(socket);

    // Create metadata for the socket handle
    let metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: AccessMode::ReadWrite,
        special_semantics: None,
    };

    // Add to handle table with metadata
    let handle_id = match task.handle_table.insert_with_metadata(kernel_obj, metadata) {
        Ok(id) => id as usize,
        Err(_) => {
            // Clean up on error
            NetworkManager::get_manager().remove_socket(socket_id);
            return usize::MAX;
        }
    };

    handle_id
}

/// System call: Bind socket to a path
///
/// Binds a socket to a named path in the socket namespace.
/// This allows other processes to connect to this socket by name.
///
/// # Arguments (via trapframe)
///
/// - `a0`: Socket handle ID
/// - `a1`: Pointer to path string (null-terminated)
/// - `a2`: Length of path string (excluding null terminator)
///
/// # Returns
///
/// 0 on success, usize::MAX (-1) on error
///
/// # Errors
///
/// Returns usize::MAX (-1) if:
/// - Invalid handle ID
/// - Invalid path pointer or length
/// - Path already in use
/// - Socket already bound
pub fn sys_socket_bind(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    tf.increment_pc_next(task);

    let handle_id = tf.get_arg(0) as u32;
    let path_ptr = tf.get_arg(1);
    let path_len = tf.get_arg(2);

    // Get the socket from handle table
    let socket_arc = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => return usize::MAX,
    };

    // Translate pointer to physical address
    let path_physical = match task.vm_manager.translate_vaddr(path_ptr) {
        Some(addr) => addr as *const u8,
        None => return usize::MAX,
    };

    if path_len == core::mem::size_of::<Inet4SocketAddress>() {
        let addr = unsafe { *(path_physical as *const Inet4SocketAddress) };
        if socket_arc.bind(&SocketAddress::Inet(addr)).is_err() {
            return usize::MAX;
        }
        return 0;
    }

    // Read path string from user space (up to path_len bytes)
    let path = unsafe {
        let mut bytes = alloc::vec::Vec::with_capacity(path_len.min(108)); // Socket path limit
        for i in 0..path_len.min(108) {
            let byte = *path_physical.add(i);
            if byte == 0 {
                break;
            }
            bytes.push(byte);
        }
        match alloc::string::String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return usize::MAX,
        }
    };

    // Bind the socket to the path
    let local_addr = match LocalSocketAddress::from_path(path.clone()) {
        Ok(addr) => addr,
        Err(_) => return usize::MAX,
    };

    // Bind updates the socket's internal state
    if socket_arc.bind(&SocketAddress::Local(local_addr)).is_err() {
        return usize::MAX;
    }

    // Register the same Arc in NetworkManager's named socket namespace
    // This ensures the registered socket and the one in handle_table are identical
    if NetworkManager::get_manager()
        .register_named_socket(&path, socket_arc.clone())
        .is_err()
    {
        return usize::MAX;
    }

    // Get the socket ID from NetworkManager for VFS integration
    let socket_id = match NetworkManager::get_manager().get_socket_id(&socket_arc) {
        Some(id) => id,
        None => return usize::MAX, // Socket not found in NetworkManager
    };

    // Create socket file in VFS for filesystem visibility
    // Note: This is optional - the socket is already functional via named_sockets
    let vfs = match &task.vfs {
        Some(vfs) => vfs.clone(),
        None => {
            // Use global VFS if task doesn't have its own
            crate::fs::vfs_v2::manager::get_global_vfs_manager()
        }
    };

    let socket_file_type = crate::fs::FileType::Socket(crate::fs::SocketFileInfo { socket_id });

    // Attempt to create the socket file in VFS
    // This may fail if:
    // - Parent directory doesn't exist
    // - File already exists
    // - Path is invalid
    // - Filesystem doesn't support socket files
    // Since the socket is already bound and registered in named_sockets,
    // we treat VFS file creation as optional and don't fail the bind operation
    if let Err(e) = vfs.create_file(&path, socket_file_type) {
        // Log the error for debugging but continue - socket is still usable
        crate::early_println!(
            "[socket_bind] Warning: Failed to create VFS socket file at '{}': {:?}",
            path,
            e
        );
    }

    0
}

/// System call: Listen for connections
///
/// Marks a socket as passive (listening for connections).
///
/// # Arguments (via trapframe)
///
/// - `a0`: Socket handle ID
/// - `a1`: Maximum backlog size (number of pending connections)
///
/// # Returns
///
/// 0 on success, usize::MAX (-1) on error
///
/// # Errors
///
/// Returns usize::MAX (-1) if:
/// - Invalid handle ID
/// - Socket not bound
/// - Socket already listening or connected
pub fn sys_socket_listen(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    tf.increment_pc_next(task);

    let handle_id = tf.get_arg(0) as u32;
    let backlog = tf.get_arg(1);

    // Get the socket from handle table
    let socket = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => {
            crate::println!("[sys_socket_listen] Invalid handle {}", handle_id);
            return usize::MAX;
        }
    };

    // Start listening
    match socket.listen(backlog) {
        Ok(()) => {
            crate::println!("[sys_socket_listen] Socket {} now listening", handle_id);
            0
        }
        Err(e) => {
            crate::println!("[sys_socket_listen] listen() failed: {:?}", e);
            usize::MAX
        }
    }
}

/// System call: Connect to a named socket
///
/// Connects a socket to another socket identified by path.
///
/// # Arguments (via trapframe)
///
/// - `a0`: Socket handle ID
/// - `a1`: Pointer to path string (null-terminated)
/// - `a2`: Length of path string (excluding null terminator)
///
/// # Returns
///
/// 0 on success, usize::MAX (-1) on error
///
/// # Errors
///
/// Returns usize::MAX (-1) if:
/// - Invalid handle ID
/// - Invalid path pointer or length
/// - Target socket not found or not listening
/// - Socket already connected
pub fn sys_socket_connect(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    tf.increment_pc_next(task);

    let handle_id = tf.get_arg(0) as u32;
    let path_ptr = tf.get_arg(1);
    let path_len = tf.get_arg(2);

    // Get the socket from handle table
    let socket = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => return usize::MAX,
    };

    // Translate pointer to physical address
    let path_physical = match task.vm_manager.translate_vaddr(path_ptr) {
        Some(addr) => addr as *const u8,
        None => return usize::MAX,
    };

    if path_len == core::mem::size_of::<Inet4SocketAddress>() {
        let addr = unsafe { *(path_physical as *const Inet4SocketAddress) };
        if socket.connect(&SocketAddress::Inet(addr)).is_err() {
            return usize::MAX;
        }
        return 0;
    }

    // Read path string from user space (up to path_len bytes)
    let path = unsafe {
        let mut bytes = alloc::vec::Vec::with_capacity(path_len.min(108)); // Socket path limit
        for i in 0..path_len.min(108) {
            let byte = *path_physical.add(i);
            if byte == 0 {
                break;
            }
            bytes.push(byte);
        }
        match alloc::string::String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return usize::MAX,
        }
    };

    // Create socket address and connect
    let peer_addr = match LocalSocketAddress::from_path(&path) {
        Ok(addr) => addr,
        Err(_) => return usize::MAX,
    };

    // Connect the socket - this updates its internal state
    if socket.connect(&SocketAddress::Local(peer_addr)).is_err() {
        return usize::MAX;
    }

    0
}

/// System call: Accept an incoming connection
///
/// Accepts a connection from the socket's backlog queue.
/// This blocks if no connections are pending (in a real implementation,
/// should return WouldBlock for non-blocking sockets).
///
/// # Arguments (via trapframe)
///
/// - `a0`: Listening socket handle ID
///
/// # Returns
///
/// Handle ID of the accepted connection socket (> 0), or usize::MAX (-1) on error
///
/// # Errors
///
/// Returns usize::MAX (-1) if:
/// - Invalid handle ID
/// - Socket not in listening state
/// - No pending connections (would block)
/// - Failed to allocate handle for new socket
pub fn sys_socket_accept(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    tf.increment_pc_next(task);

    let handle_id = tf.get_arg(0) as u32;

    // Get the listening socket from handle table
    let socket_obj = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => return usize::MAX,
    };

    // Try to downcast to LocalSocket to access accept_blocking
    use crate::network::local::LocalSocket;

    let local_socket = match LocalSocket::from_socket_object(&socket_obj) {
        Some(socket) => socket,
        None => {
            crate::println!("[sys_socket_accept] Not a LocalSocket");
            return usize::MAX;
        }
    };

    // Accept a connection with blocking
    let accepted_socket = match local_socket.accept_blocking(task.get_id(), tf) {
        Ok(socket) => socket,
        Err(e) => {
            crate::println!("[sys_socket_accept] accept_blocking failed: {:?}", e);
            return usize::MAX;
        }
    };

    // Add the accepted socket to handle table
    let kernel_obj = KernelObject::Socket(accepted_socket);
    let metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: AccessMode::ReadWrite,
        special_semantics: None,
    };

    let new_handle_id = match task.handle_table.insert_with_metadata(kernel_obj, metadata) {
        Ok(id) => id as usize,
        Err(_) => return usize::MAX,
    };

    new_handle_id
}

/// System call: Create a connected socket pair
///
/// Creates two connected local sockets for IPC.
/// This is more efficient than bind/connect for simple bidirectional communication.
///
/// # Arguments (via trapframe)
///
/// - `a0`: Pointer to array[2] for storing handle IDs
///
/// # Returns
///
/// 0 on success, usize::MAX (-1) on error.
/// On success, the handle IDs are written to the array pointed to by a0.
///
/// # Errors
///
/// Returns usize::MAX (-1) if:
/// - Invalid array pointer
/// - Failed to create socket pair
/// - Failed to allocate handles
pub fn sys_socketpair(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    tf.increment_pc_next(task);

    let array_ptr = tf.get_arg(0);

    // Validate pointer (check if we can write 2 usizes = 16 bytes)
    let array_vaddr = match task.vm_manager.translate_vaddr(array_ptr) {
        Some(addr) => addr as *mut usize,
        None => return usize::MAX,
    };

    // Create a connected socket pair using LocalSocket::create_connected_pair
    let (socket1, socket2) = LocalSocket::create_connected_pair(
        String::from("socketpair:0"),
        String::from("socketpair:1"),
    );

    // Add both sockets to handle table
    let kernel_obj1 = KernelObject::Socket(socket1);
    let metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: AccessMode::ReadWrite,
        special_semantics: None,
    };

    let handle1 = match task
        .handle_table
        .insert_with_metadata(kernel_obj1, metadata.clone())
    {
        Ok(id) => id as usize,
        Err(_) => return usize::MAX,
    };

    let kernel_obj2 = KernelObject::Socket(socket2);
    let handle2 = match task
        .handle_table
        .insert_with_metadata(kernel_obj2, metadata)
    {
        Ok(id) => id as usize,
        Err(_) => {
            // Clean up handle1 if handle2 allocation fails
            let _ = task.handle_table.remove(handle1 as u32);
            return usize::MAX;
        }
    };

    // Write handle IDs to user space array
    unsafe {
        array_vaddr.write(handle1);
        array_vaddr.add(1).write(handle2);
    }

    0
}

/// System call: Shutdown socket
///
/// Shuts down part or all of a socket connection.
///
/// # Arguments (via trapframe)
///
/// - `a0`: Socket handle ID
/// - `a1`: How to shutdown (0 = read, 1 = write, 2 = both)
///
/// # Returns
///
/// 0 on success, usize::MAX (-1) on error
///
/// # Errors
///
/// Returns usize::MAX (-1) if:
/// - Invalid handle ID
/// - Invalid shutdown mode
/// - Socket not connected
pub fn sys_socket_shutdown(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    tf.increment_pc_next(task);

    let handle_id = tf.get_arg(0) as u32;
    let how_value = tf.get_arg(1);

    // Get the socket from handle table
    let socket = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => return usize::MAX,
    };

    // Parse shutdown mode
    let how = match how_value {
        0 => ShutdownHow::Read,
        1 => ShutdownHow::Write,
        2 => ShutdownHow::Both,
        _ => return usize::MAX,
    };

    // Shutdown the socket
    if socket.shutdown(how).is_err() {
        return usize::MAX;
    }

    0
}
