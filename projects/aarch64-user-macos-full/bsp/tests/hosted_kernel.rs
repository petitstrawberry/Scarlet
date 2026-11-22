use scarlet::{
    environment::PAGE_SIZE,
    ipc::shared_memory::{SharedMemory, SharedMemoryObject},
    object::capability::memory_mapping::{AccessKind, AccessOp, MappingRequest, MemoryMappingOps},
    vm::{
        phys_to_virt, virt_to_phys,
        vmem::{MemoryArea, PhysicalMemoryArea, VirtualMemoryMap, VirtualMemoryPermission},
    },
};

#[test]
fn shared_memory_mapping_survives_growth_and_handle_release() {
    let _platform = scarlet::hosted::prepare(4, 16 * 1024 * 1024).unwrap();
    assert_invalidation_prevents_initial_fault();
    assert_sequential_writes_stop_at_mapping_end();
    let memory = SharedMemory::new(2 * PAGE_SIZE, 3).unwrap();
    let prepared = memory
        .prepare_mapping(MappingRequest {
            offset: PAGE_SIZE,
            length: PAGE_SIZE,
            is_shared: true,
            permissions: 3,
            private_demand: false,
            allow_shorter: false,
        })
        .unwrap();
    let paddr = prepared.info.unwrap().paddr;
    let alias = phys_to_virt(paddr);
    // SAFETY: This test owns the only mapping to these bytes.
    unsafe { (alias as *mut u8).write_volatile(7) };

    memory.pin_range(0, PAGE_SIZE).unwrap();
    assert!(memory.resize(32 * PAGE_SIZE).is_err());
    memory.unpin_range();
    memory.resize(32 * PAGE_SIZE).unwrap();
    assert_eq!(
        memory.get_mapping_info(PAGE_SIZE, PAGE_SIZE).unwrap().paddr,
        paddr
    );
    assert_eq!(phys_to_virt(paddr), alias);
    assert_eq!(virt_to_phys(alias), paddr);
    let host_page = scarlet_hosted_runtime::ram::host_page_size().unwrap();
    let owner = prepared.owner.unwrap();
    let mut extents = Vec::new();
    owner.visit_resident_extents(0, 64, &mut |p, n, pa| extents.push((p, n, pa)));
    let first_span = (2 * PAGE_SIZE).next_multiple_of(host_page) / PAGE_SIZE - 1;
    assert_eq!(
        extents,
        vec![
            (0, first_span, paddr),
            (
                first_span,
                31 - first_span,
                paddr + (first_span * PAGE_SIZE) as u64
            ),
        ]
    );
    let mut tail = Vec::new();
    owner.visit_resident_extents(29, 8, &mut |p, n, pa| tail.push((p, n, pa)));
    assert_eq!(tail, vec![(29, 2, paddr + (29 * PAGE_SIZE) as u64)]);
    drop(memory);

    let read = AccessKind {
        op: AccessOp::Load,
        vaddr: 0x10000,
        size: Some(1),
    };
    assert_eq!(
        owner
            .resolve_fault(&read, 0, 0x10000)
            .unwrap()
            .paddr_page_base,
        paddr
    );
    assert_eq!(unsafe { (alias as *const u8).read_volatile() }, 7);
}

fn assert_invalidation_prevents_initial_fault() {
    let memory = SharedMemory::new(PAGE_SIZE, 3).unwrap();
    let prepared = memory
        .prepare_mapping(MappingRequest {
            offset: 0,
            length: PAGE_SIZE,
            is_shared: true,
            permissions: 3,
            private_demand: false,
            allow_shorter: false,
        })
        .unwrap();
    let task = scarlet::task::new_user_task("shared-invalidation".into(), 0);
    task.init();
    let base = 0x20000;
    let paddr = prepared.info.unwrap().paddr;
    let permissions = 3 | VirtualMemoryPermission::User as usize;
    task.vm_manager
        .add_memory_map(VirtualMemoryMap::new(
            PhysicalMemoryArea::new(paddr, paddr + PAGE_SIZE as u64 - 1),
            MemoryArea::new(base, base + PAGE_SIZE - 1),
            permissions,
            true,
            prepared.owner,
        ))
        .unwrap();
    assert_eq!(task.vm_manager.translate_to_phys(base), Some(paddr));

    memory.invalidate();
    let access = AccessKind {
        op: AccessOp::Load,
        vaddr: base,
        size: Some(1),
    };
    assert!(task.vm_manager.lazy_map_page_with(access).is_err());
    assert_eq!(task.vm_manager.translate_to_phys(base), None);
    assert_eq!(task.vm_manager.translate_user_to_kva_for_read(base), None);
    assert_eq!(task.vm_manager.translate_to_kva_for_write(base), None);
}

fn assert_sequential_writes_stop_at_mapping_end() {
    use scarlet::object::capability::memory_mapping::anon_owner::AnonymousZeroPageOwner;
    use std::sync::Arc;

    let task = scarlet::task::new_user_task("sequential-writes".into(), 0);
    task.init();
    let vm = &task.vm_manager;
    let base = 0x0100_0000;
    let granule = scarlet_hosted_runtime::ram::host_page_size().unwrap();
    let end = base + 5 * granule;
    vm.add_memory_map(VirtualMemoryMap::new(
        PhysicalMemoryArea::new(0, 0),
        MemoryArea::new(base, end - 1),
        VirtualMemoryPermission::User as usize | 3,
        false,
        Some(Arc::new(AnonymousZeroPageOwner::new())),
    ))
    .unwrap();
    for (index, expected_granules) in [(0, 1), (1, 2), (3, 2)] {
        let address = base + index * granule;
        vm.lazy_map_page_with(AccessKind {
            op: AccessOp::Store,
            vaddr: address,
            size: Some(1),
        })
        .unwrap();
        let resident = vm.search_memory_map(address).unwrap();
        assert!(resident.owner.is_none());
        assert_eq!(resident.vmarea.start, address);
        assert_eq!(resident.vmarea.size(), expected_granules * granule);
    }
    assert!(vm.search_memory_map(end).is_none());
}
