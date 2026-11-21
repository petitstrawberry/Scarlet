//! Real VFS demand backing, without a worker, guest, or window.
use scarlet::{
    environment::PAGE_SIZE,
    fs::{FileType, VfsManager},
    object::capability::memory_mapping::{AccessKind, AccessOp, MappingRequest},
    vm::phys_to_virt,
};

#[test]
fn private_file_groups_follow_virtual_host_pages() {
    let _platform = scarlet::hosted::prepare(4, 64 * 1024 * 1024).unwrap();
    let granule = scarlet_hosted_runtime::ram::host_page_size().unwrap();
    let n = granule / PAGE_SIZE;
    assert!(n > 1, "This regression exercises sub-host-page VMAs");
    let vfs = VfsManager::new();
    vfs.create_file("/image", FileType::RegularFile).unwrap();
    let file = vfs.open("/image", 2).unwrap();
    let data: Vec<u8> = (0..32 * PAGE_SIZE - 139)
        .map(|i| (i / 127 % 251) as u8)
        .collect();
    assert_eq!(file.as_stream().unwrap().write(&data).unwrap(), data.len());

    for phase in 0..n {
        // Exercise both congruent and noncongruent file/virtual offsets. The
        // copied private backing must follow the VA, not the source file.
        for offset in [phase * PAGE_SIZE, (n + 1) * PAGE_SIZE, 20 * PAGE_SIZE] {
            let start = 0x100000 + phase * PAGE_SIZE;
            let length = 3 * granule;
            let prepared = file
                .as_memory_mappable()
                .unwrap()
                .prepare_mapping(MappingRequest {
                    offset,
                    length,
                    is_shared: false,
                    permissions: 1,
                    private_demand: true,
                    allow_shorter: false,
                })
                .unwrap();
            assert!(prepared.info.is_none());
            let owner = prepared.owner.unwrap();
            let mut pages = Vec::new();
            for page in 0..length / PAGE_SIZE {
                let address = start + page * PAGE_SIZE;
                let resolved = owner
                    .resolve_fault(
                        &AccessKind {
                            op: AccessOp::Load,
                            vaddr: address,
                            size: Some(1),
                        },
                        page,
                        start,
                    )
                    .unwrap();
                let alias = phys_to_virt(resolved.paddr_page_base);
                // SAFETY: The live owner retains this resolved immutable page.
                let bytes = unsafe { core::slice::from_raw_parts(alias as *const u8, PAGE_SIZE) };
                let source = offset + page * PAGE_SIZE;
                let initialized = PAGE_SIZE.min(data.len().saturating_sub(source));
                assert_eq!(&bytes[..initialized], &data[source..source + initialized]);
                assert!(bytes[initialized..].iter().all(|&byte| byte == 0));
                pages.push(resolved.paddr_page_base);
            }
            let first_full = start.next_multiple_of(granule);
            for address in (first_full..(start + length) / granule * granule).step_by(granule) {
                let page = (address - start) / PAGE_SIZE;
                assert_eq!(
                    pages[page] % granule as u64,
                    0,
                    "phase={phase}, offset={offset:#x}: full guest host page has misaligned backing"
                );
                assert_eq!(owner.stable_physical_extent(page, n), Some(pages[page]));
                for delta in 0..n {
                    assert_eq!(
                        pages[page + delta],
                        pages[page] + (delta * PAGE_SIZE) as u64
                    );
                }
            }
            let mut extents = Vec::new();
            owner.visit_resident_extents(1, length / PAGE_SIZE - 2, &mut |page, count, pa| {
                extents.push((page, count, pa))
            });
            let mut visited = Vec::new();
            for (page, count, pa) in extents {
                for delta in 0..count {
                    assert_eq!(pa + (delta * PAGE_SIZE) as u64, pages[page + delta]);
                    visited.push(page + delta);
                }
            }
            assert_eq!(visited, (1..length / PAGE_SIZE - 1).collect::<Vec<_>>());

            // Sharing an owner after fork/relocation must never change a
            // previously resolved backing page or its contents.
            let moved = start + PAGE_SIZE;
            let repeated = owner
                .resolve_fault(
                    &AccessKind {
                        op: AccessOp::Load,
                        vaddr: moved,
                        size: Some(1),
                    },
                    0,
                    moved,
                )
                .unwrap();
            assert_eq!(repeated.paddr_page_base, pages[0]);
            assert!(
                owner
                    .stable_physical_extent(length / PAGE_SIZE, 1)
                    .is_none()
            );
        }
    }

    // Ordinary anonymous BSS already materializes a congruent private host
    // page on its first read. Keep this behavior and the exact VMA boundaries.
    use scarlet::{
        object::capability::memory_mapping::anon_owner::AnonymousZeroPageOwner,
        vm::vmem::{MemoryArea, PhysicalMemoryArea, VirtualMemoryMap},
    };
    use std::sync::Arc;
    let task = scarlet::task::new_user_task("hosted-bss-congruence".into(), 0);
    task.init();
    let start = 0x200000 + PAGE_SIZE;
    let length = 3 * granule;
    task.vm_manager
        .add_memory_map(VirtualMemoryMap::new(
            PhysicalMemoryArea::new(0, 0),
            MemoryArea::new(start, start + length - 1),
            3 | 8,
            false,
            Some(Arc::new(AnonymousZeroPageOwner::new())),
        ))
        .unwrap();
    let address = start.next_multiple_of(granule);
    task.vm_manager
        .lazy_map_page_with(AccessKind {
            op: AccessOp::Load,
            vaddr: address,
            size: Some(1),
        })
        .unwrap();
    let mapped = task.vm_manager.search_memory_map(address).unwrap();
    assert!(mapped.owner.is_none());
    assert_eq!(mapped.vmarea.start, address);
    assert_eq!(mapped.vmarea.size(), granule);
    assert_eq!(mapped.pmarea.start % granule as u64, 0);
    let alias = phys_to_virt(mapped.pmarea.start);
    // SAFETY: The test task owns this newly materialized BSS host page.
    let bytes = unsafe { core::slice::from_raw_parts(alias as *const u8, granule) };
    assert!(bytes.iter().all(|&byte| byte == 0));
    assert!(task.vm_manager.search_memory_map(start - 1).is_none());
    assert!(task.vm_manager.search_memory_map(start + length).is_none());
}
