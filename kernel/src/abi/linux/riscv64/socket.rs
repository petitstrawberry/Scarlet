use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi,
    arch::Trapframe,
    task::mytask,
    ipc::pipe::UnidirectionalPipe,
};

/// Linux socket domains
pub const AF_UNIX: i32 = 1;     // Unix domain sockets
pub const AF_INET: i32 = 2;     // Internet IP Protocol
pub const AF_INET6: i32 = 10;   // IP version 6

/// Linux socket types
pub const SOCK_STREAM: i32 = 1;    // Stream socket
pub const SOCK_DGRAM: i32 = 2;     // Datagram socket
pub const SOCK_RAW: i32 = 3;       // Raw socket
pub const SOCK_SEQPACKET: i32 = 5; // Sequenced packet socket

/// Linux sys_socket implementation (mock with pipe)
///
/// Creates a socket endpoint for communication. This is a mock implementation
/// that creates a pipe and returns one end as a "socket" file descriptor.
/// This allows applications to proceed without hanging, even though real
/// network communication won't work.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: domain (communication domain, e.g., AF_UNIX, AF_INET)
///   - arg1: type (socket type, e.g., SOCK_STREAM, SOCK_DGRAM)
///   - arg2: protocol (protocol to use, usually 0)
///
/// Returns:
/// - file descriptor on success (mock socket using pipe)
/// - usize::MAX (Linux -1) on error
pub fn sys_socket(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };
    
    let _domain = trapframe.get_arg(0) as i32;
    let _type = trapframe.get_arg(1) as i32;
    let _protocol = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Create a pipe to mock a socket
    // This allows applications to get a valid fd and proceed
    let (read_obj, _write_obj) = UnidirectionalPipe::create_pair(4096);
    
    // Insert only the read end as the "socket" - we don't need the write end
    match task.handle_table.insert(read_obj) {
        Ok(handle) => {
            // Allocate a file descriptor for the "socket"
            match abi.allocate_fd(handle) {
                Ok(fd) => fd,
                Err(_) => {
                    // Clean up on error
                    let _ = task.handle_table.remove(handle);
                    usize::MAX
                }
            }
        },
        Err(_) => usize::MAX, // Failed to create pipe
    }
}

/// Linux sys_bind implementation (mock)
///
/// Binds a socket to an address. This is a mock implementation that
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
pub fn sys_bind(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
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
        Ok(handle) => {
            match abi.allocate_fd(handle) {
                Ok(fd) => fd,
                Err(_) => {
                    let _ = task.handle_table.remove(handle);
                    usize::MAX
                }
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
        task.vm_manager.translate_vaddr(addrlen_ptr)
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
        task.vm_manager.translate_vaddr(optlen_ptr)
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
