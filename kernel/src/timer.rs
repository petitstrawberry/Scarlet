//! Per-CPU one-shot hardware and software timer support.
//!
//! Software timers are queued on their owner CPU with absolute monotonic
//! nanosecond deadlines.  The timer module deliberately knows nothing about
//! trapframes or scheduling: architecture interrupt handlers own trapframes
//! and decide whether a pending reschedule can safely enter the scheduler.

extern crate alloc;

use alloc::collections::{BTreeMap, BinaryHeap};
use alloc::sync::{Arc, Weak};
use core::cell::UnsafeCell;
use core::cmp::Ordering as CmpOrdering;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use crate::arch::timer::ArchTimer;
use crate::environment::MAX_NUM_CPUS;
use crate::sync::{IrqSpinLock, Once};

const DEBUG_TIMER_STALL_LOGGING: bool = false;
const TIMER_HEARTBEAT_IRQS: u64 = 512;
const MAX_DUE_CALLBACKS_PER_IRQ: usize = 64;
const STALE_HEAP_NODE_COMPACT_THRESHOLD: usize = 32;
pub const NANOSECONDS_PER_MICROSECOND: u64 = 1_000;
pub const NANOSECONDS_PER_MILLISECOND: u64 = 1_000_000;
/// Compatibility and scheduler accounting quantum retained from the former
/// 10 ms tick. It is not a periodic hardware interrupt interval.
pub const SCHEDULER_ACCOUNTING_QUANTUM_NS: u64 = 10 * NANOSECONDS_PER_MILLISECOND;
/// Minimum delay applied to every newly inserted software timer.
///
/// Software timers retain absolute-nanosecond deadlines and FIFO ordering for
/// equal deadlines. This lower bound only prevents a newly armed past or
/// near-future deadline from retriggering the same hardware IRQ immediately.
pub const SOFTWARE_TIMER_MIN_INTERVAL_NS: u64 = NANOSECONDS_PER_MICROSECOND;
/// Minimum delay for scheduler-owned exact timers.
///
/// Scheduler budget enforcement must support intervals below the general
/// software-timer lower bound. A one-nanosecond floor still prevents a timer
/// armed at or before `now` from immediately retriggering the same IRQ.
const SCHEDULER_TIMER_MIN_INTERVAL_NS: u64 = 1;

pub struct KernelTimer {
    // SAFETY: Each CPU only accesses its own timer via cpu_id index.
    // UnsafeCell allows per-CPU mutable access without data races.
    core_local_timer: [UnsafeCell<ArchTimer>; MAX_NUM_CPUS],
    pub interval: u64,
}

// SAFETY: KernelTimer is thread-safe because each CPU only accesses its own timer.
// The ArchTimer instances are per-CPU, and the hardware registers are CPU-local.
unsafe impl Sync for KernelTimer {}

static KERNEL_TIMER: Once<KernelTimer> = Once::new();

/// Minimal CPU-local interrupt mask for timer state access.
///
/// This bypasses `IrqGuard` deliberately: monotonic time is used by lock and
/// scheduler diagnostics, so routing it through the regular preemption
/// bookkeeping would recurse. Saving and restoring the architecture mask is
/// nest-safe in both task and timer-interrupt context.
struct LocalTimerInterruptMask {
    saved: usize,
    cpu_id: Option<usize>,
    _not_send: PhantomData<*mut ()>,
}

impl LocalTimerInterruptMask {
    #[inline(always)]
    fn new() -> Self {
        let saved = crate::arch::interrupt::save_and_disable_interrupts();
        let cpu_id = crate::arch::try_get_cpuid();
        Self {
            saved,
            cpu_id,
            _not_send: PhantomData,
        }
    }
}

impl Drop for LocalTimerInterruptMask {
    #[inline(always)]
    fn drop(&mut self) {
        debug_assert!(
            self.cpu_id.is_none() || self.cpu_id == crate::arch::try_get_cpuid(),
            "local timer interrupt mask crossed CPUs"
        );
        crate::arch::interrupt::restore_interrupts(self.saved);
    }
}

pub fn get_kernel_timer() -> &'static KernelTimer {
    KERNEL_TIMER.call_once(KernelTimer::new)
}

impl KernelTimer {
    fn new() -> Self {
        Self {
            core_local_timer: core::array::from_fn(|_| UnsafeCell::new(ArchTimer::new())),
            interval: u64::MAX,
        }
    }

    /// Initialize the timer for a specific CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - The CPU whose local timer is initialized.
    pub fn init(&self, cpu_id: usize) {
        // SAFETY: Only the specified CPU's local timer is accessed during its
        // initialization before it can service interrupts.
        unsafe { (*self.core_local_timer[cpu_id].get()).stop() };
    }

    pub fn start(&self, cpu_id: usize) {
        // SAFETY: Callers only program their own CPU's local timer.
        unsafe { (*self.core_local_timer[cpu_id].get()).start() };
    }

    pub fn stop(&self, cpu_id: usize) {
        // SAFETY: Callers only program their own CPU's local timer.
        unsafe { (*self.core_local_timer[cpu_id].get()).stop() };
    }

    /// Program a local hardware timer from an absolute monotonic nanosecond
    /// deadline. The architecture implementation converts it to counter units
    /// and clamps past deadlines to a safe minimum delta.
    pub fn set_deadline_ns(&self, cpu_id: usize, deadline_ns: u64) {
        // SAFETY: Callers only program their own CPU's local timer.
        unsafe { (*self.core_local_timer[cpu_id].get()).set_deadline_ns(deadline_ns) };
    }

    pub fn get_time_ns(&self, cpu_id: usize) -> u64 {
        let _interrupt_mask = LocalTimerInterruptMask::new();
        // SAFETY: Runtime mutation of this per-CPU timer is serialized by the
        // same local interrupt mask in `reprogram_local_timer`.
        unsafe { (*self.core_local_timer[cpu_id].get()).get_time_ns() }
    }

    pub fn get_time_us(&self, cpu_id: usize) -> u64 {
        self.get_time_ns(cpu_id) / NANOSECONDS_PER_MICROSECOND
    }
}

static TIMER_IRQ_COUNTS: [AtomicU64; MAX_NUM_CPUS] = [const { AtomicU64::new(0) }; MAX_NUM_CPUS];
static TIMER_PROGRAMMED_DEADLINES_NS: [AtomicU64; MAX_NUM_CPUS] =
    [const { AtomicU64::new(0) }; MAX_NUM_CPUS];
static TIMER_PROGRAMMED_IDS: [AtomicU64; MAX_NUM_CPUS] =
    [const { AtomicU64::new(0) }; MAX_NUM_CPUS];
static TIMER_STALL_LAST_SAMPLE_NS: [AtomicU64; MAX_NUM_CPUS] =
    [const { AtomicU64::new(0) }; MAX_NUM_CPUS];

const TIMER_QUEUE_SNAPSHOT_RETRY_LIMIT: usize = 4;
const TIMER_STALL_SAMPLE_INTERVAL_NS: u64 = 1_000_000_000;

/// Lock-free diagnostic view of one CPU's software-timer queue.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TimerQueueDiagnosticSnapshot {
    /// Even publication sequence for this snapshot.
    pub sequence: u64,
    /// Live queue-head timer ID, or zero when the queue is empty.
    pub head_id: u64,
    /// Queue-head soft deadline in monotonic nanoseconds.
    pub head_soft_deadline_ns: u64,
    /// Queue-head hard deadline in monotonic nanoseconds.
    pub head_hard_deadline_ns: u64,
    /// Opaque queue-head callback context.
    pub head_context: u64,
    /// Number of live entries indexed by timer ID.
    pub live_entries: u64,
    /// Number of nodes retained in the deadline heap.
    pub heap_nodes: u64,
    /// Number of cancelled nodes awaiting heap compaction.
    pub stale_heap_nodes: u64,
}

/// Combined lock-free timer state used by cross-CPU stall diagnostics.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TimerDiagnosticSnapshot {
    /// Timer ID whose hard deadline was last programmed, or zero when stopped.
    pub programmed_id: u64,
    /// Last requested hardware deadline in monotonic nanoseconds.
    pub programmed_deadline_ns: u64,
    /// Latest logical queue snapshot.
    pub queue: TimerQueueDiagnosticSnapshot,
    /// Latest architected timer registers published by the target CPU.
    pub arch: ArchTimerDiagnosticSnapshot,
}

/// Lock-free architected-timer state published at a critical execution boundary.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ArchTimerDiagnosticSnapshot {
    /// Selected timer control register.
    pub control: u64,
    /// Selected timer counter register.
    pub counter: u64,
    /// Selected absolute compare register.
    pub compare: u64,
    /// Return SPSR observed with the timer registers.
    pub return_spsr: u64,
    /// Return PC observed with the timer registers.
    pub return_pc: u64,
}

// Keep frequently written CPU-local slots on separate Apple Silicon cache lines.
#[repr(align(128))]
struct TimerQueueDiagnosticSlot {
    sequence: AtomicU64,
    head_id: AtomicU64,
    head_soft_deadline_ns: AtomicU64,
    head_hard_deadline_ns: AtomicU64,
    head_context: AtomicU64,
    live_entries: AtomicU64,
    heap_nodes: AtomicU64,
    stale_heap_nodes: AtomicU64,
}

impl TimerQueueDiagnosticSlot {
    const fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            head_id: AtomicU64::new(0),
            head_soft_deadline_ns: AtomicU64::new(0),
            head_hard_deadline_ns: AtomicU64::new(0),
            head_context: AtomicU64::new(0),
            live_entries: AtomicU64::new(0),
            heap_nodes: AtomicU64::new(0),
            stale_heap_nodes: AtomicU64::new(0),
        }
    }

    #[inline(always)]
    fn publish(&self, snapshot: TimerQueueDiagnosticSnapshot) {
        // Queue mutation is serialized by the owner queue lock, including
        // remote cancellation, so only one publisher can own an odd sequence.
        let odd_sequence = self.sequence.load(Ordering::Relaxed).wrapping_add(1);
        self.sequence.store(odd_sequence, Ordering::SeqCst);
        self.head_id.store(snapshot.head_id, Ordering::SeqCst);
        self.head_soft_deadline_ns
            .store(snapshot.head_soft_deadline_ns, Ordering::SeqCst);
        self.head_hard_deadline_ns
            .store(snapshot.head_hard_deadline_ns, Ordering::SeqCst);
        self.head_context
            .store(snapshot.head_context, Ordering::SeqCst);
        self.live_entries
            .store(snapshot.live_entries, Ordering::SeqCst);
        self.heap_nodes.store(snapshot.heap_nodes, Ordering::SeqCst);
        self.stale_heap_nodes
            .store(snapshot.stale_heap_nodes, Ordering::SeqCst);
        self.sequence
            .store(odd_sequence.wrapping_add(1), Ordering::SeqCst);
    }

    #[inline(always)]
    fn snapshot(&self) -> TimerQueueDiagnosticSnapshot {
        for _ in 0..TIMER_QUEUE_SNAPSHOT_RETRY_LIMIT {
            let sequence_before = self.sequence.load(Ordering::SeqCst);
            if sequence_before & 1 != 0 {
                continue;
            }
            let snapshot = TimerQueueDiagnosticSnapshot {
                sequence: sequence_before,
                head_id: self.head_id.load(Ordering::SeqCst),
                head_soft_deadline_ns: self.head_soft_deadline_ns.load(Ordering::SeqCst),
                head_hard_deadline_ns: self.head_hard_deadline_ns.load(Ordering::SeqCst),
                head_context: self.head_context.load(Ordering::SeqCst),
                live_entries: self.live_entries.load(Ordering::SeqCst),
                heap_nodes: self.heap_nodes.load(Ordering::SeqCst),
                stale_heap_nodes: self.stale_heap_nodes.load(Ordering::SeqCst),
            };
            if sequence_before == self.sequence.load(Ordering::SeqCst) {
                return snapshot;
            }
        }
        TimerQueueDiagnosticSnapshot {
            sequence: u64::MAX,
            ..TimerQueueDiagnosticSnapshot::default()
        }
    }
}

static TIMER_QUEUE_DIAGNOSTICS: [TimerQueueDiagnosticSlot; MAX_NUM_CPUS] =
    [const { TimerQueueDiagnosticSlot::new() }; MAX_NUM_CPUS];

#[repr(align(128))]
struct ArchTimerDiagnosticSlot {
    sequence: AtomicU64,
    control: AtomicU64,
    counter: AtomicU64,
    compare: AtomicU64,
    return_spsr: AtomicU64,
    return_pc: AtomicU64,
}

impl ArchTimerDiagnosticSlot {
    const fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            control: AtomicU64::new(0),
            counter: AtomicU64::new(0),
            compare: AtomicU64::new(0),
            return_spsr: AtomicU64::new(0),
            return_pc: AtomicU64::new(0),
        }
    }

    #[inline(always)]
    fn publish(&self, snapshot: ArchTimerDiagnosticSnapshot) {
        let odd_sequence = self.sequence.load(Ordering::Relaxed).wrapping_add(1);
        self.sequence.store(odd_sequence, Ordering::SeqCst);
        self.control.store(snapshot.control, Ordering::SeqCst);
        self.counter.store(snapshot.counter, Ordering::SeqCst);
        self.compare.store(snapshot.compare, Ordering::SeqCst);
        self.return_spsr
            .store(snapshot.return_spsr, Ordering::SeqCst);
        self.return_pc.store(snapshot.return_pc, Ordering::SeqCst);
        self.sequence
            .store(odd_sequence.wrapping_add(1), Ordering::SeqCst);
    }

    #[inline(always)]
    fn snapshot(&self) -> ArchTimerDiagnosticSnapshot {
        for _ in 0..TIMER_QUEUE_SNAPSHOT_RETRY_LIMIT {
            let sequence_before = self.sequence.load(Ordering::SeqCst);
            if sequence_before & 1 != 0 {
                continue;
            }
            let snapshot = ArchTimerDiagnosticSnapshot {
                control: self.control.load(Ordering::SeqCst),
                counter: self.counter.load(Ordering::SeqCst),
                compare: self.compare.load(Ordering::SeqCst),
                return_spsr: self.return_spsr.load(Ordering::SeqCst),
                return_pc: self.return_pc.load(Ordering::SeqCst),
            };
            if sequence_before == self.sequence.load(Ordering::SeqCst) {
                return snapshot;
            }
        }
        ArchTimerDiagnosticSnapshot::default()
    }
}

static ARCH_TIMER_DIAGNOSTICS: [ArchTimerDiagnosticSlot; MAX_NUM_CPUS] =
    [const { ArchTimerDiagnosticSlot::new() }; MAX_NUM_CPUS];

/// Return a CPU's local timer IRQ count without taking a lock.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU whose timer IRQ count should be sampled.
///
/// # Returns
///
/// The current count, or `None` when `cpu_id` is outside the supported range.
#[inline(always)]
pub fn timer_irq_count(cpu_id: usize) -> Option<u64> {
    (cpu_id < MAX_NUM_CPUS).then(|| TIMER_IRQ_COUNTS[cpu_id].load(Ordering::Relaxed))
}

/// Return the last local hardware-timer deadline requested for a CPU.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU whose timer program state should be sampled.
///
/// # Returns
///
/// The requested absolute monotonic deadline in nanoseconds, zero when the
/// timer was stopped, or `None` when `cpu_id` is outside the supported range.
#[inline(always)]
pub fn timer_programmed_deadline_ns(cpu_id: usize) -> Option<u64> {
    (cpu_id < MAX_NUM_CPUS).then(|| TIMER_PROGRAMMED_DEADLINES_NS[cpu_id].load(Ordering::Acquire))
}

/// Return the last published timer-program and logical queue state without locking.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU whose timer state should be sampled.
///
/// # Returns
///
/// A combined diagnostic snapshot, or `None` for an invalid CPU ID.
#[inline(always)]
pub(crate) fn timer_diagnostic_snapshot(cpu_id: usize) -> Option<TimerDiagnosticSnapshot> {
    (cpu_id < MAX_NUM_CPUS).then(|| TimerDiagnosticSnapshot {
        programmed_id: TIMER_PROGRAMMED_IDS[cpu_id].load(Ordering::Acquire),
        programmed_deadline_ns: TIMER_PROGRAMMED_DEADLINES_NS[cpu_id].load(Ordering::Acquire),
        queue: TIMER_QUEUE_DIAGNOSTICS[cpu_id].snapshot(),
        arch: ARCH_TIMER_DIAGNOSTICS[cpu_id].snapshot(),
    })
}

/// Publish the current CPU's architected timer registers for cross-CPU diagnostics.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU publishing its own register state.
/// * `snapshot` - Timer registers and associated return state.
pub(crate) fn publish_arch_timer_diagnostic(cpu_id: usize, snapshot: ArchTimerDiagnosticSnapshot) {
    if cpu_id < MAX_NUM_CPUS {
        ARCH_TIMER_DIAGNOSTICS[cpu_id].publish(snapshot);
    }
}

fn should_sample_timer_stalls(cpu_id: usize, now_ns: u64) -> bool {
    let last_sample = &TIMER_STALL_LAST_SAMPLE_NS[cpu_id];
    let mut observed = last_sample.load(Ordering::Relaxed);
    loop {
        if observed != 0 && now_ns.saturating_sub(observed) < TIMER_STALL_SAMPLE_INTERVAL_NS {
            return false;
        }
        match last_sample.compare_exchange_weak(
            observed,
            now_ns.max(1),
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(actual) => observed = actual,
        }
    }
}

/// A stable reference to a software timer.
///
/// A handle is owned by one local timer queue. It can safely be cancelled from
/// any CPU, but only its owner CPU can reprogram its local hardware timer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TimerHandle {
    pub owner_cpu: usize,
    pub id: u64,
}

/// Timer entry lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TimerState {
    Pending = 0,
    Running = 1,
    Cancelled = 2,
    Completed = 3,
}

/// Permitted delivery range for a software timer.
///
/// Timers never run before their soft deadline. The hard deadline is used for
/// queue ordering and hardware programming, allowing compatible work to be
/// coalesced without rounding deadlines into fixed buckets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerPrecision {
    /// No additional delivery slack; soft and hard deadlines are identical.
    ///
    /// Exact timers still use the common
    /// [`SOFTWARE_TIMER_MIN_INTERVAL_NS`] lower bound when armed, so they do
    /// not bypass the minimum future interval.
    Exact,
    /// Modest coalescing slack for waits and non-protocol maintenance.
    Normal,
    /// Broad coalescing slack for polling and deferred device work.
    Coarse,
}

impl TimerPrecision {
    /// Return the maximum permitted delay after a timer's soft deadline.
    #[inline]
    pub const fn slack_ns(self) -> u64 {
        match self {
            Self::Exact => 0,
            Self::Normal => NANOSECONDS_PER_MILLISECOND,
            Self::Coarse => 10 * NANOSECONDS_PER_MILLISECOND,
        }
    }
}

impl TimerState {
    #[inline]
    fn from_raw(value: u8) -> Self {
        match value {
            0 => Self::Pending,
            1 => Self::Running,
            2 => Self::Cancelled,
            3 => Self::Completed,
            _ => unreachable!("invalid software timer state"),
        }
    }
}

/// Trait invoked after a software timer reaches its absolute deadline.
pub trait TimerHandler: Send + Sync {
    /// Handle a timer expiration.
    ///
    /// # Arguments
    ///
    /// * `context` - Opaque value supplied when the timer was armed.
    ///
    /// # Returns
    ///
    /// This method returns no value. It may arm, cancel, or wake timers/tasks,
    /// but runs with no timer queue lock held.
    fn on_timer_expired(self: Arc<Self>, context: usize);
}

struct SoftwareTimer {
    id: u64,
    soft_deadline_ns: u64,
    hard_deadline_ns: u64,
    sequence: u64,
    handler: Weak<dyn TimerHandler>,
    context: usize,
    state: AtomicU8,
}

impl SoftwareTimer {
    fn state(&self) -> TimerState {
        TimerState::from_raw(self.state.load(Ordering::Acquire))
    }

    fn cancel(&self) -> bool {
        self.state
            .compare_exchange(
                TimerState::Pending as u8,
                TimerState::Cancelled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn claim(&self) -> bool {
        self.state
            .compare_exchange(
                TimerState::Pending as u8,
                TimerState::Running as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn complete(&self) {
        self.state
            .store(TimerState::Completed as u8, Ordering::Release);
    }
}

#[derive(Clone)]
struct QueuedTimer(Arc<SoftwareTimer>);

impl PartialEq for QueuedTimer {
    fn eq(&self, other: &Self) -> bool {
        self.0.hard_deadline_ns == other.0.hard_deadline_ns && self.0.sequence == other.0.sequence
    }
}

impl Eq for QueuedTimer {}

impl Ord for QueuedTimer {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // BinaryHeap is a max-heap, so reverse hard deadline and sequence keys.
        other
            .0
            .hard_deadline_ns
            .cmp(&self.0.hard_deadline_ns)
            .then_with(|| other.0.sequence.cmp(&self.0.sequence))
    }
}

impl PartialOrd for QueuedTimer {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

struct TimerQueue {
    heap: BinaryHeap<QueuedTimer>,
    entries: BTreeMap<u64, Arc<SoftwareTimer>>,
    next_sequence: u64,
    stale_heap_nodes: usize,
}

impl TimerQueue {
    fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            entries: BTreeMap::new(),
            next_sequence: 0,
            stale_heap_nodes: 0,
        }
    }

    fn add(
        &mut self,
        id: u64,
        soft_deadline_ns: u64,
        precision: TimerPrecision,
        handler: &Arc<dyn TimerHandler>,
        context: usize,
    ) {
        let timer = Arc::new(SoftwareTimer {
            id,
            soft_deadline_ns,
            hard_deadline_ns: soft_deadline_ns.saturating_add(precision.slack_ns()),
            sequence: self.next_sequence,
            handler: Arc::downgrade(handler),
            context,
            state: AtomicU8::new(TimerState::Pending as u8),
        });
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.entries.insert(id, timer.clone());
        self.heap.push(QueuedTimer(timer));
    }

    fn cancel(&mut self, id: u64) -> bool {
        let cancelled = self.entries.get(&id).is_some_and(|timer| timer.cancel());
        if cancelled {
            self.entries.remove(&id);
            self.stale_heap_nodes = self.stale_heap_nodes.saturating_add(1);
            self.compact_stale_heap_nodes_if_needed();
        }
        cancelled
    }

    fn compact_stale_heap_nodes_if_needed(&mut self) {
        if self.stale_heap_nodes < STALE_HEAP_NODE_COMPACT_THRESHOLD
            && self.stale_heap_nodes.saturating_mul(2) < self.heap.len()
        {
            return;
        }

        let stale_heap = core::mem::take(&mut self.heap);
        self.heap = stale_heap
            .into_iter()
            .filter(|queued| {
                queued.0.state() == TimerState::Pending && self.entries.contains_key(&queued.0.id)
            })
            .collect();
        self.stale_heap_nodes = 0;
    }

    fn discard_non_pending_head(&mut self) {
        while let Some(timer) = self.heap.peek() {
            if timer.0.state() == TimerState::Pending {
                return;
            }
            let timer = self
                .heap
                .pop()
                .expect("software timer heap entry disappeared");
            if timer.0.state() == TimerState::Cancelled {
                self.stale_heap_nodes = self.stale_heap_nodes.saturating_sub(1);
            }
            self.entries.remove(&timer.0.id);
        }
    }

    fn earliest_live_hard_deadline(&mut self) -> Option<u64> {
        self.discard_non_pending_head();
        self.heap.peek().map(|timer| timer.0.hard_deadline_ns)
    }

    #[cfg(test)]
    fn has_due(&mut self, now_ns: u64) -> bool {
        self.discard_non_pending_head();
        self.heap
            .peek()
            .is_some_and(|timer| timer.0.soft_deadline_ns <= now_ns)
    }

    fn claim_due(&mut self, now_ns: u64) -> Option<Arc<SoftwareTimer>> {
        loop {
            self.discard_non_pending_head();
            let timer = self.heap.peek()?.0.clone();
            if timer.soft_deadline_ns > now_ns {
                return None;
            }
            let timer = self.heap.pop().expect("due software timer disappeared").0;
            if timer.claim() {
                return Some(timer);
            }
            self.entries.remove(&timer.id);
        }
    }

    fn finish(&mut self, timer: &Arc<SoftwareTimer>) {
        timer.complete();
        self.entries.remove(&timer.id);
    }

    fn diagnostic_snapshot(&mut self) -> TimerQueueDiagnosticSnapshot {
        self.discard_non_pending_head();
        let head = self.heap.peek().map(|queued| &queued.0);
        TimerQueueDiagnosticSnapshot {
            sequence: 0,
            head_id: head.map_or(0, |timer| timer.id),
            head_soft_deadline_ns: head.map_or(0, |timer| timer.soft_deadline_ns),
            head_hard_deadline_ns: head.map_or(0, |timer| timer.hard_deadline_ns),
            head_context: head.map_or(0, |timer| timer.context as u64),
            live_entries: self.entries.len() as u64,
            heap_nodes: self.heap.len() as u64,
            stale_heap_nodes: self.stale_heap_nodes as u64,
        }
    }
}

static TIMER_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static SOFTWARE_TIMER_QUEUES: Once<[IrqSpinLock<TimerQueue>; MAX_NUM_CPUS]> = Once::new();

fn timer_queues() -> &'static [IrqSpinLock<TimerQueue>; MAX_NUM_CPUS] {
    SOFTWARE_TIMER_QUEUES
        .call_once(|| core::array::from_fn(|_| IrqSpinLock::new(TimerQueue::new())))
}

#[inline(always)]
fn publish_queue_diagnostic(cpu_id: usize, queue: &mut TimerQueue) {
    let snapshot = queue.diagnostic_snapshot();
    TIMER_QUEUE_DIAGNOSTICS[cpu_id].publish(snapshot);
}

fn local_cpu_id() -> usize {
    crate::arch::get_cpu().get_cpuid()
}

#[inline]
fn clamp_software_timer_deadline(now_ns: u64, requested_deadline_ns: u64) -> u64 {
    requested_deadline_ns.max(now_ns.saturating_add(SOFTWARE_TIMER_MIN_INTERVAL_NS))
}

#[inline]
fn software_timer_deadlines(
    now_ns: u64,
    requested_deadline_ns: u64,
    precision: TimerPrecision,
) -> (u64, u64) {
    let soft_deadline_ns = clamp_software_timer_deadline(now_ns, requested_deadline_ns);
    (
        soft_deadline_ns,
        soft_deadline_ns.saturating_add(precision.slack_ns()),
    )
}

#[inline]
fn scheduler_timer_deadline(now_ns: u64, requested_deadline_ns: u64) -> u64 {
    requested_deadline_ns.max(now_ns.saturating_add(SCHEDULER_TIMER_MIN_INTERVAL_NS))
}

/// Advance a periodic timer to the first interval strictly after `now_ns`.
///
/// The returned overrun count is the number of elapsed intervals in addition
/// to the expiration currently being delivered. It coalesces delayed hardware
/// IRQs instead of replaying every missed interval.
pub(crate) fn coalesce_periodic_deadline(
    expired_deadline_ns: u64,
    interval_ns: u64,
    now_ns: u64,
) -> (u64, u64) {
    debug_assert!(interval_ns > 0);

    let elapsed_ns = now_ns.saturating_sub(expired_deadline_ns);
    let overruns = elapsed_ns / interval_ns;
    let periods = overruns.saturating_add(1);
    let next_deadline_ns = expired_deadline_ns.saturating_add(interval_ns.saturating_mul(periods));

    if next_deadline_ns > now_ns {
        (next_deadline_ns, overruns)
    } else {
        (now_ns.saturating_add(interval_ns), overruns)
    }
}

/// Add a timer to the current CPU's local queue.
///
/// # Arguments
///
/// * `deadline_ns` - Requested absolute monotonic soft expiration in nanoseconds.
/// * `precision` - Explicit permitted delivery range for this timer.
/// * `handler` - Callback object held weakly until expiration.
/// * `context` - Opaque callback context.
///
/// # Returns
///
/// A handle that can be cancelled from any CPU.
pub fn add_timer(
    deadline_ns: u64,
    precision: TimerPrecision,
    handler: &Arc<dyn TimerHandler>,
    context: usize,
) -> TimerHandle {
    let owner_cpu = local_cpu_id();
    let id = TIMER_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let (soft_deadline_ns, _) = software_timer_deadlines(get_time_ns(), deadline_ns, precision);
    {
        let mut queue = timer_queues()[owner_cpu].lock();
        queue.add(id, soft_deadline_ns, precision, handler, context);
        publish_queue_diagnostic(owner_cpu, &mut queue);
    }
    // This is deliberately local: add_timer owns the new entry on the current
    // CPU and must never program a remote CPU's local hardware comparator.
    reprogram_local_timer();
    TimerHandle { owner_cpu, id }
}

/// Add an exact scheduler timer to the current CPU's local queue.
///
/// Unlike [`add_timer`], this internal API does not apply the general 10 ms
/// software-timer lower bound. It is reserved for scheduler budget and
/// replenishment deadlines that require sub-quantum precision.
///
/// # Arguments
///
/// * `deadline_ns` - Requested absolute monotonic expiration in nanoseconds.
/// * `handler` - Callback object held weakly until expiration.
/// * `context` - Opaque callback context.
///
/// # Returns
///
/// A handle that can be cancelled from any CPU.
pub(crate) fn add_scheduler_timer(
    deadline_ns: u64,
    handler: &Arc<dyn TimerHandler>,
    context: usize,
) -> TimerHandle {
    let owner_cpu = local_cpu_id();
    let id = TIMER_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let soft_deadline_ns = scheduler_timer_deadline(get_time_ns(), deadline_ns);
    {
        let mut queue = timer_queues()[owner_cpu].lock();
        queue.add(
            id,
            soft_deadline_ns,
            TimerPrecision::Exact,
            handler,
            context,
        );
        publish_queue_diagnostic(owner_cpu, &mut queue);
    }
    reprogram_local_timer();
    TimerHandle { owner_cpu, id }
}

/// Cancel a timer if it is still pending.
///
/// # Arguments
///
/// * `handle` - Timer to cancel.
///
/// # Returns
///
/// `true` only when this call changed `Pending` to `Cancelled`. A successful
/// return guarantees the callback will not execute. Remote cancellation does
/// not program the owner's local hardware timer; a stale early IRQ is harmless.
pub fn cancel_timer(handle: TimerHandle) -> bool {
    if handle.owner_cpu >= MAX_NUM_CPUS {
        return false;
    }
    let cancelled = {
        let mut queue = timer_queues()[handle.owner_cpu].lock();
        let cancelled = queue.cancel(handle.id);
        publish_queue_diagnostic(handle.owner_cpu, &mut queue);
        cancelled
    };
    if cancelled && handle.owner_cpu == local_cpu_id() {
        reprogram_local_timer();
    }
    cancelled
}

/// Inspect the earliest live hard deadline owned by the current CPU.
pub fn peek_local_deadline() -> Option<u64> {
    let cpu_id = local_cpu_id();
    let mut queue = timer_queues()[cpu_id].lock();
    let deadline = queue.earliest_live_hard_deadline();
    publish_queue_diagnostic(cpu_id, &mut queue);
    deadline
}

/// Hardware timer policy for a local queue head.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalTimerProgram {
    Stop,
    Deadline(u64),
}

#[inline]
pub const fn local_timer_program(next_deadline_ns: Option<u64>) -> LocalTimerProgram {
    match next_deadline_ns {
        Some(deadline_ns) => LocalTimerProgram::Deadline(deadline_ns),
        None => LocalTimerProgram::Stop,
    }
}

/// Program or stop the current CPU's hardware timer from its queue head.
pub fn reprogram_local_timer() {
    let _interrupt_mask = LocalTimerInterruptMask::new();
    let cpu_id = local_cpu_id();
    match local_timer_program(peek_local_deadline()) {
        LocalTimerProgram::Deadline(deadline_ns) => {
            let programmed_id = TIMER_QUEUE_DIAGNOSTICS[cpu_id].snapshot().head_id;
            TIMER_PROGRAMMED_IDS[cpu_id].store(programmed_id, Ordering::Release);
            TIMER_PROGRAMMED_DEADLINES_NS[cpu_id].store(deadline_ns, Ordering::Release);
            crate::breadcrumb::drop(crate::breadcrumb::TIMER_PROGRAM, cpu_id as u64, deadline_ns);
            let timer = get_kernel_timer();
            timer.set_deadline_ns(cpu_id, deadline_ns);
            timer.start(cpu_id);
            crate::breadcrumb::drop(
                crate::breadcrumb::TIMER_PROGRAM_DONE,
                cpu_id as u64,
                deadline_ns,
            );
        }
        LocalTimerProgram::Stop => {
            TIMER_PROGRAMMED_IDS[cpu_id].store(0, Ordering::Release);
            TIMER_PROGRAMMED_DEADLINES_NS[cpu_id].store(0, Ordering::Release);
            crate::breadcrumb::drop(crate::breadcrumb::TIMER_PROGRAM, cpu_id as u64, 0);
            get_kernel_timer().stop(cpu_id);
            crate::breadcrumb::drop(crate::breadcrumb::TIMER_PROGRAM_DONE, cpu_id as u64, 0);
        }
    }
}

/// Drain due callbacks from the current CPU's queue.
///
/// Entries transition from `Pending` to `Running` while held by the queue lock,
/// then callbacks execute with all timer locks released. A callback may freely
/// arm, cancel, or wake other timers before the queue is inspected again.
///
/// Processing stops after the per-IRQ callback budget. Any remaining queue head
/// is reprogrammed by the caller through the architecture's safe comparator
/// delta.
fn drain_local_due_timers() {
    let cpu_id = local_cpu_id();
    for _ in 0..MAX_DUE_CALLBACKS_PER_IRQ {
        let now_ns = get_time_ns();
        let timer = {
            let mut queue = timer_queues()[cpu_id].lock();
            let timer = queue.claim_due(now_ns);
            publish_queue_diagnostic(cpu_id, &mut queue);
            timer
        };
        let Some(timer) = timer else {
            return;
        };

        if let Some(handler) = timer.handler.upgrade() {
            #[cfg(feature = "sync-debug")]
            crate::breadcrumb::drop(
                crate::breadcrumb::TIMER_CALLBACK_ENTER,
                timer.id,
                timer.context as u64,
            );
            let _callback_guard = crate::sync::PreemptGuard::new();
            handler.on_timer_expired(timer.context);
            #[cfg(feature = "sync-debug")]
            crate::breadcrumb::drop(
                crate::breadcrumb::TIMER_CALLBACK_DONE,
                timer.id,
                timer.context as u64,
            );
        }

        {
            let mut queue = timer_queues()[cpu_id].lock();
            queue.finish(&timer);
            publish_queue_diagnostic(cpu_id, &mut queue);
        }
    }
}

/// Process a local hardware timer IRQ without making scheduler decisions.
///
/// Architecture trap code must call its scheduler helper afterwards while it
/// still owns the trapframe and can evaluate whether scheduling is legal.
pub fn handle_local_timer_irq() {
    let cpu_id = local_cpu_id();
    let irq_count = TIMER_IRQ_COUNTS[cpu_id].fetch_add(1, Ordering::Relaxed) + 1;
    if DEBUG_TIMER_STALL_LOGGING && (irq_count <= 3 || irq_count % TIMER_HEARTBEAT_IRQS == 0) {
        crate::emergency_println!("[timer] irq heartbeat cpu={} count={}", cpu_id, irq_count);
        crate::breadcrumb::sample_timer_stalls(cpu_id, timer_irq_count, get_time_ns());
    }
    #[cfg(feature = "sync-debug")]
    {
        let now_ns = get_time_ns();
        if should_sample_timer_stalls(cpu_id, now_ns) {
            crate::breadcrumb::sample_timer_stalls(cpu_id, timer_irq_count, now_ns);
        }
    }
    crate::breadcrumb::drop(crate::breadcrumb::TIMER_TICK, irq_count, 0);
    crate::breadcrumb::drop(crate::breadcrumb::TIMER_SW_TIMERS, irq_count, 0);
    drain_local_due_timers();
    // A remaining queue head may already be due. Program it unchanged and let
    // the architecture timer apply its safe minimum comparator delta.
    reprogram_local_timer();
}

/// Get monotonic local time in nanoseconds.
pub fn get_time_ns() -> u64 {
    let cpu_id = local_cpu_id();
    get_kernel_timer().get_time_ns(cpu_id)
}

/// Get monotonic local time in microseconds.
pub fn get_time_us() -> u64 {
    get_time_ns() / NANOSECONDS_PER_MICROSECOND
}

/// Return 10 ms compatibility units for legacy non-deadline accounting only.
/// Software timer callers must use [`get_time_ns`] and absolute deadlines.
pub fn get_tick() -> u64 {
    get_time_ns() / SCHEDULER_ACCOUNTING_QUANTUM_NS
}

#[inline]
pub const fn ms_to_ns(milliseconds: u64) -> u64 {
    milliseconds.saturating_mul(NANOSECONDS_PER_MILLISECOND)
}

#[inline]
pub const fn us_to_ns(microseconds: u64) -> u64 {
    microseconds.saturating_mul(NANOSECONDS_PER_MICROSECOND)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    struct RecordingHandler {
        calls: Arc<IrqSpinLock<alloc::vec::Vec<usize>>>,
    }

    impl TimerHandler for RecordingHandler {
        fn on_timer_expired(self: Arc<Self>, context: usize) {
            self.calls.lock().push(context);
        }
    }

    fn queue_with_handler() -> (
        TimerQueue,
        Arc<dyn TimerHandler>,
        Arc<IrqSpinLock<alloc::vec::Vec<usize>>>,
    ) {
        let calls = Arc::new(IrqSpinLock::new(alloc::vec::Vec::new()));
        let handler: Arc<dyn TimerHandler> = Arc::new(RecordingHandler {
            calls: calls.clone(),
        });
        (TimerQueue::new(), handler, calls)
    }

    fn run_due(queue: &mut TimerQueue, now_ns: u64) {
        while let Some(timer) = queue.claim_due(now_ns) {
            if let Some(handler) = timer.handler.upgrade() {
                handler.on_timer_expired(timer.context);
            }
            queue.finish(&timer);
        }
    }

    fn run_due_with_budget(queue: &mut TimerQueue, now_ns: u64, budget: usize) -> bool {
        for _ in 0..budget {
            let Some(timer) = queue.claim_due(now_ns) else {
                return false;
            };
            if let Some(handler) = timer.handler.upgrade() {
                handler.on_timer_expired(timer.context);
            }
            queue.finish(&timer);
        }
        queue.has_due(now_ns)
    }

    #[test_case]
    fn lower_bound_clamps_only_near_deadlines() {
        let now_ns = 1_000;
        let minimum_deadline = now_ns + SOFTWARE_TIMER_MIN_INTERVAL_NS;

        assert_eq!(clamp_software_timer_deadline(now_ns, 0), minimum_deadline);
        assert_eq!(
            clamp_software_timer_deadline(now_ns, minimum_deadline - 1),
            minimum_deadline
        );
        assert_eq!(
            clamp_software_timer_deadline(now_ns, minimum_deadline + 17),
            minimum_deadline + 17
        );
    }

    #[test_case]
    fn lower_bound_saturates_at_the_end_of_time() {
        assert_eq!(clamp_software_timer_deadline(u64::MAX - 1, 0), u64::MAX);
    }

    #[test_case]
    fn precision_ranges_preserve_soft_deadlines_and_slack() {
        assert_eq!(TimerPrecision::Exact.slack_ns(), 0);
        assert_eq!(TimerPrecision::Normal.slack_ns(), ms_to_ns(1));
        assert_eq!(TimerPrecision::Coarse.slack_ns(), ms_to_ns(10));
        assert_eq!(
            software_timer_deadlines(0, ms_to_ns(20), TimerPrecision::Exact),
            (ms_to_ns(20), ms_to_ns(20))
        );
        assert_eq!(
            software_timer_deadlines(0, ms_to_ns(20), TimerPrecision::Normal),
            (ms_to_ns(20), ms_to_ns(21))
        );
        assert_eq!(
            software_timer_deadlines(0, ms_to_ns(20), TimerPrecision::Coarse),
            (ms_to_ns(20), ms_to_ns(30))
        );
    }

    #[test_case]
    fn precision_ranges_clamp_soft_deadlines_and_saturate_hard_deadlines() {
        assert_eq!(
            software_timer_deadlines(1_000, 0, TimerPrecision::Normal),
            (
                1_000 + SOFTWARE_TIMER_MIN_INTERVAL_NS,
                1_000 + SOFTWARE_TIMER_MIN_INTERVAL_NS + ms_to_ns(1),
            )
        );
        assert_eq!(
            software_timer_deadlines(u64::MAX - 1, 0, TimerPrecision::Coarse),
            (u64::MAX, u64::MAX)
        );
    }

    #[test_case]
    fn exact_precision_has_no_slack_but_keeps_the_common_minimum() {
        let (soft_deadline_ns, hard_deadline_ns) =
            software_timer_deadlines(1_000, 0, TimerPrecision::Exact);

        assert_eq!(soft_deadline_ns, 1_000 + SOFTWARE_TIMER_MIN_INTERVAL_NS);
        assert_eq!(hard_deadline_ns, soft_deadline_ns);
    }

    #[test_case]
    fn sub_quantum_sleep_deadline_is_not_inflated_to_scheduler_quantum() {
        let deadline_ns = ms_to_ns(5);

        assert!(deadline_ns < SCHEDULER_ACCOUNTING_QUANTUM_NS);
        assert_eq!(
            software_timer_deadlines(0, deadline_ns, TimerPrecision::Exact),
            (deadline_ns, deadline_ns)
        );
    }

    #[test_case]
    fn scheduler_deadline_bypasses_the_general_timer_minimum() {
        let now_ns = 1_000;

        assert_eq!(scheduler_timer_deadline(now_ns, 0), now_ns + 1);
        assert_eq!(scheduler_timer_deadline(now_ns, now_ns + 5), now_ns + 5);
        assert!(
            scheduler_timer_deadline(now_ns, now_ns + 5) < now_ns + SOFTWARE_TIMER_MIN_INTERVAL_NS
        );
    }

    #[test_case]
    fn periodic_deadline_coalesces_missed_intervals() {
        assert_eq!(coalesce_periodic_deadline(100, 10, 145), (150, 4));
        assert_eq!(coalesce_periodic_deadline(100, 10, 150), (160, 5));
    }

    #[test_case]
    fn periodic_deadline_saturates_without_replaying_overflowed_intervals() {
        assert_eq!(
            coalesce_periodic_deadline(u64::MAX - 4, 10, u64::MAX - 1),
            (u64::MAX, 0)
        );
    }

    #[test_case]
    fn absolute_deadlines_are_ordered_and_ties_are_fifo() {
        let (mut queue, handler, calls) = queue_with_handler();
        queue.add(1, 30, TimerPrecision::Exact, &handler, 3);
        queue.add(2, 10, TimerPrecision::Exact, &handler, 1);
        queue.add(3, 10, TimerPrecision::Exact, &handler, 2);

        run_due(&mut queue, 30);

        assert_eq!(*calls.lock(), alloc::vec![1, 2, 3]);
    }

    #[test_case]
    fn hardware_head_uses_hard_deadline() {
        let (mut queue, handler, _) = queue_with_handler();
        queue.add(1, ms_to_ns(20), TimerPrecision::Coarse, &handler, 1);
        queue.add(2, ms_to_ns(25), TimerPrecision::Exact, &handler, 2);

        assert_eq!(queue.earliest_live_hard_deadline(), Some(ms_to_ns(25)));
    }

    #[test_case]
    fn no_timer_fires_before_its_soft_deadline() {
        let (mut queue, handler, calls) = queue_with_handler();
        queue.add(1, 100, TimerPrecision::Coarse, &handler, 1);

        run_due(&mut queue, 99);
        assert!(calls.lock().is_empty());
        run_due(&mut queue, 100);
        assert_eq!(*calls.lock(), alloc::vec![1]);
    }

    #[test_case]
    fn exact_wake_coalesces_soft_eligible_coarse_work_in_hard_order() {
        let (mut queue, handler, calls) = queue_with_handler();
        queue.add(1, 50, TimerPrecision::Exact, &handler, 1);
        queue.add(2, 45, TimerPrecision::Coarse, &handler, 2);

        run_due(&mut queue, 50);
        assert_eq!(*calls.lock(), alloc::vec![1, 2]);
    }

    #[test_case]
    fn hard_deadline_head_blocks_later_range_until_its_soft_deadline() {
        let (mut queue, handler, calls) = queue_with_handler();
        queue.add(1, ms_to_ns(49), TimerPrecision::Normal, &handler, 1);
        queue.add(2, ms_to_ns(42), TimerPrecision::Coarse, &handler, 2);

        run_due(&mut queue, ms_to_ns(45));
        assert!(calls.lock().is_empty());
        assert_eq!(queue.earliest_live_hard_deadline(), Some(ms_to_ns(50)));

        run_due(&mut queue, ms_to_ns(49));
        assert_eq!(*calls.lock(), alloc::vec![1, 2]);
    }

    #[test_case]
    fn equal_hard_deadlines_are_fifo_across_precision_classes() {
        let (mut queue, handler, calls) = queue_with_handler();
        queue.add(1, ms_to_ns(1), TimerPrecision::Normal, &handler, 1);
        queue.add(2, ms_to_ns(2), TimerPrecision::Exact, &handler, 2);

        run_due(&mut queue, ms_to_ns(2));
        assert_eq!(*calls.lock(), alloc::vec![1, 2]);
    }

    #[test_case]
    fn due_callback_budget_defers_remaining_fifo_work() {
        let (mut queue, handler, calls) = queue_with_handler();
        queue.add(1, 10, TimerPrecision::Exact, &handler, 1);
        queue.add(2, 10, TimerPrecision::Exact, &handler, 2);
        queue.add(3, 10, TimerPrecision::Exact, &handler, 3);

        assert!(run_due_with_budget(&mut queue, 10, 2));
        assert_eq!(*calls.lock(), alloc::vec![1, 2]);
        assert!(!run_due_with_budget(&mut queue, 10, 2));
        assert_eq!(*calls.lock(), alloc::vec![1, 2, 3]);
    }

    #[test_case]
    fn cancelled_non_head_timers_are_compacted_behind_a_live_anchor() {
        let (mut queue, handler, _) = queue_with_handler();
        queue.add(1, 10, TimerPrecision::Exact, &handler, 1);
        for id in 2..=130 {
            queue.add(id, 20 + id, TimerPrecision::Exact, &handler, id as usize);
        }

        for id in 2..=130 {
            assert!(queue.cancel(id));
        }

        assert_eq!(queue.entries.len(), 1);
        assert_eq!(queue.earliest_live_hard_deadline(), Some(10));
        assert!(queue.heap.len() <= STALE_HEAP_NODE_COMPACT_THRESHOLD + 1);
        assert_eq!(queue.stale_heap_nodes, 0);
    }

    #[test_case]
    fn per_cpu_queues_are_isolated() {
        let (mut cpu_zero, handler, calls) = queue_with_handler();
        let mut cpu_one = TimerQueue::new();
        cpu_zero.add(1, 10, TimerPrecision::Exact, &handler, 0);
        cpu_one.add(2, 5, TimerPrecision::Exact, &handler, 1);

        assert_eq!(cpu_zero.earliest_live_hard_deadline(), Some(10));
        assert_eq!(cpu_one.earliest_live_hard_deadline(), Some(5));
        run_due(&mut cpu_zero, 10);
        assert_eq!(*calls.lock(), alloc::vec![0]);
        assert_eq!(cpu_one.earliest_live_hard_deadline(), Some(5));
    }

    #[test_case]
    fn cancellation_wins_while_pending_and_prevents_callback() {
        let (mut queue, handler, calls) = queue_with_handler();
        queue.add(7, 10, TimerPrecision::Exact, &handler, 7);

        assert!(queue.cancel(7));
        run_due(&mut queue, 10);

        assert!(calls.lock().is_empty());
        assert_eq!(queue.earliest_live_hard_deadline(), None);
    }

    #[test_case]
    fn claim_wins_over_late_cancellation() {
        let (mut queue, handler, calls) = queue_with_handler();
        queue.add(7, 10, TimerPrecision::Exact, &handler, 7);
        let timer = queue.claim_due(10).expect("timer must be due");

        assert!(!queue.cancel(7));
        if let Some(handler) = timer.handler.upgrade() {
            handler.on_timer_expired(timer.context);
        }
        queue.finish(&timer);

        assert_eq!(*calls.lock(), alloc::vec![7]);
    }

    struct SelfRearmingHandler {
        queue: &'static IrqSpinLock<TimerQueue>,
        weak_self: IrqSpinLock<Option<Weak<dyn TimerHandler>>>,
        calls: AtomicUsize,
    }

    impl TimerHandler for SelfRearmingHandler {
        fn on_timer_expired(self: Arc<Self>, _context: usize) {
            if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
                let handler = self
                    .weak_self
                    .lock()
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .expect("self-rearming handler must remain alive");
                self.queue
                    .lock()
                    .add(2, 20, TimerPrecision::Exact, &handler, 0);
            }
        }
    }

    #[test_case]
    fn callback_can_rearm_a_timer() {
        let queue =
            alloc::boxed::Box::leak(alloc::boxed::Box::new(IrqSpinLock::new(TimerQueue::new())));
        let handler = Arc::new(SelfRearmingHandler {
            queue,
            weak_self: IrqSpinLock::new(None),
            calls: AtomicUsize::new(0),
        });
        let handler_dyn: Arc<dyn TimerHandler> = handler.clone();
        *handler.weak_self.lock() = Some(Arc::downgrade(&handler_dyn));
        queue
            .lock()
            .add(1, 10, TimerPrecision::Exact, &handler_dyn, 0);

        {
            let mut queue_guard = queue.lock();
            let timer = queue_guard.claim_due(10).expect("first timer must be due");
            drop(queue_guard);
            handler_dyn.clone().on_timer_expired(0);
            queue.lock().finish(&timer);
        }
        let mut queue = queue.lock();
        let timer = queue.claim_due(20).expect("rearmed timer must be due");
        queue.finish(&timer);
        assert_eq!(handler.calls.load(Ordering::Relaxed), 1);
    }

    #[test_case]
    fn empty_queue_selects_stop_policy() {
        assert_eq!(local_timer_program(None), LocalTimerProgram::Stop);
        assert_eq!(
            local_timer_program(Some(123)),
            LocalTimerProgram::Deadline(123)
        );
    }

    #[test_case]
    fn conversion_helpers_saturate() {
        assert_eq!(ms_to_ns(1), 1_000_000);
        assert_eq!(us_to_ns(1), 1_000);
        assert_eq!(ms_to_ns(u64::MAX), u64::MAX);
    }
}
