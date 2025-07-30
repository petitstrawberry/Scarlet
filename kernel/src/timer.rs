//! Kernel timer module.
//! 
//! This module provides the kernel timer functionality, which is responsible for
//! managing the system timer and scheduling tasks based on time intervals.
//! 

use crate::arch::timer::ArchTimer;
use crate::environment::NUM_OF_CPUS;
use crate::sched::scheduler::get_scheduler;
use core::sync::atomic::{AtomicU64, Ordering};
extern crate alloc;
use alloc::sync::{Arc, Weak};
use alloc::collections::BinaryHeap;
use alloc::vec::Vec;
use core::cmp::Ordering as CmpOrdering;

pub struct KernelTimer {
    pub core_local_timer: [ArchTimer; NUM_OF_CPUS],
    pub interval: u64,
}

static mut KERNEL_TIMER: Option<KernelTimer> = None;

pub fn get_kernel_timer() -> &'static mut KernelTimer {
    unsafe {
        match KERNEL_TIMER {
            Some(ref mut t) => t,
            None => {
                KERNEL_TIMER = Some(KernelTimer::new());
                get_kernel_timer()
            }
        }
    }
}

impl KernelTimer {
    fn new() -> Self {
        KernelTimer {
            core_local_timer: core::array::from_fn(|_| ArchTimer::new()),
            interval: 0xffffffff_ffffffff,
        }
    }

    pub fn init(&mut self) {
        for i in 0..NUM_OF_CPUS {
            self.core_local_timer[i].stop();
        }
    }

    pub fn start(&mut self, cpu_id: usize) {
        self.core_local_timer[cpu_id].start();
    }

    pub fn stop(&mut self, cpu_id: usize) {
        self.core_local_timer[cpu_id].stop();
    }

    pub fn restart(&mut self, cpu_id: usize) {
        self.stop(cpu_id);
        self.start(cpu_id);
    }

    /* Set the interval in microseconds */
    pub fn set_interval_us(&mut self, cpu_id: usize, interval: u64) {
        self.core_local_timer[cpu_id].set_interval_us(interval);
    }

    pub fn get_time_us(&self, cpu_id: usize) -> u64 {
        self.core_local_timer[cpu_id].get_time_us()
    }
}

// Global tick counter (monotonic, incremented by timer interrupt)
static TICK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Increment the global tick counter. Call this from the timer interrupt handler.
pub fn tick() {
    let cpu_id = crate::arch::get_cpu().get_cpuid();
    let timer = get_kernel_timer();
    timer.set_interval_us(cpu_id, TICK_INTERVAL_US);
    timer.start(cpu_id);
    let now = TICK_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    check_software_timers(now);
    // Call scheduler tick handler to manage time slices
    let scheduler = get_scheduler();
    // crate::println!("[timer] Tick: {}, CPU: {}", now, cpu_id);
    scheduler.on_tick(cpu_id);
}

/// Get the current tick count (monotonic, since boot)
pub fn get_tick() -> u64 {
    TICK_COUNT.load(Ordering::Relaxed)
}

/// Trait for timer expiration callback
pub trait TimerHandler: Send + Sync {
    fn on_timer_expired(self: Arc<Self>, context: usize);
}

/// Software timer structure
pub struct SoftwareTimer {
    pub id: u64,                        // Unique timer ID
    pub expires: u64,                   // Expiration tick
    pub handler: Weak<dyn TimerHandler>,// Weak reference to callback handler
    pub context: usize,                 // User context
    pub active: bool,                   // Is this timer active?
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

use spin::Mutex;

// Heap-based timer list (protected by spin::Mutex)
static SOFTWARE_TIMER_HEAP: Mutex<BinaryHeap<SoftwareTimer>> = Mutex::new(BinaryHeap::new());

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
    SOFTWARE_TIMER_HEAP.lock().push(timer);
    id
}

/// Cancel a timer by id
pub fn cancel_timer(id: u64) {
    let mut heap = SOFTWARE_TIMER_HEAP.lock();
    let mut timers: Vec<_> = heap.drain().collect();
    timers.retain(|t| t.id != id);
    for t in timers {
        heap.push(t);
    }
}

/// Call this from tick() to check and fire expired timers
fn check_software_timers(now: u64) {
    use alloc::vec::Vec;
    let mut expired = Vec::new();
    {
        let mut heap = SOFTWARE_TIMER_HEAP.lock();
        while let Some(timer) = heap.peek() {
            if timer.active && timer.expires <= now {
                let timer = heap.pop().unwrap();
                expired.push(timer);
            } else {
                break;
            }
        }
    } // Unlock the heap to allow other operations
    for timer in expired {
        if let Some(handler) = timer.handler.upgrade() {
            handler.on_timer_expired(timer.context);
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
//             crate::early_println!("[Software Timer] Test timer expired with context: {}", context);
//             if let Some(handler) = unsafe { TEST_HANDLER.clone() } {
//                 crate::early_println!("[Software Timer] Test handler is still available.");
//                 let handler = handler.clone();
//                 add_timer(get_tick() + 100, &handler, context);
//             } else {
//                 crate::early_println!("[Software Timer] Test handler is no longer available.");
//             }
//         }
//     }

//     let handler: Arc<dyn TimerHandler>  = Arc::new(TestHandler);
//     let target_tick = get_tick() + 100; // 100 ticks from now
//     let id = add_timer(target_tick, &handler, 42);
//     crate::early_println!("Test timer registered with ID: {}, tick: {}", id, target_tick);
//     unsafe {
//         TEST_HANDLER = Some(handler);
//     }
// }

// late_initcall!(register_test_timer);