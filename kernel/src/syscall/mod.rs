//! System call interface module.
//!
//! This module provides the system call interface for the Scarlet kernel
//! using a hybrid capability-based design that balances type safety with
//! practical usability.
//!
//! ## System Call Number Organization
//!
//! The system calls are organized into logical ranges:
//!
//! - **1-99**: Process and task management (exit, clone, exec, getpid, brk, etc.)
//! - **100-199**: Handle management operations (handle_query, handle_close, dup)
//! - **200-299**: StreamOps capability (stream_read, stream_write operations)
//! - **300-399**: FileObject capability (file_seek, file_truncate, file_metadata)
//! - **400-499**: VFS operations (vfs_open, vfs_remove, vfs_create_directory, vfs_change_directory, vfs_truncate, vfs_metadata)
//! - **500-599**: Filesystem operations (fs_mount, fs_umount, fs_pivot_root)
//! - **600-699**: IPC operations (pipe, shared memory, message queues)
//! - **700-799**: Memory mapping operations (memory_map, memory_unmap)
//! - **900-999**: Socket operations (socket_create, bind, connect, accept, etc.)
//!
//! Legacy POSIX-like system calls (20-35) are maintained for backward compatibility
//! and redirect to the appropriate capability-based implementations.
//!
//! ## Current Implementation Status
//!
//! ### Process Management (1-99)
//! - Exit (1), Clone (2), Execve (3), ExecveABI (4), Waitpid (5)
//! - Getpid (7), Getppid (8), Brk (12), Sbrk (13)
//! - Session/Process Groups: CreateSession (26), GetSessionId (27),
//!   GetProcessGroupId (28), SetProcessGroup (29)
//! - Time: Sleep (20), MonotonicTime (35)
//! - Basic I/O: Putchar (16), Getchar (17)
//! - Random: GetRandom (22)
//! - ABI Zone: RegisterAbiZone (90), UnregisterAbiZone (91)
//! - Namespace: CreateNamespace (92) - Smart syscall for task/VFS isolation
//!
//! ### Handle Management (100-199)
//! - HandleQuery (100), HandleSetRole (101), HandleClose (102), HandleDuplicate (103)
//!
//! ### StreamOps Capability (200-299)
//! - StreamRead (200), StreamWrite (201)
//!
//! ### FileObject Capability (300-399)
//! - FileSeek (300), FileTruncate (301), FileMetadata (302)
//!
//! ### VFS Operations (400-499)
//! - VfsOpen (400), VfsRemove (401), VfsCreateFile (402), VfsCreateDirectory (403), VfsChangeDirectory (404), VfsTruncate (405), VfsCreateSymlink (406), VfsReadlink (407), VfsGetCwdPath (408), VfsRename (409), VfsMetadata (410), VfsCreateHardlink (411)
//!
//! ### Filesystem Operations (500-599)
//! - FsMount (500), FsUmount (501), FsPivotRoot (502)
//!
//! ### IPC Operations (600-699)
//! - Pipe (600)
//! - Event Channels: Subscribe (610), Unsubscribe (611), Publish (612)
//! - Shared Memory: Create (620)
//!
//! ### Memory Mapping Operations (700-799)
//! - MemoryMap (700), MemoryUnmap (701)
//!
//! ### Socket Operations (900-999)
//! - SocketCreate (900), SocketBind (901), SocketListen (902), SocketConnect (903)
//! - SocketAccept (904), Socketpair (905), SocketShutdown (906)
//!
//! ### Task Event Operations (800-899)  
//! - Basic Events: Send (800), SetAction (801), Block (802)
//! - Event Status: GetPending (803), HasPending (804)
//! - Signal-like Operations: Terminate, Kill, Interrupt, etc.
//!
//! ## Design Principles
//!
//! - **Capability-based security**: Objects expose specific capabilities
//! - **Type safety**: Compile-time checking of valid operations
//! - **Backward compatibility**: Legacy APIs redirect to new implementations
//! - **Clear semantics**: Descriptive names (CreateDirectory vs mkdir)
//!
//! ## System Call Table
//!
//! The system call table maps numbers to handler functions using the
//! `syscall_table!` macro for type safety and consistency.
//!

use crate::arch::Trapframe;
use crate::fs::vfs_v2::syscall::{
    sys_fs_mount, sys_fs_pivot_root, sys_fs_umount, sys_vfs_change_directory,
    sys_vfs_create_directory, sys_vfs_create_file, sys_vfs_create_hardlink, sys_vfs_create_symlink,
    sys_vfs_get_cwd_path, sys_vfs_metadata, sys_vfs_open, sys_vfs_readlink, sys_vfs_remove,
    sys_vfs_rename, sys_vfs_truncate,
};
use crate::ipc::syscall::{
    sys_event_channel_create, sys_event_handler_register, sys_event_handler_register_native,
    sys_event_handler_unregister_native, sys_event_mask, sys_event_publish, sys_event_return,
    sys_event_send_direct, sys_event_send_group, sys_event_subscribe, sys_event_unsubscribe,
    sys_pipe, sys_shared_memory_create, sys_shared_memory_resize, sys_socket_recv_handle,
    sys_socket_recv_handle_and_data, sys_socket_send_handle, sys_socket_send_handle_and_data,
};
use crate::lsm::syscall::{sys_lsm_list, sys_lsm_load, sys_lsm_unload};
use crate::network::syscall::{
    sys_network_list_interfaces, sys_network_set_gateway, sys_network_set_ipv4,
    sys_network_set_netmask,
};
use crate::network::syscall::{
    sys_socket_accept, sys_socket_bind, sys_socket_connect, sys_socket_create, sys_socket_listen,
    sys_socket_recvfrom, sys_socket_sendto, sys_socket_shutdown, sys_socketpair,
};
use crate::object::capability::file::{sys_file_metadata, sys_file_seek, sys_file_truncate};
use crate::object::capability::memory_mapping::{sys_memory_map, sys_memory_unmap};
use crate::object::capability::selectable::syscall::sys_poll;
use crate::object::capability::stream::{sys_stream_read, sys_stream_write};
use crate::object::handle::syscall::{
    sys_handle_close, sys_handle_control, sys_handle_duplicate, sys_handle_query,
    sys_handle_set_role,
};
use crate::random::sys_get_random;
use crate::task::syscall::{
    sys_brk, sys_clone, sys_create_namespace, sys_create_session, sys_execve, sys_execve_abi,
    sys_exit, sys_exit_group, sys_get_cpu_usage_info, sys_get_process_group_id, sys_get_session_id,
    sys_get_task_info_count, sys_get_task_info_list, sys_get_task_util_min, sys_get_tls,
    sys_getchar, sys_getpid, sys_getppid, sys_monotonic_time, sys_putchar, sys_register_abi_zone,
    sys_sbrk, sys_set_process_group, sys_set_task_util_min, sys_set_tid_address, sys_set_tls,
    sys_shutdown, sys_sleep, sys_system_time, sys_thread_detach, sys_thread_exit_cleanup,
    sys_unregister_abi_zone, sys_waitpid, sys_yield,
};

#[macro_use]
mod macros;

/// Debug/Profiler system call to dump profiler statistics
#[cfg(feature = "profiler")]
fn sys_profiler_dump(tf: &mut Trapframe) -> usize {
    use crate::task::mytask;
    tf.increment_pc_next(&mytask().unwrap());
    crate::profiler::print_profiling_results();
    0
}

/// Stub implementation when profiler feature is disabled
#[cfg(not(feature = "profiler"))]
fn sys_profiler_dump(tf: &mut Trapframe) -> usize {
    use crate::task::mytask;
    tf.increment_pc_next(&mytask().unwrap());
    crate::println!("[Profiler] Not available (feature disabled)");
    0
}

#[cfg(feature = "hypervisor")]
use crate::hypervisor::syscall::{sys_shv_vcpu_create, sys_shv_vcpu_run, sys_shv_vm_create};

#[cfg(not(feature = "hypervisor"))]
fn sys_shv_vm_create(tf: &mut Trapframe) -> usize {
    use crate::task::mytask;
    tf.increment_pc_next(&mytask().unwrap());
    usize::MAX
}

#[cfg(not(feature = "hypervisor"))]
fn sys_shv_vcpu_create(tf: &mut Trapframe) -> usize {
    use crate::task::mytask;
    tf.increment_pc_next(&mytask().unwrap());
    usize::MAX
}

#[cfg(not(feature = "hypervisor"))]
fn sys_shv_vcpu_run(tf: &mut Trapframe) -> usize {
    use crate::task::mytask;
    tf.increment_pc_next(&mytask().unwrap());
    usize::MAX
}

syscall_table! {
    Invalid = 0 => |_: &mut Trapframe| {
        0
    },
    Exit = 1 => sys_exit,
    Clone = 2 => sys_clone,
    Execve = 3 => sys_execve,
    ExecveABI = 4 => sys_execve_abi,
    Waitpid = 5 => sys_waitpid,
    Kill = 6 => |_: &mut Trapframe| {
        // Kill syscall is not implemented yet
        usize::MAX // -1
    },
    Getpid = 7 => sys_getpid,
    Getppid = 8 => sys_getppid,
    Brk = 12 => sys_brk,
    Sbrk = 13 => sys_sbrk,
    // BASIC I/O
    Putchar = 16 => sys_putchar,
    Getchar = 17 => sys_getchar,

    Sleep = 20 => sys_sleep,

    Yield = 21 => sys_yield,
    GetRandom = 22 => sys_get_random,

    ExitGroup = 23 => sys_exit_group, // Exit all tasks in thread group
    // Process information
    GetTaskInfoCount = 24 => sys_get_task_info_count, // Number of tasks
    GetTaskInfoList = 25 => sys_get_task_info_list,   // TaskInfo snapshots
    CreateSession = 26 => sys_create_session,
    GetSessionId = 27 => sys_get_session_id,
    GetProcessGroupId = 28 => sys_get_process_group_id,
    SetProcessGroup = 29 => sys_set_process_group,
    // TLS (Thread Local Storage) Management
    SetTls = 30 => sys_set_tls,
    GetTls = 31 => sys_get_tls,
    SetTidAddress = 32 => sys_set_tid_address,
    ThreadDetach = 33 => sys_thread_detach,
    ThreadExitCleanup = 34 => sys_thread_exit_cleanup,
    MonotonicTime = 35 => sys_monotonic_time,
    GetCpuUsageInfo = 36 => sys_get_cpu_usage_info,
    SystemTime = 37 => sys_system_time,
    SetTaskUtilMin = 38 => sys_set_task_util_min,
    GetTaskUtilMin = 39 => sys_get_task_util_min,

    // ABI Zone Management
    RegisterAbiZone = 90 => sys_register_abi_zone,
    UnregisterAbiZone = 91 => sys_unregister_abi_zone,

    // Namespace Management (Scarlet-style smart syscall)
    CreateNamespace = 92 => sys_create_namespace,

    // === Handle Management ===
    HandleQuery = 100 => sys_handle_query,     // Query handle metadata/capabilities
    HandleSetRole = 101 => sys_handle_set_role, // Change handle role after creation
    HandleClose = 102 => sys_handle_close,     // Close any handle (files, pipes, etc.)
    HandleDuplicate = 103 => sys_handle_duplicate, // Duplicate any handle
    HandleControl = 110 => sys_handle_control,  // Control operations on handles (ioctl-equivalent)

    // === StreamOps Capability ===
    // Stream operations for any KernelObject with StreamOps capability
    StreamRead = 200 => sys_stream_read,   // StreamOps::read
    StreamWrite = 201 => sys_stream_write, // StreamOps::write
    Poll = 202 => sys_poll,               // Poll handles for readiness

    // === FileObject Capability ===
    // File operations for any KernelObject with FileObject capability
    FileSeek = 300 => sys_file_seek,       // FileObject::seek
    FileTruncate = 301 => sys_file_truncate, // FileObject::truncate
    FileMetadata = 302 => sys_file_metadata, // FileObject::metadata

    // === VFS Operations ===
    VfsOpen = 400 => sys_vfs_open,             // VFS file/directory open
    VfsRemove = 401 => sys_vfs_remove,         // Remove files or directories (unified)
    VfsCreateFile = 402 => sys_vfs_create_file, // Create regular files through VFS
    VfsCreateDirectory = 403 => sys_vfs_create_directory, // Create directories through VFS
    VfsChangeDirectory = 404 => sys_vfs_change_directory, // Change current working directory
    VfsTruncate = 405 => sys_vfs_truncate,     // Truncate file by path
    VfsCreateSymlink = 406 => sys_vfs_create_symlink, // Create symbolic links through VFS
    VfsReadlink = 407 => sys_vfs_readlink,     // Read symbolic link target through VFS
    VfsGetCwdPath = 408 => sys_vfs_get_cwd_path, // Get current working directory path
    VfsRename = 409 => sys_vfs_rename,         // Rename or move files/directories through VFS
    VfsMetadata = 410 => sys_vfs_metadata,     // Get metadata for a VFS path
    VfsCreateHardlink = 411 => sys_vfs_create_hardlink, // Create hard links through VFS

    // === Filesystem Operations ===
    FsMount = 500 => sys_fs_mount,         // Mount filesystem
    FsUmount = 501 => sys_fs_umount,       // Unmount filesystem
    FsPivotRoot = 502 => sys_fs_pivot_root, // Change root filesystem

    // === IPC Operations ===
    Pipe = 600 => sys_pipe,                // Create pipe handles

    // Event System (Handle-based, ABI-layer only)
    EventChannelCreate = 610 => sys_event_channel_create,      // Create/open event channel (ABI use)
    EventSubscribe = 611 => sys_event_subscribe,               // Subscribe to channel (ABI use)
    EventUnsubscribe = 612 => sys_event_unsubscribe,           // Unsubscribe from channel (ABI use)
    EventPublish = 613 => sys_event_publish,                   // Publish event to channel (ABI use)
    EventHandlerRegister = 614 => sys_event_handler_register,  // Register event filter (ABI use)
    EventSendDirect = 615 => sys_event_send_direct,            // Send direct event to task (ABI use)
    EventSendGroup = 616 => sys_event_send_group,              // Send process-control event to process group

    // Shared Memory
    SharedMemoryCreate = 620 => sys_shared_memory_create,      // Create shared memory region
    SharedMemoryResize = 621 => sys_shared_memory_resize,      // Resize shared memory region

    // Socket Handle Transfer (similar to SCM_RIGHTS)
    SocketSendHandle = 630 => sys_socket_send_handle,          // Send kernel object handle through socket
    SocketRecvHandle = 631 => sys_socket_recv_handle,          // Receive kernel object handle from socket
    SocketSendHandleAndData = 632 => sys_socket_send_handle_and_data, // Send handle and data atomically
    SocketRecvHandleAndData = 633 => sys_socket_recv_handle_and_data, // Receive handle and data atomically

    // Scarlet Native Event Handling
    EventHandlerRegisterNative = 640 => sys_event_handler_register_native,   // Register event handler (Scarlet Native)
    EventHandlerUnregisterNative = 641 => sys_event_handler_unregister_native, // Unregister event handler (Scarlet Native)
    EventMask = 642 => sys_event_mask,                                       // Set event mask (Scarlet Native)
    EventReturn = 643 => sys_event_return,                                   // Return from event handler (Scarlet Native)


    // === Memory Mapping Operations ===
    MemoryMap = 700 => sys_memory_map,     // Memory map operation (mmap)
    MemoryUnmap = 701 => sys_memory_unmap, // Memory unmap operation (munmap)

    // === Socket Operations (Scarlet Native) ===
    SocketCreate = 900 => sys_socket_create,     // Create a socket (domain/type/protocol)
    SocketBind = 901 => sys_socket_bind,         // Bind socket to path
    SocketListen = 902 => sys_socket_listen,     // Start listening
    SocketConnect = 903 => sys_socket_connect,   // Connect to socket
    SocketAccept = 904 => sys_socket_accept,     // Accept connection
    Socketpair = 905 => sys_socketpair,          // Create socket pair
    SocketShutdown = 906 => sys_socket_shutdown, // Shutdown socket

    // === Datagram Operations (UDP/Local datagram) ===
    SocketRecvFrom = 907 => sys_socket_recvfrom, // Receive datagram with sender address
    SocketSendTo = 908 => sys_socket_sendto,     // Send datagram to specified address

    // === Network Configuration ===
    NetworkSetIpv4 = 910 => sys_network_set_ipv4,       // Set interface IPv4 address
    NetworkSetGateway = 911 => sys_network_set_gateway, // Set default gateway
    NetworkSetNetmask = 913 => sys_network_set_netmask, // Set subnet mask
    NetworkListInterfaces = 914 => sys_network_list_interfaces, // List network interfaces

    // === Task Event Operations ===

    // === System Control Operations ===
    Shutdown = 1000 => sys_shutdown,          // Shutdown the system gracefully

    // === Debug/Profiler Operations ===
    ProfilerDump = 999 => sys_profiler_dump, // Dump profiler statistics (debug only)

    // === Hypervisor Operations (Scarlet Native) ===
    ShvVmCreate = 1100 => sys_shv_vm_create,
    ShvVcpuCreate = 1101 => sys_shv_vcpu_create,
    ShvVcpuRun = 1102 => sys_shv_vcpu_run,

    // === Loadable Module Operations ===
    LsmLoad = 1200 => sys_lsm_load,
    LsmUnload = 1201 => sys_lsm_unload,
    LsmList = 1202 => sys_lsm_list,
}
