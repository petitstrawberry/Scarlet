//! Memory-range and mapping descriptors; constructing one does not allocate memory
//! or install page-table entries. [`MemoryArea`] uses inclusive end addresses.

use crate::object::capability::memory_mapping::MemoryMappingOps;
use alloc::sync::Arc;
use core::fmt;

/// Represents a mapping between physical and virtual memory areas.
///
/// This structure defines the relationship between a physical memory area
/// and its corresponding virtual memory area in the kernel's memory management system.
///
/// # Fields
///
/// * `pmarea` - The physical memory area that is being mapped
/// * `vmarea` - The virtual memory area where the physical memory is mapped to
/// * `vm_start` - The original virtual address where this mapping was first created.
///   Preserved across split operations so that owner-based page fault resolution can
///   compute correct page indices even after partial unmapping.
/// * `permissions` - The access permissions for this mapping
/// * `is_shared` - Whether this mapping is shared between processes
/// * `memory_attribute` - Cacheability/device attribute requested for this mapping
/// * `owner` - Optional strong reference to the object that provides page fault resolution.
///   The mapping owns this reference, so the owner stays alive as long as the mapping exists.
#[derive(Clone)]
pub struct VirtualMemoryMap {
    pub pmarea: PhysicalMemoryArea,
    pub vmarea: MemoryArea,
    pub vm_start: usize,
    pub permissions: usize,
    pub is_shared: bool,
    pub memory_attribute: MemoryAttribute,
    pub owner: Option<Arc<dyn MemoryMappingOps>>,
}

impl fmt::Debug for VirtualMemoryMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualMemoryMap")
            .field("pmarea", &self.pmarea)
            .field("vmarea", &self.vmarea)
            .field("vm_start", &self.vm_start)
            .field("permissions", &self.permissions)
            .field("is_shared", &self.is_shared)
            .field("memory_attribute", &self.memory_attribute)
            .field("has_owner", &self.owner.is_some())
            .finish()
    }
}

impl Default for VirtualMemoryMap {
    fn default() -> Self {
        Self {
            pmarea: PhysicalMemoryArea::new(0, 0),
            vmarea: MemoryArea::new(0, 0),
            vm_start: 0,
            permissions: 0,
            is_shared: false,
            memory_attribute: MemoryAttribute::Normal,
            owner: None,
        }
    }
}

impl VirtualMemoryMap {
    /// Creates a new virtual memory map with the given physical and virtual memory areas.
    ///
    /// This only constructs a descriptor. It does not validate the ranges or
    /// install a mapping in a page table.
    ///
    /// # Arguments
    /// * `pmarea` - The physical memory area to map
    /// * `vmarea` - The virtual memory area to map to
    /// * `permissions` - The permissions to set for the virtual memory area
    /// * `is_shared` - Whether this memory map should be shared between tasks
    /// * `owner` - Optional strong reference retained by this descriptor. `None`
    ///   means that no object supplies owner-based fault resolution for the mapping.
    ///
    /// # Returns
    /// A new virtual memory map with the given physical and virtual memory areas,
    /// `vm_start` set to `vmarea.start`, and the `Normal` memory attribute.
    pub fn new(
        pmarea: PhysicalMemoryArea,
        vmarea: MemoryArea,
        permissions: usize,
        is_shared: bool,
        owner: Option<Arc<dyn MemoryMappingOps>>,
    ) -> Self {
        VirtualMemoryMap {
            pmarea,
            vmarea,
            vm_start: vmarea.start,
            permissions,
            is_shared,
            memory_attribute: MemoryAttribute::Normal,
            owner,
        }
    }

    /// Returns this mapping with the requested memory attribute.
    ///
    /// # Arguments
    /// * `memory_attribute` - Cacheability/device attribute to apply when installing the mapping
    ///
    /// # Returns
    /// The mapping descriptor with the supplied memory attribute.
    pub fn with_memory_attribute(mut self, memory_attribute: MemoryAttribute) -> Self {
        self.memory_attribute = memory_attribute;
        self
    }
}

/// Cacheability and device attributes for a virtual memory mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAttribute {
    /// Normal cacheable memory.
    Normal,
    /// Normal memory without CPU cache allocation.
    NonCacheable,
    /// Device memory for bulk write windows where writes may be gathered.
    ///
    /// This is intended for device-backed buffers such as framebuffers or VRAM.
    /// Use [`MemoryAttribute::Device`] for MMIO registers with side effects.
    DeviceBurstable,
    /// Device memory for MMIO regions.
    Device,
}

/// A physical address range, independent of the kernel pointer width.
///
/// Physical addresses may be wider than virtual addresses (for example Sv32).
/// This descriptor cannot be dereferenced; access requires an installed virtual
/// mapping. `MemoryArea` is reserved for native virtual-address ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalMemoryArea {
    pub start: u64,
    pub end: u64,
}

impl PhysicalMemoryArea {
    pub const fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    /// Inclusive byte count, if the range is nonempty and representable.
    pub fn byte_len(&self) -> Option<u64> {
        self.end.checked_sub(self.start)?.checked_add(1)
    }

    /// Length of a physical range being used as one native-size allocation or mapping.
    /// The range's addresses remain 64-bit even when its byte count is native.
    pub fn size(&self) -> usize {
        usize::try_from(self.byte_len().expect("invalid physical memory range"))
            .expect("physical span is too large for one native-size mapping")
    }
}

/// An inclusive address range, without ownership or mapping validation.
///
/// This descriptor contains virtual addresses. Physical ranges use
/// `PhysicalMemoryArea`. Copying a descriptor does not retain or map memory.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryArea {
    /// First address in the range.
    pub start: usize,
    /// Last address in the range, inclusive.
    pub end: usize,
}

impl MemoryArea {
    /// Creates a new memory area with the given start and end addresses
    ///
    /// # Arguments
    ///
    /// * `start` - First address in the range.
    /// * `end` - Inclusive last address; not an exclusive bound.
    ///
    /// # Returns
    ///
    /// An unchecked range descriptor. No memory is allocated or accessed.
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Creates a new memory area from a pointer and size
    ///
    /// # Arguments
    ///
    /// * `ptr` - Address to record; this constructor does not dereference it.
    /// * `size` - Nonzero byte count. The inclusive last address must fit in `usize`.
    ///
    /// # Returns
    ///
    /// A descriptor of the supplied addresses, without retaining the allocation,
    /// or `None` for zero bytes or address overflow. Inclusive ranges cannot
    /// represent an empty allocation; a single byte at `usize::MAX` is valid.
    pub fn from_ptr(ptr: *const u8, size: usize) -> Option<Self> {
        let start = ptr as usize;
        let end = start.checked_add(size.checked_sub(1)?)?;
        Some(Self { start, end })
    }

    /// Returns the size of the memory area in bytes
    ///
    /// # Returns
    ///
    /// `end - start + 1`. The inclusive byte count must fit in `usize`.
    ///
    /// # Panics
    ///
    /// Panics if `start > end`, or on arithmetic overflow when overflow checks
    /// are enabled.
    pub fn size(&self) -> usize {
        if self.start > self.end {
            panic!(
                "Invalid memory area: start > end: {:#x} > {:#x}",
                self.start, self.end
            );
        }
        self.end - self.start + 1
    }

    /// Returns a slice reference to the memory area
    ///
    /// # Arguments
    ///
    /// * `self` - An inclusive range of kernel virtual addresses, not physical addresses.
    ///
    /// # Safety
    /// The entire range must be non-null, readable, initialized memory within a
    /// single live allocation, with a representable length no greater than
    /// `isize::MAX`. It must remain mapped and allocated for the returned borrow.
    /// No writes, including concurrent CPU or DMA writes, may occur during that
    /// borrow. The descriptor itself does not establish any of these conditions;
    /// violating them can cause undefined behavior.
    ///
    /// # Returns
    ///
    /// A slice reference to the memory area
    ///
    pub unsafe fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.start as *const u8, self.size()) }
    }

    /// Returns a mutable slice reference to the memory area
    ///
    /// # Arguments
    ///
    /// * `self` - An inclusive range of kernel virtual addresses, not physical addresses.
    ///
    /// # Safety
    /// The entire range must be non-null, readable and writable, initialized
    /// memory within a single live allocation, with a representable length no
    /// greater than `isize::MAX`. It must remain mapped and allocated for the
    /// returned borrow. The caller must guarantee exclusive access for that
    /// lifetime: no overlapping references or concurrent CPU/DMA accesses are
    /// allowed, including accesses through copies of this descriptor. A shared
    /// borrow of `MemoryArea` does not enforce this exclusivity. Violating these
    /// conditions can cause undefined behavior.
    ///
    /// # Returns
    ///
    /// A mutable slice reference to the memory area
    ///
    pub unsafe fn as_slice_mut(&self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.start as *mut u8, self.size()) }
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryArea;

    #[test_case]
    fn memory_area_from_ptr_rejects_empty_ranges() {
        for address in [0, 0x1000, usize::MAX] {
            assert_eq!(MemoryArea::from_ptr(address as *const u8, 0), None);
        }
    }

    #[test_case]
    fn memory_area_from_ptr_preserves_inclusive_bounds() {
        for (address, size) in [(0, 1), (0x1000, 4096), (usize::MAX, 1)] {
            let area = MemoryArea::from_ptr(address as *const u8, size)
                .expect("nonempty representable range");
            assert_eq!(area.start, address);
            assert_eq!(area.end, address + (size - 1));
            assert_eq!(area.size(), size);
        }
    }

    #[test_case]
    fn memory_area_from_ptr_rejects_overflow() {
        assert_eq!(MemoryArea::from_ptr(usize::MAX as *const u8, 2), None);
        let address = (usize::MAX - 3) as *const u8;
        assert_eq!(MemoryArea::from_ptr(address, 5), None);
        assert_eq!(
            MemoryArea::from_ptr(address, 4),
            Some(MemoryArea::new(usize::MAX - 3, usize::MAX))
        );
    }
}

#[derive(Debug, Clone, Copy)]
pub enum VirtualMemoryPermission {
    Read = 0x01,
    Write = 0x02,
    Execute = 0x04,
    User = 0x08,
}

impl From<usize> for VirtualMemoryPermission {
    fn from(value: usize) -> Self {
        match value {
            0x01 => VirtualMemoryPermission::Read,
            0x02 => VirtualMemoryPermission::Write,
            0x04 => VirtualMemoryPermission::Execute,
            0x08 => VirtualMemoryPermission::User,
            _ => panic!("Invalid permission value: {}", value),
        }
    }
}

impl VirtualMemoryPermission {
    pub fn contained_in(&self, permissions: usize) -> bool {
        permissions & (*self as usize) != 0
    }
}

pub enum VirtualMemoryRegion {
    Text,
    Data,
    Bss,
    Heap,
    Stack,
    Guard,
    Unknown,
}

impl VirtualMemoryRegion {
    pub fn default_permissions(&self) -> usize {
        match self {
            VirtualMemoryRegion::Text => {
                VirtualMemoryPermission::Read as usize
                    | VirtualMemoryPermission::Execute as usize
                    | VirtualMemoryPermission::User as usize
            }
            VirtualMemoryRegion::Data => {
                VirtualMemoryPermission::Read as usize
                    | VirtualMemoryPermission::Write as usize
                    | VirtualMemoryPermission::User as usize
            }
            VirtualMemoryRegion::Bss => {
                VirtualMemoryPermission::Read as usize
                    | VirtualMemoryPermission::Write as usize
                    | VirtualMemoryPermission::User as usize
            }
            VirtualMemoryRegion::Heap => {
                VirtualMemoryPermission::Read as usize
                    | VirtualMemoryPermission::Write as usize
                    | VirtualMemoryPermission::User as usize
            }
            VirtualMemoryRegion::Stack => {
                VirtualMemoryPermission::Read as usize
                    | VirtualMemoryPermission::Write as usize
                    | VirtualMemoryPermission::User as usize
            }
            VirtualMemoryRegion::Guard => 0, // Any access to the guard page should cause a page fault
            VirtualMemoryRegion::Unknown => panic!("Unknown memory segment"),
        }
    }

    /// Returns whether this memory region should be shared between tasks by default
    pub fn is_shareable(&self) -> bool {
        match self {
            VirtualMemoryRegion::Text => true, // Text segments can be shared (read-only executable code)
            VirtualMemoryRegion::Data => false, // Data segments should not be shared (writable)
            VirtualMemoryRegion::Bss => false, // BSS segments should not be shared (writable)
            VirtualMemoryRegion::Heap => false, // Heap should not be shared (writable)
            VirtualMemoryRegion::Stack => false, // Stack should not be shared (writable, task-specific)
            VirtualMemoryRegion::Guard => true,  // Guard pages can be shared (no physical backing)
            VirtualMemoryRegion::Unknown => false, // Unknown segments should not be shared by default
        }
    }
}
