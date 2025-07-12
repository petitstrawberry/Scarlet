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
//! - **100-199**: Handle management and IPC operations (handle_query, handle_close, dup, pipe)
//! - **200-299**: StreamOps capability (stream_read, stream_write operations)
//! - **300-399**: FileObject capability (file_seek, file_truncate, file_metadata)
//! - **400-499**: VFS operations (vfs_open, vfs_remove, vfs_create_directory, vfs_change_directory, vfs_truncate)
//! - **500-599**: Filesystem operations (fs_mount, fs_umount, fs_pivot_root)
//! 
//! Legacy POSIX-like system calls (20-35) are maintained for backward compatibility
//! and redirect to the appropriate capability-based implementations.
//! 
//! ## Current Implementation Status
//! 
//! ### Process Management (1-99)
//! - Exit (1), Clone (2), Execve (3), ExecveABI (4), Waitpid (5)
//! - Getpid (7), Getppid (8), Brk (12), Sbrk (13)
//! - Basic I/O: Putchar (16), Getchar (17)
//! 
//! ### Legacy POSIX Compatibility (20-35)
//! - Open (20) → VfsOpen, Close (21) → HandleClose, Read (22) → StreamRead, Write (23) → StreamWrite
//! - Lseek (24) → FileSeek, Truncate (26) → VfsTruncate, Dup (27) → HandleDuplicate
//! - CreateDir (30) → VfsCreateDirectory, Mount (32) → FsMount
//! - Umount (33) → FsUmount, PivotRoot (34) → FsPivotRoot, Chdir (35) → VfsChangeDirectory
//! 
//! ### Handle Management (100-199)
//! - HandleQuery (100), HandleSetRole (101), HandleClose (102), HandleDuplicate (103)
//! - Pipe (104)
//! 
//! ### StreamOps Capability (200-299)
//! - StreamRead (200), StreamWrite (201)
//! 
//! ### FileObject Capability (300-399)
//! - FileSeek (300), FileTruncate (301), FileMetadata (302)
//! 
//! ### VFS Operations (400-499)
//! - VfsOpen (400), VfsRemove (401), VfsCreateDirectory (402), VfsChangeDirectory (403), VfsTruncate (404)
//! 
//! ### Filesystem Operations (500-599)
//! - FsMount (500), FsUmount (501), FsPivotRoot (502)
//! 
//! Legacy POSIX-like system calls (20-35) are maintained for compatibility
//! but new code should prefer the capability-based variants.
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
use crate::fs::vfs_v2::syscall::{sys_vfs_remove, sys_vfs_open, sys_vfs_create_directory, sys_vfs_change_directory, sys_fs_mount, sys_fs_umount, sys_fs_pivot_root, sys_vfs_truncate};
use crate::task::syscall::{sys_brk, sys_clone, sys_execve, sys_execve_abi, sys_exit, sys_getchar, sys_getpid, sys_getppid, sys_putchar, sys_sbrk, sys_waitpid};
use crate::ipc::syscall::sys_pipe;
use crate::object::handle::syscall::{sys_handle_query, sys_handle_set_role, sys_handle_close, sys_handle_duplicate};
use crate::object::capability::stream::{sys_stream_read, sys_stream_write};
use crate::object::capability::file::{sys_file_seek, sys_file_truncate, sys_file_metadata};

#[macro_use]
mod macros;

syscall_table! {
    Invalid = 0 => |_: &mut Trapframe| {
        0
    },
    Exit = 1 => sys_exit,
    Clone = 2 => sys_clone,
    Execve = 3 => sys_execve,
    ExecveABI = 4 => sys_execve_abi,
    Waitpid = 5 => sys_waitpid,
    Getpid = 7 => sys_getpid,
    Getppid = 8 => sys_getppid,
    Brk = 12 => sys_brk,
    Sbrk = 13 => sys_sbrk,
    // BASIC I/O
    Putchar = 16 => sys_putchar,
    Getchar = 17 => sys_getchar,
    
    // === Legacy POSIX-like Operations ===
    Open = 20 => sys_vfs_open,          // Legacy - redirects to VfsOpen
    Close = 21 => sys_handle_close,      // Legacy - redirects to HandleClose
    Read = 22 => sys_stream_read,        // Legacy - redirects to StreamRead
    Write = 23 => sys_stream_write,      // Legacy - redirects to StreamWrite
    Lseek = 24 => sys_file_seek,         // Redirect to FileSeek for compatibility
    // Ftruncate (25) deprecated - use FileTruncate (301)
    Truncate = 26 => sys_vfs_truncate,   // Legacy - redirects to VfsTruncate
    Dup = 27 => sys_handle_duplicate,    // Legacy - redirects to HandleDuplicate
    
    // === Legacy Compatibility ===
    CreateDir = 30 => sys_vfs_create_directory, // Legacy alias for VfsCreateDirectory
    Mount = 32 => sys_fs_mount,                 // Legacy alias for FsMount  
    Umount = 33 => sys_fs_umount,               // Legacy alias for FsUmount
    PivotRoot = 34 => sys_fs_pivot_root,        // Legacy alias for FsPivotRoot
    Chdir = 35 => sys_vfs_change_directory,     // Legacy alias for VfsChangeDirectory
    
    // === Handle Management ===
    HandleQuery = 100 => sys_handle_query,     // Query handle metadata/capabilities
    HandleSetRole = 101 => sys_handle_set_role, // Change handle role after creation
    HandleClose = 102 => sys_handle_close,     // Close any handle (files, pipes, etc.)
    HandleDuplicate = 103 => sys_handle_duplicate, // Duplicate any handle  
    Pipe = 104 => sys_pipe,                    // Create pipe handles
    
    // === StreamOps Capability ===
    // Stream operations for any KernelObject with StreamOps capability
    StreamRead = 200 => sys_stream_read,   // StreamOps::read
    StreamWrite = 201 => sys_stream_write, // StreamOps::write
    
    // === FileObject Capability ===
    // File operations for any KernelObject with FileObject capability
    FileSeek = 300 => sys_file_seek,       // FileObject::seek
    FileTruncate = 301 => sys_file_truncate, // FileObject::truncate
    FileMetadata = 302 => sys_file_metadata, // FileObject::metadata
    
    // === VFS Operations ===
    VfsOpen = 400 => sys_vfs_open,             // VFS file/directory open
    VfsRemove = 401 => sys_vfs_remove,         // Remove files or directories (unified)
    VfsCreateDirectory = 402 => sys_vfs_create_directory, // Create directories through VFS
    VfsChangeDirectory = 403 => sys_vfs_change_directory, // Change current working directory
    VfsTruncate = 404 => sys_vfs_truncate,     // Truncate file by path
    
    // === Filesystem Operations ===
    FsMount = 500 => sys_fs_mount,         // Mount filesystem
    FsUmount = 501 => sys_fs_umount,       // Unmount filesystem  
    FsPivotRoot = 502 => sys_fs_pivot_root, // Change root filesystem
}
