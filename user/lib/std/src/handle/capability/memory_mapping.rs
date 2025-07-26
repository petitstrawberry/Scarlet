//! Memory mapping operations capability for Scarlet Native API
//!
//! This module provides memory mapping functionality for handles that support
//! memory mapping operations.

use crate::syscall::{syscall6, syscall2, Syscall};

/// Memory mapping protection flags (PROT_*)
pub mod prot {
    /// Page can be read
    pub const READ: usize = 0x1;
    /// Page can be written
    pub const WRITE: usize = 0x2;
    /// Page can be executed
    pub const EXEC: usize = 0x4;
    /// Page cannot be accessed
    pub const NONE: usize = 0x0;
}

/// Memory mapping flags (MAP_*)
pub mod flags {
    /// Share changes
    pub const SHARED: usize = 0x01;
    /// Changes are private
    pub const PRIVATE: usize = 0x02;
    /// Interpret addr exactly
    pub const FIXED: usize = 0x10;
    /// Don't use a file
    pub const ANONYMOUS: usize = 0x20;
}

/// Memory map a handle into the current process's address space
///
/// # Arguments
/// * `handle` - Handle to the KernelObject that supports memory mapping
/// * `addr` - Preferred virtual address (0 = let kernel choose)
/// * `length` - Length of the mapping in bytes
/// * `prot` - Protection flags (combination of prot::* constants)
/// * `flags` - Mapping flags (combination of flags::* constants)
/// * `offset` - Offset within the object to start mapping from
///
/// # Returns
/// * `Ok(address)` - Virtual address where the mapping was created
/// * `Err(())` - Mapping failed
///
/// # Examples
/// ```no_run
/// use scarlet_std::handle::capability::memory_mapping::{mmap, prot, flags};
/// 
/// // Map a file handle with read/write permissions
/// let addr = mmap(file_handle, 0, 4096, prot::READ | prot::WRITE, flags::PRIVATE, 0)?;
/// ```
pub fn mmap(handle: u32, addr: usize, length: usize, prot: usize, flags: usize, offset: usize) -> Result<usize, ()> {
    let result = syscall6(Syscall::MemoryMap, handle as usize, addr, length, prot, flags, offset);
    if result == usize::MAX {
        Err(())
    } else {
        Ok(result)
    }
}

/// Unmap a memory region from the current process's address space
///
/// # Arguments
/// * `addr` - Virtual address of the mapping to unmap
/// * `length` - Length of the mapping to unmap
///
/// # Returns
/// * `Ok(())` - Unmapping successful
/// * `Err(())` - Unmapping failed
///
/// # Examples
/// ```no_run
/// use scarlet_std::handle::capability::memory_mapping::munmap;
/// 
/// // Unmap a previously mapped region
/// munmap(mapped_addr, 4096)?;
/// ```
pub fn munmap(addr: usize, length: usize) -> Result<(), ()> {
    let result = syscall2(Syscall::MemoryUnmap, addr, length);
    if result == usize::MAX {
        Err(())
    } else {
        Ok(())
    }
}