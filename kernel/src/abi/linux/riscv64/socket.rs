use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi,
    arch::Trapframe,
    ipc::pipe::UnidirectionalPipe,
    network::{NetworkManager, SocketDomain, SocketProtocol, SocketType, local::LocalSocket},
    object::KernelObject,
    task::mytask,
};
use alloc::sync::Arc;

/// Linux socket domains
pub const AF_UNIX: i32 = 1; // Unix domain sockets
pub const AF_INET: i32 = 2; // Internet IP Protocol
pub const AF_INET6: i32 = 10; // IP version 6

/// Linux socket types
pub const SOCK_STREAM: i32 = 1; // Stream socket
pub const SOCK_DGRAM: i32 = 2; // Datagram socket
pub const SOCK_RAW: i32 = 3; // Raw socket
pub const SOCK_SEQPACKET: i32 = 5; // Sequenced packet socket

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

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Map Linux socket domain to Scarlet domain
    let scarlet_domain = match domain {
        AF_UNIX => SocketDomain::Local,
        AF_INET => SocketDomain::Inet,
        AF_INET6 => SocketDomain::Inet6,
        _ => return usize::MAX, // Unsupported domain
    };

    // Map Linux socket type to Scarlet type
    let scarlet_type = match socket_type {
        SOCK_STREAM => SocketType::Stream,
        SOCK_DGRAM => SocketType::Datagram,
        SOCK_RAW => SocketType::Raw,
        SOCK_SEQPACKET => SocketType::SeqPacket,
        _ => return usize::MAX, // Unsupported type
    };

    // For now, only support AF_UNIX (Local domain)
    if scarlet_domain != SocketDomain::Local {
        // Fall back to mock behavior for non-local sockets
        let (read_obj, _write_obj) = UnidirectionalPipe::create_pair(4096);
        return match task.handle_table.insert(read_obj) {
            Ok(handle) => match abi.allocate_fd(handle) {
                Ok(fd) => fd,
                Err(_) => {
                    let _ = task.handle_table.remove(handle);
                    usize::MAX
                }
            },
            Err(_) => usize::MAX,
        };
    }

    // Create a LocalSocket for AF_UNIX
    let socket = Arc::new(LocalSocket::new(scarlet_type, SocketProtocol::Default));

    // Create socket through NetworkManager (which assigns ID automatically)
    let socket_obj = match NetworkManager::get_manager().create_socket(
        scarlet_domain,
        scarlet_type,
        SocketProtocol::Default,
    ) {
        Ok(KernelObject::Socket(s)) => s,
        _ => return usize::MAX,
    };

    // Wrap in KernelObject
    let kernel_obj = KernelObject::Socket(socket_obj);

    // Insert into handle table
    match task.handle_table.insert(kernel_obj) {
        Ok(handle) => {
            // Allocate a file descriptor for the socket
            match abi.allocate_fd(handle) {
                Ok(fd) => fd,
                Err(_) => {
                    // Clean up on error
                    let _ = task.handle_table.remove(handle);
                    usize::MAX
                }
            }
        }
        Err(_) => usize::MAX,
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
        None => return usize::MAX,
    };

    // Get the socket object from handle table
    let socket_arc = match task.handle_table.get(handle_id) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => return usize::MAX,
    };

    // Translate address pointer to physical
    let addr_paddr = match task.vm_manager.translate_vaddr(addr_ptr) {
        Some(addr) => addr,
        None => return usize::MAX,
    };

    // Read sockaddr structure from userspace
    // sockaddr_un structure: { sa_family: u16, sun_path: [u8; 108] }
    if addrlen < 2 {
        return usize::MAX; // Too small
    }

    unsafe {
        let sa_family = *(addr_paddr as *const u16);

        // Only support AF_UNIX for now
        if sa_family != AF_UNIX as u16 {
            return usize::MAX;
        }

        // Read the socket path (starts at offset 2)
        let path_start = (addr_paddr + 2) as *const u8;
        let max_path_len = (addrlen - 2) as usize;

        // Find the null terminator or max length
        let mut path_len = 0;
        while path_len < max_path_len && *path_start.add(path_len) != 0 {
            path_len += 1;
        }

        if path_len == 0 || path_len > 108 {
            return usize::MAX; // Invalid path
        }

        // Convert to string
        let path_bytes = core::slice::from_raw_parts(path_start, path_len);
        let path = match core::str::from_utf8(path_bytes) {
            Ok(s) => s,
            Err(_) => return usize::MAX,
        };

        // Bind the socket to the address
        let socket_addr = match crate::network::LocalSocketAddress::from_path(path) {
            Ok(addr) => crate::network::SocketAddress::Local(addr),
            Err(_) => return usize::MAX,
        };

        if let Err(_) = socket_arc.bind(&socket_addr) {
            return usize::MAX;
        }

        // Get the socket ID from NetworkManager
        let socket_id = match NetworkManager::get_manager().get_socket_id(&socket_arc) {
            Some(id) => id,
            None => return usize::MAX, // Socket not found in NetworkManager
        };

        // Create socket file in VFS
        let vfs = match &task.vfs {
            Some(vfs) => vfs.clone(),
            None => {
                // Use global VFS if task doesn't have its own
                crate::fs::vfs_v2::manager::get_global_vfs_manager()
            }
        };

        let socket_file_type = crate::fs::FileType::Socket(crate::fs::SocketFileInfo { socket_id });

        // Create the socket file
        if let Err(_) = vfs.create_file(path, socket_file_type) {
            return usize::MAX;
        }

        0 // Success
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
pub fn sys_listen(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let _sockfd = trapframe.get_arg(0) as i32;
    let _backlog = trapframe.get_arg(1) as i32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Mock implementation - always succeed
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

    let _sockfd = trapframe.get_arg(0) as i32;
    let _addr_ptr = trapframe.get_arg(1);
    let _addrlen_ptr = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Mock implementation - create a new pipe as the "accepted" connection
    let (read_obj, _write_obj) = UnidirectionalPipe::create_pair(4096);

    match task.handle_table.insert(read_obj) {
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
pub fn sys_connect(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let _sockfd = trapframe.get_arg(0) as i32;
    let _addr_ptr = trapframe.get_arg(1);
    let _addrlen = trapframe.get_arg(2) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Mock implementation - always succeed
    0
}

/// Linux sys_getsockname implementation (mock)
///
/// Gets the current address of a socket. This is a mock implementation that
/// writes dummy data and succeeds to allow applications to proceed.
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
pub fn sys_getsockname(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let _sockfd = trapframe.get_arg(0) as i32;
    let addr_ptr = trapframe.get_arg(1);
    let addrlen_ptr = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Mock implementation - write minimal valid sockaddr and return success
    if let (Some(addr_paddr), Some(addrlen_paddr)) = (
        task.vm_manager.translate_vaddr(addr_ptr),
        task.vm_manager.translate_vaddr(addrlen_ptr),
    ) {
        unsafe {
            // Read the provided length
            let addrlen = *(addrlen_paddr as *const u32);

            // Write minimal sockaddr_un structure for Unix domain socket
            if addrlen >= 2 {
                let sockaddr = addr_paddr as *mut u16;
                *sockaddr = AF_UNIX as u16; // sa_family = AF_UNIX

                // Update the actual length used
                *(addrlen_paddr as *mut u32) = 2;
            }
        }
        0 // Success
    } else {
        usize::MAX // Invalid pointers
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
        task.vm_manager.translate_vaddr(optval_ptr),
        task.vm_manager.translate_vaddr(optlen_ptr),
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
