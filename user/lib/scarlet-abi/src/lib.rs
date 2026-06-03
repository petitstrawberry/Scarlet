//! Scarlet Native ABI definitions.
//!
//! This crate contains raw ABI definitions shared by Scarlet userland
//! libraries and, eventually, Rust `std`'s Scarlet PAL. It intentionally avoids
//! syscall assembly or safe wrappers so it can stay `no_std` and dependency-free.

#![no_std]

/// Raw kernel object handle value used at the Scarlet Native ABI boundary.
pub type RawHandle = i32;

/// Raw process identifier exposed by Scarlet Native process syscalls.
pub type Pid = u32;

/// Raw thread identifier exposed by Scarlet Native thread syscalls.
pub type Tid = u32;

/// Scarlet Native syscall numbers.
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    // Basic I/O
    Putchar = 16,
    Getchar = 17,

    Sleep = 20,
    Yield = 21,
    ExitGroup = 23,

    // Process information
    GetTaskInfoCount = 24,
    GetTaskInfoList = 25,
    CreateSession = 26,
    GetSessionId = 27,
    GetProcessGroupId = 28,
    SetProcessGroup = 29,

    // TLS management
    SetTls = 30,
    GetTls = 31,
    SetTidAddress = 32,
    ThreadDetach = 33,
    ThreadExitCleanup = 34,

    // ABI zone management
    RegisterAbiZone = 90,
    UnregisterAbiZone = 91,

    // Namespace management
    CreateNamespace = 92,

    // Handle management
    HandleQuery = 100,
    HandleSetRole = 101,
    HandleClose = 102,
    HandleDuplicate = 103,
    HandleControl = 110,

    // Core capabilities
    StreamRead = 200,
    StreamWrite = 201,
    Poll = 202,

    // FileObject capability
    FileSeek = 300,
    FileTruncate = 301,

    // VFS operations
    VfsOpen = 400,
    VfsRemove = 401,
    VfsCreateFile = 402,
    VfsCreateDirectory = 403,
    VfsChangeDirectory = 404,
    VfsTruncate = 405,
    VfsCreateSymlink = 406,
    VfsReadlink = 407,
    VfsGetCwdPath = 408,
    VfsRename = 409,

    // Filesystem operations
    FsMount = 500,
    FsUmount = 501,
    FsPivotRoot = 502,

    // IPC operations
    Pipe = 600,
    EventSendDirect = 615,
    EventSendGroup = 616,

    // Shared memory
    SharedMemoryCreate = 620,
    SharedMemoryResize = 621,

    // Socket handle transfer
    SocketSendHandle = 630,
    SocketRecvHandle = 631,
    SocketSendHandleAndData = 632,
    SocketRecvHandleAndData = 633,

    // Scarlet Native event handling
    EventHandlerRegister = 640,
    EventHandlerUnregister = 641,
    EventMask = 642,
    EventReturn = 643,

    // Memory mapping operations
    MemoryMap = 700,
    MemoryUnmap = 701,

    // Socket operations
    SocketCreate = 900,
    SocketBind = 901,
    SocketListen = 902,
    SocketConnect = 903,
    SocketAccept = 904,
    Socketpair = 905,
    SocketShutdown = 906,

    // Datagram operations
    SocketRecvFrom = 907,
    SocketSendTo = 908,

    // Network configuration
    NetworkSetIpv4 = 910,
    NetworkSetGateway = 911,
    NetworkSetDns = 912,
    NetworkSetNetmask = 913,
    NetworkListInterfaces = 914,

    // Debug/profiler operations
    ProfilerDump = 999,

    // System control operations
    Shutdown = 1000,

    // Hypervisor operations
    ShvVmCreate = 1100,
    ShvVcpuCreate = 1101,
    ShvVcpuRun = 1102,

    // Loadable module operations
    LsmLoad = 1200,
    LsmUnload = 1201,
    LsmList = 1202,
}
