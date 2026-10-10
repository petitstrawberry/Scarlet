//! Opt-in cumulative network diagnostics for `/dev/net_profile`.
//!
//! Counters are relaxed atomics, not a coherent multi-field snapshot. They are
//! never reset. Disabling stops new observations; sampled spans already in
//! flight can finish afterward. Timings sample every 16th enabled stage call.
//! They measure elapsed wall time (including preemption and lock waits), not
//! task CPU time. Nested/overlapping stages must not be added together.
//! Profiling adds atomic/timer overhead; compare throughput disabled/enabled.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub(crate) const SAMPLE_EVERY: u64 = 16;

#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum Stage {
    XhciRxInvalidate,
    XhciRxCopy,
    XhciRxRequeue,
    XhciTxCopy,
    XhciTxClean,
    NcmParse,
    NcmEnqueue,
    NcmQueueWait,
    StackDispatch,
    TcpReceive,
    TcpDrain,
    NcmTxBuild,
    RxNtb,
    RxErrors,
    RxQueueDrops,
    XhciFullEventBudget,
    NcmFullRxBudget,
    TxQueued,
    TxQueueFull,
}

pub(crate) const STAGE_NAMES: [&str; 19] = [
    "xhci_rx_invalidate",
    "xhci_rx_copy",
    "xhci_rx_requeue",
    "xhci_tx_copy",
    "xhci_tx_clean",
    "ncm_parse",
    "ncm_enqueue",
    "ncm_queue_wait",
    "stack_dispatch",
    "tcp_receive",
    "tcp_drain",
    "ncm_tx_build",
    "rx_ntb",
    "rx_errors",
    "rx_queue_drops",
    "xhci_full_event_budget",
    "ncm_full_rx_budget",
    "tx_queued",
    "tx_queue_full",
];

#[repr(align(64))]
struct Counter {
    calls: AtomicU64,
    bytes: AtomicU64,
    capacity: AtomicU64,
    timed_calls: AtomicU64,
    timed_bytes: AtomicU64,
    timed_capacity: AtomicU64,
    timed_ns: AtomicU64,
}

impl Counter {
    const fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            capacity: AtomicU64::new(0),
            timed_calls: AtomicU64::new(0),
            timed_bytes: AtomicU64::new(0),
            timed_capacity: AtomicU64::new(0),
            timed_ns: AtomicU64::new(0),
        }
    }

    fn observe(&self, calls: u64, bytes: u64, capacity: u64) -> u64 {
        let ordinal = self.calls.fetch_add(calls, Ordering::Relaxed);
        if bytes != 0 {
            self.bytes.fetch_add(bytes, Ordering::Relaxed);
        }
        if capacity != 0 {
            self.capacity.fetch_add(capacity, Ordering::Relaxed);
        }
        ordinal
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static COUNTERS: [Counter; STAGE_NAMES.len()] = [const { Counter::new() }; STAGE_NAMES.len()];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub calls: u64,
    pub bytes: u64,
    pub capacity: u64,
    pub timed_calls: u64,
    pub timed_bytes: u64,
    pub timed_capacity: u64,
    pub timed_ns: u64,
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// One bounded write command, with no reset operation or partial commands.
pub(crate) fn command(bytes: &[u8]) -> Result<(), &'static str> {
    let value = match bytes {
        b"0" | b"0\n" => false,
        b"1" | b"1\n" => true,
        _ => return Err("net_profile: expected 0 or 1, optionally followed by newline"),
    };
    ENABLED.store(value, Ordering::Relaxed);
    Ok(())
}

/// A sampled wall-time span. No heap allocation, printing or profiling lock.
pub(crate) struct Span {
    counter: &'static Counter,
    start_ns: u64,
    bytes: u64,
    capacity: u64,
}

impl Drop for Span {
    fn drop(&mut self) {
        let elapsed = crate::timer::get_time_ns().saturating_sub(self.start_ns);
        self.counter.timed_ns.fetch_add(elapsed, Ordering::Relaxed);
        self.counter
            .timed_bytes
            .fetch_add(self.bytes, Ordering::Relaxed);
        self.counter
            .timed_capacity
            .fetch_add(self.capacity, Ordering::Relaxed);
        self.counter.timed_calls.fetch_add(1, Ordering::Relaxed);
    }
}

/// Count every enabled observation, but start a timer for only 1 in 16 calls.
/// `bytes` is the actual byte range; `capacity` describes full cache ranges.
#[inline]
pub(crate) fn begin(stage: Stage, bytes: usize, capacity: usize) -> Option<Span> {
    if !enabled() {
        return None;
    }
    let counter = &COUNTERS[stage as usize];
    let ordinal = counter.observe(1, bytes as u64, capacity as u64);
    if ordinal % SAMPLE_EVERY != 0 {
        return None;
    }
    Some(Span {
        counter,
        start_ns: crate::timer::get_time_ns(),
        bytes: bytes as u64,
        capacity: capacity as u64,
    })
}

/// Untimed events, such as budget exhaustion, queue drops and NTB completion.
#[inline]
pub(crate) fn event(stage: Stage, bytes: usize, capacity: usize) {
    if !enabled() {
        return;
    }
    COUNTERS[stage as usize].observe(1, bytes as u64, capacity as u64);
}

/// Count an already bounded batch of untimed events with one atomic update.
#[inline]
pub(crate) fn event_count(stage: Stage, calls: usize) {
    if !enabled() || calls == 0 {
        return;
    }
    COUNTERS[stage as usize].observe(calls as u64, 0, 0);
}

pub(crate) fn snapshot() -> [Counts; STAGE_NAMES.len()] {
    core::array::from_fn(|index| {
        let counter = &COUNTERS[index];
        Counts {
            calls: counter.calls.load(Ordering::Relaxed),
            bytes: counter.bytes.load(Ordering::Relaxed),
            capacity: counter.capacity.load(Ordering::Relaxed),
            timed_calls: counter.timed_calls.load(Ordering::Relaxed),
            timed_bytes: counter.timed_bytes.load(Ordering::Relaxed),
            timed_capacity: counter.timed_capacity.load(Ordering::Relaxed),
            timed_ns: counter.timed_ns.load(Ordering::Relaxed),
        }
    })
}
