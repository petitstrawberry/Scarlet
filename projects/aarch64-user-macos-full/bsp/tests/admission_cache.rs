//! Exercise the production cache without worker/GUI startup, including real
//! shared owners whose publication changes outside an address space's VM lock.
extern crate alloc;

#[path = "../../../../kernel/src/hosted/execution/admission_cache.rs"]
mod admission_cache;

use admission_cache::{Cache, Snapshot};
use scarlet::{
    environment::PAGE_SIZE,
    ipc::shared_memory::{SharedMemory, SharedMemoryObject},
    object::capability::memory_mapping::{MappingRequest, MemoryMappingOps},
};
use std::{cell::Cell, sync::Arc};

#[test]
fn stable_admission_skips_snapshot_and_enumeration() {
    let mut cache = Cache::default();
    assert!(cache.update(0, || vec![1], |_| Some(0), |m| *m));
    assert!(!cache.update(
        0,
        || panic!("VM snapshot repeated"),
        |_| Some(0),
        |_| panic!("Stable owner enumerated")
    ));
}

#[test]
fn unknown_unchanged_owner_is_scanned_without_rebuilding_desired() {
    let mut cache = Cache::default();
    let scans = Cell::new(0);
    let value = Cell::new(7);
    let update = |cache: &mut Cache<usize, usize>| {
        cache.update(
            0,
            || vec![0, 1],
            |m| if *m == 0 { Some(0) } else { None },
            |m| {
                scans.set(scans.get() + 1);
                if *m == 0 { 42 } else { value.get() }
            },
        )
    };
    assert!(update(&mut cache));
    assert_eq!(scans.get(), 2);
    assert!(!update(&mut cache));
    assert_eq!(scans.get(), 3); // Only the unknown owner is rescanned.
    value.set(8);
    assert!(update(&mut cache));
    assert_eq!(cache.values().copied().collect::<Vec<_>>(), vec![42, 8]);
    assert!(!update(&mut cache));
}

#[test]
fn mutation_during_owner_scan_cannot_stamp_a_stale_snapshot_current() {
    let mut cache = Cache::default();
    let generation = Cell::new(0);
    assert!(cache.update(
        0,
        || vec![()],
        |_| Some(generation.get()),
        |_| {
            generation.set(1);
            10
        }
    ));
    assert!(cache.update(0, || unreachable!(), |_| Some(generation.get()), |_| 11));
    assert!(!cache.update(0, || unreachable!(), |_| Some(1), |_| unreachable!()));
}

#[test]
fn topology_generation_forces_rebuild_even_when_final_extents_match() {
    // W->X can return to the same final descriptor; VM invalidation must still
    // reach the installed republish flag. A clone starts with an empty cache.
    let mut parent = Cache::default();
    let mut child = Cache::default();
    assert!(parent.update(0, || vec![1], |_| Some(0), |m| *m));
    assert!(parent.update(1, || vec![1], |_| Some(0), |m| *m));
    assert!(child.update(1, || vec![1], |_| Some(0), |m| *m));
    assert!(parent.update(2, Vec::new, |_| Some(0), |m| *m));
    assert_eq!(parent.values().count(), 0);
    assert_eq!(child.values().copied().collect::<Vec<_>>(), vec![1]);
}

type OwnerCache = Cache<Arc<dyn MemoryMappingOps>, Vec<(usize, usize, u64)>>;

fn update_owner(cache: &mut OwnerCache, owner: &Arc<dyn MemoryMappingOps>) -> bool {
    cache.update(
        0,
        || vec![owner.clone()],
        |o| o.publication_generation(),
        |o| {
            let mut extents = Vec::new();
            o.visit_resident_extents(0, 64, &mut |p, n, pa| extents.push((p, n, pa)));
            extents
        },
    )
}

#[test]
fn shared_resize_and_revoke_invalidate_both_address_space_caches() {
    let _platform = scarlet::hosted::prepare(4, 64 * 1024 * 1024).unwrap();
    private_fault_and_fork_keep_copy_authority();
    let memory = SharedMemory::new(2 * PAGE_SIZE, 3).unwrap();
    let view = || {
        memory
            .prepare_mapping(MappingRequest {
                offset: PAGE_SIZE,
                length: PAGE_SIZE,
                is_shared: true,
                permissions: 3,
                private_demand: false,
                allow_shorter: false,
            })
            .unwrap()
            .owner
            .unwrap()
    };
    let a = view();
    let b = view();
    let mut first = OwnerCache::default();
    let mut second = OwnerCache::default();
    for (cache, owner) in [(&mut first, &a), (&mut second, &b)] {
        assert!(update_owner(cache, owner));
        assert!(!update_owner(cache, owner));
        assert_eq!(cache.values().next().unwrap()[0].1, 1);
    }
    let initial = a.publication_generation();
    memory.pin_range(0, PAGE_SIZE).unwrap();
    assert!(memory.resize(32 * PAGE_SIZE).is_err());
    assert_eq!(initial, a.publication_generation());
    memory.unpin_range();
    for size in [32, 2, 32] {
        let before = a.publication_generation();
        memory.resize(size * PAGE_SIZE).unwrap();
        assert_ne!(before, a.publication_generation());
        assert_eq!(a.publication_generation(), b.publication_generation());
        for (cache, owner) in [(&mut first, &a), (&mut second, &b)] {
            assert!(update_owner(cache, owner));
            assert!(!update_owner(cache, owner));
            assert_eq!(
                cache
                    .values()
                    .next()
                    .unwrap()
                    .iter()
                    .map(|(_, n, _)| n)
                    .sum::<usize>(),
                size - 1
            );
        }
    }
    memory.invalidate();
    for (cache, owner) in [(&mut first, &a), (&mut second, &b)] {
        assert!(update_owner(cache, owner));
        assert!(cache.values().next().unwrap().is_empty());
        assert!(!update_owner(cache, owner));
    }
}

fn private_fault_and_fork_keep_copy_authority() {
    use scarlet::{
        object::capability::memory_mapping::{
            AccessKind, AccessOp, anon_owner::AnonymousZeroPageOwner,
        },
        task::{self, CloneFlags, CloneFlagsDef},
        vm::vmem::{MemoryArea, PhysicalMemoryArea, VirtualMemoryMap},
    };
    const BASE: usize = 0x400_0000;
    let parent = task::new_user_task("admission-cow".into(), 0);
    parent.init();
    parent
        .vm_manager
        .add_memory_map(VirtualMemoryMap::new(
            PhysicalMemoryArea::new(0, 0),
            MemoryArea::new(BASE, BASE + 16383),
            3 | 8,
            false,
            Some(Arc::new(AnonymousZeroPageOwner::new())),
        ))
        .unwrap();
    let before = parent.vm_manager.translate_to_kva_for_write(BASE).unwrap();
    unsafe {
        (before as *mut u8).write(17);
    }
    let child = parent.clone_task(CloneFlags::default()).unwrap();
    let cow = parent.vm_manager.get_memory_map_by_addr(BASE).unwrap();
    let owner = cow.owner.as_ref().expect("Fork installs parent COW owner");
    assert_eq!(owner.publication_generation(), Some(0));
    let store = AccessKind {
        op: AccessOp::Store,
        vaddr: BASE,
        size: Some(1),
    };
    assert!(owner.private_fault_requires_copy(&store));
    assert!(!owner.private_fault_requires_copy(&AccessKind {
        op: AccessOp::Load,
        ..store
    }));
    let child_alias = child.vm_manager.translate_to_kva_for_write(BASE).unwrap();
    assert_ne!(child_alias, before);
    assert_eq!(unsafe { (child_alias as *const u8).read() }, 17);
    unsafe {
        (child_alias as *mut u8).write(29);
    }
    assert_eq!(unsafe { (before as *const u8).read() }, 17);
    let mut flags = CloneFlags::default();
    flags.set(CloneFlagsDef::Vm);
    let sibling = child.clone_task(flags).unwrap();
    assert_eq!(
        sibling.vm_manager.translate_to_kva_for_write(BASE),
        Some(child_alias)
    );
    sibling.vm_manager.remove_memory_map_range(BASE, 16384);
    assert!(child.vm_manager.get_memory_map_by_addr(BASE).is_none());
    assert!(parent.vm_manager.get_memory_map_by_addr(BASE).is_some());
}

impl Snapshot for usize {
    fn key(&self) -> usize {
        *self
    }
    fn same(&self, other: &Self) -> bool {
        self == other
    }
}
impl Snapshot for i32 {
    fn key(&self) -> usize {
        *self as usize
    }
    fn same(&self, other: &Self) -> bool {
        self == other
    }
}
impl Snapshot for () {
    fn key(&self) -> usize {
        0
    }
    fn same(&self, _: &Self) -> bool {
        true
    }
}
impl Snapshot for Arc<dyn MemoryMappingOps> {
    fn key(&self) -> usize {
        Arc::as_ptr(self) as *const () as usize
    }
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(self, other)
    }
}

#[test]
fn cow_split_preserves_unchanged_owners_across_generation_churn() {
    #[derive(Clone)]
    struct Map(usize, usize);
    impl Snapshot for Map {
        fn key(&self) -> usize {
            self.0
        }
        fn same(&self, other: &Self) -> bool {
            self.0 == other.0 && self.1 == other.1
        }
    }
    let scans = Cell::new(0);
    let mut cache = Cache::default();
    let build = |m: &Map| {
        scans.set(scans.get() + 1);
        m.1
    };
    assert!(cache.update(
        0,
        || (0..1000).map(|i| Map(i * 2, 0)).collect(),
        |_| Some(0),
        build
    ));
    assert_eq!(scans.replace(0), 1000);
    // A COW split changes one descriptor and inserts its resident sibling.
    assert!(cache.update(
        1,
        || {
            let mut maps: Vec<_> = (0..1000).map(|i| Map(i * 2, usize::from(i == 1))).collect();
            maps.insert(2, Map(3, 1));
            maps
        },
        |_| Some(0),
        build
    ));
    assert_eq!(scans.replace(0), 2);
    // Removing that sibling must not discard the remaining stable entries.
    assert!(cache.update(
        2,
        || (0..1000).map(|i| Map(i * 2, usize::from(i == 1))).collect(),
        |_| Some(0),
        build
    ));
    assert_eq!(scans.get(), 0);
    assert_eq!(cache.values().count(), 1000);
}

#[test]
fn fixed_descriptors_leave_the_admission_scan_until_replaced() {
    struct Map(usize, usize);
    impl Snapshot for Map {
        fn key(&self) -> usize { self.0 }
        fn same(&self, other: &Self) -> bool { self.1 == other.1 }
        fn needs_owner_check(&self) -> bool { self.0 == 1 }
    }
    let mut cache = Cache::default();
    let owner = Cell::new(0);
    let version = |m: &Map| {
        assert_eq!(m.0, 1);
        Some(owner.get())
    };
    let build = |m: &Map| if m.0 == 1 { owner.get() } else { m.1 as u64 };
    assert!(cache.update(0, || vec![Map(0, 4), Map(1, 0)], version, build));
    assert!(!cache.update(0, || unreachable!(), version, |_| unreachable!()));
    owner.set(3);
    assert!(cache.update(0, || unreachable!(), version, build));
    assert!(cache.update(1, || vec![Map(0, 7), Map(1, 0)], version, build));
    assert_eq!(cache.values().copied().collect::<Vec<_>>(), [7, 3]);
}
