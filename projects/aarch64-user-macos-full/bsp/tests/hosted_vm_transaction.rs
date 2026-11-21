use scarlet::{
    environment::PAGE_SIZE,
    hosted,
    interrupt::InterruptManager,
    library::std::usercopy::{UserCopyError, copy_from_user, copy_to_user},
    mem::page::ContiguousPages,
    object::capability::memory_mapping::{
        AccessKind, MemoryMappingInfo, MemoryMappingOps, ResolveFaultError, ResolveFaultResult,
        anon_owner::AnonymousZeroPageOwner,
    },
    sched::scheduler,
    task::{self, BlockedType, Task, TaskState},
    timer::get_kernel_timer,
    vm::vmem::{MemoryArea, PhysicalMemoryArea, VirtualMemoryMap},
};
use std::{
    env,
    process::{self, Command},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const ADDRESS: usize = 0x40000;
static TARGET: OnceLock<Task> = OnceLock::new();
static MUTATOR: AtomicUsize = AtomicUsize::new(0);
static RESOLVING_ALIAS: AtomicBool = AtomicBool::new(false);
static REMOVED: AtomicBool = AtomicBool::new(false);

struct TestBacking(ContiguousPages);

impl MemoryMappingOps for TestBacking {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Test mapping is demand resolved")
    }

    fn supports_mmap(&self) -> bool {
        true
    }

    fn resolve_fault(
        &self,
        _: &AccessKind,
        _: usize,
        _: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        RESOLVING_ALIAS.store(true, Ordering::Release);
        let mutator = scheduler::get_task_by_id(MUTATOR.load(Ordering::Acquire)).unwrap();
        spin_until(|| {
            assert!(
                !REMOVED.load(Ordering::Acquire),
                "unmap ran during alias resolution"
            );
            mutator.get_state() == TaskState::Blocked(BlockedType::Uninterruptible)
        });
        Ok(ResolveFaultResult {
            paddr_page_base: self.0.as_paddr(),
            is_tail: false,
        })
    }
}

#[test]
fn usercopy_excludes_unmap_through_alias_resolution_and_memcpy() {
    const CHILD: &str = "SCARLET_VM_TRANSACTION_TEST_CHILD";
    if env::var_os(CHILD).is_some() {
        run_kernel();
    }
    let status = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "usercopy_excludes_unmap_through_alias_resolution_and_memcpy",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .status()
        .unwrap();
    assert!(
        status.success(),
        "VM transaction child exited with {status}"
    );
}

fn run_kernel() -> ! {
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(10));
        eprintln!("VM transaction test did not complete");
        process::exit(1);
    });
    let mut platform = hosted::prepare(2, 256 * 1024 * 1024).unwrap();
    let interrupts = InterruptManager::global();
    interrupts.init_controllers();
    interrupts.init_controllers_for_cpu(0);
    scarlet::interrupt::enable_cpu_interrupts();
    get_kernel_timer().init(0);

    let pages = ContiguousPages::new(1).unwrap();
    unsafe { (pages.as_ptr() as *mut u8).write(0x57) };
    let target = task::new_user_task("usercopy-owner".into(), 0);
    target.init();
    target
        .vm_manager
        .add_memory_map(VirtualMemoryMap::new(
            PhysicalMemoryArea::new(0, 0),
            MemoryArea::new(ADDRESS, ADDRESS + PAGE_SIZE - 1),
            9,
            true,
            Some(Arc::new(TestBacking(pages))),
        ))
        .unwrap();
    assert!(TARGET.set(target).is_ok());

    scheduler::register_online_cpu(0);
    scheduler::spawn_idle_task(0);
    let reader = register_worker("usercopy-reader", 0, reader);
    scheduler::enqueue_task(reader, 0);
    assert_eq!(scheduler::start_scheduler(), Some(reader));
    platform.start_secondary_cpus().unwrap();
    scheduler::first_switch_to_kernel_task(reader);
}

fn register_worker(name: &str, cpu: usize, entry: fn()) -> usize {
    let worker = task::new_kernel_task(name.into(), 0, entry);
    worker.set_pinned_cpu(Some(cpu));
    worker.init();
    scheduler::register_task(worker)
}

fn reader() {
    let mutator = register_worker("usercopy-unmap", 1, unmap);
    MUTATOR.store(mutator, Ordering::Release);
    scheduler::enqueue_task(mutator, 1);
    let target = TARGET.get().unwrap();
    let mut result = [0];
    copy_from_user(target, ADDRESS, &mut result).unwrap();
    assert_eq!(result, [0x57]);
    spin_until(|| REMOVED.load(Ordering::Acquire));
    assert_eq!(
        copy_from_user(target, ADDRESS, &mut result),
        Err(UserCopyError::TranslationError)
    );

    target
        .vm_manager
        .add_memory_map(VirtualMemoryMap::new(
            PhysicalMemoryArea::new(0, 0),
            MemoryArea::new(ADDRESS, ADDRESS + PAGE_SIZE - 1),
            11,
            false,
            Some(Arc::new(AnonymousZeroPageOwner::new())),
        ))
        .unwrap();
    copy_to_user(target, ADDRESS, &[0x83]).unwrap();
    copy_from_user(target, ADDRESS, &mut result).unwrap();
    assert_eq!(result, [0x83]);
    process::exit(0);
}

fn unmap() {
    spin_until(|| RESOLVING_ALIAS.load(Ordering::Acquire));
    let other = task::new_user_task("independent-vm".into(), 0);
    other.allocate_data_pages(ADDRESS, 1).unwrap();
    copy_to_user(&other, ADDRESS, &[0xaa]).unwrap();
    let vm = TARGET.get().unwrap().vm_manager.clone();
    assert_eq!(vm.remove_memory_map_range(ADDRESS, PAGE_SIZE).len(), 1);
    REMOVED.store(true, Ordering::Release);
}

fn spin_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !ready() {
        assert!(Instant::now() < deadline, "VM transaction test stalled");
        core::hint::spin_loop();
    }
}
