pub(crate) mod uevent;
pub(crate) mod netlink;
#[path = "local_datagram.rs"]
mod datagram;

use crate::ipc::IpcError;
use crate::object::capability::StreamError;
use crate::{
    abi::linux::generic::LinuxAbi,
    abi::linux::generic::errno,
    abi::linux::generic::fs::{FD_CLOEXEC, IoVec, O_NONBLOCK},
    arch::Trapframe,
    library::std::usercopy::{copy_from_user, copy_to_user},
    network::{
        NetworkManager, SocketDomain, SocketError, SocketProtocol, SocketType,
        local::{LocalSocket, MAX_HANDLE_QUEUE_SIZE, MAX_SEQPACKET_MESSAGE_SIZE},
    },
    object::KernelObject,
    object::capability::selectable::Selectable,
    task::mytask,
};
use alloc::string::String;
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

    fn from_inet(address: &crate::network::Inet4SocketAddress) -> Self {
        Self {
            sin_family: AF_INET_U16,
            sin_port: address.port.to_be(),
            // sin_addr is an octet sequence in memory, not a host-order integer.
            sin_addr: u32::from_ne_bytes(address.addr),
            sin_zero: [0; 8],
        }
    }

    fn to_inet(self) -> crate::network::Inet4SocketAddress {
        crate::network::Inet4SocketAddress::new(
            self.sin_addr.to_ne_bytes(),
            u16::from_be(self.sin_port),
        )
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
pub const SO_TYPE: i32 = 3;
pub const SO_ERROR: i32 = 4;
pub const SO_RCVTIMEO: i32 = 20;
pub const SO_SNDTIMEO: i32 = 21;
pub const SCM_RIGHTS: i32 = 1;
pub const SCM_CREDENTIALS: i32 = 2;
pub const SO_PASSCRED: i32 = 16;
pub const SO_PEERCRED: i32 = 17;
pub const MSG_DONTWAIT: i32 = 0x40;
pub const MSG_PEEK: u32 = 0x2;
pub const MSG_TRUNC: u32 = 0x20;

// Enable only while tracing Mozc's Linux IPC compatibility path.
const MOZC_IPC_TRACE_ENABLED: bool = false;

const MICROSECONDS_PER_SECOND: u128 = 1_000_000;
const NANOSECONDS_PER_MILLISECOND: u128 = 1_000_000;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinuxTimeval64 {
    tv_sec: i64,
    tv_usec: i64,
}

fn linux_timeval64_timeout_ms(timeval: LinuxTimeval64) -> Result<usize, usize> {
    if timeval.tv_sec < 0
        || timeval.tv_usec < 0
        || timeval.tv_usec as u128 >= MICROSECONDS_PER_SECOND
    {
        return Err(errno::EINVAL);
    }

    let nanoseconds = (timeval.tv_sec as u128)
        .saturating_mul(1_000_000_000)
        .saturating_add((timeval.tv_usec as u128).saturating_mul(1_000));
    let milliseconds = nanoseconds
        .saturating_add(NANOSECONDS_PER_MILLISECOND - 1)
        / NANOSECONDS_PER_MILLISECOND;
    if milliseconds > i32::MAX as u128 {
        return Err(errno::EINVAL);
    }
    Ok(milliseconds as usize)
}

fn timeout_ms_to_linux_timeval64(timeout_ms: usize) -> LinuxTimeval64 {
    let timeout_ms = timeout_ms as u64;
    LinuxTimeval64 {
        tv_sec: (timeout_ms / 1_000) as i64,
        tv_usec: ((timeout_ms % 1_000) * 1_000) as i64,
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxUcred {
    pid: i32,
    uid: u32,
    gid: u32,
}

fn is_mozc_server_task(task: &crate::task::Task) -> bool {
    let matches_name = |name: &str| {
        name.ends_with("/mozc_server")
            || name.ends_with("/mozc-server")
            || name == "mozc_server"
            || name == "mozc-server"
    };
    task.executable_path()
        .is_some_and(|path| matches_name(&path))
        || matches_name(&task.name.read())
}

fn socket_error_to_errno(error: SocketError) -> usize {
    match error {
        SocketError::PermissionDenied => errno::EACCES,
        SocketError::DestinationRequired => errno::EDESTADDRREQ,
        SocketError::MessageTooLarge => errno::EMSGSIZE,
        SocketError::ProtocolMismatch => errno::EPROTOTYPE,
        SocketError::BrokenPipe => errno::EPIPE,
        SocketError::InvalidAddress | SocketError::InvalidArgument => errno::EINVAL,
        SocketError::AddressInUse => errno::EADDRINUSE,
        SocketError::AddressNotAvailable => errno::EADDRNOTAVAIL,
        SocketError::ConnectionRefused => errno::ECONNREFUSED,
        SocketError::ConnectionReset => errno::ECONNRESET,
        SocketError::ConnectionAborted => errno::ECONNABORTED,
        SocketError::NotConnected => errno::ENOTCONN,
        SocketError::AlreadyConnected => errno::EISCONN,
        SocketError::InvalidOperation => errno::EINVAL,
        SocketError::NotListening => errno::EINVAL,
        SocketError::NoConnections | SocketError::WouldBlock | SocketError::TransmitBusy => {
            errno::EAGAIN
        }
        SocketError::Interrupted => errno::EINTR,
        SocketError::TimedOut => errno::ETIMEDOUT,
        SocketError::NotSupported => errno::EOPNOTSUPP,
        SocketError::NoRoute => errno::ENETUNREACH,
        SocketError::ProtocolNotSupported => errno::EPROTONOSUPPORT,
        SocketError::InvalidPacket | SocketError::Other(_) => errno::EIO,
    }
}

fn socket_io_error_to_errno(error: SocketError) -> usize {
    match error {
        SocketError::TimedOut | SocketError::WouldBlock => errno::EAGAIN,
        SocketError::Interrupted => errno::EINTR,
        error => socket_error_to_errno(error),
    }
}

fn inet_connect_error_to_errno(error: SocketError) -> usize {
    match error {
        SocketError::WouldBlock => errno::EINPROGRESS,
        error => socket_error_to_errno(error),
    }
}

fn socket_status_option(
    socket: &dyn crate::network::SocketObject,
    option: i32,
) -> Result<i32, usize> {
    match option {
        SO_TYPE => Ok(match socket.socket_type() {
            SocketType::Stream => SOCK_STREAM,
            SocketType::Datagram => SOCK_DGRAM,
            SocketType::Raw => SOCK_RAW,
            SocketType::SeqPacket => SOCK_SEQPACKET,
        }),
        SO_ERROR => Ok(socket
            .take_pending_error()
            .map(socket_error_to_errno)
            .unwrap_or(0) as i32),
        _ => Err(errno::ENOPROTOOPT),
    }
}

fn write_socket_address_to_user(
    task: &crate::task::Task,
    addr_ptr: usize,
    addrlen_ptr: usize,
    socket_addr: crate::network::SocketAddress,
) -> Result<(), usize> {
    if addr_ptr == 0 || addrlen_ptr == 0 {
        return Ok(());
    }

    let addr_paddr = task
        .vm_manager
        .translate_to_kva(addr_ptr)
        .ok_or(errno::EFAULT)?;
    let addrlen_paddr = task
        .vm_manager
        .translate_to_kva(addrlen_ptr)
        .ok_or(errno::EFAULT)?;

    unsafe {
        let addrlen = *(addrlen_paddr as *const u32);

        match socket_addr {
            crate::network::SocketAddress::Local(addr) => {
                if addrlen < 2 {
                    return Err(errno::EINVAL);
                }

                let sockaddr = addr_paddr as *mut u16;
                *sockaddr = AF_UNIX_U16;

                let available_path_len = (addrlen - 2) as usize;
                let path_start = (addr_paddr + 2) as *mut u8;
                let path_len = if addr.is_abstract() {
                    if available_path_len == 0 {
                        return Err(errno::EINVAL);
                    }
                    *path_start = 0;
                    let path = addr.path().as_bytes();
                    let copied = path.len().min(available_path_len - 1);
                    if copied > 0 {
                        core::ptr::copy_nonoverlapping(path.as_ptr(), path_start.add(1), copied);
                    }
                    copied + 1
                } else {
                    let path = addr.path().as_bytes();
                    let copied = path.len().min(available_path_len);
                    if copied > 0 {
                        core::ptr::copy_nonoverlapping(path.as_ptr(), path_start, copied);
                    }
                    if copied < available_path_len {
                        *(path_start.add(copied)) = 0;
                    }
                    copied
                };

                if path_len < available_path_len && addr.is_abstract() {
                    *(path_start.add(path_len)) = 0;
                }

                *(addrlen_paddr as *mut u32) = 2 + path_len as u32;
                Ok(())
            }
            crate::network::SocketAddress::Inet(inet) => {
                if addrlen < size_of::<SockaddrIn>() as u32 {
                    return Err(errno::EINVAL);
                }

                let sockaddr = addr_paddr as *mut SockaddrIn;
                core::ptr::write_unaligned(sockaddr, SockaddrIn::from_inet(&inet));

                *(addrlen_paddr as *mut u32) = size_of::<SockaddrIn>() as u32;
                Ok(())
            }
            _ => Err(errno::EAFNOSUPPORT),
        }
    }
}

fn linux_local_sockaddr_from_user(
    task: &crate::task::Task,
    path_start: *const u8,
    max_path_len: usize,
    binding: bool,
) -> Result<(crate::network::SocketAddress, String, bool), usize> {
    if max_path_len == 0 {
        return Err(errno::EINVAL);
    }

    let path_bytes = unsafe { core::slice::from_raw_parts(path_start, max_path_len) };

    if path_bytes[0] == 0 {
        let mut name_len = max_path_len.saturating_sub(1);
        while name_len > 0 && path_bytes[1 + name_len - 1] == 0 {
            name_len -= 1;
        }
        if name_len == 0 || name_len > 107 {
            crate::println!("[linux socket] invalid abstract socket length {}", name_len);
            return Err(errno::EINVAL);
        }
        let name = core::str::from_utf8(&path_bytes[1..1 + name_len]).map_err(|_| {
            crate::println!("[linux socket] abstract socket name utf8 error");
            errno::EINVAL
        })?;
        let addr = crate::network::LocalSocketAddress::from_abstract(name)
            .map_err(socket_error_to_errno)?;
        let mut registry_name = String::new();
        registry_name.push('\0');
        registry_name.push_str(addr.path());
        Ok((
            crate::network::SocketAddress::Local(addr),
            registry_name,
            true,
        ))
    } else {
        let mut path_len = 0;
        while path_len < max_path_len && path_bytes[path_len] != 0 {
            path_len += 1;
        }
        if path_len == 0 || path_len > 108 {
            crate::println!("[linux socket] invalid socket path length {}", path_len);
            return Err(errno::EINVAL);
        }
        let path = core::str::from_utf8(&path_bytes[..path_len]).map_err(|_| {
            crate::println!("[linux socket] socket path utf8 error");
            errno::EINVAL
        })?;
        // A relative pathname is scoped to the binding/connecting task's cwd.
        // Wine binds "socket" in each prefix's private server directory; using
        // that literal as a global registry key makes unrelated prefixes collide.
        let vfs = task.vfs.read().clone().ok_or(errno::EINVAL)?;
        let path = if binding {
            match vfs.resolve_path_with_options(
                path,
                &crate::fs::vfs_v2::PathResolutionOptions::no_follow(),
            ) {
                Ok(_) => return Err(errno::EADDRINUSE),
                Err(error) if errno::from_fs_error(&error) == errno::ENOENT => {}
                Err(error) => return Err(errno::from_fs_error(&error)),
            }
            let (parent, name) = path.rsplit_once('/').unwrap_or((".", path));
            if name.is_empty() {
                return Err(errno::EINVAL);
            }
            let (entry, mount) = vfs
                .resolve_path(if parent.is_empty() { "/" } else { parent })
                .map_err(|error| errno::from_fs_error(&error))?;
            if !entry
                .node()
                .is_directory()
                .map_err(|error| errno::from_fs_error(&error))?
            {
                return Err(errno::ENOTDIR);
            }
            let parent = vfs.build_absolute_path(&entry, &mount);
            if parent == "/" {
                alloc::format!("/{}", name)
            } else {
                alloc::format!("{}/{}", parent, name)
            }
        } else {
            let (entry, mount) = vfs
                .resolve_path(path)
                .map_err(|error| errno::from_fs_error(&error))?;
            vfs.build_absolute_path(&entry, &mount)
        };
        let addr =
            crate::network::LocalSocketAddress::from_path(&path).map_err(socket_error_to_errno)?;
        Ok((crate::network::SocketAddress::Local(addr), path, false))
    }
}

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
struct LinuxMmsghdr {
    msg_hdr: LinuxMsghdr,
    msg_len: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxCmsghdr {
    cmsg_len: usize,
    cmsg_level: i32,
    cmsg_type: i32,
}

fn read_linux_msghdr(task: &crate::task::Task, address: usize) -> Result<LinuxMsghdr, usize> {
    let mut message = unsafe { core::mem::zeroed::<LinuxMsghdr>() };
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(
            (&mut message as *mut LinuxMsghdr).cast::<u8>(),
            size_of::<LinuxMsghdr>(),
        )
    };
    copy_from_user(task, address, bytes).map_err(|_| errno::EFAULT)?;
    Ok(message)
}

fn read_linux_iovecs(
    task: &crate::task::Task,
    address: usize,
    count: usize,
) -> Result<Vec<IoVec>, usize> {
    let mut iovecs = alloc::vec![IoVec {
        iov_base: core::ptr::null_mut(),
        iov_len: 0,
    }; count];
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(
            iovecs.as_mut_ptr().cast::<u8>(),
            count * size_of::<IoVec>(),
        )
    };
    copy_from_user(task, address, bytes).map_err(|_| errno::EFAULT)?;
    Ok(iovecs)
}

/// Linux sys_socket implementation
///
/// Creates a socket endpoint for communication. Now properly integrated with
/// NetworkManager and VFS for Unix domain socket support.
///
/// Arguments:
/// - abi: LinuxAbi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: domain (communication domain, e.g., AF_UNIX, AF_INET)
///   - arg1: type (socket type, e.g., SOCK_STREAM, SOCK_DGRAM)
///   - arg2: protocol (protocol to use, usually 0)
///
/// Returns:
/// - file descriptor on success
/// - usize::MAX (Linux -1) on error
pub fn sys_socket(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
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
    trapframe.increment_pc_next(&task);

    // Android uses a close-only PF_KEY socket as an RCU quiescence barrier
    // after swapping BPF maps. Scarlet's map state is synchronously visible,
    // so provide the required socket lifetime without emulating PF_KEY data.
    if domain == 15 && _protocol == 2 && socket_base_type == SOCK_RAW {
        let socket: Arc<dyn crate::network::SocketObject> = match LocalSocket::try_new(
            SocketType::Datagram,
            SocketProtocol::Default,
        ) {
            Ok(socket) => {
                let socket = Arc::new(socket);
                LocalSocket::init_self_weak(&socket);
                socket
            }
            Err(error) => return errno::to_result(socket_error_to_errno(error)),
        };
        let _ = NetworkManager::get_manager().allocate_socket_id(Arc::clone(&socket));
        let handle = match task
            .handle_table
            .insert(KernelObject::from_socket_object(socket))
        {
            Ok(handle) => handle,
            Err(_) => return errno::to_result(errno::EMFILE),
        };
        let fd = match abi.allocate_fd(handle) {
            Ok(fd) => fd,
            Err(_) => {
                let _ = task.handle_table.remove(handle);
                return errno::to_result(errno::EMFILE);
            }
        };
        if set_cloexec {
            let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
        }
        if set_nonblock {
            let _ = abi.set_file_status_flags(fd, O_NONBLOCK as u32);
        }
        return fd;
    }

    if domain==16 {
        return match _protocol {
            15 => uevent::create(abi,&task,socket_base_type,socket_flags,_protocol),
            0 | 12 => netlink::create(abi,&task,socket_base_type,socket_flags,_protocol),
            _ => errno::to_result(errno::EPROTONOSUPPORT),
        };
    }

    // The native INET stack currently exposes IPv4 only. Reject IPv6 up front
    // so callers can fall back instead of receiving an IPv4-backed socket that
    // later fails when bound or connected as AF_INET6.
    if domain == AF_INET6 {
        return errno::to_result(errno::EAFNOSUPPORT);
    }

    // Map Linux socket domain to Scarlet domain
    let scarlet_domain = match domain {
        AF_UNIX => SocketDomain::Local,
        AF_INET => SocketDomain::Inet4,
        AF_INET6 => SocketDomain::Inet6,
        _ => {
            crate::println!("[linux socket] unsupported domain {}", domain);
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
            crate::println!("[linux socket] unsupported type {}", socket_type);
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
            let local_socket = match LocalSocket::try_new(scarlet_type,SocketProtocol::Default) {Ok(socket)=>Arc::new(socket),Err(error)=>return errno::to_result(socket_error_to_errno(error))};
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
                    crate::println!(
                        "[linux socket] failed to create INET socket protocol={:?}",
                        scarlet_protocol
                    );
                    return usize::MAX;
                }
            }
        }
        _ => {
            crate::println!("[linux socket] unsupported domain {:?}", scarlet_domain);
            return usize::MAX;
        }
    };
    if NetworkManager::get_manager()
        .allocate_socket_id(Arc::clone(&socket_obj))
        .is_err()
    {
        crate::println!("[linux socket] allocate_socket_id failed");
    }

    if set_nonblock {
        if let Some(local_socket) = LocalSocket::from_socket_object(socket_obj.as_ref()) {
            local_socket.set_nonblocking(true);
        }
    }

    // Wrap in KernelObject
    let kernel_obj = KernelObject::from_socket_object(socket_obj);

    // Insert into handle table
    match task.handle_table.insert(kernel_obj) {
        Ok(handle) => {
            // Allocate a file descriptor for the socket
            match abi.allocate_fd(handle) {
                Ok(fd) => {
                    if set_cloexec {
                        let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
                    }
                    if set_nonblock {
                        let _ = abi.set_file_status_flags(fd, O_NONBLOCK as u32);
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
            crate::println!("[linux socket] handle table insert failed");
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
/// - abi: LinuxAbi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: addr (pointer to socket address structure)
///   - arg2: addrlen (size of address structure)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_bind(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let addr_ptr = trapframe.get_arg(1);
    let addrlen = trapframe.get_arg(2) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(&task);
    if let Some(object)=abi.get_handle(sockfd as usize).and_then(|handle|task.handle_table.get(handle)) {
        if let Some(socket)=uevent::from_object(&object) {return socket.bind(&task,addr_ptr,addrlen as usize);}
        if let Some(socket)=netlink::from_object(&object) {return socket.bind(&task,addr_ptr,addrlen as usize);}
    }


    // Get the file descriptor handle
    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::println!("[linux socket] bind invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    // Get the socket object from handle table
    let socket_arc = match task.handle_table.get_arc_clone(handle_id) {
        Some(KernelObject::Socket(socket)) => socket,
        _ => {
            crate::println!("[linux socket] bind fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    // A sockaddr may straddle user pages, which need not be physically
    // contiguous. Copy it before parsing the family and Unix path.
    let mut addr_bytes = [0u8; 128];
    if addrlen < 2 || addrlen as usize > addr_bytes.len() {
        return errno::to_result(errno::EINVAL);
    }
    if copy_from_user(&task, addr_ptr, &mut addr_bytes[..addrlen as usize]).is_err() {
        return errno::to_result(errno::EFAULT);
    }
    let addr_paddr = addr_bytes.as_ptr() as usize;

    unsafe {
        let sa_family = *(addr_paddr as *const u16);

        match sa_family {
            AF_UNIX_U16 => {
                // Read the socket path (starts at offset 2)
                let path_start = (addr_paddr + 2) as *const u8;
                let max_path_len = (addrlen - 2) as usize;

                let (socket_addr, registry_name, is_abstract) =
                    match linux_local_sockaddr_from_user(&task, path_start, max_path_len, true) {
                        Ok(addr) => addr,
                        Err(errno) => return errno::to_result(errno),
                    };

                let log_name = registry_name
                    .strip_prefix('\0')
                    .unwrap_or(registry_name.as_str());
                if MOZC_IPC_TRACE_ENABLED && is_abstract && log_name.contains(".mozc.") {
                    crate::println!("[linux mozc-ipc] bind abstract '{}'", log_name);
                }

                if let Err(error) = NetworkManager::get_manager()
                    .register_named_socket(&registry_name, Arc::clone(socket_arc.as_arc()))
                {
                    return errno::to_result(socket_error_to_errno(error));
                }
                if let Err(error) = socket_arc.bind(&socket_addr) {
                    NetworkManager::get_manager()
                        .unregister_named_socket(&registry_name, socket_arc.as_ref());
                    return errno::to_result(socket_error_to_errno(error));
                }

                if is_abstract {
                    return 0;
                }

                let path = match socket_addr {
                    crate::network::SocketAddress::Local(ref addr) => addr.path(),
                    _ => return usize::MAX,
                };

                // Get the socket ID from NetworkManager
                let socket_id =
                    match NetworkManager::get_manager().get_socket_id(socket_arc.as_arc()) {
                        Some(id) => id,
                        None => {
                            crate::println!("[linux socket] get_socket_id failed {}", path);
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
                    crate::println!(
                        "[sys_bind] Warning: Failed to create VFS socket file at '{}': {:?}",
                        path,
                        e
                    );
                }

                0 // Success
            }
            AF_INET_U16 => {
                if addrlen < size_of::<SockaddrIn>() as u32 {
                    return errno::to_result(errno::EINVAL);
                }
                let addr_struct = core::ptr::read_unaligned(addr_paddr as *const SockaddrIn);
                let socket_addr = crate::network::SocketAddress::Inet(addr_struct.to_inet());

                if socket_arc.bind(&socket_addr).is_err() {
                    crate::println!("[linux socket] bind failed for INET address");
                    return usize::MAX;
                }

                0
            }
            _ => {
                crate::println!("[linux socket] bind unsupported family {}", sa_family);
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
/// - abi: LinuxAbi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: backlog (maximum queue length for pending connections)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_listen(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let backlog = trapframe.get_arg(1) as i32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(&task);

    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::println!("[linux socket] listen invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    let socket = match task
        .handle_table
        .get(handle_id)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => {
            crate::println!("[linux socket] listen fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    if socket.listen(backlog.max(0) as usize).is_err() {
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
/// - abi: LinuxAbi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: addr (pointer to socket address structure for peer)
///   - arg2: addrlen (pointer to size of address structure)
///
/// Returns:
/// - new socket file descriptor on success
/// - usize::MAX (Linux -1) indicating failure
fn accept_with_flags(abi: &mut LinuxAbi, trapframe: &mut Trapframe, flags: i32) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let addr_ptr = trapframe.get_arg(1);
    let addrlen_ptr = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(&task);

    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::println!("[linux socket] accept invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    let socket_obj = match task
        .handle_table
        .get(handle_id)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => {
            crate::println!("[linux socket] accept fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    // Try LocalSocket first, then TcpSocket
    let accepted_socket =
        if let Some(local_socket) = LocalSocket::from_socket_object(socket_obj.as_ref()) {
            local_socket.accept_blocking(task.get_id(), trapframe)
        } else if let Some(tcp_socket) =
            crate::network::tcp::TcpSocket::from_socket_object(socket_obj.as_ref())
        {
            tcp_socket.accept_blocking(task.get_id(), trapframe)
        } else {
            crate::println!("[linux socket] accept not supported socket type");
            return usize::MAX;
        };

    let accepted_socket = match accepted_socket {
        Ok(socket) => socket,
        Err(error) => {
            crate::println!("[linux socket] accept_blocking failed");
            return errno::to_result(socket_io_error_to_errno(error));
        }
    };

    let peer_addr = accepted_socket
        .getpeername()
        .unwrap_or(crate::network::SocketAddress::Local(
            crate::network::LocalSocketAddress::unnamed(),
        ));
    if let Err(errno) = write_socket_address_to_user(&task, addr_ptr, addrlen_ptr, peer_addr) {
        return errno::to_result(errno);
    }

    if (flags & SOCK_NONBLOCK) != 0 {
        if let Some(local_socket) = LocalSocket::from_socket_object(accepted_socket.as_ref()) {
            local_socket.set_nonblocking(true);
        }
    }

    let kernel_obj = KernelObject::from_socket_object(accepted_socket);
    match task.handle_table.insert(kernel_obj) {
        Ok(handle) => match abi.allocate_fd(handle) {
            Ok(fd) => {
                if (flags & SOCK_CLOEXEC) != 0 {
                    let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
                }
                if (flags & SOCK_NONBLOCK) != 0 {
                    let _ = abi.set_file_status_flags(fd, O_NONBLOCK as u32);
                }
                fd
            }
            Err(_) => {
                let _ = task.handle_table.remove(handle);
                usize::MAX
            }
        },
        Err(_) => usize::MAX,
    }
}

pub fn sys_accept(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    accept_with_flags(abi, trapframe, 0)
}

pub fn sys_accept4(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let flags = trapframe.get_arg(3) as i32;
    let supported = SOCK_CLOEXEC | SOCK_NONBLOCK;
    if (flags & !supported) != 0 {
        if let Some(task) = mytask() {
            trapframe.increment_pc_next(&task);
        }
        return errno::to_result(errno::EINVAL);
    }
    accept_with_flags(abi, trapframe, flags)
}

/// Linux sys_connect implementation (mock)
///
/// Connects a socket to an address. This is a mock implementation that
/// always succeeds to allow applications to proceed.
///
/// Arguments:
/// - abi: LinuxAbi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: addr (pointer to socket address structure)
///   - arg2: addrlen (size of address structure)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_connect(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let addr_ptr = trapframe.get_arg(1);
    let addrlen = trapframe.get_arg(2) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(&task);

    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::println!("[linux socket] connect invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    let socket_arc = match task
        .handle_table
        .get(handle_id)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => {
            crate::println!("[linux socket] connect fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    let mut addr_bytes = [0u8; 128];
    if addrlen < 2 || addrlen as usize > addr_bytes.len() {
        return errno::to_result(errno::EINVAL);
    }
    if copy_from_user(&task, addr_ptr, &mut addr_bytes[..addrlen as usize]).is_err() {
        return errno::to_result(errno::EFAULT);
    }
    let addr_paddr = addr_bytes.as_ptr() as usize;

    unsafe {
        let sa_family = *(addr_paddr as *const u16);
        match sa_family {
            0 if LocalSocket::from_socket_object(socket_arc.as_ref()).is_some_and(|local|local.is_datagram()) => return socket_arc.connect(&crate::network::SocketAddress::Unspecified).map_or_else(|error|errno::to_result(socket_error_to_errno(error)),|_|0),
            AF_UNIX_U16 => {
                let path_start = (addr_paddr + 2) as *const u8;
                let max_path_len = (addrlen - 2) as usize;
                let (socket_addr, _, _) =
                    match linux_local_sockaddr_from_user(&task, path_start, max_path_len, false) {
                        Ok(addr) => addr,
                        Err(errno) => return errno::to_result(errno),
                    };

                let mozc_path = if MOZC_IPC_TRACE_ENABLED {
                    match &socket_addr {
                        crate::network::SocketAddress::Local(address)
                            if address.path().contains(".mozc.") =>
                        {
                            Some(address.path())
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(path) = mozc_path {
                    crate::println!(
                        "[linux mozc-ipc] connect begin task_global={} task_local={} path='{}'",
                        task.get_id(),
                        task.get_namespace_id(),
                        path
                    );
                }
                if let Err(error) = socket_arc.connect(&socket_addr) {
                    if let Some(path) = mozc_path {
                        crate::println!(
                            "[linux mozc-ipc] connect failed path='{}' error={:?}",
                            path,
                            error
                        );
                    }
                    return errno::to_result(socket_error_to_errno(error));
                }
                if let Some(path) = mozc_path {
                    crate::println!("[linux mozc-ipc] connect ok path='{}'", path);
                };
            }
            AF_INET_U16 => {
                if addrlen < size_of::<SockaddrIn>() as u32 {
                    return errno::to_result(errno::EINVAL);
                }
                let addr_struct = core::ptr::read_unaligned(addr_paddr as *const SockaddrIn);
                let socket_addr = crate::network::SocketAddress::Inet(addr_struct.to_inet());

                if let Err(error) = socket_arc.connect(&socket_addr) {
                    crate::println!(
                        "[linux socket] connect failed for INET address: {:?}",
                        error
                    );
                    return errno::to_result(inet_connect_error_to_errno(error));
                }
            }
            _ => {
                crate::println!("[linux socket] connect unsupported family {}", sa_family);
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
/// - abi: LinuxAbi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: addr (pointer to socket address structure)
///   - arg2: addrlen (pointer to size of address structure)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) indicating failure
pub fn sys_getsockname(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let addr_ptr = trapframe.get_arg(1);
    let addrlen_ptr = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(&task);

    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::println!("[linux socket] getsockname invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    let socket_arc = match task
        .handle_table
        .get(handle_id)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => {
            crate::println!("[linux socket] getsockname fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    if !matches!(
        (
            task.vm_manager.translate_to_kva(addr_ptr),
            task.vm_manager.translate_to_kva(addrlen_ptr),
        ),
        (Some(_), Some(_))
    ) {
        crate::println!("[linux socket] getsockname invalid pointers");
        return usize::MAX;
    }

    let socket_addr = match socket_arc.getsockname() {
        Ok(addr) => addr,
        Err(_) => {
            crate::println!("[linux socket] getsockname failed");
            return usize::MAX;
        }
    };

    match write_socket_address_to_user(&task, addr_ptr, addrlen_ptr, socket_addr) {
        Ok(()) => 0,
        Err(errno) => errno::to_result(errno),
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
pub fn sys_getpeername(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let addr_ptr = trapframe.get_arg(1);
    let addrlen_ptr = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(&task);

    let handle_id = match abi.get_handle(sockfd as usize) {
        Some(h) => h,
        None => {
            crate::println!("[linux socket] getpeername invalid fd {}", sockfd);
            return usize::MAX;
        }
    };

    let socket_arc = match task
        .handle_table
        .get(handle_id)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => {
            crate::println!("[linux socket] getpeername fd {} not socket", sockfd);
            return usize::MAX;
        }
    };

    if !matches!(
        (
            task.vm_manager.translate_to_kva(addr_ptr),
            task.vm_manager.translate_to_kva(addrlen_ptr),
        ),
        (Some(_), Some(_))
    ) {
        crate::println!("[linux socket] getpeername invalid pointers");
        return usize::MAX;
    }

    let socket_addr = match socket_arc.getpeername() {
        Ok(addr) => addr,
        Err(_) => {
            crate::println!("[linux socket] getpeername failed");
            return usize::MAX;
        }
    };

    match write_socket_address_to_user(&task, addr_ptr, addrlen_ptr, socket_addr) {
        Ok(()) => 0,
        Err(errno) => errno::to_result(errno),
    }
}

/// Linux sys_getsockopt implementation
///
/// Gets socket options, including credentials and socket I/O timeouts, and
/// retains the compatibility fallback for unrelated options.
///
/// Arguments:
/// - abi: LinuxAbi context
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
pub fn sys_getsockopt(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let level = trapframe.get_arg(1) as i32;
    let optname = trapframe.get_arg(2) as i32;
    let optval_ptr = trapframe.get_arg(3);
    let optlen_ptr = trapframe.get_arg(4);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(&task);
    if let Some(object)=abi.get_handle(sockfd as usize).and_then(|handle|task.handle_table.get(handle)) {
        if let Some(socket)=uevent::from_object(&object) {return socket.get_option(&task,level,optname,optval_ptr,optlen_ptr);}
        if let Some(socket)=netlink::from_object(&object) {return socket.get_option(&task,level,optname,optval_ptr,optlen_ptr);}
    }


    let mut optlen_bytes = [0u8; size_of::<u32>()];
    if copy_from_user(&task, optlen_ptr, &mut optlen_bytes).is_err() {
        return errno::to_result(errno::EFAULT);
    }
    let optlen = u32::from_ne_bytes(optlen_bytes);

    if level == SOL_SOCKET && matches!(optname, SO_TYPE | SO_ERROR) {
        if optlen < size_of::<i32>() as u32 {
            return errno::to_result(errno::EINVAL);
        }
        let handle = match abi.get_handle(sockfd as usize) {
            Some(handle) => handle,
            None => return errno::to_result(errno::EBADF),
        };
        let socket = match task
            .handle_table
            .get(handle)
            .and_then(KernelObject::into_socket_arc)
        {
            Some(socket) => socket,
            None => return errno::to_result(errno::ENOTSOCK),
        };
        let value = match socket_status_option(socket.as_ref(), optname) {
            Ok(value) => value,
            Err(error) => return errno::to_result(error),
        };
        if copy_to_user(&task, optval_ptr, &value.to_ne_bytes()).is_err()
            || copy_to_user(&task, optlen_ptr, &(size_of::<i32>() as u32).to_ne_bytes()).is_err()
        {
            return errno::to_result(errno::EFAULT);
        }
        return 0;
    }

    if level == SOL_SOCKET && optname == SO_PASSCRED {
        if optlen < size_of::<i32>() as u32 {
            return errno::to_result(errno::EINVAL);
        }
        let handle = match abi.get_handle(sockfd as usize) {
            Some(handle) => handle,
            None => return errno::to_result(errno::EBADF),
        };
        let socket = match task
            .handle_table
            .get(handle)
            .and_then(KernelObject::into_socket_arc)
        {
            Some(socket) => socket,
            None => return errno::to_result(errno::ENOTSOCK),
        };
        let local_socket = match LocalSocket::from_socket_object(socket.as_ref()) {
            Some(socket) => socket,
            None => return errno::to_result(errno::ENOPROTOOPT),
        };
        let enabled = i32::from(local_socket.passcred());
        if copy_to_user(&task, optval_ptr, &enabled.to_ne_bytes()).is_err()
            || copy_to_user(&task, optlen_ptr, &(size_of::<i32>() as u32).to_ne_bytes()).is_err()
        {
            return errno::to_result(errno::EFAULT);
        }
        return 0;
    }

    if level == SOL_SOCKET && optname == 31 { // SO_PEERSEC
        let result = (|| {
            let handle = abi.get_handle(sockfd as usize).ok_or(errno::EBADF)?;
            let socket = task.handle_table.get(handle).and_then(KernelObject::into_socket_arc).ok_or(errno::ENOTSOCK)?;
            let local = LocalSocket::from_socket_object(socket.as_ref()).ok_or(errno::ENOPROTOOPT)?;
            if local.peer_credentials().is_none() {return Err(errno::ENOTCONN);}
            let subject = local.peer_security_label().ok_or(errno::ENODATA)?;
            let bytes = abi.extensions.peer_security_context(&task, &subject).ok_or(errno::ENOPROTOOPT)??;
            copy_to_user(&task, optlen_ptr, &(bytes.len() as u32).to_ne_bytes()).map_err(|_| errno::EFAULT)?;
            if (optlen as usize) < bytes.len() { return Err(errno::ERANGE); }
            copy_to_user(&task, optval_ptr, &bytes).map_err(|_| errno::EFAULT)?;
            Ok(0)
        })();
        return result.unwrap_or_else(errno::to_result);
    }

    if level == SOL_SOCKET && optname == SO_PEERCRED {
        if optlen < size_of::<LinuxUcred>() as u32 {
            return errno::to_result(errno::EINVAL);
        }
        let handle = match abi.get_handle(sockfd as usize) {
            Some(handle) => handle,
            None => return errno::to_result(errno::EBADF),
        };
        let socket = match task
            .handle_table
            .get(handle)
            .and_then(KernelObject::into_socket_arc)
        {
            Some(socket) => socket,
            None => return errno::to_result(errno::ENOTSOCK),
        };
        let local_socket = match LocalSocket::from_socket_object(socket.as_ref()) {
            Some(socket) => socket,
            None => return errno::to_result(errno::ENOPROTOOPT),
        };
        let peer_credentials = match local_socket.peer_credentials() {
            Some(credentials) => credentials,
            None => return errno::to_result(errno::ENOTCONN),
        };
        let pid = peer_credentials.pid_in(&task.get_namespace());
        let cred = LinuxUcred {
            pid: pid as i32,
            uid: peer_credentials.identity.principal.0,
            gid: peer_credentials.identity.group.0,
        };
        if MOZC_IPC_TRACE_ENABLED && is_mozc_server_task(&task) {
            crate::println!(
                "[linux mozc-ipc] peercred task_global={} task_local={} fd={} peer_pid={}",
                task.get_id(),
                task.try_get_namespace_id().unwrap_or(0),
                sockfd,
                pid
            );
        }
        if copy_to_user(&task, optval_ptr, unsafe {
            core::slice::from_raw_parts(
                (&cred as *const LinuxUcred).cast::<u8>(),
                size_of::<LinuxUcred>(),
            )
        })
        .is_err()
        {
            return errno::to_result(errno::EFAULT);
        }
        if copy_to_user(
            &task,
            optlen_ptr,
            &(size_of::<LinuxUcred>() as u32).to_ne_bytes(),
        )
        .is_err()
        {
            return errno::to_result(errno::EFAULT);
        }
        return 0;
    }

    if level == SOL_SOCKET && matches!(optname, SO_RCVTIMEO | SO_SNDTIMEO) {
        let timeval_size = size_of::<LinuxTimeval64>();
        if optlen < timeval_size as u32 {
            return errno::to_result(errno::EINVAL);
        }
        let handle = match abi.get_handle(sockfd as usize) {
            Some(handle) => handle,
            None => return errno::to_result(errno::EBADF),
        };
        let socket = match task
            .handle_table
            .get(handle)
            .and_then(KernelObject::into_socket_arc)
        {
            Some(socket) => socket,
            None => return errno::to_result(errno::ENOTSOCK),
        };
        let control = match socket.as_control_ops() {
            Some(control) => control,
            None => return errno::to_result(errno::ENOPROTOOPT),
        };
        let command = if optname == SO_RCVTIMEO {
            crate::network::socket::socket_ctl::SCTL_SOCKET_GET_READ_TIMEOUT_MS
        } else {
            crate::network::socket::socket_ctl::SCTL_SOCKET_GET_WRITE_TIMEOUT_MS
        };
        let timeout_ms = match control.control(command, 0) {
            Ok(timeout_ms) if timeout_ms >= 0 => timeout_ms as usize,
            Ok(_) => return errno::to_result(errno::EINVAL),
            Err(_) => return errno::to_result(errno::ENOPROTOOPT),
        };
        if timeout_ms > i32::MAX as usize {
            return errno::to_result(errno::EINVAL);
        }
        let timeval = timeout_ms_to_linux_timeval64(timeout_ms);
        let mut bytes = [0u8; size_of::<LinuxTimeval64>()];
        bytes[..8].copy_from_slice(&timeval.tv_sec.to_ne_bytes());
        bytes[8..].copy_from_slice(&timeval.tv_usec.to_ne_bytes());
        if copy_to_user(&task, optval_ptr, &bytes).is_err()
            || copy_to_user(
                &task,
                optlen_ptr,
                &(timeval_size as u32).to_ne_bytes(),
            )
            .is_err()
        {
            return errno::to_result(errno::EFAULT);
        }
        return 0;
    }

    // Fallback for options that are not semantically important yet.
    if optlen >= 4 && optval_ptr != 0 {
        if copy_to_user(&task, optval_ptr, &1u32.to_ne_bytes()).is_err()
            || copy_to_user(&task, optlen_ptr, &4u32.to_ne_bytes()).is_err()
        {
            return errno::to_result(errno::EFAULT);
        }
    }
    0
}

/// Linux sys_setsockopt implementation.
///
/// Implements SO_PASSCRED plus SO_RCVTIMEO/SO_SNDTIMEO through the native
/// millisecond controls. Other options retain their compatibility fallback.
///
/// Arguments:
/// - abi: LinuxAbi context
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
pub fn sys_setsockopt(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0) as i32;
    let level = trapframe.get_arg(1) as i32;
    let optname = trapframe.get_arg(2) as i32;
    let optval_ptr = trapframe.get_arg(3);
    let optlen = trapframe.get_arg(4) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(&task);
    if let Some(object)=abi.get_handle(sockfd as usize).and_then(|handle|task.handle_table.get(handle)) {
        if let Some(socket)=uevent::from_object(&object) {return socket.set_option(&task,level,optname,optval_ptr,optlen as usize);}
        if let Some(socket)=netlink::from_object(&object) {return socket.set_option(&task,level,optname,optval_ptr,optlen as usize);}
    }


    if level == SOL_SOCKET && optname == SO_PASSCRED {
        if optlen != size_of::<i32>() as u32 {
            return errno::to_result(errno::EINVAL);
        }
        let mut value = [0u8; size_of::<i32>()];
        if copy_from_user(&task, optval_ptr, &mut value).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        let handle = match abi.get_handle(sockfd as usize) {
            Some(handle) => handle,
            None => return errno::to_result(errno::EBADF),
        };
        let socket = match task
            .handle_table
            .get(handle)
            .and_then(KernelObject::into_socket_arc)
        {
            Some(socket) => socket,
            None => return errno::to_result(errno::ENOTSOCK),
        };
        let local_socket = match LocalSocket::from_socket_object(socket.as_ref()) {
            Some(socket) => socket,
            None => return errno::to_result(errno::ENOPROTOOPT),
        };
        local_socket.set_passcred(i32::from_ne_bytes(value) != 0);
        return 0;
    }

    if level == SOL_SOCKET && matches!(optname, SO_RCVTIMEO | SO_SNDTIMEO) {
        let timeval_size = size_of::<LinuxTimeval64>();
        if optlen < timeval_size as u32 {
            return errno::to_result(errno::EINVAL);
        }
        let mut bytes = [0u8; size_of::<LinuxTimeval64>()];
        if copy_from_user(&task, optval_ptr, &mut bytes).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        let timeval = LinuxTimeval64 {
            tv_sec: i64::from_ne_bytes(bytes[..8].try_into().unwrap()),
            tv_usec: i64::from_ne_bytes(bytes[8..].try_into().unwrap()),
        };
        let timeout_ms = match linux_timeval64_timeout_ms(timeval) {
            Ok(timeout_ms) => timeout_ms,
            Err(error) => return errno::to_result(error),
        };
        let handle = match abi.get_handle(sockfd as usize) {
            Some(handle) => handle,
            None => return errno::to_result(errno::EBADF),
        };
        let socket = match task
            .handle_table
            .get(handle)
            .and_then(KernelObject::into_socket_arc)
        {
            Some(socket) => socket,
            None => return errno::to_result(errno::ENOTSOCK),
        };
        let control = match socket.as_control_ops() {
            Some(control) => control,
            None => return errno::to_result(errno::ENOPROTOOPT),
        };
        let command = if optname == SO_RCVTIMEO {
            crate::network::socket::socket_ctl::SCTL_SOCKET_SET_READ_TIMEOUT_MS
        } else {
            crate::network::socket::socket_ctl::SCTL_SOCKET_SET_WRITE_TIMEOUT_MS
        };
        return match control.control(command, timeout_ms) {
            Ok(0) => 0,
            Ok(_) => errno::to_result(errno::EINVAL),
            Err(_) => errno::to_result(errno::ENOPROTOOPT),
        };
    }

    // Mock implementation - always succeed
    0
}

/// Linux sys_sendmsg implementation (minimal)
pub fn sys_sendmsg(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0);
    let msg_ptr = trapframe.get_arg(1);
    let flags = trapframe.get_arg(2) as i32;

    trapframe.increment_pc_next(&task);

    sendmsg_for_task(abi, trapframe, &task, sockfd, msg_ptr, flags)
}

fn sendmsg_for_task(
    abi: &mut LinuxAbi,
    trapframe: &mut Trapframe,
    task: &crate::task::Task,
    sockfd: usize,
    msg_ptr: usize,
    flags: i32,
) -> usize {
    let handle = match abi.get_handle(sockfd) {
        Some(h) => h,
        None => {
            crate::println!("[linux socket] sendmsg bad fd {}", sockfd);
            return errno::to_result(errno::EBADF);
        }
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => {
            crate::println!("[linux socket] sendmsg missing handle {}", sockfd);
            return errno::to_result(errno::EBADF);
        }
    };

    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => {
            crate::println!("[linux socket] sendmsg not a stream");
            return errno::to_result(errno::ENOTSOCK);
        }
    };

    let nonblocking = (flags & MSG_DONTWAIT) != 0
        || abi
            .get_file_status_flags(sockfd)
            .map(|f| ((f as i32) & O_NONBLOCK) != 0)
            .unwrap_or(false);

    let msg = match read_linux_msghdr(&task, msg_ptr) {
        Ok(msg) => msg,
        Err(error) => return errno::to_result(error),
    };
    let iovcnt = msg.msg_iovlen as usize;
    if iovcnt == 0 && !kernel_obj.as_socket().and_then(LocalSocket::from_socket_object).is_some_and(|local|local.is_datagram()) {
        return 0;
    }

    const IOV_MAX: usize = 1024;
    if iovcnt > IOV_MAX {
        return errno::to_result(errno::EINVAL);
    }

    let iovecs = match read_linux_iovecs(&task, msg.msg_iov as usize, iovcnt) {
        Ok(iovecs) => iovecs,
        Err(error) => return errno::to_result(error),
    };

    if let Some(local)=kernel_obj.as_socket().and_then(LocalSocket::from_socket_object).filter(|local|local.is_datagram()) {
        let flags=flags | if abi.get_file_status_flags(sockfd).is_some_and(|flags|flags as i32&O_NONBLOCK!=0) {MSG_DONTWAIT}else {0};
        return datagram::send(abi,task,local,&msg,&iovecs,flags);
    }
    let mut attached_handles = Vec::new();
    if msg.msg_control != 0 && msg.msg_controllen as usize >= size_of::<LinuxCmsghdr>() {
        let socket = match kernel_obj.as_socket() {
            Some(socket) => socket,
            None => return errno::to_result(errno::ENOTSOCK),
        };

        if LocalSocket::from_socket_object(socket).is_some() {
            let mut cmsg_bytes = [0u8; size_of::<LinuxCmsghdr>()];
            if copy_from_user(&task, msg.msg_control as usize, &mut cmsg_bytes).is_err() {
                return errno::to_result(errno::EFAULT);
            }
            // SAFETY: `cmsg_bytes` contains exactly one copied header. An
            // unaligned read avoids imposing alignment on the byte array.
            let cmsg =
                unsafe { core::ptr::read_unaligned(cmsg_bytes.as_ptr() as *const LinuxCmsghdr) };
            if cmsg.cmsg_level == SOL_SOCKET && cmsg.cmsg_type == SCM_RIGHTS {
                if cmsg.cmsg_len > msg.msg_controllen as usize
                    || cmsg.cmsg_len < size_of::<LinuxCmsghdr>()
                {
                    return errno::to_result(errno::EINVAL);
                }
                let data_len = cmsg.cmsg_len.saturating_sub(size_of::<LinuxCmsghdr>());
                if !data_len.is_multiple_of(size_of::<i32>()) {
                    return errno::to_result(errno::EINVAL);
                }
                let fd_count = data_len / size_of::<i32>();
                if fd_count > MAX_HANDLE_QUEUE_SIZE {
                    return errno::to_result(errno::EMSGSIZE);
                }

                let data_addr =
                    match (msg.msg_control as usize).checked_add(size_of::<LinuxCmsghdr>()) {
                        Some(address) => address,
                        None => return errno::to_result(errno::EFAULT),
                    };
                let mut fd_data = alloc::vec![0; data_len];
                if copy_from_user(&task, data_addr, &mut fd_data).is_err() {
                    return errno::to_result(errno::EFAULT);
                }

                for fd_bytes in fd_data.chunks_exact(size_of::<i32>()) {
                    let fd =
                        i32::from_ne_bytes([fd_bytes[0], fd_bytes[1], fd_bytes[2], fd_bytes[3]]);
                    if fd < 0 {
                        return errno::to_result(errno::EBADF);
                    }
                    let send_handle = match abi.get_handle(fd as usize) {
                        Some(handle) => handle,
                        None => {
                            crate::println!("[linux socket] sendmsg bad fd in cmsg {}", fd);
                            return errno::to_result(errno::EBADF);
                        }
                    };
                    let entry = match task.handle_table.clone_for_dup(send_handle) {
                        Some(entry) => entry,
                        None => {
                            crate::println!("[linux socket] sendmsg clone_for_dup failed");
                            return errno::to_result(errno::EBADF);
                        }
                    };
                    attached_handles.push(entry);
                }
            }
        }
    }

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

    if !attached_handles.is_empty() {
        let total_len = match iovecs
            .iter()
            .try_fold(0usize, |total, iovec| total.checked_add(iovec.iov_len))
        {
            Some(total) => total,
            None => return errno::to_result(errno::EMSGSIZE),
        };
        if total_len > crate::network::local::MAX_HANDLE_DATA_RECORD_SIZE {
            return errno::to_result(errno::EMSGSIZE);
        }

        let mut record_data = Vec::with_capacity(total_len);
        for iovec in iovecs {
            if iovec.iov_len == 0 {
                continue;
            }
            let old_len = record_data.len();
            record_data.resize(old_len + iovec.iov_len, 0);
            if copy_from_user(&task, iovec.iov_base as usize, &mut record_data[old_len..]).is_err()
            {
                return errno::to_result(errno::EFAULT);
            }
        }

        let socket = match kernel_obj.as_socket() {
            Some(socket) => socket,
            None => return errno::to_result(errno::ENOTSOCK),
        };
        let local_socket = match LocalSocket::from_socket_object(socket) {
            Some(socket) => socket,
            None => return errno::to_result(errno::EOPNOTSUPP),
        };
        return match local_socket.send_handles_and_data(attached_handles, &record_data) {
            Ok(()) => record_data.len(),
            Err(IpcError::ChannelFull) => errno::to_result(errno::EAGAIN),
            Err(IpcError::BufferTooSmall { .. }) => errno::to_result(errno::EMSGSIZE),
            Err(_) => errno::to_result(errno::EIO),
        };
    }

    // One sendmsg is one stream operation. Gather a bounded prefix so writes
    // to message-oriented local sockets retain a single record boundary and
    // blocking streams observe one deadline instead of one per iovec.
    const MAX_SENDMSG_PREFIX: usize = 1024 * 1024;
    let total_len = match iovecs
        .iter()
        .try_fold(0usize, |total, iovec| total.checked_add(iovec.iov_len))
    {
        Some(total) => total,
        None => return errno::to_result(errno::EMSGSIZE),
    };
    let seqpacket = kernel_obj
        .as_socket()
        .is_some_and(|socket| socket.socket_type() == SocketType::SeqPacket);
    if seqpacket && total_len > MAX_SEQPACKET_MESSAGE_SIZE {
        return errno::to_result(errno::EMSGSIZE);
    }
    let transfer_len = if seqpacket {
        total_len
    } else {
        total_len.min(MAX_SENDMSG_PREFIX)
    };
    if transfer_len == 0 {
        return 0;
    }
    let mut buffer = Vec::new();
    if buffer.try_reserve_exact(transfer_len).is_err() {
        return errno::to_result(errno::ENOMEM);
    }
    buffer.resize(transfer_len, 0);
    let mut copied = 0usize;
    for iovec in &iovecs {
        let part = iovec.iov_len.min(transfer_len - copied);
        if part != 0 {
            if copy_from_user(
                &task,
                iovec.iov_base as usize,
                &mut buffer[copied..copied + part],
            )
            .is_err()
            {
                return errno::to_result(errno::EFAULT);
            }
            copied += part;
        }
        if copied == transfer_len {
            break;
        }
    }

    match stream.write(&buffer[..copied]) {
        Ok(n) if n <= copied => n,
        Ok(_) => errno::to_result(errno::EIO),
        Err(StreamError::WouldBlock | StreamError::TimedOut) => {
            errno::to_result(errno::EAGAIN)
        }
        Err(StreamError::Interrupted) => errno::to_result(errno::EINTR),
        Err(_) => errno::to_result(errno::EIO),
    }
}

/// Send a batch of messages, recording each successful message's byte count.
/// glibc uses this for parallel A/AAAA DNS queries on a connected UDP socket.
pub fn sys_sendmmsg(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let sockfd = trapframe.get_arg(0);
    let messages = trapframe.get_arg(1);
    let count = (trapframe.get_arg(2) as u32 as usize).min(1024);
    let flags = trapframe.get_arg(3) as i32;
    trapframe.increment_pc_next(&task);

    let Some(handle) = abi.get_handle(sockfd) else {
        return errno::to_result(errno::EBADF);
    };
    if !task
        .handle_table
        .get(handle)
        .is_some_and(|object| object.as_socket().is_some())
    {
        return errno::to_result(errno::ENOTSOCK);
    }

    for index in 0..count {
        let Some(address) = messages.checked_add(index * size_of::<LinuxMmsghdr>()) else {
            return if index == 0 {
                errno::to_result(errno::EFAULT)
            } else {
                index
            };
        };
        let written = sendmsg_for_task(abi, trapframe, &task, sockfd, address, flags);
        if (written as isize) < 0 {
            return if index == 0 { written } else { index };
        }
        let Some(length_address) = address.checked_add(size_of::<LinuxMsghdr>()) else {
            return if index == 0 {
                errno::to_result(errno::EFAULT)
            } else {
                index
            };
        };
        if copy_to_user(&task, length_address, &(written as u32).to_ne_bytes()).is_err() {
            return if index == 0 {
                errno::to_result(errno::EFAULT)
            } else {
                index
            };
        }
    }
    count
}

/// Linux sys_recvmsg implementation (minimal)
pub fn sys_recvmsg(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let sockfd = trapframe.get_arg(0);
    let msg_ptr = trapframe.get_arg(1);
    let flags = trapframe.get_arg(2) as i32;
    // crate::println!(
    //     "[linux recvmsg] fd={} msg_ptr={:#x} flags={:#x}",
    //     sockfd,
    //     msg_ptr,
    //     flags
    // );

    trapframe.increment_pc_next(&task);
    if let Some(object)=abi.get_handle(sockfd as usize).and_then(|handle|task.handle_table.get(handle)) {
        if let Some(socket)=uevent::from_object(&object) {return socket.receive_message(&task,msg_ptr,flags);}
        if let Some(socket)=netlink::from_object(&object) {return socket.receive_message(&task,msg_ptr,flags);}
    }


    let handle = match abi.get_handle(sockfd) {
        Some(h) => h,
        None => {
            crate::println!("[linux socket] recvmsg bad fd {}", sockfd);
            return errno::to_result(errno::EBADF);
        }
    };

    // crate::println!("[linux recvmsg] handle={}", handle);

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => {
            crate::println!("[linux socket] recvmsg missing handle {}", sockfd);
            return errno::to_result(errno::EBADF);
        }
    };

    if let Some(local)=kernel_obj.as_socket().and_then(LocalSocket::from_socket_object).filter(|local|local.is_datagram()) {
        return datagram::receive(abi,&task,local,sockfd,msg_ptr,flags);
    }
    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => {
            crate::println!("[linux socket] recvmsg not a stream");
            return errno::to_result(errno::ENOTSOCK);
        }
    };

    let nonblocking = (flags & MSG_DONTWAIT) != 0
        || abi
            .get_file_status_flags(sockfd)
            .map(|f| ((f as i32) & O_NONBLOCK) != 0)
            .unwrap_or(false);

    let mut msg = match read_linux_msghdr(&task, msg_ptr) {
        Ok(msg) => msg,
        Err(error) => return errno::to_result(error),
    };
    // crate::println!(
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

    let iovecs = match read_linux_iovecs(&task, msg.msg_iov as usize, iovcnt) {
        Ok(iovecs) => iovecs,
        Err(error) => return errno::to_result(error),
    };
    // crate::println!("[linux recvmsg] iovcnt={}", iovecs.len());
    let mut total_read = 0usize;
    let local_socket = kernel_obj
        .as_socket()
        .and_then(LocalSocket::from_socket_object);
    let local_read_deadline = local_socket.and_then(|socket| socket.read_deadline());
    let wants_credentials = local_socket.is_some_and(LocalSocket::passcred);
    let mut sender_process_id = None;
    let mut pending_fds: Vec<i32> = Vec::new();
    fn rollback_received_fds(abi: &mut LinuxAbi, task: &crate::task::Task, fds: &[i32]) {
        for &fd in fds {
            if let Some(handle) = abi.remove_fd(fd as usize) {
                let _ = task.handle_table.remove(handle);
            }
        }
    }
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
    let total_buffer_size = match iovecs
        .iter()
        .try_fold(0usize, |total, iovec| total.checked_add(iovec.iov_len))
    {
        Some(total) if total <= isize::MAX as usize => total,
        _ => return errno::to_result(errno::EINVAL),
    };
    if total_buffer_size == 0 {
        return 0;
    }
    let can_receive_handle = msg.msg_control != 0
        && (msg.msg_controllen as usize) >= size_of::<LinuxCmsghdr>() + size_of::<i32>();
    let max_received_handles =
        (msg.msg_controllen as usize).saturating_sub(size_of::<LinuxCmsghdr>()) / size_of::<i32>();
    let mut retried_atomic_after_stream_wake = false;

    'receive: loop {
        let mut atomic_data: Option<Vec<u8>> = None;

        // Try atomic handle+data receive if control buffer is provided.
        if can_receive_handle {
            let socket = match kernel_obj.as_socket() {
                Some(socket) => socket,
                None => return errno::to_result(errno::ENOTSOCK),
            };

            if let Some(local_socket) = LocalSocket::from_socket_object(socket) {
                match local_socket.recv_handles_and_data_batch_with_sender(
                    total_buffer_size,
                    max_received_handles,
                ) {
                    Ok((handles, data, sender)) => {
                        if !data.is_empty() {
                            sender_process_id = Some(sender);
                        }
                        for (obj, metadata) in handles {
                            let access_mode = metadata.access_mode;
                            let new_handle =
                                match task.handle_table.insert_with_metadata(obj, metadata) {
                                    Ok(h) => h,
                                    Err(_) => {
                                        rollback_received_fds(abi, &task, &pending_fds);
                                        return errno::to_result(errno::EMFILE);
                                    }
                                };
                            let new_fd = match abi.allocate_fd(new_handle) {
                                Ok(fd) => fd,
                                Err(_) => {
                                    let _ = task.handle_table.remove(new_handle);
                                    rollback_received_fds(abi, &task, &pending_fds);
                                    return errno::to_result(errno::EMFILE);
                                }
                            };
                            let status_flags = match access_mode {
                                crate::object::handle::AccessMode::ReadOnly => 0,
                                crate::object::handle::AccessMode::WriteOnly => 1,
                                crate::object::handle::AccessMode::ReadWrite => 2,
                            };
                            let _ = abi.set_file_status_flags(new_fd, status_flags);
                            if flags & 0x4000_0000 != 0 {
                                let _ = abi.set_fd_flags(new_fd, FD_CLOEXEC);
                            }
                            pending_fds.push(new_fd as i32);
                        }
                        atomic_data = Some(data);
                    }
                    Err(IpcError::ChannelEmpty) => {
                        // A byte segment may be first; fall back to regular stream read.
                    }
                    Err(IpcError::BufferTooSmall { .. }) => {
                        // Preserve the complete record for a later recvmsg with
                        // sufficient iovec capacity.
                        return errno::to_result(errno::EMSGSIZE);
                    }
                    Err(_) => return errno::to_result(errno::EIO),
                }
            }
        }

        // Copy data from atomic receive or read from stream.
        if let Some(ref data) = atomic_data {
            let mut data_offset = 0;
            let data_len = data.len();
            for iovec in &iovecs {
                if data_offset >= data_len {
                    break;
                }
                if iovec.iov_len == 0 {
                    continue;
                }

                let remaining = data.len() - data_offset;
                let to_copy = remaining.min(iovec.iov_len);
                if copy_to_user(
                    &task,
                    iovec.iov_base as usize,
                    &data[data_offset..data_offset + to_copy],
                )
                .is_err()
                {
                    rollback_received_fds(abi, &task, &pending_fds);
                    return errno::to_result(errno::EFAULT);
                }
                data_offset += to_copy;
                total_read += to_copy;
            }
        } else {
            // One recvmsg is one stream receive, followed by scatter-copy.
            // Filling the first iovec must not trigger a second blocking read:
            // a Wine/Wayland peer can be waiting for our next request already.
            let length = total_buffer_size.min(1024 * 1024);
            let mut buffer = Vec::new();
            if buffer.try_reserve_exact(length).is_err() {
                return errno::to_result(errno::ENOMEM);
            }
            buffer.resize(length, 0);
            let read_result = match local_socket {
                Some(socket) => {
                    socket.read_with_sender_until(&mut buffer, local_read_deadline)
                }
                None => stream.read(&mut buffer).map(|count| (count, crate::network::local::SocketCredentials::unknown())),
            };
            let (amount, sender) = match read_result {
                Ok(result) => result,
                Err(StreamError::TimedOut) => return errno::to_result(errno::EAGAIN),
                Err(StreamError::WouldBlock) => {
                    if can_receive_handle && !retried_atomic_after_stream_wake {
                        retried_atomic_after_stream_wake = true;
                        continue 'receive;
                    }
                    return errno::to_result(errno::EAGAIN);
                }
                Err(StreamError::Interrupted) => return errno::to_result(errno::EINTR),
                Err(StreamError::EndOfStream) => (0, crate::network::local::SocketCredentials::unknown()),
                Err(_) => return errno::to_result(errno::EIO),
            };
            if amount > buffer.len() {
                return errno::to_result(errno::EIO);
            }
            if amount != 0 {
                sender_process_id = Some(sender);
            }
            for iovec in &iovecs {
                let part = iovec.iov_len.min(amount - total_read);
                if part != 0
                    && copy_to_user(
                        &task,
                        iovec.iov_base as usize,
                        &buffer[total_read..total_read + part],
                    )
                    .is_err()
                {
                    return errno::to_result(errno::EFAULT);
                }
                total_read += part;
                if total_read == amount {
                    break;
                }
            }
        }
        break 'receive;
    }

    let mut control = Vec::new();
    if !pending_fds.is_empty() {
        let rights_len = size_of::<LinuxCmsghdr>() + pending_fds.len() * size_of::<i32>();
        control.resize(rights_len, 0);
        let header = LinuxCmsghdr {
            cmsg_len: rights_len,
            cmsg_level: SOL_SOCKET,
            cmsg_type: SCM_RIGHTS,
        };
        unsafe {
            core::ptr::copy_nonoverlapping(
                (&header as *const LinuxCmsghdr).cast::<u8>(),
                control.as_mut_ptr(),
                size_of::<LinuxCmsghdr>(),
            );
        }
        for (index, fd) in pending_fds.iter().enumerate() {
            let start = size_of::<LinuxCmsghdr>() + index * size_of::<i32>();
            control[start..start + size_of::<i32>()].copy_from_slice(&fd.to_ne_bytes());
        }
    }

    msg.msg_flags = 0;
    if wants_credentials && let Some(sender) = sender_process_id {
        let aligned = (control.len() + size_of::<usize>() - 1) & !(size_of::<usize>() - 1);
        let cred_len = size_of::<LinuxCmsghdr>() + size_of::<LinuxUcred>();
        if msg.msg_control != 0 && aligned + cred_len <= msg.msg_controllen as usize {
            control.resize(aligned + cred_len, 0);
            let header = LinuxCmsghdr {
                cmsg_len: cred_len,
                cmsg_level: SOL_SOCKET,
                cmsg_type: SCM_CREDENTIALS,
            };
            let cred = LinuxUcred {
                pid: sender.pid_in(&task.get_namespace()) as i32,
                uid: sender.identity.principal.0,
                gid: sender.identity.group.0,
            };
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (&header as *const LinuxCmsghdr).cast::<u8>(),
                    control.as_mut_ptr().add(aligned),
                    size_of::<LinuxCmsghdr>(),
                );
                core::ptr::copy_nonoverlapping(
                    (&cred as *const LinuxUcred).cast::<u8>(),
                    control
                        .as_mut_ptr()
                        .add(aligned + size_of::<LinuxCmsghdr>()),
                    size_of::<LinuxUcred>(),
                );
            }
        } else {
            msg.msg_flags |= 0x8; // MSG_CTRUNC
        }
    }

    if !control.is_empty() && copy_to_user(&task, msg.msg_control as usize, &control).is_err() {
        rollback_received_fds(abi, &task, &pending_fds);
        return errno::to_result(errno::EFAULT);
    }

    msg.msg_controllen = control.len() as u64;
    let message_bytes = unsafe {
        core::slice::from_raw_parts(
            (&msg as *const LinuxMsghdr).cast::<u8>(),
            size_of::<LinuxMsghdr>(),
        )
    };
    if copy_to_user(&task, msg_ptr, message_bytes).is_err() {
        rollback_received_fds(abi, &task, &pending_fds);
        return errno::to_result(errno::EFAULT);
    }

    total_read
}

/// Linux sys_sendto implementation
///
/// Send a message on a socket. Unlike sendmsg, this directly takes a buffer and address.
///
/// Arguments:
/// - abi: LinuxAbi context
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
pub fn sys_sendto(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
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

    trapframe.increment_pc_next(&task);

    // Get socket handle
    let handle = match abi.get_handle(sockfd) {
        Some(h) => h,
        None => return errno::to_result(errno::EBADF),
    };

    if let Some(object)=task.handle_table.get(handle) {
        if let Some(socket)=netlink::from_object(&object) {
            return socket.send_message(&task, buf_ptr, len, dest_addr_ptr, addrlen as usize);
        }
    }

    // Get socket object
    let socket = match task
        .handle_table
        .get(handle)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => return errno::to_result(errno::ENOTSOCK),
    };

    if LocalSocket::from_socket_object(socket.as_ref()).is_some_and(|local| local.is_datagram())
        && len > crate::network::local::MAX_DATAGRAM_BYTES
    {
        return errno::to_result(errno::EMSGSIZE);
    }
    let mut data = Vec::new();
    if data.try_reserve_exact(len).is_err() {
        return errno::to_result(errno::ENOMEM);
    }
    data.resize(len, 0);
    if copy_from_user(&task, buf_ptr, &mut data).is_err() {
        return errno::to_result(errno::EFAULT);
    }

    // Parse destination address if provided
    let dest_addr = if dest_addr_ptr != 0 && addrlen > 0 {
        let mut addr_bytes = [0u8; 128];
        if addrlen < 2 || addrlen as usize > addr_bytes.len() {
            return errno::to_result(errno::EINVAL);
        }
        if copy_from_user(&task, dest_addr_ptr, &mut addr_bytes[..addrlen as usize]).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        let addr_kaddr = addr_bytes.as_ptr() as usize;

        // Read address family
        let sa_family = unsafe { *(addr_kaddr as *const u16) };

        match sa_family {
            AF_INET_U16 => {
                if addrlen < size_of::<SockaddrIn>() as u32 {
                    return errno::to_result(errno::EINVAL);
                }
                let sockaddr =
                    unsafe { core::ptr::read_unaligned(addr_kaddr as *const SockaddrIn) };
                crate::network::SocketAddress::Inet(sockaddr.to_inet())
            }
            AF_UNIX_U16 => {
                match linux_local_sockaddr_from_user(&task,(addr_kaddr+2) as *const u8,addrlen as usize-2,false) {Ok((address,_,_))=>address,Err(error)=>return errno::to_result(error)}
            }
            _ => return errno::to_result(errno::EAFNOSUPPORT),
        }
    } else {
        // No address - use connected peer
        crate::network::SocketAddress::Unspecified
    };

    // Send data
    let datagram=LocalSocket::from_socket_object(socket.as_ref()).is_some_and(|local|local.is_datagram());
    if datagram && flags & !((MSG_DONTWAIT as u32)|0x4000)!=0 {return errno::to_result(errno::EOPNOTSUPP);}
    let native_flags=if datagram {if flags&MSG_DONTWAIT as u32!=0 {crate::network::socket::message_flags::NONBLOCK}else {0}}else {flags};
    let result = socket.sendto(&data, &dest_addr, native_flags);
    if MOZC_IPC_TRACE_ENABLED && is_mozc_server_task(&task) {
        match &result {
            Ok(written) => crate::println!(
                "[linux mozc-ipc] send task_global={} task_local={} fd={} requested={} written={}",
                task.get_id(),
                task.try_get_namespace_id().unwrap_or(0),
                sockfd,
                len,
                written
            ),
            Err(error) => crate::println!(
                "[linux mozc-ipc] send task_global={} task_local={} fd={} requested={} error={:?}",
                task.get_id(),
                task.try_get_namespace_id().unwrap_or(0),
                sockfd,
                len,
                error
            ),
        }
    }
    match result {
        Ok(n) => n,
        Err(error) => errno::to_result(socket_io_error_to_errno(error)),
    }
}

/// Linux sys_recvfrom implementation
///
/// Receive a message from a socket. Returns the source address if provided.
///
/// Arguments:
/// - abi: LinuxAbi context
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
pub fn sys_recvfrom(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
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

    trapframe.increment_pc_next(&task);

    // Get socket handle
    let handle = match abi.get_handle(sockfd) {
        Some(h) => h,
        None => return errno::to_result(errno::EBADF),
    };

    // Get socket object
    let socket = match task
        .handle_table
        .get(handle)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => return errno::to_result(errno::ENOTSOCK),
    };

    let mut buffer = Vec::new();
    if buffer.try_reserve_exact(len).is_err() {
        return errno::to_result(errno::ENOMEM);
    }
    buffer.resize(len, 0);

    // Check for non-blocking mode
    let nonblocking = (flags & (MSG_DONTWAIT as u32)) != 0
        || abi
            .get_file_status_flags(sockfd)
            .map(|f| ((f as i32) & O_NONBLOCK) != 0)
            .unwrap_or(false);

    // MSG_DONTWAIT affects this receive only. Preserve the socket's existing
    // mode, including when another descriptor shares the same socket.
    let datagram=LocalSocket::from_socket_object(socket.as_ref()).is_some_and(|local|local.is_datagram());
    let previous_nonblocking = socket.as_selectable().filter(|_|!datagram).map(|selectable| {
        let previous = selectable.is_nonblocking();
        if nonblocking && !previous {
            selectable.set_nonblocking(true);
        }
        previous
    });

    // Receive data
    let result = if flags & MSG_PEEK != 0 && !LocalSocket::from_socket_object(socket.as_ref()).is_some_and(|local|local.is_datagram()) {
        match LocalSocket::from_socket_object(socket.as_ref()) {
            Some(local) => local.peek_record(&mut buffer).map(|record_len| {
                let count =
                    if flags & MSG_TRUNC != 0 && socket.socket_type() == SocketType::SeqPacket {
                        record_len
                    } else {
                        record_len.min(buffer.len())
                    };
                (count, crate::network::SocketAddress::Unspecified)
            }),
            None => Err(SocketError::NotSupported),
        }
    } else {
        if datagram {
            use crate::network::socket::message_flags as native;
            if flags & !((MSG_DONTWAIT as u32)|MSG_PEEK|MSG_TRUNC|0x100)!=0 {return errno::to_result(errno::EOPNOTSUPP);}
            let options=if nonblocking {native::NONBLOCK}else {0} | if flags&MSG_PEEK!=0 {native::PEEK}else {0} | if flags&MSG_TRUNC!=0 {native::FULL_LENGTH}else {0};
            socket.recvfrom(&mut buffer,options)
        }else {socket.recvfrom(&mut buffer,flags)}
    };

    if nonblocking && previous_nonblocking == Some(false) {
        if let Some(selectable) = socket.as_selectable() {
            selectable.set_nonblocking(false);
        }
    }

    if MOZC_IPC_TRACE_ENABLED && is_mozc_server_task(&task) {
        match &result {
            Ok((received, _)) => crate::println!(
                "[linux mozc-ipc] recv task_global={} task_local={} fd={} requested={} received={}",
                task.get_id(),
                task.try_get_namespace_id().unwrap_or(0),
                sockfd,
                len,
                received
            ),
            Err(error) => crate::println!(
                "[linux mozc-ipc] recv task_global={} task_local={} fd={} requested={} error={:?}",
                task.get_id(),
                task.try_get_namespace_id().unwrap_or(0),
                sockfd,
                len,
                error
            ),
        }
    }

    match result {
        Ok((n, src_addr)) => {
            // MSG_TRUNC can report a record length larger than the copy buffer.
            if copy_to_user(&task, buf_ptr, &buffer[..n.min(buffer.len())]).is_err() {
                return errno::to_result(errno::EFAULT);
            }
            // Store source address if requested
            if src_addr_ptr != 0 && addrlen_ptr != 0 {
                let mut addrlen_bytes = [0u8; size_of::<u32>()];
                if copy_from_user(&task, addrlen_ptr, &mut addrlen_bytes).is_err() {
                    return errno::to_result(errno::EFAULT);
                }
                let provided_len = u32::from_ne_bytes(addrlen_bytes);

                match src_addr {
                    crate::network::SocketAddress::Inet(inet) => {
                        if provided_len >= size_of::<SockaddrIn>() as u32 {
                            let sockaddr = SockaddrIn::from_inet(&inet);
                            let sockaddr_bytes = unsafe {
                                core::slice::from_raw_parts(
                                    (&sockaddr as *const SockaddrIn).cast::<u8>(),
                                    size_of::<SockaddrIn>(),
                                )
                            };
                            if copy_to_user(&task, src_addr_ptr, sockaddr_bytes).is_err() {
                                return errno::to_result(errno::EFAULT);
                            }
                            addrlen_bytes = (size_of::<SockaddrIn>() as u32).to_ne_bytes();
                        }
                    }
                    crate::network::SocketAddress::Local(address) => {
                        let mut bytes=Vec::from(AF_UNIX_U16.to_ne_bytes());
                        if address.is_abstract() {bytes.push(0);bytes.extend_from_slice(address.path().as_bytes());}
                        else if !address.path().is_empty() {bytes.extend_from_slice(address.path().as_bytes());bytes.push(0);}
                        if copy_to_user(&task,src_addr_ptr,&bytes[..bytes.len().min(provided_len as usize)]).is_err() {return errno::to_result(errno::EFAULT);}
                        addrlen_bytes=(bytes.len() as u32).to_ne_bytes();
                    }
                    _ => addrlen_bytes = 0u32.to_ne_bytes(),
                }
                if copy_to_user(&task, addrlen_ptr, &addrlen_bytes).is_err() {
                    return errno::to_result(errno::EFAULT);
                }
            }
            n
        }
        Err(error) => errno::to_result(socket_io_error_to_errno(error)),
    }
}

/// Linux sys_socketpair implementation
///
/// Create a pair of connected sockets. This is primarily used for AF_UNIX sockets
/// to create a bidirectional communication channel between processes.
///
/// Arguments:
/// - abi: LinuxAbi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: domain (address family, must be AF_UNIX)
///   - arg1: type (socket type, e.g., SOCK_STREAM)
///   - arg2: protocol (usually 0)
///   - arg3: sv (pointer to `int[2]` to receive the file descriptors)
///
/// Returns:
/// - 0 on success
/// - negative errno on error
pub fn sys_socketpair(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::ESRCH),
    };

    let domain = trapframe.get_arg(0) as i32;
    let socket_type = trapframe.get_arg(1) as i32;
    let _protocol = trapframe.get_arg(2) as i32;
    let sv_ptr = trapframe.get_arg(3);

    trapframe.increment_pc_next(&task);

    // Validate domain - socketpair only supports AF_UNIX
    if domain != AF_UNIX {
        return errno::to_result(errno::EAFNOSUPPORT);
    }

    // Extract socket type flags
    let base_type = socket_type & SOCK_TYPE_MASK;
    let flags = socket_type & !SOCK_TYPE_MASK;
    let nonblocking = (flags & SOCK_NONBLOCK) != 0;
    let cloexec = (flags & SOCK_CLOEXEC) != 0;

    let local_type = match base_type {
        SOCK_STREAM => SocketType::Stream,
        SOCK_DGRAM => SocketType::Datagram,
        SOCK_SEQPACKET => SocketType::SeqPacket,
        _ => return errno::to_result(errno::ESOCKTNOSUPPORT),
    };

    // Translate sv pointer (needs to write 2 i32 values)
    let sv_paddr = match task.vm_manager.translate_to_kva(sv_ptr) {
        Some(addr) => addr as *mut i32,
        None => return errno::to_result(errno::EFAULT),
    };

    // Create connected socket pair
    let (socket1, socket2) = match LocalSocket::try_create_connected_pair_with_type(
        local_type,
        alloc::string::String::from("socketpair:0"),
        alloc::string::String::from("socketpair:1"),
    ) {Ok(pair)=>pair,Err(error)=>return errno::to_result(socket_error_to_errno(error))};

    // Set non-blocking mode if requested
    if nonblocking {
        socket1.set_nonblocking(true);
        socket2.set_nonblocking(true);
    }

    // Add first socket to handle table
    let kernel_obj1 = KernelObject::from_socket_object(socket1);
    let handle1 = match task.handle_table.insert(kernel_obj1) {
        Ok(id) => id,
        Err(_) => return errno::to_result(errno::EMFILE),
    };

    // Add second socket to handle table
    let kernel_obj2 = KernelObject::from_socket_object(socket2);
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
/// - abi: LinuxAbi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: sockfd (socket file descriptor)
///   - arg1: how (0=SHUT_RD, 1=SHUT_WR, 2=SHUT_RDWR)
///
/// Returns:
/// - 0 on success
/// - negative errno on error
pub fn sys_shutdown(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::ESRCH),
    };

    let sockfd = trapframe.get_arg(0);
    let how = trapframe.get_arg(1) as u32;

    trapframe.increment_pc_next(&task);

    // Get socket handle
    let handle = match abi.get_handle(sockfd) {
        Some(h) => h,
        None => return errno::to_result(errno::EBADF),
    };

    // Get socket object
    let socket = match task
        .handle_table
        .get(handle)
        .and_then(KernelObject::into_socket_arc)
    {
        Some(socket) => socket,
        None => return errno::to_result(errno::ENOTSOCK),
    };

    // Convert how to ShutdownHow
    let shutdown_how = match how {
        0 => crate::network::socket::ShutdownHow::Read,
        1 => crate::network::socket::ShutdownHow::Write,
        2 => crate::network::socket::ShutdownHow::Both,
        _ => return errno::to_result(errno::EINVAL),
    };

    let result = socket.shutdown(shutdown_how);
    if MOZC_IPC_TRACE_ENABLED && is_mozc_server_task(&task) {
        crate::println!(
            "[linux mozc-ipc] shutdown task_global={} task_local={} fd={} how={} result={:?}",
            task.get_id(),
            task.try_get_namespace_id().unwrap_or(0),
            sockfd,
            how,
            result
        );
    }
    match result {
        Ok(()) => 0,
        Err(crate::network::socket::SocketError::NotConnected) => errno::to_result(errno::ENOTCONN),
        Err(_) => errno::to_result(errno::EIO),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sched::scheduler::get_task_by_id;

    #[cfg(target_arch = "aarch64")]
    #[test_case]
    fn socket_reply_does_not_wait_to_fill_pages_or_iovecs() {
        use crate::environment::{PAGE_SIZE, USER_STACK_END};
        use crate::object::capability::StreamOps;
        use crate::sched::scheduler::{
            add_task, get_task_by_id, register_online_cpu, remove_from_ready_queues, reset,
            set_current_task_for_test,
        };
        use crate::task::{Task, TaskState, TaskType};
        use core::sync::atomic::Ordering;

        reset();
        let cpu = crate::arch::get_cpu().get_cpuid();
        register_online_cpu(cpu);
        let id = add_task(
            Task::new("socket-short-reply".into(), 1, TaskType::User),
            cpu,
        );
        let task = get_task_by_id(id).unwrap();
        task.set_state(TaskState::Running);
        task.running_cpu.store(cpu, Ordering::SeqCst);
        set_current_task_for_test(cpu, Some(id));
        remove_from_ready_queues(id);
        let base = USER_STACK_END - 2 * PAGE_SIZE;
        task.allocate_stack_pages(base, 2).unwrap();
        let (client, server) = LocalSocket::create_connected_pair("client".into(), "server".into());
        let handle = task
            .handle_table
            .insert(KernelObject::from_socket_object(client))
            .unwrap();
        let mut abi = LinuxAbi::default();
        let fd = abi.allocate_fd(handle).unwrap();

        // Keep the peer open and send exactly enough to fill the first page
        // fragment. A second read waits forever: the peer expects our request.
        let address = base + PAGE_SIZE - 16;
        server.write(&[42; 16]).unwrap();
        let mut frame = Trapframe::new();
        frame.set_arg(0, fd);
        frame.set_arg(1, address);
        frame.set_arg(2, 64);
        assert_eq!(super::super::fs::sys_read(&mut abi, &mut frame), 16);
        let mut bytes = [0; 16];
        copy_from_user(&task, address, &mut bytes).unwrap();
        assert_eq!(bytes, [42; 16]);

        // Native 64-bit iovec/msghdr wire layouts; the first iovec exactly
        // fits one reply, and a larger reply must also scatter into the next.
        let mut vectors = [0u8; 32];
        vectors[..8].copy_from_slice(&(address as u64).to_ne_bytes());
        vectors[8..16].copy_from_slice(&16u64.to_ne_bytes());
        vectors[16..24].copy_from_slice(&((address + 16) as u64).to_ne_bytes());
        vectors[24..32].copy_from_slice(&48u64.to_ne_bytes());
        copy_to_user(&task, base + 128, &vectors).unwrap();
        for length in [16usize, 40] {
            let mut header = [0u8; size_of::<LinuxMsghdr>()];
            header[16..24].copy_from_slice(&((base + 128) as u64).to_ne_bytes());
            header[24..32].copy_from_slice(&2u64.to_ne_bytes());
            copy_to_user(&task, base, &header).unwrap();
            server.write(&[43; 40][..length]).unwrap();
            frame.set_arg(0, fd);
            frame.set_arg(1, base);
            frame.set_arg(2, 0); // Blocking recvmsg, not MSG_DONTWAIT.
            assert_eq!(sys_recvmsg(&mut abi, &mut frame), length);
            let mut bytes = [0; 40];
            copy_from_user(&task, address, &mut bytes[..length]).unwrap();
            assert_eq!(&bytes[..length], &[43; 40][..length]);
        }
        assert_eq!(task.get_state(), TaskState::Running);
        abi.remove_fd(fd);
        task.handle_table.remove(handle).unwrap();
        drop(server);
        set_current_task_for_test(cpu, None);
        task.running_cpu.store(usize::MAX, Ordering::SeqCst);
        drop(task);
        reset();
    }

    #[test_case]
    fn ipv4_sockaddr_preserves_network_order_octets() {
        let address = crate::network::Inet4SocketAddress::new([10, 0, 2, 15], 0x1234);
        let sockaddr = SockaddrIn::from_inet(&address);
        assert_eq!(sockaddr.sin_addr.to_ne_bytes(), [10, 0, 2, 15]);
        assert_eq!(sockaddr.sin_port.to_ne_bytes(), [0x12, 0x34]);

        let received = SockaddrIn {
            sin_family: AF_INET_U16,
            sin_port: u16::from_ne_bytes([0, 53]),
            sin_addr: u32::from_ne_bytes([10, 0, 2, 3]),
            sin_zero: [0; 8],
        }
        .to_inet();
        assert_eq!(received.addr, [10, 0, 2, 3]);
        assert_eq!(received.port, 53);
    }

    #[test_case]
    fn socket_status_does_not_report_a_spurious_permission_error() {
        let socket = crate::network::tcp::TcpSocket::new(alloc::sync::Weak::new());
        assert_eq!(
            socket_status_option(socket.as_ref(), SO_TYPE),
            Ok(SOCK_STREAM)
        );
        assert_eq!(socket_status_option(socket.as_ref(), SO_ERROR), Ok(0));
        assert_eq!(
            inet_connect_error_to_errno(SocketError::WouldBlock),
            errno::EINPROGRESS
        );
        assert_eq!(
            socket_error_to_errno(SocketError::WouldBlock),
            errno::EAGAIN
        );
        assert_eq!(
            socket_error_to_errno(SocketError::TimedOut),
            errno::ETIMEDOUT
        );
        assert_eq!(
            socket_io_error_to_errno(SocketError::TimedOut),
            errno::EAGAIN
        );
    }

    #[test_case]
    fn ipv6_socket_creation_fails_before_allocating_ipv4_backed_state() {
        use crate::sched::scheduler::{
            add_task, register_online_cpu, remove_from_ready_queues, reset,
            set_current_task_for_test,
        };
        use crate::task::{Task, TaskState, TaskType};
        use core::sync::atomic::Ordering;

        reset();
        let cpu = crate::arch::get_cpu().get_cpuid();
        register_online_cpu(cpu);
        let id = add_task(
            Task::new("ipv6-unsupported".into(), 1, TaskType::User),
            cpu,
        );
        let task = get_task_by_id(id).unwrap();
        task.set_state(TaskState::Running);
        task.running_cpu.store(cpu, Ordering::SeqCst);
        set_current_task_for_test(cpu, Some(id));
        remove_from_ready_queues(id);
        let mut abi = LinuxAbi::default();
        let mut frame = Trapframe::new();
        frame.set_arg(0, AF_INET6 as usize);
        frame.set_arg(1, SOCK_STREAM as usize);
        frame.set_arg(2, 0);
        assert_eq!(
            sys_socket(&mut abi, &mut frame),
            errno::to_result(errno::EAFNOSUPPORT)
        );
        assert!(abi.get_handle(0).is_none());
        assert!(task.handle_table.get(0).is_none());

        set_current_task_for_test(cpu, None);
        task.running_cpu.store(usize::MAX, Ordering::SeqCst);
        drop(task);
        reset();
    }

    #[cfg(target_arch = "aarch64")]
    #[test_case]
    fn so_timeout_syscalls_validate_and_round_trip_for_local_sockets() {
        use crate::environment::{PAGE_SIZE, USER_STACK_END};
        use crate::sched::scheduler::{
            add_task, register_online_cpu, remove_from_ready_queues, reset,
            set_current_task_for_test,
        };
        use crate::task::{Task, TaskState, TaskType};
        use core::sync::atomic::Ordering;

        reset();
        let cpu = crate::arch::get_cpu().get_cpuid();
        register_online_cpu(cpu);
        let id = add_task(
            Task::new("socket-timeval".into(), 1, TaskType::User),
            cpu,
        );
        let task = get_task_by_id(id).unwrap();
        task.set_state(TaskState::Running);
        task.running_cpu.store(cpu, Ordering::SeqCst);
        set_current_task_for_test(cpu, Some(id));
        remove_from_ready_queues(id);
        let base = USER_STACK_END - PAGE_SIZE;
        task.allocate_stack_pages(base, 1).unwrap();

        let socket = LocalSocket::new(SocketType::Stream, SocketProtocol::Default);
        let handle = task
            .handle_table
            .insert(KernelObject::from_socket_object(arc_for_test(socket)))
            .unwrap();
        let mut abi = LinuxAbi::default();
        let fd = abi.allocate_fd(handle).unwrap();
        let value_ptr = base + 256;
        let length_ptr = value_ptr + size_of::<LinuxTimeval64>();
        let mut frame = Trapframe::new();

        assert_so_timeout_round_trip(
            &mut abi,
            &task,
            &mut frame,
            fd,
            SO_RCVTIMEO,
            value_ptr,
            length_ptr,
            LinuxTimeval64 {
                tv_sec: 1,
                tv_usec: 250_000,
            },
            LinuxTimeval64 {
                tv_sec: 1,
                tv_usec: 250_000,
            },
        );
        assert_so_timeout_round_trip(
            &mut abi,
            &task,
            &mut frame,
            fd,
            SO_RCVTIMEO,
            value_ptr,
            length_ptr,
            LinuxTimeval64 {
                tv_sec: 0,
                tv_usec: 1,
            },
            LinuxTimeval64 {
                tv_sec: 0,
                tv_usec: 1_000,
            },
        );
        assert_so_timeout_round_trip(
            &mut abi,
            &task,
            &mut frame,
            fd,
            SO_RCVTIMEO,
            value_ptr,
            length_ptr,
            LinuxTimeval64 {
                tv_sec: 0,
                tv_usec: 0,
            },
            LinuxTimeval64 {
                tv_sec: 0,
                tv_usec: 0,
            },
        );
        assert_eq!(
            set_so_timeout_test(
                &mut abi,
                &task,
                &mut frame,
                fd,
                SO_RCVTIMEO,
                value_ptr,
                LinuxTimeval64 {
                    tv_sec: 0,
                    tv_usec: 1_000_000,
                },
            ),
            errno::to_result(errno::EINVAL)
        );
        assert_so_timeout_round_trip(
            &mut abi,
            &task,
            &mut frame,
            fd,
            SO_SNDTIMEO,
            value_ptr,
            length_ptr,
            LinuxTimeval64 {
                tv_sec: 2,
                tv_usec: 500_000,
            },
            LinuxTimeval64 {
                tv_sec: 2,
                tv_usec: 500_000,
            },
        );

        abi.remove_fd(fd);
        task.handle_table.remove(handle).unwrap();
        set_current_task_for_test(cpu, None);
        task.running_cpu.store(usize::MAX, Ordering::SeqCst);
        drop(task);
        reset();
    }

    #[cfg(target_arch = "aarch64")]
    #[test_case]
    fn seqpacket_sendmsg_gathers_multiple_iovecs_into_one_record() {
        use crate::environment::{PAGE_SIZE, USER_STACK_END};
        use crate::object::capability::{Selectable, StreamOps};
        use crate::sched::scheduler::{
            add_task, register_online_cpu, remove_from_ready_queues, reset,
            set_current_task_for_test,
        };
        use crate::task::{Task, TaskState, TaskType};
        use core::sync::atomic::Ordering;

        reset();
        let cpu = crate::arch::get_cpu().get_cpuid();
        register_online_cpu(cpu);
        let id = add_task(
            Task::new("sendmsg-seqpacket".into(), 1, TaskType::User),
            cpu,
        );
        let task = get_task_by_id(id).unwrap();
        task.set_state(TaskState::Running);
        task.running_cpu.store(cpu, Ordering::SeqCst);
        set_current_task_for_test(cpu, Some(id));
        remove_from_ready_queues(id);
        let base = USER_STACK_END - PAGE_SIZE;
        task.allocate_stack_pages(base, 1).unwrap();

        let (sender, receiver) = LocalSocket::create_connected_pair_with_type(
            SocketType::SeqPacket,
            "sendmsg-sender".into(),
            "sendmsg-receiver".into(),
        );
        let handle = task
            .handle_table
            .insert(KernelObject::from_socket_object(sender))
            .unwrap();
        let mut abi = LinuxAbi::default();
        let fd = abi.allocate_fd(handle).unwrap();
        copy_to_user(&task, base + 256, b"abc").unwrap();
        copy_to_user(&task, base + 260, b"def").unwrap();
        let mut vectors = [0u8; 32];
        vectors[..8].copy_from_slice(&((base + 256) as u64).to_ne_bytes());
        vectors[8..16].copy_from_slice(&3u64.to_ne_bytes());
        vectors[16..24].copy_from_slice(&((base + 260) as u64).to_ne_bytes());
        vectors[24..32].copy_from_slice(&3u64.to_ne_bytes());
        copy_to_user(&task, base + 128, &vectors).unwrap();
        let mut header = [0u8; size_of::<LinuxMsghdr>()];
        header[16..24].copy_from_slice(&((base + 128) as u64).to_ne_bytes());
        header[24..32].copy_from_slice(&2u64.to_ne_bytes());
        copy_to_user(&task, base, &header).unwrap();

        let mut frame = Trapframe::new();
        assert_eq!(
            sendmsg_for_task(&mut abi, &mut frame, &task, fd, base, 0),
            6
        );
        let mut packet = [0u8; 16];
        assert_eq!(receiver.read(&mut packet).unwrap(), 6);
        assert_eq!(&packet[..6], b"abcdef");
        receiver.set_nonblocking(true);
        assert!(matches!(receiver.read(&mut packet), Err(StreamError::WouldBlock)));

        abi.remove_fd(fd);
        task.handle_table.remove(handle).unwrap();
        set_current_task_for_test(cpu, None);
        task.running_cpu.store(usize::MAX, Ordering::SeqCst);
        drop(task);
        reset();
    }

    fn assert_so_timeout_round_trip(
        abi: &mut LinuxAbi,
        task: &crate::task::Task,
        frame: &mut Trapframe,
        fd: usize,
        optname: i32,
        value_ptr: usize,
        length_ptr: usize,
        set: LinuxTimeval64,
        expected: LinuxTimeval64,
    ) {
        assert_eq!(
            set_so_timeout_test(abi, task, frame, fd, optname, value_ptr, set),
            0
        );
        assert_eq!(
            get_so_timeout_test(abi, task, frame, fd, optname, value_ptr, length_ptr),
            expected
        );
    }

    fn set_so_timeout_test(
        abi: &mut LinuxAbi,
        task: &crate::task::Task,
        frame: &mut Trapframe,
        fd: usize,
        optname: i32,
        value_ptr: usize,
        timeval: LinuxTimeval64,
    ) -> usize {
        let mut bytes = [0u8; size_of::<LinuxTimeval64>()];
        bytes[..8].copy_from_slice(&timeval.tv_sec.to_ne_bytes());
        bytes[8..].copy_from_slice(&timeval.tv_usec.to_ne_bytes());
        copy_to_user(task, value_ptr, &bytes).unwrap();
        frame.set_arg(0, fd);
        frame.set_arg(1, SOL_SOCKET as usize);
        frame.set_arg(2, optname as usize);
        frame.set_arg(3, value_ptr);
        frame.set_arg(4, size_of::<LinuxTimeval64>());
        sys_setsockopt(abi, frame)
    }

    fn get_so_timeout_test(
        abi: &mut LinuxAbi,
        task: &crate::task::Task,
        frame: &mut Trapframe,
        fd: usize,
        optname: i32,
        value_ptr: usize,
        length_ptr: usize,
    ) -> LinuxTimeval64 {
        copy_to_user(
            task,
            length_ptr,
            &(size_of::<LinuxTimeval64>() as u32).to_ne_bytes(),
        )
        .unwrap();
        frame.set_arg(0, fd);
        frame.set_arg(1, SOL_SOCKET as usize);
        frame.set_arg(2, optname as usize);
        frame.set_arg(3, value_ptr);
        frame.set_arg(4, length_ptr);
        assert_eq!(sys_getsockopt(abi, frame), 0);
        let mut length = [0u8; size_of::<u32>()];
        copy_from_user(task, length_ptr, &mut length).unwrap();
        assert_eq!(
            u32::from_ne_bytes(length) as usize,
            size_of::<LinuxTimeval64>()
        );
        let mut bytes = [0u8; size_of::<LinuxTimeval64>()];
        copy_from_user(task, value_ptr, &mut bytes).unwrap();
        LinuxTimeval64 {
            tv_sec: i64::from_ne_bytes(bytes[..8].try_into().unwrap()),
            tv_usec: i64::from_ne_bytes(bytes[8..].try_into().unwrap()),
        }
    }

    fn arc_for_test(socket: LocalSocket) -> Arc<dyn crate::network::SocketObject> {
        let socket = Arc::new(socket);
        LocalSocket::init_self_weak(&socket);
        socket
    }

    #[test_case]
    fn socket_timeval64_validation_and_round_trip_use_native_milliseconds() {
        assert_eq!(size_of::<LinuxTimeval64>(), 16);
        for timeout_ms in [0usize, 1, 999, 1_000, 1_500, i32::MAX as usize] {
            assert_eq!(
                linux_timeval64_timeout_ms(timeout_ms_to_linux_timeval64(timeout_ms)),
                Ok(timeout_ms)
            );
        }
        assert_eq!(
            linux_timeval64_timeout_ms(LinuxTimeval64 {
                tv_sec: 0,
                tv_usec: 1,
            }),
            Ok(1),
            "sub-millisecond timeouts must round up"
        );

        for invalid in [
            LinuxTimeval64 {
                tv_sec: -1,
                tv_usec: 0,
            },
            LinuxTimeval64 {
                tv_sec: 0,
                tv_usec: -1,
            },
            LinuxTimeval64 {
                tv_sec: 0,
                tv_usec: 1_000_000,
            },
            LinuxTimeval64 {
                tv_sec: i32::MAX as i64 / 1_000 + 1,
                tv_usec: 0,
            },
        ] {
            assert_eq!(linux_timeval64_timeout_ms(invalid), Err(errno::EINVAL));
        }
    }
}
