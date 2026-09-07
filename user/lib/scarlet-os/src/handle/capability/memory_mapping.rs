//! Memory mapping operations capability for Scarlet Native API
//!
//! This module provides memory mapping functionality for handles that support
//! memory mapping operations.

use crate::handle::Handle;
use scarlet_sys::{Syscall, syscall2, syscall6};

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
    ///
    /// # Arguments
    ///
    /// * `addr` - Address hint, or the exact address with `flags::FIXED`.
    /// * `length` - Mapping length in bytes.
    /// * `prot` - Requested memory protection flags.
    /// * `flags` - Mapping flags interpreted by the kernel.
    /// * `offset` - Byte offset in the backing object.
    ///
    /// # Returns
    ///
    /// A raw mapping address, or `Err(())`. The result does not borrow this
    /// handle and is not an owning Rust mapping guard.
    ///
    /// # Safety
    ///
    /// Replacing mappings must not invalidate live references, allocations,
    /// executable code or stacks. The caller must coordinate backing-object
    /// changes and all accesses to shared/device memory, and keep any Rust
    /// references within the mapping's lifetime and protection/aliasing rules.
    ///
    /// ```compile_fail,E0133
    /// # fn map(handle: &scarlet_os::Handle) {
    /// let mapper = handle.as_memory_mapping().unwrap();
    /// mapper.mmap(0, 4096, 3, 1, 0);
    /// # }
    /// ```
    pub unsafe fn mmap(
        &self,
        addr: usize,
        length: usize,
        prot: usize,
        flags: usize,
        offset: usize,
    ) -> Result<usize, ()> {
        // SAFETY: The caller guarantees mapping ownership, replacement safety and shared-memory access invariants.
        let result = unsafe {
            syscall6(
                Syscall::MemoryMap,
                self.handle.as_raw() as usize,
                addr,
                length,
                prot,
                flags,
                offset,
            )
        };
        if result == usize::MAX {
            Err(())
        } else {
            Ok(result)
        }
    }

    /// Unmap a memory region from the current process's address space.
    ///
    /// # Arguments
    ///
    /// * `addr` - Page-aligned start of the region to release.
    /// * `length` - Length of the region in bytes.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or `Err(())` if the kernel rejects the request.
    ///
    /// # Safety
    ///
    /// The caller must satisfy the same ownership and quiescence requirements
    /// as the free [`munmap`] function.
    ///
    /// ```compile_fail,E0133
    /// use scarlet_os::handle::capability::memory_mapping::MemoryMappingOps;
    /// MemoryMappingOps::munmap(0x1000, 4096);
    /// ```
    pub unsafe fn munmap(addr: usize, length: usize) -> Result<(), ()> {
        // SAFETY: The caller guarantees exclusive authority to release this region.
        unsafe { munmap(addr, length) }
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
/// # Safety
///
/// The caller must own the entire affected mapping range and ensure that no
/// live reference, allocation, running code, thread stack or in-flight device
/// access relies on it. Coordinate other threads and mapping owners before
/// releasing it. Kernel address validation cannot enforce these Rust lifetimes.
///
/// # Examples
/// ```no_run
/// use scarlet_os::handle::capability::memory_mapping::munmap;
///
/// # unsafe fn release_owned_mapping(mapped_addr: usize) -> Result<(), ()> {
/// // Unmap a previously mapped region
/// // SAFETY: This helper's caller owns the mapping and has ended all accesses.
/// unsafe { munmap(mapped_addr, 4096)? };
/// # Ok(())
/// # }
/// ```
///
/// ```compile_fail,E0133
/// scarlet_os::handle::capability::memory_mapping::munmap(0x1000, 4096);
/// ```
pub unsafe fn munmap(addr: usize, length: usize) -> Result<(), ()> {
    // SAFETY: The caller owns this range and guarantees that no live references or accesses depend on it.
    let result = unsafe { syscall2(Syscall::MemoryUnmap, addr, length) };
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
///
/// # Safety
///
/// A fixed mapping must not replace memory used by live Rust references,
/// allocations, code or stacks. The caller owns the returned raw mapping and
/// must enforce its protection, aliasing and eventual unmapping requirements.
///
/// ```compile_fail,E0133
/// scarlet_os::handle::capability::memory_mapping::mmap_anonymous(0, 4096, 3, 2);
/// ```
pub unsafe fn mmap_anonymous(
    addr: usize,
    length: usize,
    prot: usize,
    flags: usize,
) -> Result<usize, ()> {
    // SAFETY: The caller guarantees mapping ownership, replacement safety and shared-memory access invariants.
    let result = unsafe {
        syscall6(
            Syscall::MemoryMap,
            0,
            addr,
            length,
            prot,
            flags | flags::ANONYMOUS,
            0,
        )
    };
    if result == usize::MAX {
        Err(())
    } else {
        Ok(result)
    }
}
