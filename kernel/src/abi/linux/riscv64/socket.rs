use crate::ipc::IpcError;
use crate::object::capability::StreamError;
use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi,
    abi::linux::riscv64::errno,
    abi::linux::riscv64::fs::{FD_CLOEXEC, IoVec, O_NONBLOCK},
    arch::Trapframe,
    network::{NetworkManager, SocketDomain, SocketProtocol, SocketType, local::LocalSocket},
    object::KernelObject,
    object::capability::selectable::Selectable,
    sched::scheduler::get_scheduler,
    task::mytask,
};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem::size_of;

/// Linux socket domains
pub const AF_UNIX: i32 = 1; // Unix domain sockets
pub const AF_INET: i32 = 2; // Internet IP Protocol
pub const AF_INET6: i32 = 10; // IP version 6

/// Linux socket domain as u16 constants for pattern matching
const AF_UNIX_U16: u16 = AF_UNIX as u16;
const AF_INET_U16: u16 = AF_INET as u16;

/// IPv4 socket address structure
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SockaddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

impl SockaddrIn {
    pub fn new() -> Self {
        Self {
            sin_family: AF_INET as u16,
            sin_port: 0,
            sin_addr: 0,
            sin_zero: [0; 8],
        }
    }
}

/// Linux socket types
pub const SOCK_STREAM: i32 = 1; // Stream socket
pub const SOCK_DGRAM: i32 = 2; // Datagram socket
pub const SOCK_RAW: i32 = 3; // Raw socket
pub const SOCK_SEQPACKET: i32 = 5; // Sequenced packet socket
pub const SOCK_NONBLOCK: i32 = 0x800;
pub const SOCK_CLOEXEC: i32 = 0x80000;
pub const SOCK_TYPE_MASK: i32 = 0xF;

pub const SOL_SOCKET: i32 = 1;
pub const SCM_RIGHTS: i32 = 1;
pub const MSG_DONTWAIT: i32 = 0x40;

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxMsghdr {
    msg_name: u64,
    msg_namelen: u32,
    __pad1: u32,
    msg_iov: u64,
    msg_iovlen: u64,
    msg_control: u64,
    msg_controllen: u64,
    msg_flags: u32,
    __pad2: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxCmsghdr {
    cmsg_len: usize,
    cmsg_level: i32,
    cmsg_type: i32,
}

/// Linux sys_socket implementation
///
/// Creates a socket endpoint for communication. Now properly integrated with
/// NetworkManager and VFS for Unix domain socket support.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: domain (communication domain, e.g., AF_UNIX, AF_INET)
///   - arg1: type (socket type, e.g., SOCK_STREAM, SOCK_DGRAM)
///   - arg2: protocol (protocol to use, usually 0)
///
/// Returns:
/// - file descriptor on success
/// - usize::MAX (Linux -1) on error
pub fn sys_socket(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let domain = trapframe.get_arg(0) as i32;
    let socket_type = trapframe.get_arg(1) as i32;
    let _protocol = trapframe.get_arg(2) as i32;
    let socket_base_type = socket_type & SOCK_TYPE_MASK;
    let socket_flags = socket_type & !SOCK_TYPE_MASK;
    let set_nonblock = (socket_flags & SOCK_NONBLOCK) != 0;
    let set_cloexec = (socket_flags & SOCK_CLOEXEC) != 0;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Map Linux socket domain to Scarlet domain
    let scarlet_domain = match domain {
        AF_UNIX => SocketDomain::Local,
        AF_INET => SocketDomain::Inet4,
        AF_INET6 => SocketDomain::Inet6,
        _ => {
            crate::early_println!("[linux socket] unsupported domain {}", domain);
            return usize::MAX;
        }
    };

    // Map Linux socket type to Scarlet type
    let scarlet_type = match socket_base_type {
        SOCK_STREAM => SocketType::Stream,
        SOCK_DGRAM => SocketType::Datagram,
        SOCK_RAW => SocketType::Raw,
        SOCK_SEQPACKET => SocketType::SeqPacket,
        _ => {
            crate::early_println!("[linux socket] unsupported type {}", socket_type);
            return usize::MAX;
        }
    };

    // Map protocol
    let scarlet_protocol = match scarlet_domain {
        SocketDomain::Local => SocketProtocol::Default,
        SocketDomain::Inet4 | SocketDomain::Inet6 => match (_protocol, socket_base_type) {
            (0, SOCK_STREAM) => SocketProtocol::Tcp,
            (0, SOCK_DGRAM) => SocketProtocol::Udp,
            (6, _) => SocketProtocol::Tcp,
            (17, _) => SocketProtocol::Udp,
            (1, _) => SocketProtocol::Icmp,
            _ => SocketProtocol::Default,
        },
        _ => SocketProtocol::Default,
    };

    let socket_obj: Arc<dyn crate::network::SocketObject> = match scarlet_domain {
        SocketDomain::Local => {
            let local_socket = Arc::new(LocalSocket::new(scarlet_type, SocketProtocol::Default));
            LocalSocket::init_self_weak(&local_socket);
            local_socket as Arc<dyn crate::network::SocketObject>
        }
        SocketDomain::Inet4 | SocketDomain::Inet6 => {
            let socket = match scarlet_protocol {
                SocketProtocol::Tcp => {
                    NetworkManager::get_manager().get_layer("tcp").map(|layer| {
                        let tcp = layer
                            .as_any()
                            .downcast_ref::<crate::network::tcp::TcpLayer>()
                            .expect("tcp layer type mismatch");
                        tcp.create_socket() as Arc<dyn crate::network::SocketObject>
                    })
                }
                SocketProtocol::Udp => {
                    NetworkManager::get_manager().get_layer("udp").map(|layer| {
                        let udp = layer
                            .as_any()
                            .downcast_ref::<crate::network::udp::UdpLayer>()
                            .expect("udp layer type mismatch");
                        udp.create_socket() as Arc<dyn crate::network::SocketObject>
                    })
                }
                SocketProtocol::Icmp => {
                    NetworkManager::get_manager()
                        .get_layer("icmp")
                        .map(|layer| {
                            let icmp = layer
                                .as_any()
                                .downcast_ref::<crate::network::icmp::IcmpLayer>()
                                .expect("icmp layer type mismatch");
                            icmp.create_socket() as Arc<dyn crate::network::SocketObject>
                        })
                }
                _ => None,
            };

            match socket {
                Some(socket) => socket,
                None => {
                    crate::early_println!(
                        "[linux socket] failed to create INET socket protocol={:?}",
                        scarlet_protocol
                    );
                    return usize::MAX;
                }
            }
        }
        _ => {
            crate::early_println!("[linux socket] unsupported domain {:?}", scarlet_domain);
            return usize::MAX;
        }
    };
    if NetworkManager::get_manager()
        .allocate_socket_id(Arc::clone(&socket_obj))
        .is_err()
    {
        crate::early_println!("[linux socket] allocate_socket_id failed");
    }

    if set_nonblock {
        if let Some(local_socket) = LocalSocket::from_socket_object(&socket_obj) {
            local_socket.set_nonblocking(true);
        }
    }

    // Wrap in KernelObject
    let kernel_obj = KernelObject::Socket(socket_obj);

    // Insert into handle table
    match task.handle_table.insert(kernel_obj) {
        Ok(handle) => {
            // Allocate a file descriptor for the socket
            match abi.allocate_fd(handle) {
                Ok(fd) => {
                    if set_cloexec {
                        let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
                    }
                    fd
                }
                Err(_) => {
                    // Clean up on error
                    let _ = task.handle_table.remove(handle);
                    usize::MAX
                }
            }
        }
        Err(_) => {
            crate::early_println!("[linux socket] handle table insert failed");
            usize::MAX
        }
    }
}

/// Linux sys_bind implementation
///
/// Binds a socket to an address. For AF_UNIX sockets, this creates a socket file
/// in the VFS at the specified path.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: addr (pointer to socket address structure)
///   - arg2: addrlen (size of address structure)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_bind(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let addr_ptr = trapframe.get_arg(1);
    let addrlen = trapframe.get_arg(2) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Get the file descriptor handle
    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::early_println!("[linux socket] bind invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    // Get the socket object from handle table
    let socket_arc = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => {
            crate::early_println!("[linux socket] bind fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    // Translate address pointer to physical
    let addr_paddr = match task.vm_manager.translate_to_kva(addr_ptr) {
        Some(addr) => addr,
        None => {
            crate::early_println!("[linux socket] bind bad addr {:x}", addr_ptr);
            return usize::MAX;
        }
    };

    // Read sockaddr structure from userspace
    // sockaddr_un structure: { sa_family: u16, sun_path: [u8; 108] }
    if addrlen < 2 {
        return usize::MAX; // Too small
    }

    unsafe {
        let sa_family = *(addr_paddr as *const u16);

        match sa_family {
            AF_UNIX_U16 => {
                // Read the socket path (starts at offset 2)
                let path_start = (addr_paddr + 2) as *const u8;
                let max_path_len = (addrlen - 2) as usize;

                // Find the null terminator or max length
                let mut path_len = 0;
                while path_len < max_path_len && *path_start.add(path_len) != 0 {
                    path_len += 1;
                }

                if path_len == 0 || path_len > 108 {
                    crate::early_println!("[linux socket] bind invalid path length {}", path_len);
                    return usize::MAX;
                }

                // Convert to string
                let path_bytes = core::slice::from_raw_parts(path_start, path_len);
                let path = match core::str::from_utf8(path_bytes) {
                    Ok(s) => s,
                    Err(_) => {
                        crate::early_println!("[linux socket] bind path utf8 error");
                        return usize::MAX;
                    }
                };

                // Bind the socket to the address
                let socket_addr = match crate::network::LocalSocketAddress::from_path(path) {
                    Ok(addr) => crate::network::SocketAddress::Local(addr),
                    Err(_) => return usize::MAX,
                };

                if socket_arc.bind(&socket_addr).is_err() {
                    crate::early_println!("[linux socket] bind failed for {}", path);
                    return usize::MAX;
                }

                if NetworkManager::get_manager()
                    .register_named_socket(path, socket_arc.clone())
                    .is_err()
                {
                    crate::early_println!("[linux socket] register_named_socket failed {}", path);
                    return usize::MAX;
                }

                // Get the socket ID from NetworkManager
                let socket_id = match NetworkManager::get_manager().get_socket_id(&socket_arc) {
                    Some(id) => id,
                    None => {
                        crate::early_println!("[linux socket] get_socket_id failed {}", path);
                        return usize::MAX;
                    }
                };

                // Create socket file in VFS on a best-effort basis
                // The socket has already been successfully bound, so VFS file creation
                // is optional - the socket remains functional even if this fails
                let vfs = match task.vfs.read().clone() {
                    Some(vfs) => vfs.clone(),
                    None => {
                        // Use global VFS if task doesn't have its own
                        crate::fs::vfs_v2::manager::get_global_vfs_manager()
                    }
                };

                let socket_file_type =
                    crate::fs::FileType::Socket(crate::fs::SocketFileInfo { socket_id });

                // Attempt to create the socket file - log on failure but don't fail the bind
                if let Err(e) = vfs.create_file(path, socket_file_type) {
                    crate::early_println!(
                        "[sys_bind] Warning: Failed to create VFS socket file at '{}': {:?}",
                        path,
                        e
                    );
                }

                0 // Success
            }
            AF_INET_U16 => {
                let addr_struct = &*(addr_paddr as *const SockaddrIn);
                let port = u16::from_be(addr_struct.sin_port);
                let addr_bytes = u32::to_be(addr_struct.sin_addr).to_be_bytes();
                let socket_addr = crate::network::SocketAddress::Inet(
                    crate::network::Inet4SocketAddress::new(addr_bytes, port),
                );

                if socket_arc.bind(&socket_addr).is_err() {
                    crate::early_println!("[linux socket] bind failed for INET address");
                    return usize::MAX;
                }

                0
            }
            _ => {
                crate::early_println!("[linux socket] bind unsupported family {}", sa_family);
                usize::MAX
            }
        }
    }
}

/// Linux sys_listen implementation (mock)
///
/// Marks a socket as passive, ready to accept connections. This is a mock
/// implementation that always succeeds to allow applications to proceed.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: backlog (maximum queue length for pending connections)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_listen(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let backlog = trapframe.get_arg(1) as i32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::early_println!("[linux socket] listen invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    let socket_arc = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => {
            crate::early_println!("[linux socket] listen fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    if socket_arc.listen(backlog.max(0) as usize).is_err() {
        crate::early_println!("[linux socket] listen failed fd {}", sockfd);
        return usize::MAX;
    }

    0
}

/// Linux sys_accept implementation (mock)
///
/// Accepts a connection on a socket. This is a mock implementation that
/// creates a new pipe and returns it as a "connected" socket fd.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: addr (pointer to socket address structure for peer)
///   - arg2: addrlen (pointer to size of address structure)
///
/// Returns:
/// - new socket file descriptor on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_accept(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let _addr_ptr = trapframe.get_arg(1);
    let _addrlen_ptr = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::early_println!("[linux socket] accept invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    let socket_obj = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => {
            crate::early_println!("[linux socket] accept fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    // Try LocalSocket first, then TcpSocket
    let accepted_socket = if let Some(local_socket) = LocalSocket::from_socket_object(&socket_obj) {
        local_socket.accept_blocking(task.get_id(), trapframe)
    } else if let Some(tcp_socket) = crate::network::tcp::TcpSocket::from_socket_object(&socket_obj)
    {
        tcp_socket.accept_blocking(task.get_id(), trapframe)
    } else {
        crate::early_println!("[linux socket] accept not supported socket type");
        return usize::MAX;
    };

    let accepted_socket = match accepted_socket {
        Ok(socket) => socket,
        Err(_) => {
            crate::early_println!("[linux socket] accept_blocking failed");
            return usize::MAX;
        }
    };

    let kernel_obj = KernelObject::Socket(accepted_socket);
    match task.handle_table.insert(kernel_obj) {
        Ok(handle) => match abi.allocate_fd(handle) {
            Ok(fd) => fd,
            Err(_) => {
                let _ = task.handle_table.remove(handle);
                usize::MAX
            }
        },
        Err(_) => usize::MAX,
    }
}

/// Linux sys_connect implementation (mock)
///
/// Connects a socket to an address. This is a mock implementation that
/// always succeeds to allow applications to proceed.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: addr (pointer to socket address structure)
///   - arg2: addrlen (size of address structure)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_connect(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let addr_ptr = trapframe.get_arg(1);
    let addrlen = trapframe.get_arg(2) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::early_println!("[linux socket] connect invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    let socket_arc = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => {
            crate::early_println!("[linux socket] connect fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    let addr_paddr = match task.vm_manager.translate_to_kva(addr_ptr) {
        Some(addr) => addr,
        None => {
            crate::early_println!("[linux socket] connect bad addr {:x}", addr_ptr);
            return usize::MAX;
        }
    };

    if addrlen < 2 {
        return usize::MAX;
    }

    unsafe {
        let sa_family = *(addr_paddr as *const u16);
        match sa_family {
            AF_UNIX_U16 => {
                let path_start = (addr_paddr + 2) as *const u8;
                let max_path_len = (addrlen - 2) as usize;
                let mut path_len = 0;
                while path_len < max_path_len && *path_start.add(path_len) != 0 {
                    path_len += 1;
                }

                if path_len == 0 || path_len > 108 {
                    crate::early_println!(
                        "[linux socket] connect invalid path length {}",
                        path_len
                    );
                    return usize::MAX;
                }

                let path_bytes = core::slice::from_raw_parts(path_start, path_len);
                let path = match core::str::from_utf8(path_bytes) {
                    Ok(s) => s,
                    Err(_) => {
                        crate::early_println!("[linux socket] connect path utf8 error");
                        return usize::MAX;
                    }
                };

                let socket_addr = match crate::network::LocalSocketAddress::from_path(path) {
                    Ok(addr) => crate::network::SocketAddress::Local(addr),
                    Err(_) => return usize::MAX,
                };

                if socket_arc.connect(&socket_addr).is_err() {
                    crate::early_println!("[linux socket] connect failed {}", path);
                    return usize::MAX;
                }
            }
            AF_INET_U16 => {
                let addr_struct = &*(addr_paddr as *const SockaddrIn);
                let port = u16::from_be(addr_struct.sin_port);
                let addr_bytes = u32::to_be(addr_struct.sin_addr).to_be_bytes();
                let socket_addr = crate::network::SocketAddress::Inet(
                    crate::network::Inet4SocketAddress::new(addr_bytes, port),
                );

                if socket_arc.connect(&socket_addr).is_err() {
                    crate::early_println!("[linux socket] connect failed for INET address");
                    return usize::MAX;
                }
            }
            _ => {
                crate::early_println!("[linux socket] connect unsupported family {}", sa_family);
                return usize::MAX;
            }
        }
    }

    0
}

/// Linux sys_getsockname implementation
///
/// Gets the current address of a socket.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: addr (pointer to socket address structure)
///   - arg2: addrlen (pointer to size of address structure)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_getsockname(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let addr_ptr = trapframe.get_arg(1);
    let addrlen_ptr = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::early_println!("[linux socket] getsockname invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    let socket_arc = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => {
            crate::early_println!("[linux socket] getsockname fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    let (addr_paddr, addrlen_paddr) = match (
        task.vm_manager.translate_to_kva(addr_ptr),
        task.vm_manager.translate_to_kva(addrlen_ptr),
    ) {
        (Some(addr), Some(len)) => (addr, len),
        _ => {
            crate::early_println!("[linux socket] getsockname invalid pointers");
            return usize::MAX;
        }
    };

    let socket_addr = match socket_arc.getsockname() {
        Ok(addr) => addr,
        Err(_) => {
            crate::early_println!("[linux socket] getsockname failed");
            return usize::MAX;
        }
    };

    unsafe {
        let addrlen = *(addrlen_paddr as *const u32);

        match socket_addr {
            crate::network::SocketAddress::Local(addr) => {
                if addrlen >= 2 {
                    let sockaddr = addr_paddr as *mut u16;
                    *sockaddr = AF_UNIX_U16;

                    let path_start = (addr_paddr + 2) as *mut u8;
                    let path = addr.path().as_bytes();
                    let path_len = path.len().min((addrlen - 2) as usize);
                    core::ptr::copy_nonoverlapping(path.as_ptr(), path_start, path_len);
                    if path_len < (addrlen - 2) as usize {
                        *(path_start.add(path_len)) = 0;
                    }

                    *(addrlen_paddr as *mut u32) = (2 + path_len as u32).min(addrlen);
                    0
                } else {
                    usize::MAX
                }
            }
            crate::network::SocketAddress::Inet(inet) => {
                if addrlen >= size_of::<SockaddrIn>() as u32 {
                    let sockaddr = addr_paddr as *mut SockaddrIn;
                    (*sockaddr).sin_family = AF_INET_U16;
                    (*sockaddr).sin_port = u16::to_be(inet.port);
                    (*sockaddr).sin_addr = u32::from_be_bytes(inet.addr);

                    *(addrlen_paddr as *mut u32) = size_of::<SockaddrIn>() as u32;
                    0
                } else {
                    usize::MAX
                }
            }
            _ => usize::MAX,
        }
    }
}

/// Linux sys_getpeername implementation
///
/// Gets the address of the peer connected to the socket.
///
/// Arguments:
///   - arg0: sockfd (socket file descriptor)
///   - arg1: addr (pointer to socket address structure)
///   - arg2: addrlen (pointer to size of address structure)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_getpeername(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let addr_ptr = trapframe.get_arg(1);
    let addrlen_ptr = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::early_println!("[linux socket] getpeername invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    let socket_arc = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => {
            crate::early_println!("[linux socket] getpeername fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    let (addr_paddr, addrlen_paddr) = match (
        task.vm_manager.translate_to_kva(addr_ptr),
        task.vm_manager.translate_to_kva(addrlen_ptr),
    ) {
        (Some(addr), Some(len)) => (addr, len),
        _ => {
            crate::early_println!("[linux socket] getpeername invalid pointers");
            return usize::MAX;
        }
    };

    let socket_addr = match socket_arc.getpeername() {
        Ok(addr) => addr,
        Err(_) => {
            crate::early_println!("[linux socket] getpeername failed");
            return usize::MAX;
        }
    };

    unsafe {
        let addrlen = *(addrlen_paddr as *const u32);

        match socket_addr {
            crate::network::SocketAddress::Local(addr) => {
                if addrlen >= 2 {
                    let sockaddr = addr_paddr as *mut u16;
                    *sockaddr = AF_UNIX_U16;

                    let path_start = (addr_paddr + 2) as *mut u8;
                    let path = addr.path().as_bytes();
                    let path_len = path.len().min((addrlen - 2) as usize);
                    core::ptr::copy_nonoverlapping(path.as_ptr(), path_start, path_len);
                    if path_len < (addrlen - 2) as usize {
                        *(path_start.add(path_len)) = 0;
                    }

                    *(addrlen_paddr as *mut u32) = (2 + path_len as u32).min(addrlen);
                    0
                } else {
                    usize::MAX
                }
            }
            crate::network::SocketAddress::Inet(inet) => {
                if addrlen >= size_of::<SockaddrIn>() as u32 {
                    let sockaddr = addr_paddr as *mut SockaddrIn;
                    (*sockaddr).sin_family = AF_INET_U16;
                    (*sockaddr).sin_port = u16::to_be(inet.port);
                    (*sockaddr).sin_addr = u32::from_be_bytes(inet.addr);

                    *(addrlen_paddr as *mut u32) = size_of::<SockaddrIn>() as u32;
                    0
                } else {
                    usize::MAX
                }
            }
            _ => usize::MAX,
        }
    }
}

/// Linux sys_getsockopt implementation (mock)
///
/// Gets socket options. This is a mock implementation that
/// writes dummy data and succeeds to allow applications to proceed.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: level (protocol level)
///   - arg2: optname (option name)
///   - arg3: optval (pointer to option value buffer)
///   - arg4: optlen (pointer to option length)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_getsockopt(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let _sockfd = trapframe.get_arg(0) as i32;
    let _level = trapframe.get_arg(1) as i32;
    let _optname = trapframe.get_arg(2) as i32;
    let optval_ptr = trapframe.get_arg(3);
    let optlen_ptr = trapframe.get_arg(4);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Mock implementation - write minimal valid data and return success
    if let (Some(optval_paddr), Some(optlen_paddr)) = (
        task.vm_manager.translate_to_kva(optval_ptr),
        task.vm_manager.translate_to_kva(optlen_ptr),
    ) {
        unsafe {
            // Read the provided length
            let optlen = *(optlen_paddr as *const u32);

            // Write dummy option value (typically an integer)
            if optlen >= 4 && optval_ptr != 0 {
                let optval = optval_paddr as *mut u32;
                *optval = 1; // Generic "enabled" value

                // Update the actual length used
                *(optlen_paddr as *mut u32) = 4;
            }
        }
        0 // Success
    } else {
        usize::MAX // Invalid pointers
    }
}

/// Linux sys_setsockopt implementation (mock)
///
/// Sets socket options. This is a mock implementation that
/// always succeeds to allow applications to proceed.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: level (protocol level)
///   - arg2: optname (option name)
///   - arg3: optval (pointer to option value)
///   - arg4: optlen (option length)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_setsockopt(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let _sockfd = trapframe.get_arg(0) as i32;
    let _level = trapframe.get_arg(1) as i32;
    let _optname = trapframe.get_arg(2) as i32;
    let _optval_ptr = trapframe.get_arg(3);
    let _optlen = trapframe.get_arg(4) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Mock implementation - always succeed
    0
}

/// Linux sys_sendmsg implementation (minimal)
pub fn sys_sendmsg(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0);
    let msg_ptr = trapframe.get_arg(1);
    let flags = trapframe.get_arg(2) as i32;

    trapframe.increment_pc_next(task);

    let handle = match abi.get_handle(sockfd) {
        Some(h) => h,
        None => {
            crate::early_println!("[linux socket] sendmsg bad fd {}", sockfd);
            return errno::to_result(errno::EBADF);
        }
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => {
            crate::early_println!("[linux socket] sendmsg missing handle {}", sockfd);
            return errno::to_result(errno::EBADF);
        }
    };

    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => {
            crate::early_println!("[linux socket] sendmsg not a stream");
            return errno::to_result(errno::ENOTSOCK);
        }
    };

    let nonblocking = (flags & MSG_DONTWAIT) != 0
        || abi
            .get_file_status_flags(sockfd)
            .map(|f| ((f as i32) & O_NONBLOCK) != 0)
            .unwrap_or(false);

    let msg_addr = match task.vm_manager.translate_to_kva(msg_ptr) {
        Some(addr) => addr as *const LinuxMsghdr,
        None => {
            crate::early_println!("[linux socket] sendmsg bad msg ptr {:x}", msg_ptr);
            return errno::to_result(errno::EFAULT);
        }
    };

    if msg_addr.is_null() {
        crate::early_println!("[linux socket] sendmsg null msg ptr");
        return errno::to_result(errno::EFAULT);
    }

    let msg = unsafe { *msg_addr };
    let iovcnt = msg.msg_iovlen as usize;
    if iovcnt == 0 {
        return 0;
    }

    const IOV_MAX: usize = 1024;
    if iovcnt > IOV_MAX {
        return errno::to_result(errno::EINVAL);
    }

    let iovec_addr = match task.vm_manager.translate_to_kva(msg.msg_iov as usize) {
        Some(addr) => addr as *const IoVec,
        None => {
            crate::early_println!("[linux socket] sendmsg bad iov ptr {:x}", msg.msg_iov);
            return errno::to_result(errno::EFAULT);
        }
    };

    if iovec_addr.is_null() {
        crate::early_println!("[linux socket] sendmsg null iov ptr");
        return errno::to_result(errno::EFAULT);
    }

    let iovecs = unsafe { core::slice::from_raw_parts(iovec_addr, iovcnt) };

    if msg.msg_control != 0 && msg.msg_controllen as usize >= size_of::<LinuxCmsghdr>() {
        let socket_arc = match &kernel_obj {
            KernelObject::Socket(socket) => Arc::clone(socket),
            _ => return errno::to_result(errno::ENOTSOCK),
        };

        if let Some(local_socket) = LocalSocket::from_socket_object(&socket_arc) {
            let cmsg_addr = match task.vm_manager.translate_to_kva(msg.msg_control as usize) {
                Some(addr) => addr as *const LinuxCmsghdr,
                None => {
                    crate::early_println!(
                        "[linux socket] sendmsg bad cmsg ptr {:x}",
                        msg.msg_control
                    );
                    return errno::to_result(errno::EFAULT);
                }
            };

            if !cmsg_addr.is_null() {
                let cmsg = unsafe { *cmsg_addr };
                if cmsg.cmsg_level == SOL_SOCKET && cmsg.cmsg_type == SCM_RIGHTS {
                    let data_len = cmsg.cmsg_len.saturating_sub(size_of::<LinuxCmsghdr>());
                    let fd_count = data_len / size_of::<i32>();
                    let data_ptr = unsafe { cmsg_addr.add(1) } as *const i32;

                    for index in 0..fd_count {
                        let fd = unsafe { *data_ptr.add(index) };
                        if fd < 0 {
                            return errno::to_result(errno::EBADF);
                        }
                        let send_handle = match abi.get_handle(fd as usize) {
                            Some(h) => h,
                            None => {
                                crate::early_println!(
                                    "[linux socket] sendmsg bad fd in cmsg {}",
                                    fd
                                );
                                return errno::to_result(errno::EBADF);
                            }
                        };
                        let dup_obj = match task.handle_table.clone_for_dup(send_handle) {
                            Some(obj) => obj,
                            None => {
                                crate::early_println!(
                                    "[linux socket] sendmsg clone_for_dup failed"
                                );
                                return errno::to_result(errno::EBADF);
                            }
                        };
                        if local_socket.send_handle(dup_obj).is_err() {
                            crate::early_println!("[linux socket] sendmsg send_handle failed");
                            return errno::to_result(errno::EIO);
                        }
                    }
                }
            }
        }
    }

    let mut total_written = 0usize;
    struct NonblockGuard<'a> {
        sel: Option<&'a dyn Selectable>,
        prev: bool,
    }

    impl<'a> Drop for NonblockGuard<'a> {
        fn drop(&mut self) {
            if let Some(sel) = self.sel {
                sel.set_nonblocking(self.prev);
            }
        }
    }

    let _nonblock_guard = if nonblocking {
        if let Some(sel) = kernel_obj.as_selectable() {
            let prev = sel.is_nonblocking();
            if !prev {
                sel.set_nonblocking(true);
                Some(NonblockGuard {
                    sel: Some(sel),
                    prev,
                })
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    for iovec in iovecs {
        if iovec.iov_len == 0 {
            continue;
        }

        let buf_addr = match task.vm_manager.translate_to_kva(iovec.iov_base as usize) {
            Some(addr) => addr as *const u8,
            None => {
                crate::early_println!(
                    "[linux socket] sendmsg bad buf ptr {:x}",
                    iovec.iov_base as usize
                );
                return errno::to_result(errno::EFAULT);
            }
        };

        if buf_addr.is_null() {
            crate::early_println!("[linux socket] sendmsg null buf ptr");
            return errno::to_result(errno::EFAULT);
        }

        let buffer = unsafe { core::slice::from_raw_parts(buf_addr, iovec.iov_len) };

        match stream.write(buffer) {
            Ok(n) => {
                total_written = total_written.saturating_add(n);
                if n < iovec.iov_len {
                    break;
                }
            }
            Err(StreamError::WouldBlock) => {
                if nonblocking {
                    crate::early_println!("[linux socket] sendmsg would block");
                    return if total_written == 0 {
                        errno::to_result(errno::EAGAIN)
                    } else {
                        total_written
                    };
                }
                get_scheduler().schedule(trapframe);
                return usize::MAX;
            }
            Err(_) => {
                crate::early_println!("[linux socket] sendmsg write error");
                return errno::to_result(errno::EIO);
            }
        }
    }

    total_written
}

/// Linux sys_recvmsg implementation (minimal)
pub fn sys_recvmsg(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0);
    let msg_ptr = trapframe.get_arg(1);
    let flags = trapframe.get_arg(2) as i32;
    // crate::early_println!(
    //     "[linux recvmsg] fd={} msg_ptr={:#x} flags={:#x}",
    //     sockfd,
    //     msg_ptr,
    //     flags
    // );

    trapframe.increment_pc_next(task);

    let handle = match abi.get_handle(sockfd) {
        Some(h) => h,
        None => {
            crate::early_println!("[linux socket] recvmsg bad fd {}", sockfd);
            return errno::to_result(errno::EBADF);
        }
    };

    // crate::early_println!("[linux recvmsg] handle={}", handle);

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => {
            crate::early_println!("[linux socket] recvmsg missing handle {}", sockfd);
            return errno::to_result(errno::EBADF);
        }
    };

    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => {
            crate::early_println!("[linux socket] recvmsg not a stream");
            return errno::to_result(errno::ENOTSOCK);
        }
    };

    let nonblocking = (flags & MSG_DONTWAIT) != 0
        || abi
            .get_file_status_flags(sockfd)
            .map(|f| ((f as i32) & O_NONBLOCK) != 0)
            .unwrap_or(false);

    let msg_addr = match task.vm_manager.translate_to_kva(msg_ptr) {
        Some(addr) => addr as *mut LinuxMsghdr,
        None => {
            crate::early_println!("[linux socket] recvmsg bad msg ptr {:x}", msg_ptr);
            return errno::to_result(errno::EFAULT);
        }
    };

    if msg_addr.is_null() {
        crate::early_println!("[linux socket] recvmsg null msg ptr");
        return errno::to_result(errno::EFAULT);
    }

    let msg = unsafe { *msg_addr };
    // crate::early_println!(
    //     "[linux recvmsg] iov_ptr={:#x} iovlen={} control_ptr={:#x} controllen={}",
    //     msg.msg_iov,
    //     msg.msg_iovlen,
    //     msg.msg_control,
    //     msg.msg_controllen
    // );
    let iovcnt = msg.msg_iovlen as usize;
    if iovcnt == 0 {
        return 0;
    }

    const IOV_MAX: usize = 1024;
    if iovcnt > IOV_MAX {
        return errno::to_result(errno::EINVAL);
    }

    let iovec_addr = match task.vm_manager.translate_to_kva(msg.msg_iov as usize) {
        Some(addr) => addr as *const IoVec,
        None => {
            crate::early_println!("[linux socket] recvmsg bad iov ptr {:x}", msg.msg_iov);
            return errno::to_result(errno::EFAULT);
        }
    };

    if iovec_addr.is_null() {
        crate::early_println!("[linux socket] recvmsg null iov ptr");
        return errno::to_result(errno::EFAULT);
    }

    let iovecs = unsafe { core::slice::from_raw_parts(iovec_addr, iovcnt) };
    // crate::early_println!("[linux recvmsg] iovcnt={}", iovecs.len());
    let mut total_read = 0usize;
    let mut pending_fd: Option<i32> = None;
    let mut msg_controllen = 0usize;
    struct NonblockGuard<'a> {
        sel: Option<&'a dyn Selectable>,
        prev: bool,
    }

    impl<'a> Drop for NonblockGuard<'a> {
        fn drop(&mut self) {
            if let Some(sel) = self.sel {
                sel.set_nonblocking(self.prev);
            }
        }
    }

    let _nonblock_guard = if nonblocking {
        if let Some(sel) = kernel_obj.as_selectable() {
            let prev = sel.is_nonblocking();
            if !prev {
                sel.set_nonblocking(true);
                Some(NonblockGuard {
                    sel: Some(sel),
                    prev,
                })
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Calculate total buffer size for potential handle+data receive
    let total_buffer_size: usize = iovecs.iter().map(|i| i.iov_len).sum();
    let mut atomic_data: Option<Vec<u8>> = None;

    // Try atomic handle+data receive if control buffer is provided
    if msg.msg_control != 0
        && (msg.msg_controllen as usize) >= size_of::<LinuxCmsghdr>() + size_of::<i32>()
    {
        let socket_arc = match &kernel_obj {
            KernelObject::Socket(socket) => Arc::clone(socket),
            _ => {
                return errno::to_result(errno::ENOTSOCK);
            }
        };

        if let Some(local_socket) = LocalSocket::from_socket_object(&socket_arc) {
            match local_socket.recv_handle_and_data(total_buffer_size) {
                Ok((obj, data)) => {
                    let new_handle = match task.handle_table.insert(obj) {
                        Ok(h) => h,
                        Err(_) => return errno::to_result(errno::EMFILE),
                    };
                    let new_fd = match abi.allocate_fd(new_handle) {
                        Ok(fd) => fd,
                        Err(_) => {
                            let _ = task.handle_table.remove(new_handle);
                            return errno::to_result(errno::EMFILE);
                        }
                    };
                    pending_fd = Some(new_fd as i32);
                    atomic_data = Some(data);
                }
                Err(IpcError::ChannelEmpty) => {
                    // No handle available; fall back to regular stream read
                }
                Err(_) => {
                    return errno::to_result(errno::EIO);
                }
            }
        }
    }

    // Copy data from atomic receive or read from stream
    if let Some(ref data) = atomic_data {
        // Copy atomically received data into iovecs
        let mut data_offset = 0;
        let data_len = data.len();
        for iovec in iovecs {
            if data_offset >= data_len {
                break;
            }
            if iovec.iov_len == 0 {
                continue;
            }

            let buf_addr = match task.vm_manager.translate_to_kva(iovec.iov_base as usize) {
                Some(addr) => addr as *mut u8,
                None => return errno::to_result(errno::EFAULT),
            };

            if buf_addr.is_null() {
                return errno::to_result(errno::EFAULT);
            }

            let remaining = data.len() - data_offset;
            let to_copy = remaining.min(iovec.iov_len);
            let buffer = unsafe { core::slice::from_raw_parts_mut(buf_addr, to_copy) };
            buffer.copy_from_slice(&data[data_offset..data_offset + to_copy]);
            data_offset += to_copy;
            total_read += to_copy;
        }
    } else {
        // No handle received; read data from stream
        for iovec in iovecs {
            if iovec.iov_len == 0 {
                continue;
            }

            let buf_addr = match task.vm_manager.translate_to_kva(iovec.iov_base as usize) {
                Some(addr) => addr as *mut u8,
                None => {
                    return errno::to_result(errno::EFAULT);
                }
            };

            if buf_addr.is_null() {
                return errno::to_result(errno::EFAULT);
            }

            let buffer = unsafe { core::slice::from_raw_parts_mut(buf_addr, iovec.iov_len) };

            match stream.read(buffer) {
                Ok(n) => {
                    total_read = total_read.saturating_add(n);
                    if n < iovec.iov_len {
                        break;
                    }
                }
                Err(StreamError::WouldBlock) => {
                    return if total_read == 0 {
                        errno::to_result(errno::EAGAIN)
                    } else {
                        total_read
                    };
                }
                Err(_) => {
                    return errno::to_result(errno::EIO);
                }
            }
        }
    }

    if let Some(fd_value) = pending_fd {
        let cmsg_addr = match task.vm_manager.translate_to_kva(msg.msg_control as usize) {
            Some(addr) => addr as *mut LinuxCmsghdr,
            None => return errno::to_result(errno::EFAULT),
        };

        if cmsg_addr.is_null() {
            return errno::to_result(errno::EFAULT);
        }

        unsafe {
            (*cmsg_addr).cmsg_len = size_of::<LinuxCmsghdr>() + size_of::<i32>();
            (*cmsg_addr).cmsg_level = SOL_SOCKET;
            (*cmsg_addr).cmsg_type = SCM_RIGHTS;
            let data_ptr = cmsg_addr.add(1) as *mut i32;
            *data_ptr = fd_value;
        }

        msg_controllen = size_of::<LinuxCmsghdr>() + size_of::<i32>();
    }

    unsafe {
        (*msg_addr).msg_flags = 0;
        (*msg_addr).msg_controllen = msg_controllen as u64;
    }

    total_read
}

/// Linux sys_sendto implementation
///
/// Send a message on a socket. Unlike sendmsg, this directly takes a buffer and address.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: buf (pointer to data buffer)
///   - arg2: len (length of data)
///   - arg3: flags (send flags)
///   - arg4: dest_addr (pointer to destination address, may be NULL for connected sockets)
///   - arg5: addrlen (size of destination address)
///
/// Returns:
/// - number of bytes sent on success
/// - negative errno on error
pub fn sys_sendto(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::ESRCH),
    };

    let sockfd = trapframe.get_arg(0);
    let buf_ptr = trapframe.get_arg(1);
    let len = trapframe.get_arg(2);
    let flags = trapframe.get_arg(3) as u32;
    let dest_addr_ptr = trapframe.get_arg(4);
    let addrlen = trapframe.get_arg(5) as u32;

    trapframe.increment_pc_next(task);

    // Get socket handle
    let handle = match abi.get_handle(sockfd) {
        Some(h) => h,
        None => return errno::to_result(errno::EBADF),
    };

    // Get socket object
    let socket = match task.handle_table.get(handle) {
        Some(KernelObject::Socket(s)) => s.clone(),
        _ => return errno::to_result(errno::ENOTSOCK),
    };

    // Translate buffer pointer
    let buf_kaddr = match task.vm_manager.translate_to_kva(buf_ptr) {
        Some(addr) => addr,
        None => return errno::to_result(errno::EFAULT),
    };

    let data = unsafe { core::slice::from_raw_parts(buf_kaddr as *const u8, len) };

    // Parse destination address if provided
    let dest_addr = if dest_addr_ptr != 0 && addrlen > 0 {
        let addr_kaddr = match task.vm_manager.translate_to_kva(dest_addr_ptr) {
            Some(addr) => addr,
            None => return errno::to_result(errno::EFAULT),
        };

        // Read address family
        let sa_family = unsafe { *(addr_kaddr as *const u16) };

        match sa_family {
            AF_INET_U16 => {
                if addrlen < size_of::<SockaddrIn>() as u32 {
                    return errno::to_result(errno::EINVAL);
                }
                let sockaddr = unsafe { *(addr_kaddr as *const SockaddrIn) };
                let port = u16::from_be(sockaddr.sin_port);
                let addr_bytes = sockaddr.sin_addr.to_be_bytes();
                crate::network::SocketAddress::Inet(crate::network::Inet4SocketAddress::new(
                    addr_bytes, port,
                ))
            }
            AF_UNIX_U16 => {
                // Unix domain socket sendto - usually not used for stream sockets
                crate::network::SocketAddress::Unspecified
            }
            _ => return errno::to_result(errno::EAFNOSUPPORT),
        }
    } else {
        // No address - use connected peer
        crate::network::SocketAddress::Unspecified
    };

    // Send data
    match socket.sendto(data, &dest_addr, flags) {
        Ok(n) => n,
        Err(crate::network::socket::SocketError::WouldBlock) => errno::to_result(errno::EAGAIN),
        Err(crate::network::socket::SocketError::NotConnected) => errno::to_result(errno::ENOTCONN),
        Err(crate::network::socket::SocketError::NoRoute) => errno::to_result(errno::ENETUNREACH),
        Err(crate::network::socket::SocketError::InvalidAddress) => errno::to_result(errno::EINVAL),
        Err(_) => errno::to_result(errno::EIO),
    }
}

/// Linux sys_recvfrom implementation
///
/// Receive a message from a socket. Returns the source address if provided.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: buf (pointer to receive buffer)
///   - arg2: len (length of buffer)
///   - arg3: flags (receive flags)
///   - arg4: src_addr (pointer to store source address, may be NULL)
///   - arg5: addrlen (pointer to address length, input/output)
///
/// Returns:
/// - number of bytes received on success
/// - negative errno on error
pub fn sys_recvfrom(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::ESRCH),
    };

    let sockfd = trapframe.get_arg(0);
    let buf_ptr = trapframe.get_arg(1);
    let len = trapframe.get_arg(2);
    let flags = trapframe.get_arg(3) as u32;
    let src_addr_ptr = trapframe.get_arg(4);
    let addrlen_ptr = trapframe.get_arg(5);

    trapframe.increment_pc_next(task);

    // Get socket handle
    let handle = match abi.get_handle(sockfd) {
        Some(h) => h,
        None => return errno::to_result(errno::EBADF),
    };

    // Get socket object
    let socket = match task.handle_table.get(handle) {
        Some(KernelObject::Socket(s)) => s.clone(),
        _ => return errno::to_result(errno::ENOTSOCK),
    };

    // Translate buffer pointer
    let buf_kaddr = match task.vm_manager.translate_to_kva(buf_ptr) {
        Some(addr) => addr,
        None => return errno::to_result(errno::EFAULT),
    };

    let buffer = unsafe { core::slice::from_raw_parts_mut(buf_kaddr as *mut u8, len) };

    // Check for non-blocking mode
    let nonblocking = (flags & (MSG_DONTWAIT as u32)) != 0
        || abi
            .get_file_status_flags(sockfd)
            .map(|f| ((f as i32) & O_NONBLOCK) != 0)
            .unwrap_or(false);

    // Set non-blocking if requested
    if let Some(selectable) = socket.as_selectable() {
        if nonblocking {
            selectable.set_nonblocking(true);
        }
    }

    // Receive data
    let result = socket.recvfrom(buffer, flags);

    // Restore blocking mode if we changed it
    if nonblocking {
        if let Some(selectable) = socket.as_selectable() {
            selectable.set_nonblocking(false);
        }
    }

    match result {
        Ok((n, src_addr)) => {
            // Store source address if requested
            if src_addr_ptr != 0 && addrlen_ptr != 0 {
                let addrlen_paddr = match task.vm_manager.translate_to_kva(addrlen_ptr) {
                    Some(addr) => addr as *mut u32,
                    None => return errno::to_result(errno::EFAULT),
                };

                let provided_len = unsafe { *addrlen_paddr };

                match src_addr {
                    crate::network::SocketAddress::Inet(inet) => {
                        if provided_len >= size_of::<SockaddrIn>() as u32 {
                            let addr_paddr = match task.vm_manager.translate_to_kva(src_addr_ptr) {
                                Some(addr) => addr as *mut SockaddrIn,
                                None => return errno::to_result(errno::EFAULT),
                            };

                            let sockaddr = SockaddrIn {
                                sin_family: AF_INET as u16,
                                sin_port: inet.port.to_be(),
                                sin_addr: u32::from_be_bytes(inet.addr),
                                sin_zero: [0; 8],
                            };
                            unsafe {
                                *addr_paddr = sockaddr;
                                *addrlen_paddr = size_of::<SockaddrIn>() as u32;
                            }
                        }
                    }
                    crate::network::SocketAddress::Local(_) => {
                        // Unix domain socket - store sockaddr_un
                        unsafe {
                            *addrlen_paddr = 0;
                        }
                    }
                    crate::network::SocketAddress::Unspecified => unsafe {
                        *addrlen_paddr = 0;
                    },
                    _ => unsafe {
                        *addrlen_paddr = 0;
                    },
                }
            }
            n
        }
        Err(crate::network::socket::SocketError::WouldBlock) => errno::to_result(errno::EAGAIN),
        Err(crate::network::socket::SocketError::NotConnected) => errno::to_result(errno::ENOTCONN),
        Err(_) => errno::to_result(errno::EIO),
    }
}

/// Linux sys_socketpair implementation
///
/// Create a pair of connected sockets. This is primarily used for AF_UNIX sockets
/// to create a bidirectional communication channel between processes.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: domain (address family, must be AF_UNIX)
///   - arg1: type (socket type, e.g., SOCK_STREAM)
///   - arg2: protocol (usually 0)
///   - arg3: sv (pointer to int[2] to receive the file descriptors)
///
/// Returns:
/// - 0 on success
/// - negative errno on error
pub fn sys_socketpair(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::ESRCH),
    };

    let domain = trapframe.get_arg(0) as i32;
    let socket_type = trapframe.get_arg(1) as i32;
    let _protocol = trapframe.get_arg(2) as i32;
    let sv_ptr = trapframe.get_arg(3);

    trapframe.increment_pc_next(task);

    // Validate domain - socketpair only supports AF_UNIX
    if domain != AF_UNIX {
        return errno::to_result(errno::EAFNOSUPPORT);
    }

    // Extract socket type flags
    let base_type = socket_type & SOCK_TYPE_MASK;
    let flags = socket_type & !SOCK_TYPE_MASK;
    let nonblocking = (flags & SOCK_NONBLOCK) != 0;
    let cloexec = (flags & SOCK_CLOEXEC) != 0;

    // Validate socket type - we support SOCK_STREAM and SOCK_DGRAM
    if base_type != SOCK_STREAM && base_type != SOCK_DGRAM {
        return errno::to_result(errno::ESOCKTNOSUPPORT);
    }

    // Translate sv pointer (needs to write 2 i32 values)
    let sv_paddr = match task.vm_manager.translate_to_kva(sv_ptr) {
        Some(addr) => addr as *mut i32,
        None => return errno::to_result(errno::EFAULT),
    };

    // Create connected socket pair
    let (socket1, socket2) = LocalSocket::create_connected_pair(
        alloc::string::String::from("socketpair:0"),
        alloc::string::String::from("socketpair:1"),
    );

    // Set non-blocking mode if requested
    if nonblocking {
        socket1.set_nonblocking(true);
        socket2.set_nonblocking(true);
    }

    // Add first socket to handle table
    let kernel_obj1 = KernelObject::Socket(socket1);
    let handle1 = match task.handle_table.insert(kernel_obj1) {
        Ok(id) => id,
        Err(_) => return errno::to_result(errno::EMFILE),
    };

    // Add second socket to handle table
    let kernel_obj2 = KernelObject::Socket(socket2);
    let handle2 = match task.handle_table.insert(kernel_obj2) {
        Ok(id) => id,
        Err(_) => {
            // Clean up handle1 if handle2 allocation fails
            let _ = task.handle_table.remove(handle1);
            return errno::to_result(errno::EMFILE);
        }
    };

    // Allocate file descriptors
    let fd1 = match abi.allocate_fd(handle1) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = task.handle_table.remove(handle1);
            let _ = task.handle_table.remove(handle2);
            return errno::to_result(errno::EMFILE);
        }
    };

    let fd2 = match abi.allocate_fd(handle2) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = abi.remove_fd(fd1);
            let _ = task.handle_table.remove(handle1);
            let _ = task.handle_table.remove(handle2);
            return errno::to_result(errno::EMFILE);
        }
    };

    // Set flags
    if nonblocking {
        let _ = abi.set_file_status_flags(fd1, O_NONBLOCK as u32);
        let _ = abi.set_file_status_flags(fd2, O_NONBLOCK as u32);
    }
    if cloexec {
        let _ = abi.set_fd_flags(fd1, FD_CLOEXEC);
        let _ = abi.set_fd_flags(fd2, FD_CLOEXEC);
    }

    // Write file descriptors to user space
    unsafe {
        *sv_paddr = fd1 as i32;
        *sv_paddr.add(1) = fd2 as i32;
    }

    0
}

/// Linux sys_shutdown implementation
///
/// Shut down part of a full-duplex connection.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: how (0=SHUT_RD, 1=SHUT_WR, 2=SHUT_RDWR)
///
/// Returns:
/// - 0 on success
/// - negative errno on error
pub fn sys_shutdown(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::ESRCH),
    };

    let sockfd = trapframe.get_arg(0);
    let how = trapframe.get_arg(1) as u32;

    trapframe.increment_pc_next(task);

    // Get socket handle
    let handle = match abi.get_handle(sockfd) {
        Some(h) => h,
        None => return errno::to_result(errno::EBADF),
    };

    // Get socket object
    let socket = match task.handle_table.get(handle) {
        Some(KernelObject::Socket(s)) => s.clone(),
        _ => return errno::to_result(errno::ENOTSOCK),
    };

    // Convert how to ShutdownHow
    let shutdown_how = match how {
        0 => crate::network::socket::ShutdownHow::Read,
        1 => crate::network::socket::ShutdownHow::Write,
        2 => crate::network::socket::ShutdownHow::Both,
        _ => return errno::to_result(errno::EINVAL),
    };

    match socket.shutdown(shutdown_how) {
        Ok(()) => 0,
        Err(crate::network::socket::SocketError::NotConnected) => errno::to_result(errno::ENOTCONN),
        Err(_) => errno::to_result(errno::EIO),
    }
}
