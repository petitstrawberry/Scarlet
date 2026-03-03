//! Memory mapping operations capability for Scarlet Native API
//!
//! This module provides memory mapping functionality for handles that support
//! memory mapping operations.

use crate::handle::Handle;
use crate::syscall::{Syscall, syscall2, syscall6};

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

/// Memory mapping operations capability
pub struct MemoryMappingOps<'a> {
    handle: &'a Handle,
}

impl<'a> MemoryMappingOps<'a> {
    /// Create MemoryMappingOps from a Handle reference.
    ///
    /// This is crate-internal to prevent bypassing `Handle::as_memory_mapping` validation.
    pub(crate) fn from_handle(handle: &'a Handle) -> Self {
        Self { handle }
    }

    /// Memory map this handle into the current process's address space.
    pub fn mmap(
        &self,
        addr: usize,
        length: usize,
        prot: usize,
        flags: usize,
        offset: usize,
    ) -> Result<usize, ()> {
        let result = syscall6(
            Syscall::MemoryMap,
            self.handle.as_raw() as usize,
            addr,
            length,
            prot,
            flags,
            offset,
        );
        if result == usize::MAX {
            Err(())
        } else {
            Ok(result)
        }
    }

    /// Unmap a memory region from the current process's address space.
    pub fn munmap(addr: usize, length: usize) -> Result<(), ()> {
        munmap(addr, length)
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

/// Map anonymous memory into the current process's address space.
///
/// # Arguments
/// * `addr` - Hint for the address (0 for any)
/// * `length` - Length of the mapping
/// * `prot` - Protection flags (prot::* constants)
/// * `flags` - Mapping flags (flags::* constants, ANONYMOUS is always added)
///
/// # Returns
/// * `Ok(addr)` - Address of the mapping
/// * `Err(())` - Mapping failed
pub fn mmap_anonymous(addr: usize, length: usize, prot: usize, flags: usize) -> Result<usize, ()> {
    let result = syscall6(
        Syscall::MemoryMap,
        0,
        addr,
        length,
        prot,
        flags | flags::ANONYMOUS,
        0,
    );
    if result == usize::MAX {
        Err(())
    } else {
        Ok(result)
    }
}
