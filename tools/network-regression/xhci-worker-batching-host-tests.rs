//! Actual worker loop; controller, scheduler, IRQ lock and latched wait are models.
#![allow(dead_code)]
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
static COUNT: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if COUNT.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;
struct Registry(Mutex<Vec<Weak<XhciController>>>);
impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Weak<XhciController>>> {
        self.0.lock().unwrap()
    }
    fn try_lock(
        &self,
    ) -> std::sync::TryLockResult<std::sync::MutexGuard<'_, Vec<Weak<XhciController>>>> {
        self.0.try_lock()
    }
}
static CONTROLLERS: LazyLock<Registry> = LazyLock::new(|| Registry(Mutex::new(Vec::new())));
fn xhci_worker_controllers() -> &'static Registry {
    &CONTROLLERS
}
struct XhciController {
    work: AtomicUsize,
    calls: AtomicUsize,
}
impl XhciController {
    fn process_deferred_interrupt_work(&self) -> bool {
        assert!(
            CONTROLLERS.try_lock().is_ok(),
            "controller invoked under registry lock"
        );
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.work
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1))
            .is_ok()
    }
}
static SCHEDULES: AtomicUsize = AtomicUsize::new(0);
static WAITS: AtomicUsize = AtomicUsize::new(0);
static STOP_AT_YIELD: AtomicBool = AtomicBool::new(false);
static INJECT_WAIT: AtomicBool = AtomicBool::new(false);
fn boundary() {
    let registry = CONTROLLERS
        .try_lock()
        .expect("registry locked at schedule/wait");
    for weak in registry.iter() {
        assert_eq!(
            weak.strong_count(),
            1,
            "worker kept strong reference across wait/yield"
        );
    }
}
fn stop() -> ! {
    COUNT.store(false, Ordering::Relaxed);
    panic!("bounded worker test complete")
}
mod task {
    pub struct Task;
    impl Task {
        pub fn get_id(&self) -> usize {
            1
        }
        pub fn get_trapframe(&self) -> usize {
            0
        }
    }
    pub fn mytask() -> Option<Task> {
        Some(Task)
    }
}
mod arch {
    pub mod instruction {
        pub fn idle() -> ! {
            panic!("missing test task")
        }
    }
}
mod sched {
    pub mod scheduler {
        pub fn schedule(_: usize) {
            crate::boundary();
            crate::SCHEDULES.fetch_add(1, crate::Ordering::Relaxed);
            if crate::STOP_AT_YIELD.load(crate::Ordering::Relaxed) {
                crate::stop();
            }
        }
    }
}
struct Waiter;
static XHCI_WORKER_WAKER: Waiter = Waiter;
impl Waiter {
    fn wait(&self, _: usize, _: usize) {
        boundary();
        WAITS.fetch_add(1, Ordering::Relaxed);
        if INJECT_WAIT.swap(false, Ordering::Relaxed) {
            // Model the existing latched wake arriving between the idle check
            // and wait. The production Waker itself is not under test here.
            CONTROLLERS.lock()[0]
                .upgrade()
                .unwrap()
                .work
                .store(1, Ordering::Relaxed);
            return;
        }
        stop();
    }
}
/* PRODUCTION_WORKER */
fn run(work: usize, stop_yield: bool, inject: bool) -> (usize, usize, usize, usize) {
    let c = Arc::new(XhciController {
        work: AtomicUsize::new(work),
        calls: AtomicUsize::new(0),
    });
    *CONTROLLERS.lock() = vec![Arc::downgrade(&c)];
    SCHEDULES.store(0, Ordering::Relaxed);
    WAITS.store(0, Ordering::Relaxed);
    STOP_AT_YIELD.store(stop_yield, Ordering::Relaxed);
    INJECT_WAIT.store(inject, Ordering::Relaxed);
    ALLOCS.store(0, Ordering::Relaxed);
    COUNT.store(true, Ordering::Relaxed);
    assert!(std::panic::catch_unwind(xhci_worker_entry).is_err());
    COUNT.store(false, Ordering::Relaxed);
    let result = (
        c.calls.load(Ordering::Relaxed),
        SCHEDULES.load(Ordering::Relaxed),
        WAITS.load(Ordering::Relaxed),
        ALLOCS.load(Ordering::Relaxed),
    );
    CONTROLLERS.lock().clear();
    result
}
#[test]
fn idle_waits_without_yield_or_polling() {
    assert_eq!(run(0, false, false), (1, 0, 1, 1));
}
#[test]
fn partial_burst_drains_then_sleeps_without_forced_yield() {
    assert_eq!(run(3, false, false), (4, 0, 1, 1));
}
#[test]
fn continuous_work_yields_at_bounded_four_passes() {
    assert_eq!(run(100, true, false), (4, 1, 0, 1));
}
#[test]
fn repeated_bursts_reuse_controller_storage() {
    assert_eq!(run(12, false, false), (13, 3, 1, 1));
}
#[test]
fn work_arriving_at_wait_is_serviced_and_budget_resets() {
    assert_eq!(run(3, false, true), (6, 0, 2, 1));
}
