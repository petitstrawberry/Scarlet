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
    
    // === Handle Management ===
    HandleQuery = 100,
    HandleSetRole = 101,
    HandleClose = 102,      // Close any handle (files, pipes, etc.)
    HandleDuplicate = 103,  // Duplicate any handle
    Pipe = 104,             // Create pipe handles
    
    // === Core Capabilities (Object-oriented) ===
    // StreamOps Capability - read/write operations
    StreamRead = 200,
    StreamWrite = 201,
    
    // FileObject Capability - file-specific operations (extends StreamOps)
    FileSeek = 300,
    FileTruncate = 301,
    // FileMetadata = 302,
    
    // === VFS Operations (VFS layer management and file access) ===
    VfsOpen = 400,          // Open files/directories through VFS
    VfsRemove = 401,        // Remove files or directories (unified Remove/Unlink)
    VfsCreateFile = 402,    // Create regular files through VFS
    VfsCreateDirectory = 403, // Create directories through VFS
    VfsChangeDirectory = 404, // Change current working directory
    VfsTruncate = 405,      // Truncate files by path
    
    // === Filesystem Operations (mount management) ===
    FsMount = 500,
    FsUmount = 501,
    FsPivotRoot = 502
}

// Backward compatibility aliases
#[allow(non_upper_case_globals)]
impl Syscall {
    // Legacy names for compatibility - these map to the actual legacy syscall numbers
    // defined above (30-35) which redirect to the new implementations
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

pub fn syscall5(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize) -> usize {
    arch_syscall5(syscall, arg1, arg2, arg3, arg4, arg5)
}
