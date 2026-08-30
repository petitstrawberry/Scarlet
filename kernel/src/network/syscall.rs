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
use alloc::vec;
use alloc::vec::Vec;

use crate::arch::Trapframe;
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::network::{
    Inet4SocketAddress, Ipv4Address, LocalSocketAddress, NetworkManager, ShutdownHow,
    SocketAddress, SocketDomain, SocketObject, SocketProtocol, SocketType, local::LocalSocket,
};
use crate::object::KernelObject;
use crate::object::handle::{AccessMode, HandleMetadata, HandleType};
use crate::task::mytask;

fn local_socket_address_from_user_bytes(
    bytes: &[u8],
) -> Result<(LocalSocketAddress, String, bool), ()> {
    if bytes.is_empty() {
        return Err(());
    }

    if bytes[0] == 0 {
        let mut name_len = bytes.len().saturating_sub(1);
        while name_len > 0 && bytes[1 + name_len - 1] == 0 {
            name_len -= 1;
        }
        if name_len == 0 {
            return Err(());
        }
        let name = core::str::from_utf8(&bytes[1..1 + name_len]).map_err(|_| ())?;
        let addr = LocalSocketAddress::from_abstract(name).map_err(|_| ())?;
        let mut registry_name = String::new();
        registry_name.push('\0');
        registry_name.push_str(addr.path());
        Ok((addr, registry_name, true))
    } else {
        let mut path_len = 0;
        while path_len < bytes.len() && bytes[path_len] != 0 {
            path_len += 1;
        }
        if path_len == 0 {
            return Err(());
        }
        let path = core::str::from_utf8(&bytes[..path_len]).map_err(|_| ())?;
        let addr = LocalSocketAddress::from_path(path).map_err(|_| ())?;
        Ok((addr, String::from(path), false))
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NetworkSetIpv4Request {
    iface_ptr: usize,
    iface_len: usize,
    addr: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NetworkInterfaceInfo {
    name: [u8; 32],
    ip_address: [u8; 4],
    mac_address: [u8; 6],
    ip_set: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NetworkStatus {
    gateway: [u8; 4],
    gateway_set: u8,
    netmask: [u8; 4],
    interface_count: u32,
    interfaces_ptr: usize,
}

const NETWORK_CONFIGURE_HAS_GATEWAY: u32 = 1 << 0;
const NETWORK_CONFIGURE_MAKE_DEFAULT: u32 = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct NetworkConfigureIpv4Request {
    iface_ptr: usize,
    iface_len: usize,
    address: [u8; 4],
    netmask: [u8; 4],
    gateway: [u8; 4],
    flags: u32,
    metric: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NetworkInterfaceInfoV2 {
    name: [u8; 32],
    ip_address: [u8; 4],
    netmask: [u8; 4],
    gateway: [u8; 4],
    mac_address: [u8; 6],
    ip_set: u8,
    gateway_set: u8,
    is_default: u8,
    reserved: [u8; 3],
    metric: u32,
}

const _: [(); 40] = [(); core::mem::size_of::<NetworkConfigureIpv4Request>()];
const _: [(); 60] = [(); core::mem::size_of::<NetworkInterfaceInfoV2>()];

fn read_user_string(ptr: usize, len: usize) -> Option<String> {
    let task = mytask()?;
    if len == 0 {
        return None;
    }
    if len > 256 {
        return None;
    }
    let mut bytes = Vec::with_capacity(len);
    bytes.resize(len, 0);
    if copy_from_user(&task, ptr, &mut bytes).is_err() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn read_user_ipv4(ptr: usize) -> Option<Ipv4Address> {
    let task = mytask()?;
    let mut bytes = [0u8; 4];
    if copy_from_user(&task, ptr, &mut bytes).is_err() {
        None
    } else {
        Some(Ipv4Address::from_bytes(bytes))
    }
}

pub fn sys_network_set_ipv4(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(&task);

    let req_ptr = tf.get_arg(0);
    let mut req_bytes = [0u8; core::mem::size_of::<NetworkSetIpv4Request>()];
    if copy_from_user(&task, req_ptr, &mut req_bytes).is_err() {
        return usize::MAX;
    }
    let req =
        unsafe { core::ptr::read_unaligned(req_bytes.as_ptr() as *const NetworkSetIpv4Request) };
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

    match crate::network::config::set_interface_ip(&iface, ip) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
}

pub fn sys_network_set_gateway(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(&task);

    let addr_ptr = tf.get_arg(0);
    let gateway = match read_user_ipv4(addr_ptr) {
        Some(addr) => addr,
        None => return usize::MAX,
    };
    crate::network::get_network_manager().set_default_gateway(gateway);
    0
}

pub fn sys_network_set_netmask(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(&task);

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
    tf.increment_pc_next(&task);

    let status_ptr = tf.get_arg(0);
    let interfaces_ptr = tf.get_arg(1);
    let max_interfaces = tf.get_arg(2);

    if status_ptr == 0 || interfaces_ptr == 0 || max_interfaces == 0 {
        return usize::MAX;
    }

    let network_manager = crate::network::get_network_manager();
    let interface_names = network_manager.list_interfaces();
    let config = network_manager.get_config();

    let mut status = NetworkStatus {
        gateway: config.default_gateway.map_or([0u8; 4], |ip| ip.as_bytes()),
        gateway_set: config.default_gateway.map_or(0, |_| 1),
        netmask: config.subnet_mask.as_bytes(),
        interface_count: 0,
        interfaces_ptr: interfaces_ptr as usize,
    };

    let mut interfaces = Vec::new();
    for name in &interface_names {
        if interfaces.len() >= max_interfaces as usize {
            break;
        }

        if let Some(iface) = network_manager.get_interface(name) {
            let ip = iface.ip_address().map_or([0u8; 4], |ip| ip.as_bytes());
            let mac = iface.mac_address().clone();

            let mut name_buf = [0u8; 32];
            let name_bytes = name.as_bytes();
            let copy_len = name_bytes.len().min(name_buf.len() - 1);
            name_buf[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

            interfaces.push(NetworkInterfaceInfo {
                name: name_buf,
                ip_address: ip,
                mac_address: *mac.as_bytes(),
                ip_set: iface.ip_address().map_or(0, |_| 1),
            });
        }
    }

    status.interface_count = interfaces.len() as u32;

    let status_bytes = unsafe {
        core::slice::from_raw_parts(
            (&status as *const NetworkStatus).cast::<u8>(),
            core::mem::size_of::<NetworkStatus>(),
        )
    };
    if copy_to_user(&task, status_ptr, status_bytes).is_err() {
        return usize::MAX;
    }

    if !interfaces.is_empty() {
        let item_size = core::mem::size_of::<NetworkInterfaceInfo>();
        for (idx, info) in interfaces.iter().enumerate() {
            let info_bytes = unsafe {
                core::slice::from_raw_parts(
                    (info as *const NetworkInterfaceInfo).cast::<u8>(),
                    item_size,
                )
            };
            if copy_to_user(&task, interfaces_ptr + idx * item_size, info_bytes).is_err() {
                return usize::MAX;
            }
        }
    }

    0
}

/// Configure an interface's IPv4 address, netmask, and default route.
///
/// # Arguments
///
/// The first trapframe argument points to a [`NetworkConfigureIpv4Request`]
/// in user memory.
///
/// # Returns
///
/// Zero on success, or `usize::MAX` if validation or configuration fails.
pub fn sys_network_configure_ipv4(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(&task);

    let request_ptr = tf.get_arg(0);
    let mut request_bytes = [0u8; core::mem::size_of::<NetworkConfigureIpv4Request>()];
    if copy_from_user(&task, request_ptr, &mut request_bytes).is_err() {
        return usize::MAX;
    }
    // SAFETY: `request_bytes` has exactly the size of the fixed-layout request,
    // and `read_unaligned` does not require the byte array to share its alignment.
    let request = unsafe {
        core::ptr::read_unaligned(request_bytes.as_ptr() as *const NetworkConfigureIpv4Request)
    };
    if request.flags & !(NETWORK_CONFIGURE_HAS_GATEWAY | NETWORK_CONFIGURE_MAKE_DEFAULT) != 0 {
        return usize::MAX;
    }
    let interface = match read_user_string(request.iface_ptr, request.iface_len) {
        Some(interface) => interface,
        None => return usize::MAX,
    };

    let address = Ipv4Address::from_bytes(request.address);
    if address.is_any() || address.is_broadcast() {
        return usize::MAX;
    }
    let netmask = Ipv4Address::from_bytes(request.netmask);
    let gateway = if request.flags & NETWORK_CONFIGURE_HAS_GATEWAY != 0 {
        let gateway = Ipv4Address::from_bytes(request.gateway);
        if gateway.is_any() || gateway.is_broadcast() {
            return usize::MAX;
        }
        Some(gateway)
    } else {
        None
    };

    match crate::network::config::configure_interface_ipv4(
        &interface,
        address,
        netmask,
        gateway,
        request.metric,
        request.flags & NETWORK_CONFIGURE_MAKE_DEFAULT != 0,
    ) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
}

/// List interface-local IPv4 configuration records.
///
/// # Arguments
///
/// * Trapframe argument 0 - User pointer to an array of
///   [`NetworkInterfaceInfoV2`] records.
/// * Trapframe argument 1 - Maximum number of records in the array.
///
/// # Returns
///
/// The number of records written, or `usize::MAX` on failure.
pub fn sys_network_list_interfaces_v2(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(&task);

    let interfaces_ptr = tf.get_arg(0);
    let max_interfaces = tf.get_arg(1);
    if interfaces_ptr == 0 || max_interfaces == 0 {
        return usize::MAX;
    }

    let manager = crate::network::get_network_manager();
    let default_interface = manager.default_interface_name();
    let ip_layer = manager.get_layer("ip");
    let ipv4 = ip_layer.as_ref().and_then(|layer| {
        layer
            .as_any()
            .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
    });

    let mut records = Vec::new();
    for name in manager.list_interfaces() {
        if records.len() >= max_interfaces {
            break;
        }
        let Some(interface) = manager.get_interface(&name) else {
            continue;
        };

        let address_info = ipv4.and_then(|layer| layer.get_primary_address_info(&name));
        let route = ipv4.and_then(|layer| layer.get_default_route(&name));
        let address = address_info
            .as_ref()
            .map_or_else(|| interface.ip_address(), |info| Some(info.address));
        let gateway = route.as_ref().and_then(|entry| entry.gateway);
        let mut name_buffer = [0u8; 32];
        let copy_len = name.len().min(name_buffer.len() - 1);
        name_buffer[..copy_len].copy_from_slice(&name.as_bytes()[..copy_len]);

        records.push(NetworkInterfaceInfoV2 {
            name: name_buffer,
            ip_address: address.map_or([0; 4], |value| value.as_bytes()),
            netmask: address_info.map_or([0; 4], |info| info.netmask.as_bytes()),
            gateway: gateway.map_or([0; 4], |value| value.as_bytes()),
            mac_address: *interface.mac_address().as_bytes(),
            ip_set: u8::from(address.is_some()),
            gateway_set: u8::from(gateway.is_some()),
            is_default: u8::from(default_interface.as_deref() == Some(name.as_str())),
            reserved: [0; 3],
            metric: route.map_or(0, |entry| entry.metric),
        });
    }

    let item_size = core::mem::size_of::<NetworkInterfaceInfoV2>();
    for (index, record) in records.iter().enumerate() {
        // SAFETY: `record` is alive for the duration of this copy and the slice
        // covers exactly its fixed-layout representation.
        let record_bytes = unsafe {
            core::slice::from_raw_parts(
                (record as *const NetworkInterfaceInfoV2).cast::<u8>(),
                item_size,
            )
        };
        let Some(destination) = interfaces_ptr.checked_add(index.saturating_mul(item_size)) else {
            return usize::MAX;
        };
        if copy_to_user(&task, destination, record_bytes).is_err() {
            return usize::MAX;
        }
    }

    records.len()
}

/// Clear all IPv4 configuration from one interface.
///
/// # Arguments
///
/// * Trapframe argument 0 - User pointer to the interface name.
/// * Trapframe argument 1 - Interface name length.
///
/// # Returns
///
/// Zero on success, or `usize::MAX` when the name is invalid or the interface
/// cannot be cleared.
pub fn sys_network_clear_ipv4(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(&task);

    let interface = match read_user_string(tf.get_arg(0), tf.get_arg(1)) {
        Some(interface) => interface,
        None => return usize::MAX,
    };
    match crate::network::config::clear_interface_ipv4(&interface) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
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

    tf.increment_pc_next(&task);

    let domain = tf.get_arg(0) as u32;
    let socket_type = tf.get_arg(1) as u32;
    let protocol = tf.get_arg(2) as u32;

    let domain = match domain {
        0 | 1 => SocketDomain::Local,
        2 => SocketDomain::Inet4,
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

    let socket = match domain {
        SocketDomain::Local => {
            let socket = Arc::new(LocalSocket::new(socket_type, protocol));
            LocalSocket::init_self_weak(&socket);
            socket as Arc<dyn SocketObject>
        }
        SocketDomain::Inet4 | SocketDomain::Inet6 => {
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
    let kernel_obj = KernelObject::from_socket_object(socket);

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

    tf.increment_pc_next(&task);

    let handle_id = tf.get_arg(0) as u32;
    let path_ptr = tf.get_arg(1);
    let path_len = tf.get_arg(2);

    // Get the socket from handle table
    let socket_arc = match task.handle_table.get_arc_clone(handle_id) {
        Some(KernelObject::Socket(socket)) => socket,
        _ => return usize::MAX,
    };

    if path_len == core::mem::size_of::<Inet4SocketAddress>() {
        let mut addr_bytes = [0u8; core::mem::size_of::<Inet4SocketAddress>()];
        if copy_from_user(&task, path_ptr, &mut addr_bytes).is_err() {
            return usize::MAX;
        }
        let addr =
            unsafe { core::ptr::read_unaligned(addr_bytes.as_ptr() as *const Inet4SocketAddress) };
        if socket_arc.bind(&SocketAddress::Inet(addr)).is_err() {
            return usize::MAX;
        }
        return 0;
    }

    let mut path_bytes = vec![0u8; path_len.min(108)];
    if copy_from_user(&task, path_ptr, &mut path_bytes).is_err() {
        return usize::MAX;
    }
    let (local_addr, registry_name, is_abstract) =
        match local_socket_address_from_user_bytes(&path_bytes) {
            Ok(addr) => addr,
            Err(()) => return usize::MAX,
        };

    // Bind updates the socket's internal state
    if socket_arc
        .bind(&SocketAddress::Local(local_addr.clone()))
        .is_err()
    {
        return usize::MAX;
    }

    // Register the same Arc in NetworkManager's named socket namespace
    // This ensures the registered socket and the one in handle_table are identical
    if NetworkManager::get_manager()
        .register_named_socket(&registry_name, Arc::clone(socket_arc.as_arc()))
        .is_err()
    {
        return usize::MAX;
    }

    if is_abstract {
        return 0;
    }

    // Get the socket ID from NetworkManager for VFS integration
    let socket_id = match NetworkManager::get_manager().get_socket_id(socket_arc.as_arc()) {
        Some(id) => id,
        None => return usize::MAX, // Socket not found in NetworkManager
    };

    // Create socket file in VFS for filesystem visibility
    // Note: This is optional - the socket is already functional via named_sockets
    let vfs_guard = task.vfs.read();
    let vfs = match vfs_guard.as_ref() {
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
    if let Err(e) = vfs.create_file(local_addr.path(), socket_file_type) {
        // Log the error for debugging but continue - socket is still usable
        crate::println!(
            "[socket_bind] Warning: Failed to create VFS socket file at '{}': {:?}",
            local_addr.path(),
            e
        );
    }

    0
}

/// Bind an IPv4 datagram socket to a registered network interface.
///
/// # Arguments
///
/// * Trapframe argument 0 - Socket handle.
/// * Trapframe argument 1 - User pointer to the interface name.
/// * Trapframe argument 2 - Interface name length.
///
/// # Returns
///
/// Zero on success, or `usize::MAX` if the handle, name, interface, or socket
/// type is invalid.
pub fn sys_socket_bind_interface(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    tf.increment_pc_next(&task);

    let handle_id = tf.get_arg(0) as u32;
    let interface = match read_user_string(tf.get_arg(1), tf.get_arg(2)) {
        Some(interface) => interface,
        None => return usize::MAX,
    };
    if crate::network::get_network_manager()
        .get_interface(&interface)
        .is_none()
    {
        return usize::MAX;
    }

    let socket = match task.handle_table.get_arc_clone(handle_id) {
        Some(KernelObject::Socket(socket)) => socket,
        _ => return usize::MAX,
    };
    let Some(udp) = socket
        .as_any()
        .downcast_ref::<crate::network::udp::UdpSocket>()
    else {
        return usize::MAX;
    };

    match udp.bind_interface(&interface) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
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

    tf.increment_pc_next(&task);

    let handle_id = tf.get_arg(0) as u32;
    let backlog = tf.get_arg(1);

    // Get the socket from handle table
    let socket = match task
        .handle_table
        .get(handle_id)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => {
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

    tf.increment_pc_next(&task);

    let handle_id = tf.get_arg(0) as u32;
    let path_ptr = tf.get_arg(1);
    let path_len = tf.get_arg(2);

    // Get the socket from handle table
    let socket = match task
        .handle_table
        .get(handle_id)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => return usize::MAX,
    };

    if path_len == core::mem::size_of::<Inet4SocketAddress>() {
        let mut addr_bytes = [0u8; core::mem::size_of::<Inet4SocketAddress>()];
        if copy_from_user(&task, path_ptr, &mut addr_bytes).is_err() {
            return usize::MAX;
        }
        let addr =
            unsafe { core::ptr::read_unaligned(addr_bytes.as_ptr() as *const Inet4SocketAddress) };
        if socket.connect(&SocketAddress::Inet(addr)).is_err() {
            return usize::MAX;
        }
        return 0;
    }

    let mut path_bytes = vec![0u8; path_len.min(108)];
    if copy_from_user(&task, path_ptr, &mut path_bytes).is_err() {
        return usize::MAX;
    }
    let (peer_addr, _, _) = match local_socket_address_from_user_bytes(&path_bytes) {
        Ok(addr) => addr,
        Err(()) => return usize::MAX,
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

    tf.increment_pc_next(&task);

    let handle_id = tf.get_arg(0) as u32;

    // Get the listening socket from handle table
    let socket_obj = match task
        .handle_table
        .get(handle_id)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => return usize::MAX,
    };

    // Try to downcast to LocalSocket or TcpSocket
    use crate::network::local::LocalSocket;

    let accepted_socket =
        if let Some(local_socket) = LocalSocket::from_socket_object(socket_obj.as_ref()) {
            // LocalSocket accept
            match local_socket.accept_blocking(task.get_id(), tf) {
                Ok(socket) => socket,
                Err(e) => {
                    crate::println!(
                        "[sys_socket_accept] LocalSocket accept_blocking failed: {:?}",
                        e
                    );
                    return usize::MAX;
                }
            }
        } else if let Some(tcp_socket) =
            crate::network::tcp::TcpSocket::from_socket_object(socket_obj.as_ref())
        {
            // TcpSocket accept
            match tcp_socket.accept_blocking(task.get_id(), tf) {
                Ok(socket) => socket,
                Err(_) => return usize::MAX,
            }
        } else {
            crate::println!("[sys_socket_accept] Not a supported socket type");
            return usize::MAX;
        };

    // Add the accepted socket to handle table
    let kernel_obj = KernelObject::from_socket_object(accepted_socket);
    let metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: AccessMode::ReadWrite,
        special_semantics: None,
    };

    match task.handle_table.insert_with_metadata(kernel_obj, metadata) {
        Ok(id) => id as usize,
        Err(_) => usize::MAX,
    }
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

    tf.increment_pc_next(&task);

    let array_ptr = tf.get_arg(0);

    // Create a connected socket pair using LocalSocket::create_connected_pair
    let (socket1, socket2) = LocalSocket::create_connected_pair(
        String::from("socketpair:0"),
        String::from("socketpair:1"),
    );

    // Add both sockets to handle table
    let kernel_obj1 = KernelObject::from_socket_object(socket1);
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

    let kernel_obj2 = KernelObject::from_socket_object(socket2);
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

    let mut out = [0u8; core::mem::size_of::<usize>() * 2];
    let first = handle1.to_le_bytes();
    let second = handle2.to_le_bytes();
    out[..core::mem::size_of::<usize>()].copy_from_slice(&first);
    out[core::mem::size_of::<usize>()..].copy_from_slice(&second);
    if copy_to_user(&task, array_ptr, &out).is_err() {
        let _ = task.handle_table.remove(handle1 as u32);
        let _ = task.handle_table.remove(handle2 as u32);
        return usize::MAX;
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

    tf.increment_pc_next(&task);

    let handle_id = tf.get_arg(0) as u32;
    let how_value = tf.get_arg(1);

    // Get the socket from handle table
    let socket = match task
        .handle_table
        .get(handle_id)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => return usize::MAX,
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

/// System call: Receive datagram with sender address
///
/// Receives a datagram from a socket and returns the sender's address.
/// Used for UDP and Local datagram sockets.
///
/// # Arguments (via trapframe)
///
/// - `a0`: Socket handle ID
/// - `a1`: Pointer to buffer for receiving data
/// - `a2`: Buffer length
/// - `a3`: Pointer to SocketAddress structure for storing sender address (can be null)
///
/// # Returns
///
/// Number of bytes received on success, usize::MAX (-1) on error
///
/// # Errors
///
/// Returns usize::MAX (-1) if:
/// - Invalid handle ID
/// - Invalid buffer pointer
/// - Socket error
pub fn sys_socket_recvfrom(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    tf.increment_pc_next(&task);

    let handle_id = tf.get_arg(0) as u32;
    let buf_ptr = tf.get_arg(1);
    let buf_len = tf.get_arg(2);
    let addr_ptr = tf.get_arg(3);

    // Get the socket from handle table
    let socket = match task
        .handle_table
        .get(handle_id)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => return usize::MAX,
    };

    // Create a temporary buffer
    let mut temp_buf = vec![0u8; buf_len];

    // Receive datagram
    match socket.recvfrom(&mut temp_buf, 0) {
        Ok((len, addr)) => {
            if copy_to_user(&task, buf_ptr, &temp_buf[..len]).is_err() {
                return usize::MAX;
            }

            // Store sender address if pointer is provided
            if addr_ptr != 0 {
                match addr {
                    SocketAddress::Inet(inet) => {
                        let port_bytes = inet.port.to_be_bytes();
                        let sockaddr = [
                            2,
                            0,
                            inet.addr[0],
                            inet.addr[1],
                            inet.addr[2],
                            inet.addr[3],
                            port_bytes[0],
                            port_bytes[1],
                        ];
                        if copy_to_user(&task, addr_ptr, &sockaddr).is_err() {
                            return usize::MAX;
                        }
                    }
                    _ => {}
                }
            }

            len
        }
        Err(crate::network::socket::SocketError::WouldBlock) => (-(11i32)) as usize,
        Err(_) => usize::MAX,
    }
}

/// System call: Send datagram to specified address
///
/// Sends a datagram to a specific address.
/// Used for UDP and Local datagram sockets.
///
/// # Arguments (via trapframe)
///
/// - `a0`: Socket handle ID
/// - `a1`: Pointer to data buffer
/// - `a2`: Data length
/// - `a3`: Pointer to SocketAddress structure (destination address)
///
/// # Returns
///
/// Number of bytes sent on success, usize::MAX (-1) on error
///
/// # Errors
///
/// Returns usize::MAX (-1) if:
/// - Invalid handle ID
/// - Invalid buffer pointer
/// - Invalid address
/// - Socket error
pub fn sys_socket_sendto(tf: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    tf.increment_pc_next(&task);

    let handle_id = tf.get_arg(0) as u32;
    let buf_ptr = tf.get_arg(1);
    let buf_len = tf.get_arg(2);
    let addr_ptr = tf.get_arg(3);

    let mut data = vec![0u8; buf_len];
    if copy_from_user(&task, buf_ptr, &mut data).is_err() {
        return usize::MAX;
    }

    // Get the socket from handle table
    let socket = match task
        .handle_table
        .get(handle_id)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => return usize::MAX,
    };

    // Parse destination address
    let addr = if addr_ptr != 0 {
        let mut sockaddr = [0u8; 8];
        if copy_from_user(&task, addr_ptr, &mut sockaddr).is_err() {
            return usize::MAX;
        }
        match sockaddr[0] {
            2 => {
                let ip_bytes = [sockaddr[2], sockaddr[3], sockaddr[4], sockaddr[5]];
                let port = u16::from_be_bytes([sockaddr[6], sockaddr[7]]);
                SocketAddress::Inet(Inet4SocketAddress::new(ip_bytes, port))
            }
            _ => return usize::MAX,
        }
    } else {
        return usize::MAX;
    };

    // Send datagram
    match socket.sendto(&data, &addr, 0) {
        Ok(len) => len,
        Err(_) => usize::MAX,
    }
}
