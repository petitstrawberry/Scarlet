use crate::arch::*;

#[derive(Debug, Clone, Copy)]
pub enum Syscall {
    Invalid = 0,
    Exit = 1,
    Clone = 2,
    Execve = 3,
    ExecveABI = 4,
    Waitpid = 5,
    Kill = 6,
    Getpid = 7,
    Getppid = 8,
    Brk = 12,
    Sbrk = 13,
    // BASIC I/O
    Putchar = 16,
    Getchar = 17,

    Sleep = 20,

    Yield = 21,

    ExitGroup = 23, // Exit all tasks in thread group

    // TLS (Thread Local Storage) Management
    SetTls = 30,
    GetTls = 31,
    SetTidAddress = 32,

    // ABI Zone Management
    RegisterAbiZone = 90,
    UnregisterAbiZone = 91,

    // Namespace Management (Scarlet-style)
    CreateNamespace = 92,

    // === Handle Management ===
    HandleQuery = 100,
    HandleSetRole = 101,
    HandleClose = 102,     // Close any handle (files, pipes, etc.)
    HandleDuplicate = 103, // Duplicate any handle
    HandleControl = 110,   // Control operations on handles (ioctl-equivalent)

    // === Core Capabilities (Object-oriented) ===
    // StreamOps Capability - read/write operations
    StreamRead = 200,
    StreamWrite = 201,

    // FileObject Capability - file-specific operations (extends StreamOps)
    FileSeek = 300,
    FileTruncate = 301,
    // FileMetadata = 302,

    // === VFS Operations (VFS layer management and file access) ===
    VfsOpen = 400,            // Open files/directories through VFS
    VfsRemove = 401,          // Remove files or directories (unified Remove/Unlink)
    VfsCreateFile = 402,      // Create regular files through VFS
    VfsCreateDirectory = 403, // Create directories through VFS
    VfsChangeDirectory = 404, // Change current working directory
    VfsTruncate = 405,        // Truncate files by path
    VfsCreateSymlink = 406,   // Create symbolic links through VFS
    VfsReadlink = 407,        // Read symbolic link target through VFS
    VfsGetCwdPath = 408,      // Get current working directory path

    // === Filesystem Operations (mount management) ===
    FsMount = 500,
    FsUmount = 501,
    FsPivotRoot = 502,

    // === IPC Operations ===
    Pipe = 600, // Create pipe handles

    // Shared Memory
    SharedMemoryCreate = 620, // Create shared memory region
    SharedMemoryResize = 621, // Resize shared memory region

    // Event System (Scarlet Native)
    EventHandlerRegister = 640,   // Register event handler
    EventHandlerUnregister = 641, // Unregister event handler
    EventMask = 642,              // Set event mask
    EventReturn = 643,            // Return from event handler

    // Socket Handle Transfer (similar to SCM_RIGHTS)
    SocketSendHandle = 630,        // Send kernel object handle through socket
    SocketRecvHandle = 631,        // Receive kernel object handle from socket
    SocketSendHandleAndData = 632, // Send handle and data atomically (for Wayland)
    SocketRecvHandleAndData = 633, // Receive handle and data atomically (for Wayland)

    // === Memory Mapping Operations ===
    MemoryMap = 700,   // Memory map operation (mmap)
    MemoryUnmap = 701, // Memory unmap operation (munmap)

    // === Socket Operations (Scarlet Native) ===
    SocketCreate = 900,   // Create a socket (domain/type/protocol)
    SocketBind = 901,     // Bind socket to path
    SocketListen = 902,   // Start listening
    SocketConnect = 903,  // Connect to socket
    SocketAccept = 904,   // Accept connection
    Socketpair = 905,     // Create socket pair
    SocketShutdown = 906, // Shutdown socket

    // === Datagram Operations (UDP/Local datagram) ===
    SocketRecvFrom = 907, // Receive datagram with sender address
    SocketSendTo = 908,   // Send datagram to specified address

    // === Network Configuration ===
    NetworkSetIpv4 = 910,        // Set interface IPv4 address
    NetworkSetGateway = 911,     // Set default gateway
    NetworkSetDns = 912,         // Set DNS server
    NetworkSetNetmask = 913,     // Set subnet mask
    NetworkListInterfaces = 914, // List network interfaces

    // === Debug/Profiler Operations ===
    ProfilerDump = 999, // Dump profiler statistics (debug only)

    // === System Control Operations ===
    Shutdown = 1000, // Shutdown the system gracefully

    // === Hypervisor Operations ===
    ShvVmCreate = 1100,
    ShvVcpuCreate = 1101,
    ShvVcpuRun = 1102,

    // === Loadable Module Operations ===
    LsmLoad = 1200,
}

pub fn syscall0(syscall: Syscall) -> usize {
    arch_syscall0(syscall)
}

pub fn syscall1(syscall: Syscall, arg1: usize) -> usize {
    arch_syscall1(syscall, arg1)
}

pub fn syscall2(syscall: Syscall, arg1: usize, arg2: usize) -> usize {
    arch_syscall2(syscall, arg1, arg2)
}

pub fn syscall3(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize) -> usize {
    arch_syscall3(syscall, arg1, arg2, arg3)
}

pub fn syscall4(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> usize {
    arch_syscall4(syscall, arg1, arg2, arg3, arg4)
}

pub fn syscall5(
    syscall: Syscall,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> usize {
    arch_syscall5(syscall, arg1, arg2, arg3, arg4, arg5)
}

pub fn syscall6(
    syscall: Syscall,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
    arg6: usize,
) -> usize {
    arch_syscall6(syscall, arg1, arg2, arg3, arg4, arg5, arg6)
}
