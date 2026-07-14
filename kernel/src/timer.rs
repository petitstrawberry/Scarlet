//! Kernel timer module.
//!
//! This module provides the kernel timer functionality, which is responsible for
//! managing the system timer and scheduling tasks based on time intervals.
//!

use crate::arch::Trapframe;
use crate::arch::timer::ArchTimer;
use crate::environment::MAX_NUM_CPUS;
use crate::sched::scheduler::sched_on_tick;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
extern crate alloc;
use alloc::collections::BinaryHeap;
use alloc::sync::{Arc, Weak};
use core::cmp::Ordering as CmpOrdering;

pub struct KernelTimer {
    // SAFETY: Each CPU only accesses its own timer via cpu_id index.
    // UnsafeCell allows per-CPU mutable access without data races.
    core_local_timer: [UnsafeCell<ArchTimer>; MAX_NUM_CPUS],
    pub interval: u64,
}

// SAFETY: KernelTimer is thread-safe because each CPU only accesses its own timer.
// The ArchTimer instances are per-CPU, and the hardware registers are CPU-local.
unsafe impl Sync for KernelTimer {}

static KERNEL_TIMER: spin::Once<KernelTimer> = spin::Once::new();

pub fn get_kernel_timer() -> &'static KernelTimer {
    KERNEL_TIMER.call_once(|| KernelTimer::new())
}

impl KernelTimer {
    fn new() -> Self {
        KernelTimer {
            core_local_timer: core::array::from_fn(|_| UnsafeCell::new(ArchTimer::new())),
            interval: 0xffffffff_ffffffff,
        }
    }

    /// Initialize the timer for a specific CPU.
    /// This must be called by each CPU individually during its initialization.
    ///
    /// # Arguments
    /// * `cpu_id` - The ID of the CPU whose timer should be initialized
    pub fn init(&self, cpu_id: usize) {
        // SAFETY: Only the specified CPU's timer is accessed, maintaining
        // the per-CPU access invariant.
        unsafe { (*self.core_local_timer[cpu_id].get()).stop() };
    }

    pub fn start(&self, cpu_id: usize) {
        // SAFETY: Each CPU only accesses its own timer
        unsafe { (*self.core_local_timer[cpu_id].get()).start() };
    }

    pub fn stop(&self, cpu_id: usize) {
        // SAFETY: Each CPU only accesses its own timer
        unsafe { (*self.core_local_timer[cpu_id].get()).stop() };
    }

    pub fn restart(&self, cpu_id: usize) {
        self.stop(cpu_id);
        self.start(cpu_id);
    }

    /* Set the interval in microseconds */
    pub fn set_interval_us(&self, cpu_id: usize, interval: u64) {
        // SAFETY: Each CPU only accesses its own timer
        unsafe { (*self.core_local_timer[cpu_id].get()).set_interval_us(interval) };
    }

    pub fn get_time_us(&self, cpu_id: usize) -> u64 {
        // SAFETY: Each CPU only accesses its own timer
        unsafe { (*self.core_local_timer[cpu_id].get()).get_time_us() }
    }
}

// Last architected-counter tick processed by the global timekeeper.
static TICK_COUNT: AtomicU64 = AtomicU64::new(0);
static TIMER_IRQ_COUNTS: [AtomicU64; MAX_NUM_CPUS] = [const { AtomicU64::new(0) }; MAX_NUM_CPUS];

#[inline]
fn current_counter_tick() -> u64 {
    get_time_us() / TICK_INTERVAL_US
}

/// The CPU ID designated as the global timekeeper.
/// Only this CPU advances TICK_COUNT and fires software timers.
/// Initialized to u64::MAX (unset); call set_global_timekeeper() during boot.
static GLOBAL_TIMEKEEPER_CPU: AtomicU64 = AtomicU64::new(u64::MAX);

/// Designate a CPU as the global timekeeper.
/// Must be called once during boot (from start_kernel) before any timer interrupts fire.
pub fn set_global_timekeeper(cpu_id: usize) {
    GLOBAL_TIMEKEEPER_CPU.store(cpu_id as u64, Ordering::Relaxed);
}

/// Timer interrupt handler. Called on every CPU local timer interrupt.
///
/// - Re-arms the per-CPU local timer (all CPUs).
/// - Advances software timers from the architected counter only on the
///   designated global timekeeper CPU. Delayed/coalesced timer IRQs therefore
///   cannot make software time run slow.
/// - Always calls sched_on_tick() for per-CPU scheduler accounting.
pub fn tick(trapframe: &mut Trapframe) {
    tick_with_scheduler(trapframe, true);
}

/// Timer interrupt handler with optional scheduler accounting.
///
/// Kernel-mode traps on a non-idle user task must not preempt via the normal
/// user-task scheduler path: the trapframe describes an in-kernel continuation,
/// not the task's user context.  Such paths still need timer re-arming and
/// global timer processing, but scheduler accounting is deferred until the task
/// returns to user mode or blocks explicitly.
pub fn tick_with_scheduler(trapframe: &mut Trapframe, run_scheduler: bool) {
    let cpu_id = crate::arch::get_cpu().get_cpuid();
    let irq_count = TIMER_IRQ_COUNTS[cpu_id].fetch_add(1, Ordering::Relaxed) + 1;
    let heartbeat_ticks = 5_000_000 / TICK_INTERVAL_US;
    if irq_count <= 3 || irq_count % heartbeat_ticks == 0 {
        crate::early_println!(
            "[timer] irq heartbeat cpu={} count={} scheduler={}",
            cpu_id,
            irq_count,
            run_scheduler
        );
    }
    let timer = get_kernel_timer();
    timer.set_interval_us(cpu_id, TICK_INTERVAL_US);
    timer.start(cpu_id);

    // Only the designated global timekeeper advances global time and fires
    // software timers. All other CPUs skip this block entirely.
    if cpu_id as u64 == GLOBAL_TIMEKEEPER_CPU.load(Ordering::Relaxed) {
        let now = current_counter_tick();
        TICK_COUNT.store(now, Ordering::Relaxed);
        check_software_timers(now);
    }

    if run_scheduler {
        // Per-CPU scheduler accounting runs on every CPU when preemption is safe.
        sched_on_tick(cpu_id, trapframe);
    }
}

/// Get the current tick count (monotonic, since boot)
pub fn get_tick() -> u64 {
    current_counter_tick()
}

pub fn get_time_ns() -> u64 {
    let cpu_id = crate::arch::get_cpu().get_cpuid();
    let timer = get_kernel_timer();
    timer.get_time_us(cpu_id) * 1_000
}

pub fn get_time_us() -> u64 {
    let cpu_id = crate::arch::get_cpu().get_cpuid();
    let timer = get_kernel_timer();
    timer.get_time_us(cpu_id)
}

/// Trait for timer expiration callback
pub trait TimerHandler: Send + Sync {
    fn on_timer_expired(self: Arc<Self>, context: usize);
}

/// Software timer structure
pub struct SoftwareTimer {
    pub id: u64,                         // Unique timer ID
    pub expires: u64,                    // Expiration tick
    pub handler: Weak<dyn TimerHandler>, // Weak reference to callback handler
    pub context: usize,                  // User context
    pub active: bool,                    // Is this timer active?
}

// Global timer ID counter
static TIMER_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

impl PartialEq for SoftwareTimer {
    fn eq(&self, other: &Self) -> bool {
        self.expires == other.expires
            && self.context == other.context
            && self.active == other.active
    }
}

impl Eq for SoftwareTimer {}

impl Ord for SoftwareTimer {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // Reverse order for min-heap (BinaryHeap is max-heap by default)
        other.expires.cmp(&self.expires)
    }
}

impl PartialOrd for SoftwareTimer {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

use alloc::collections::BTreeMap;
use spin::{Mutex, RwLock};

// Heap-based timer list (protected by spin::Mutex)
static SOFTWARE_TIMER_HEAP: Mutex<BinaryHeap<SoftwareTimer>> = Mutex::new(BinaryHeap::new());

// Active timer flags (protected by RwLock for efficient concurrent reads)
// Maps timer ID -> active status
static TIMER_ACTIVE_FLAGS: RwLock<BTreeMap<u64, bool>> = RwLock::new(BTreeMap::new());

/// Add a new software timer. Returns timer id.
pub fn add_timer(expires: u64, handler: &Arc<dyn TimerHandler>, context: usize) -> u64 {
    let id = TIMER_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timer = SoftwareTimer {
        id,
        expires,
        handler: Arc::downgrade(handler),
        context,
        active: true,
    };

    // Mark as active in the flags map
    TIMER_ACTIVE_FLAGS.write().insert(id, true);

    SOFTWARE_TIMER_HEAP.lock().push(timer);
    id
}

/// Cancel a timer by id (O(1) operation - just marks as inactive)
pub fn cancel_timer(id: u64) {
    // Simply mark as inactive - the timer will be skipped in check_software_timers()
    // and cleaned up when it expires
    if let Some(active) = TIMER_ACTIVE_FLAGS.write().get_mut(&id) {
        *active = false;
    }
}

/// Check if a timer is active (used by check_software_timers)
#[inline]
fn is_timer_active(id: u64) -> bool {
    TIMER_ACTIVE_FLAGS.read().get(&id).copied().unwrap_or(false)
}

/// Call this from tick() to check and fire expired timers
fn check_software_timers(now: u64) {
    use alloc::vec::Vec;
    let mut expired = Vec::new();
    let mut cleanup_ids = Vec::new();

    {
        let mut heap = SOFTWARE_TIMER_HEAP.lock();
        let active_flags = TIMER_ACTIVE_FLAGS.read();

        while let Some(timer) = heap.peek() {
            if timer.expires <= now {
                let timer = heap.pop().unwrap();
                // Check if still active
                if active_flags.get(&timer.id).copied().unwrap_or(false) {
                    expired.push(timer);
                } else {
                    // Mark for cleanup (will be done outside locks)
                    cleanup_ids.push(timer.id);
                }
            } else {
                break;
            }
        }
    } // Unlock the heap

    // Clean up inactive timers (outside of read lock to avoid deadlock)
    if !cleanup_ids.is_empty() {
        let mut active_flags = TIMER_ACTIVE_FLAGS.write();
        for id in cleanup_ids {
            active_flags.remove(&id);
        }
    }

    // Execute callbacks outside of all locks
    for timer in expired {
        // Double-check active status before executing
        let should_execute = {
            let active_flags = TIMER_ACTIVE_FLAGS.read();
            active_flags.get(&timer.id).copied().unwrap_or(false)
        };

        if should_execute {
            // Clean up from flags map before executing handler
            TIMER_ACTIVE_FLAGS.write().remove(&timer.id);

            if let Some(handler) = timer.handler.upgrade() {
                handler.on_timer_expired(timer.context);
            }
        }
    }
}

// Tick interval in microseconds (e.g., 10_000 for 10ms tick)
pub const TICK_INTERVAL_US: u64 = 10_000; // 10ms tick

/// Convert milliseconds to ticks
#[inline]
pub fn ms_to_ticks(ms: u64) -> u64 {
    (ms * 1_000) / TICK_INTERVAL_US
}

/// Convert microseconds to ticks
#[inline]
pub fn us_to_ticks(us: u64) -> u64 {
    us / TICK_INTERVAL_US
}

/// Convert nanoseconds to ticks
#[inline]
pub fn ns_to_ticks(ns: u64) -> u64 {
    (ns / 1_000) / TICK_INTERVAL_US
}

/// Convert ticks to milliseconds
#[inline]
pub fn ticks_to_ms(ticks: u64) -> u64 {
    (ticks * TICK_INTERVAL_US) / 1_000
}

/// Convert ticks to microseconds
#[inline]
pub fn ticks_to_us(ticks: u64) -> u64 {
    ticks * TICK_INTERVAL_US
}

/// Convert ticks to nanoseconds
#[inline]
pub fn ticks_to_ns(ticks: u64) -> u64 {
    (ticks * TICK_INTERVAL_US) * 1_000
}

// static mut TEST_HANDLER: Option<Arc<dyn TimerHandler>> = None;

// // TEST
// fn register_test_timer() {
//     use alloc::sync::Arc;

//     struct TestHandler;
//     impl TimerHandler for TestHandler {
//         #[allow(static_mut_refs)]
//         fn on_timer_expired(&self, context: usize) {
//             crate::println!("[Software Timer] Test timer expired with context: {}", context);
//             if let Some(handler) = unsafe { TEST_HANDLER.clone() } {
//                 crate::println!("[Software Timer] Test handler is still available.");
//                 let handler = handler.clone();
//                 add_timer(get_tick() + 100, &handler, context);
//             } else {
//                 crate::println!("[Software Timer] Test handler is no longer available.");
//             }
//         }
//     }

//     let handler: Arc<dyn TimerHandler>  = Arc::new(TestHandler);
//     let target_tick = get_tick() + 100; // 100 ticks from now
//     let id = add_timer(target_tick, &handler, 42);
//     crate::println!("Test timer registered with ID: {}, tick: {}", id, target_tick);
//     unsafe {
//         TEST_HANDLER = Some(handler);
//     }
// }

// late_initcall!(register_test_timer);
