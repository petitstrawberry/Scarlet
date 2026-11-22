//! Guest permission checks must not inherit the host's 16 KiB granule.
//! This exercises the VM fault authority, independently of worker execution.
use scarlet::{
    hosted,
    mem::page::ContiguousPages,
    object::capability::memory_mapping::{AccessKind, AccessOp},
    task,
    vm::vmem::{MemoryArea, PhysicalMemoryArea, VirtualMemoryMap},
};

#[test]
fn read_write_execute_and_hole_rotate_through_every_guest_quarter() {
    const QUARTER: usize = 4096;
    const BASE: usize = 0x400_0000;
    let _platform = hosted::prepare(4, 32 * 1024 * 1024).unwrap();
    assert_eq!(
        scarlet_hosted_runtime::ram::host_page_size().unwrap(),
        4 * QUARTER
    );
    // Include write-only, read/write, execute-only and read/execute. Rotate
    // each layout with an unmapped quarter so no quarter
    // accidentally benefits from being the first/last fragment in a host page.
    for permissions in [[1usize, 2, 4, 0], [1, 3, 4, 0], [1, 3, 5, 0]] {
        for rotation in 0..4 {
            let pages = ContiguousPages::new_aligned(4, 4 * QUARTER).unwrap();
            let target = task::new_user_task(format!("quarter-permissions-{rotation}"), 0);
            target.init();
            for quarter in 0..4 {
                let perm = permissions[(quarter + rotation) % 4];
                if perm == 0 {
                    continue;
                }
                let va = BASE + quarter * QUARTER;
                let pa = pages.as_paddr() + (quarter * QUARTER) as u64;
                target
                    .vm_manager
                    .add_memory_map(VirtualMemoryMap::new(
                        PhysicalMemoryArea::new(pa, pa + QUARTER as u64 - 1),
                        MemoryArea::new(va, va + QUARTER - 1),
                        perm | 8,
                        true,
                        None,
                    ))
                    .unwrap();
            }
            // Repeat after successful faults have populated PTEs: a cached page
            // must not turn a denied operation into an allowed one.
            for pass in 0..2 {
                for quarter in 0..4 {
                    let perm = permissions[(quarter + rotation) % 4];
                    for offset in [0, QUARTER - 4] {
                        for (op, bit) in [
                            (AccessOp::Load, 1),
                            (AccessOp::Store, 2),
                            (AccessOp::Instruction, 4),
                        ] {
                            let result = target.vm_manager.lazy_map_page_with(AccessKind {
                                op,
                                vaddr: BASE + quarter * QUARTER + offset,
                                size: Some(4),
                            });
                            assert_eq!(
                                result.is_ok(),
                                perm & bit != 0,
                                "rotation={rotation} quarter={quarter} offset={offset} pass={pass} op={op:?}: {result:?}"
                            );
                        }
                    }
                }
            }
            // Drop the VM before freeing its externally retained physical pages.
            drop(target);
            drop(pages);
        }
    }
}
