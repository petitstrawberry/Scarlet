use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi,
    abi::linux::riscv64::errno,
    abi::linux::riscv64::fs::{FD_CLOEXEC, IoVec, O_NONBLOCK},
    arch::Trapframe,
    ipc::pipe::UnidirectionalPipe,
    network::{NetworkManager, SocketDomain, SocketProtocol, SocketType, local::LocalSocket},
    object::capability::selectable::Selectable,
    object::KernelObject,
    sched::scheduler::get_scheduler,
    task::mytask,
};
use alloc::sync::Arc;
use core::mem::size_of;
use crate::ipc::IpcError;
use crate::object::capability::StreamError;

/// Linux socket domains
pub const AF_UNIX: i32 = 1; // Unix domain sockets
pub const AF_INET: i32 = 2; // Internet IP Protocol
pub const AF_INET6: i32 = 10; // IP version 6

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
        AF_INET => SocketDomain::Inet,
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
            Err(_) => {
                crate::early_println!("[linux socket] pipe insert failed");
                usize::MAX
            }
        };
    }

    // Mirror Scarlet Native: create LocalSocket directly for AF_UNIX.
    let local_socket = Arc::new(LocalSocket::new(scarlet_type, SocketProtocol::Default));
    LocalSocket::init_self_weak(&local_socket);
    let socket_obj: Arc<dyn crate::network::SocketObject> = local_socket;
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
    let addr_paddr = match task.vm_manager.translate_vaddr(addr_ptr) {
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

        // Only support AF_UNIX for now
        if sa_family != AF_UNIX as u16 {
            crate::early_println!("[linux socket] bind unsupported family {}", sa_family);
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
        let vfs = match &task.vfs {
            Some(vfs) => vfs.clone(),
            None => {
                // Use global VFS if task doesn't have its own
                crate::fs::vfs_v2::manager::get_global_vfs_manager()
            }
        };

        let socket_file_type = crate::fs::FileType::Socket(crate::fs::SocketFileInfo { socket_id });

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

    let local_socket = match LocalSocket::from_socket_object(&socket_obj) {
        Some(socket) => socket,
        None => {
            crate::early_println!("[linux socket] accept not LocalSocket");
            return usize::MAX;
        }
    };

    let accepted_socket = match local_socket.accept_blocking(task.get_id(), trapframe) {
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

    let addr_paddr = match task.vm_manager.translate_vaddr(addr_ptr) {
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
        if sa_family != AF_UNIX as u16 {
            crate::early_println!("[linux socket] connect unsupported family {}", sa_family);
            return usize::MAX;
        }

        let path_start = (addr_paddr + 2) as *const u8;
        let max_path_len = (addrlen - 2) as usize;
        let mut path_len = 0;
        while path_len < max_path_len && *path_start.add(path_len) != 0 {
            path_len += 1;
        }

        if path_len == 0 || path_len > 108 {
            crate::early_println!("[linux socket] connect invalid path length {}", path_len);
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

/// Linux sys_sendmsg implementation (minimal)
pub fn sys_sendmsg(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as usize;
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

    let msg_addr = match task.vm_manager.translate_vaddr(msg_ptr) {
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

    let iovec_addr = match task.vm_manager.translate_vaddr(msg.msg_iov as usize) {
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
                let cmsg_addr = match task.vm_manager.translate_vaddr(msg.msg_control as usize) {
                    Some(addr) => addr as *const LinuxCmsghdr,
                    None => {
                        crate::early_println!("[linux socket] sendmsg bad cmsg ptr {:x}", msg.msg_control);
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
                                    crate::early_println!("[linux socket] sendmsg bad fd in cmsg {}", fd);
                                    return errno::to_result(errno::EBADF);
                                }
                            };
                            let dup_obj = match task.handle_table.clone_for_dup(send_handle) {
                                Some(obj) => obj,
                                None => {
                                    crate::early_println!("[linux socket] sendmsg clone_for_dup failed");
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

        let buf_addr = match task.vm_manager.translate_vaddr(iovec.iov_base as usize) {
            Some(addr) => addr as *const u8,
            None => {
                crate::early_println!("[linux socket] sendmsg bad buf ptr {:x}", iovec.iov_base as usize);
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

    let sockfd = trapframe.get_arg(0) as usize;
    let msg_ptr = trapframe.get_arg(1);
    let flags = trapframe.get_arg(2) as i32;
    crate::early_println!(
        "[linux recvmsg] fd={} msg_ptr={:#x} flags={:#x}",
        sockfd,
        msg_ptr,
        flags
    );

    trapframe.increment_pc_next(task);

    let handle = match abi.get_handle(sockfd) {
        Some(h) => h,
        None => {
            crate::early_println!("[linux socket] recvmsg bad fd {}", sockfd);
            return errno::to_result(errno::EBADF);
        }
    };

    crate::early_println!("[linux recvmsg] handle={}", handle);

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

    let msg_addr = match task.vm_manager.translate_vaddr(msg_ptr) {
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
    crate::early_println!(
        "[linux recvmsg] iov_ptr={:#x} iovlen={} control_ptr={:#x} controllen={}",
        msg.msg_iov,
        msg.msg_iovlen,
        msg.msg_control,
        msg.msg_controllen
    );
    let iovcnt = msg.msg_iovlen as usize;
    if iovcnt == 0 {
        return 0;
    }

    const IOV_MAX: usize = 1024;
    if iovcnt > IOV_MAX {
        return errno::to_result(errno::EINVAL);
    }

    let iovec_addr = match task.vm_manager.translate_vaddr(msg.msg_iov as usize) {
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
    crate::early_println!("[linux recvmsg] iovcnt={}", iovecs.len());
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

    if msg.msg_control != 0 {
        if (msg.msg_controllen as usize) >= size_of::<LinuxCmsghdr>() + size_of::<i32>() {
            let socket_arc = match &kernel_obj {
                KernelObject::Socket(socket) => Arc::clone(socket),
                _ => {
                    crate::early_println!("[linux socket] recvmsg not socket for cmsg");
                    return errno::to_result(errno::ENOTSOCK);
                }
            };

            if let Some(local_socket) = LocalSocket::from_socket_object(&socket_arc) {
                let recv_result = local_socket.recv_handle();

                match recv_result {
                    Ok(obj) => {
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
                    }
                    Err(IpcError::ChannelEmpty) => {
                        // No ancillary data; still allow data receive to proceed.
                    }
                    Err(_) => {
                        crate::early_println!("[linux socket] recvmsg recv_handle failed");
                        return errno::to_result(errno::EIO);
                    }
                }
            }
        }
    }

    for iovec in iovecs {
        if iovec.iov_len == 0 {
            continue;
        }

        let buf_addr = match task.vm_manager.translate_vaddr(iovec.iov_base as usize) {
            Some(addr) => addr as *mut u8,
            None => {
                crate::early_println!("[linux socket] recvmsg bad buf ptr {:x}", iovec.iov_base as usize);
                return errno::to_result(errno::EFAULT);
            }
        };

        if buf_addr.is_null() {
            crate::early_println!("[linux socket] recvmsg null buf ptr");
            return errno::to_result(errno::EFAULT);
        }

        let buffer = unsafe { core::slice::from_raw_parts_mut(buf_addr, iovec.iov_len) };

        crate::early_println!(
            "[linux recvmsg] read attempt len={}",
            iovec.iov_len
        );
        match stream.read(buffer) {
            Ok(n) => {
                crate::early_println!("[linux recvmsg] read ok n={}", n);
                total_read = total_read.saturating_add(n);
                if n < iovec.iov_len {
                    break;
                }
            }
            Err(StreamError::WouldBlock) => {
                crate::early_println!(
                    "[linux recvmsg] would block total_read={}",
                    total_read
                );
                return if total_read == 0 {
                    errno::to_result(errno::EAGAIN)
                } else {
                    total_read
                };
            }
            Err(_) => {
                crate::early_println!("[linux socket] recvmsg read error");
                return errno::to_result(errno::EIO);
            }
        }
    }

    if let Some(fd_value) = pending_fd {
        let cmsg_addr = match task.vm_manager.translate_vaddr(msg.msg_control as usize) {
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
