//! Native RW eligibility and weak (partial-page) isolation for anonymous/COW
//! mappings at all four 4 KiB origins. No guest or GUI is started.
use scarlet::{
    environment::PAGE_SIZE,
    object::capability::memory_mapping::{
        AccessKind, AccessOp, MemoryMappingOps,
        anon_owner::{AnonymousPageOwner, AnonymousZeroPageOwner},
    },
    task::{self, CloneFlags},
    vm::vmem::{MemoryArea, PhysicalMemoryArea, VirtualMemoryMap},
};
use std::sync::Arc;

const HOST: usize = 16384;
const BASE: usize = 0x400_0000;
fn access(address: usize) -> AccessKind {
    AccessKind {
        op: AccessOp::Store,
        vaddr: address,
        size: Some(1),
    }
}

#[test]
fn anonymous_native_groups_and_cow_are_congruent_at_every_origin() {
    let _platform = scarlet::hosted::prepare(4, 64 * 1024 * 1024).unwrap();
    for quarter in 0..4 {
        let start = BASE + quarter * PAGE_SIZE;
        let native = (start + HOST - 1) & !(HOST - 1);
        for shared in [false, true] {
            let owner: Arc<dyn MemoryMappingOps> = if shared {
                Arc::new(AnonymousPageOwner::new())
            } else {
                Arc::new(AnonymousZeroPageOwner::new())
            };
            let parent = task::new_user_task("anonymous-congruence".into(), 0);
            parent.init();
            parent
                .vm_manager
                .add_memory_map(VirtualMemoryMap::new(
                    PhysicalMemoryArea::new(0, 0),
                    MemoryArea::new(start, start + 3 * HOST - 1),
                    3 | 8,
                    shared,
                    Some(owner.clone()),
                ))
                .unwrap();
            parent
                .vm_manager
                .lazy_map_page_with(access(native))
                .unwrap();
            let map = parent.vm_manager.search_memory_map(native).unwrap();
            let pa = if shared {
                owner
                    .stable_physical_extent((native - start) / PAGE_SIZE, HOST / PAGE_SIZE)
                    .unwrap()
            } else {
                map.pmarea.start + (native - map.vmarea.start) as u64
            };
            // These are the physical prerequisites for a full native RW map,
            // not merely four readable guest PTEs backed by unrelated frames.
            assert_eq!(pa % HOST as u64, 0, "quarter={quarter}, shared={shared}");
            assert!(map.vmarea.start <= native && map.vmarea.end >= native + HOST - 1);
            let alias = parent
                .vm_manager
                .translate_to_kva_for_write(native)
                .unwrap();
            unsafe {
                (alias as *mut u8).write(73);
            }
            if !shared {
                let child = parent.clone_task(CloneFlags::default()).unwrap();
                child.vm_manager.lazy_map_page_with(access(native)).unwrap();
                let copied = child.vm_manager.search_memory_map(native).unwrap();
                assert_eq!(copied.pmarea.start % HOST as u64, 0);
                assert_ne!(copied.pmarea.start, pa);
                assert_eq!(
                    unsafe { (scarlet::vm::phys_to_virt(copied.pmarea.start) as *const u8).read() },
                    73
                );
            }
        }
        untracked_fork_copy_is_congruent(start);
        weak_mapping_keeps_holes(start);
        shared_partial_release_and_clone(start);
    }
}

fn weak_mapping_keeps_holes(start: usize) {
    let target = task::new_user_task("anonymous-weak".into(), 0);
    target.init();
    target
        .vm_manager
        .add_memory_map(VirtualMemoryMap::new(
            PhysicalMemoryArea::new(0, 0),
            MemoryArea::new(start, start + PAGE_SIZE - 1),
            3 | 8,
            false,
            Some(Arc::new(AnonymousZeroPageOwner::new())),
        ))
        .unwrap();
    target.vm_manager.lazy_map_page_with(access(start)).unwrap();
    let map = target.vm_manager.search_memory_map(start).unwrap();
    assert_eq!(map.vmarea.size(), PAGE_SIZE);
    assert_eq!(map.pmarea.start as usize % HOST, start % HOST);
    assert!(
        target
            .vm_manager
            .lazy_map_page_with(access(start + PAGE_SIZE))
            .is_err()
    );
    assert!(
        target
            .vm_manager
            .lazy_map_page_with(access(start - PAGE_SIZE))
            .is_err()
    );
}

fn shared_partial_release_and_clone(start: usize) {
    let owner = AnonymousPageOwner::new();
    let native = (start + HOST - 1) & !(HOST - 1);
    let first = (native - start) / PAGE_SIZE;
    let pa = owner
        .resolve_fault(&access(native), first, start)
        .unwrap()
        .paddr_page_base;
    assert_eq!(owner.stable_physical_extent(first, 4), Some(pa));
    // A differently aligned alias must share the existing bytes, rather than
    // silently obtaining a separate allocation to satisfy native alignment.
    let alias = owner
        .resolve_fault(&access(native + PAGE_SIZE), first, start + PAGE_SIZE)
        .unwrap()
        .paddr_page_base;
    assert_eq!(alias, pa);
    assert_ne!(alias as usize % HOST, (native + PAGE_SIZE) % HOST);

    unsafe {
        (scarlet::vm::phys_to_virt(pa + PAGE_SIZE as u64) as *mut u8).write(99);
    }
    let generation = owner.publication_generation();
    owner.release_pages(first + 1, 1);
    assert_ne!(owner.publication_generation(), generation);
    assert!(owner.stable_physical_extent(first, 4).is_none());
    let mut spans = Vec::new();
    owner.visit_resident_extents(first, 4, &mut |p, n, _| spans.push((p, n)));
    assert_eq!(spans, vec![(first, 1), (first + 2, 2)]);
    let clone = owner.fork_clone().unwrap();
    assert_eq!(clone.is_page_resident(first + 1), Some(false));
    let revived = owner
        .resolve_fault(&access(native + PAGE_SIZE), first + 1, start)
        .unwrap()
        .paddr_page_base;
    assert_eq!(
        unsafe { (scarlet::vm::phys_to_virt(revived) as *const u8).read() },
        0
    );
    assert_eq!(owner.stable_physical_extent(first, 4), Some(pa));
    assert_eq!(clone.is_page_resident(first + 1), Some(false));
    owner.release_pages(first, 4);
    assert_eq!(owner.is_page_resident(first), Some(false));
}

fn untracked_fork_copy_is_congruent(start: usize) {
    let allocation = scarlet::mem::page::ContiguousPages::new(12).unwrap();
    let parent = task::new_user_task("untracked-fork-congruence".into(), 0);
    parent.init();
    let pa = allocation.as_paddr();
    unsafe {
        (allocation.as_ptr() as *mut u8).write(61);
    }
    parent
        .vm_manager
        .add_memory_map(VirtualMemoryMap::new(
            PhysicalMemoryArea::new(pa, pa + (3 * HOST) as u64 - 1),
            MemoryArea::new(start, start + 3 * HOST - 1),
            3 | 8,
            false,
            None,
        ))
        .unwrap();
    let child = parent.clone_task(CloneFlags::default()).unwrap();
    let snapshot = child.vm_manager.search_memory_map(start).unwrap();
    let owner = snapshot.owner.as_ref().unwrap();
    let first = owner
        .resolve_fault(
            &AccessKind {
                op: AccessOp::Load,
                ..access(start)
            },
            0,
            start,
        )
        .unwrap()
        .paddr_page_base;
    assert_eq!(first as usize % HOST, start % HOST);
    assert_eq!(
        unsafe { (scarlet::vm::phys_to_virt(first) as *const u8).read() },
        61
    );
    let native = (start + HOST - 1) & !(HOST - 1);
    let native_pa = owner
        .stable_physical_extent((native - start) / PAGE_SIZE, 4)
        .unwrap();
    assert_eq!(native_pa % HOST as u64, 0);
    drop(child);
    drop(parent);
    drop(allocation);
}
