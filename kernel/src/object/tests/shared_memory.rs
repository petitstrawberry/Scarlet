//! SharedMemory KernelObject tests

use crate::ipc::shared_memory::SharedMemory;
use crate::object::KernelObject;
use alloc::sync::Arc;

#[test_case]
fn test_shared_memory_kernel_object_creation() {
    // Create a shared memory object from a physical address
    let paddr = 0x80000000;
    let size = 4096;
    let permissions = 0x3; // Read + Write

    let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };
    let kernel_obj = KernelObject::from_shared_memory_object(Arc::new(shmem));

    // Test as_shared_memory capability
    assert!(kernel_obj.as_shared_memory().is_some());

    // Test as_memory_mappable capability
    assert!(kernel_obj.as_memory_mappable().is_some());

    // Verify that it doesn't have capabilities it shouldn't
    assert!(kernel_obj.as_stream().is_none());
    assert!(kernel_obj.as_file().is_none());
    assert!(kernel_obj.as_pipe().is_none());
}

#[test_case]
fn test_shared_memory_memory_mapping_ops() {
    let paddr = 0x80000000;
    let size = 8192;
    let permissions = 0x3; // Read + Write

    let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };
    let kernel_obj = KernelObject::from_shared_memory_object(Arc::new(shmem));

    if let Some(mem_mappable) = kernel_obj.as_memory_mappable() {
        // Test get_mapping_info
        let result = mem_mappable.get_mapping_info(0, 4096);
        assert!(result.is_ok());

        let (mapped_paddr, mapped_perms, is_shared) = result.unwrap();
        assert_eq!(mapped_paddr, paddr);
        assert_eq!(mapped_perms, permissions);
        assert!(is_shared); // Shared memory should always be shared

        // Test supports_mmap
        assert!(mem_mappable.supports_mmap());

        // Test on_mapped notification
        mem_mappable.on_mapped(0x10000000, paddr, 4096, 0);

        // Test on_unmapped notification
        mem_mappable.on_unmapped(0x10000000, 4096);
    } else {
        panic!("Expected memory mappable capability");
    }
}

#[test_case]
fn test_shared_memory_weak_reference() {
    let paddr = 0x80000000;
    let size = 4096;
    let permissions = 0x3;

    let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };
    let kernel_obj = KernelObject::from_shared_memory_object(Arc::new(shmem));

    // Test as_memory_mappable_weak
    let weak_ref = kernel_obj.as_memory_mappable_weak();
    assert!(weak_ref.is_some());

    // The weak reference should be upgradeable
    if let Some(weak) = weak_ref {
        assert!(weak.upgrade().is_some());
    }
}

#[test_case]
fn test_shared_memory_clone() {
    let paddr = 0x80000000;
    let size = 4096;
    let permissions = 0x3;

    let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };
    let kernel_obj1 = KernelObject::from_shared_memory_object(Arc::new(shmem));

    // Clone the kernel object
    let kernel_obj2 = kernel_obj1.clone();

    // Both should work independently but share the same underlying memory
    assert!(kernel_obj1.as_shared_memory().is_some());
    assert!(kernel_obj2.as_shared_memory().is_some());
    assert!(kernel_obj1.as_memory_mappable().is_some());
    assert!(kernel_obj2.as_memory_mappable().is_some());

    // Both should report the same size
    if let (Some(shmem1), Some(shmem2)) = (
        kernel_obj1.as_shared_memory(),
        kernel_obj2.as_shared_memory(),
    ) {
        assert_eq!(shmem1.size(), size);
        assert_eq!(shmem2.size(), size);
        assert_eq!(shmem1.size(), shmem2.size());
    }
}

#[test_case]
fn test_shared_memory_mapping_offset() {
    let paddr = 0x80000000;
    let size = 16384; // 16KB
    let permissions = 0x3;

    let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };
    let kernel_obj = KernelObject::from_shared_memory_object(Arc::new(shmem));

    if let Some(mem_mappable) = kernel_obj.as_memory_mappable() {
        // Test mapping with different offsets
        let offset1 = 0;
        let result1 = mem_mappable.get_mapping_info(offset1, 4096);
        assert!(result1.is_ok());
        let (paddr1, _, _) = result1.unwrap();
        assert_eq!(paddr1, paddr + offset1);

        let offset2 = 4096;
        let result2 = mem_mappable.get_mapping_info(offset2, 4096);
        assert!(result2.is_ok());
        let (paddr2, _, _) = result2.unwrap();
        assert_eq!(paddr2, paddr + offset2);

        let offset3 = 8192;
        let result3 = mem_mappable.get_mapping_info(offset3, 4096);
        assert!(result3.is_ok());
        let (paddr3, _, _) = result3.unwrap();
        assert_eq!(paddr3, paddr + offset3);

        // Test out of bounds mapping
        let result_oob = mem_mappable.get_mapping_info(size, 1);
        assert!(result_oob.is_err());
    }
}

#[test_case]
fn test_shared_memory_invalidation() {
    let paddr = 0x80000000;
    let size = 4096;
    let permissions = 0x3;

    let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };
    let shmem_arc = Arc::new(shmem);

    // Initial state should be valid
    assert!(shmem_arc.is_valid());

    let kernel_obj = KernelObject::from_shared_memory_object(shmem_arc.clone());

    // Invalidate the shared memory
    shmem_arc.invalidate();

    // Should no longer be valid
    assert!(!shmem_arc.is_valid());

    // Memory mapping operations should fail after invalidation
    if let Some(mem_mappable) = kernel_obj.as_memory_mappable() {
        assert!(!mem_mappable.supports_mmap());
        assert!(mem_mappable.get_mapping_info(0, 4096).is_err());
    }
}
