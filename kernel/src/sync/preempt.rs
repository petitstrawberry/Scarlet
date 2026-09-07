//! Per-CPU preemption counter.
//!
//! `preempt_count > 0` marks the current CPU as non-preemptible. While the
//! count is elevated, the scheduler must not switch out the running task.
//! Locks own a `PreemptGuard` to prevent preemption for the duration of
//! their critical sections.
//!
//! # Current Status
//!
//! Scarlet still disables interrupts at trap entry, which already prevents
//! involuntary preemption. `preempt_count` becomes load-bearing once
//! interrupt-enabled trap handlers are introduced; today it documents the
//! "no schedule here" contract that lock holders rely on and gives the
//! scheduler a single predicate to check at safe boundaries.
//!
//! # Ordering
//!
//! Operations use `Relaxed` ordering. The counter is per-CPU and read by
//! the same CPU that writes it; cross-CPU synchronization of the count
//! itself is unnecessary. Reentrancy via interrupt is gated by IRQ state,
//! not by the count value.

use core::marker::PhantomData;
#[cfg(feature = "sync-debug")]
use core::panic::Location;
#[cfg(feature = "sync-debug")]
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::arch::try_get_cpuid;
use crate::environment::MAX_NUM_CPUS;

static PREEMPT_COUNT: [AtomicU32; MAX_NUM_CPUS] = [const { AtomicU32::new(0) }; MAX_NUM_CPUS];

/// Origin of a preemption guard, used by the optional sync diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum PreemptSourceKind {
    Explicit = 1,
    IrqGuard = 2,
    SpinLock = 3,
    IrqSpinLock = 4,
    RwSpinLockRead = 5,
    RwSpinLockWrite = 6,
    IrqRwSpinLockRead = 7,
    IrqRwSpinLockWrite = 8,
}

impl PreemptSourceKind {
    #[cfg(feature = "sync-debug")]
    fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            1 => Some(Self::Explicit),
            2 => Some(Self::IrqGuard),
            3 => Some(Self::SpinLock),
            4 => Some(Self::IrqSpinLock),
            5 => Some(Self::RwSpinLockRead),
            6 => Some(Self::RwSpinLockWrite),
            7 => Some(Self::IrqRwSpinLockRead),
            8 => Some(Self::IrqRwSpinLockWrite),
            _ => None,
        }
    }

    #[cfg(feature = "sync-debug")]
    fn name(self) -> &'static str {
        match self {
            Self::Explicit => "PreemptGuard",
            Self::IrqGuard => "IrqGuard",
            Self::SpinLock => "SpinLock",
            Self::IrqSpinLock => "IrqSpinLock",
            Self::RwSpinLockRead => "RwSpinLock(read)",
            Self::RwSpinLockWrite => "RwSpinLock(write)",
            Self::IrqRwSpinLockRead => "IrqRwSpinLock(read)",
            Self::IrqRwSpinLockWrite => "IrqRwSpinLock(write)",
        }
    }
}

#[cfg(feature = "sync-debug")]
const PREEMPT_DEBUG_SLOT_COUNT: usize = 32;
#[cfg(feature = "sync-debug")]
const DEBUG_SLOT_EMPTY: u8 = 0;
#[cfg(feature = "sync-debug")]
const DEBUG_SLOT_WRITING: u8 = 1;
#[cfg(feature = "sync-debug")]
const DEBUG_SLOT_ACTIVE: u8 = 2;

#[cfg(feature = "sync-debug")]
const DEBUG_PHASE_ACQUIRING: u8 = 1;
#[cfg(feature = "sync-debug")]
const DEBUG_PHASE_HELD: u8 = 2;
#[cfg(feature = "sync-debug")]
const DEBUG_PHASE_RELEASED: u8 = 3;

#[cfg(feature = "sync-debug")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreemptDebugPhase {
    Acquiring,
    Held,
    Released,
}

#[cfg(feature = "sync-debug")]
impl PreemptDebugPhase {
    fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            DEBUG_PHASE_ACQUIRING => Some(Self::Acquiring),
            DEBUG_PHASE_HELD => Some(Self::Held),
            DEBUG_PHASE_RELEASED => Some(Self::Released),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Acquiring => "acquiring",
            Self::Held => "held",
            Self::Released => "released",
        }
    }
}

#[cfg(feature = "sync-debug")]
#[derive(Clone, Copy, Debug)]
pub(crate) struct PreemptDebugSnapshot {
    pub(crate) source: PreemptSourceKind,
    pub(crate) phase: PreemptDebugPhase,
    pub(crate) lock_address: usize,
    pub(crate) task_id: usize,
    pub(crate) spin_iterations: u64,
    pub(crate) acquired_at_ns: u64,
    pub(crate) acquisition_pc: usize,
    pub(crate) acquisition_lr: usize,
    pub(crate) location: *const Location<'static>,
}

#[cfg(feature = "sync-debug")]
struct PreemptDebugSlot {
    state: AtomicU8,
    source: AtomicU8,
    lock_address: AtomicUsize,
    /// Snapshot of the acquiring task's id, captured at registration time.
    task_id: AtomicUsize,
    /// Lifecycle phase, written under the WRITING/ACTIVE sequence.
    phase: AtomicU8,
    /// Acquisition-attempt iterations sampled when the watchdog fires.
    spin_iterations: AtomicU64,
    /// Monotonic time at which acquisition completed.
    acquired_at_ns: AtomicU64,
    /// Instruction address sampled when acquisition completed.
    acquisition_pc: AtomicUsize,
    /// Link/return address sampled when acquisition completed.
    acquisition_lr: AtomicUsize,
    location: AtomicPtr<Location<'static>>,
}

#[cfg(feature = "sync-debug")]
impl PreemptDebugSlot {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(DEBUG_SLOT_EMPTY),
            source: AtomicU8::new(0),
            lock_address: AtomicUsize::new(0),
            task_id: AtomicUsize::new(0),
            phase: AtomicU8::new(0),
            spin_iterations: AtomicU64::new(0),
            acquired_at_ns: AtomicU64::new(0),
            acquisition_pc: AtomicUsize::new(0),
            acquisition_lr: AtomicUsize::new(0),
            location: AtomicPtr::new(core::ptr::null_mut()),
        }
    }
}

#[cfg(feature = "sync-debug")]
static PREEMPT_DEBUG_SLOTS: [[PreemptDebugSlot; PREEMPT_DEBUG_SLOT_COUNT]; MAX_NUM_CPUS] =
    [const { [const { PreemptDebugSlot::new() }; PREEMPT_DEBUG_SLOT_COUNT] }; MAX_NUM_CPUS];
#[cfg(feature = "sync-debug")]
static PREEMPT_DEBUG_UNTRACKED: [AtomicU32; MAX_NUM_CPUS] =
    [const { AtomicU32::new(0) }; MAX_NUM_CPUS];

#[cfg(feature = "sync-debug")]
fn register_preempt_source(
    cpu: usize,
    source: PreemptSourceKind,
    lock_address: usize,
    location: &'static Location<'static>,
) -> Option<u8> {
    for (index, slot) in PREEMPT_DEBUG_SLOTS[cpu].iter().enumerate() {
        if slot
            .state
            .compare_exchange(
                DEBUG_SLOT_EMPTY,
                DEBUG_SLOT_WRITING,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_err()
        {
            continue;
        }

        slot.source.store(source as u8, Ordering::Relaxed);
        slot.lock_address.store(lock_address, Ordering::Relaxed);
        slot.task_id.store(
            crate::sched::scheduler::current_task_id(cpu).unwrap_or(0),
            Ordering::Relaxed,
        );
        slot.phase.store(DEBUG_PHASE_ACQUIRING, Ordering::Relaxed);
        slot.spin_iterations.store(0, Ordering::Relaxed);
        slot.acquired_at_ns.store(0, Ordering::Relaxed);
        slot.acquisition_pc.store(0, Ordering::Relaxed);
        slot.acquisition_lr.store(0, Ordering::Relaxed);
        slot.location.store(
            location as *const Location<'static> as *mut Location<'static>,
            Ordering::Relaxed,
        );
        slot.state.store(DEBUG_SLOT_ACTIVE, Ordering::Release);
        return Some(index as u8);
    }

    PREEMPT_DEBUG_UNTRACKED[cpu].fetch_add(1, Ordering::Relaxed);
    None
}

#[cfg(feature = "sync-debug")]
fn unregister_preempt_source(cpu: usize, slot_index: Option<u8>) {
    let Some(slot_index) = slot_index else {
        let previous = PREEMPT_DEBUG_UNTRACKED[cpu].fetch_sub(1, Ordering::Relaxed);
        debug_assert!(previous > 0);
        return;
    };

    let slot = &PREEMPT_DEBUG_SLOTS[cpu][slot_index as usize];
    slot.phase.store(DEBUG_PHASE_RELEASED, Ordering::Relaxed);
    let previous = slot.state.swap(DEBUG_SLOT_EMPTY, Ordering::AcqRel);
    debug_assert_eq!(previous, DEBUG_SLOT_ACTIVE);
}

/// Publish a stable snapshot of one diagnostic slot, or `None` when the slot
/// is empty or being republished.
#[cfg(feature = "sync-debug")]
fn snapshot_debug_slot(cpu: usize, slot_index: usize) -> Option<PreemptDebugSnapshot> {
    if cpu >= MAX_NUM_CPUS || slot_index >= PREEMPT_DEBUG_SLOT_COUNT {
        return None;
    }
    let slot = &PREEMPT_DEBUG_SLOTS[cpu][slot_index];
    if slot.state.load(Ordering::Acquire) != DEBUG_SLOT_ACTIVE {
        return None;
    }
    let source = PreemptSourceKind::from_raw(slot.source.load(Ordering::Relaxed))?;
    let phase = PreemptDebugPhase::from_raw(slot.phase.load(Ordering::Acquire))
        .unwrap_or(PreemptDebugPhase::Acquiring);
    let snapshot = PreemptDebugSnapshot {
        source,
        phase,
        lock_address: slot.lock_address.load(Ordering::Relaxed),
        task_id: slot.task_id.load(Ordering::Relaxed),
        spin_iterations: slot.spin_iterations.load(Ordering::Relaxed),
        acquired_at_ns: slot.acquired_at_ns.load(Ordering::Relaxed),
        acquisition_pc: slot.acquisition_pc.load(Ordering::Relaxed),
        acquisition_lr: slot.acquisition_lr.load(Ordering::Relaxed),
        location: slot.location.load(Ordering::Relaxed),
    };
    if slot.state.load(Ordering::Acquire) == DEBUG_SLOT_ACTIVE {
        Some(snapshot)
    } else {
        None
    }
}

#[inline]
fn current_cpu() -> Option<usize> {
    try_get_cpuid()
}

#[inline]
fn increment_preempt_count(cpu: usize) {
    let previous = PREEMPT_COUNT[cpu]
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            count.checked_add(1)
        })
        .expect("preempt_count overflow");
    debug_assert!(previous < u32::MAX);
}

#[inline]
fn decrement_preempt_count(cpu: usize) {
    let previous = PREEMPT_COUNT[cpu]
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            count.checked_sub(1)
        })
        .expect("preempt_count underflow");
    debug_assert!(previous > 0);
}

/// Return the current CPU's preemption counter.
///
/// # Returns
///
/// The current preempt count for the executing CPU, or `0` if the per-CPU
/// substrate has not been published yet (early boot, before
/// `sscratch`/`TPIDR_EL1` is programmed).
#[inline]
pub fn preempt_count() -> u32 {
    match current_cpu() {
        Some(cpu) => PREEMPT_COUNT[cpu].load(Ordering::Relaxed),
        None => 0,
    }
}

/// Return whether the current CPU may be preempted.
///
/// # Returns
///
/// `true` when the preempt count is zero. Also `true` while the per-CPU
/// substrate is uninitialized, since lock operations are no-ops in that
/// window.
#[inline]
pub fn preemptible() -> bool {
    preempt_count() == 0
}

/// Increment the preempt count on the current CPU.
///
/// Must be paired with [`preempt_enable`]. Prefer [`PreemptGuard::new`] for
/// RAII safety. No-op when the per-CPU substrate has not been published,
/// so callers on an uninitialized CPU behave as plain busy-wait spinlocks.
#[inline]
pub fn preempt_disable() {
    if let Some(cpu) = current_cpu() {
        increment_preempt_count(cpu);
    }
}

/// Decrement the preempt count on the current CPU.
///
/// # Panics
///
/// Panics on underflow to catch unbalanced
/// `preempt_disable`/`preempt_enable` use. No-op when the per-CPU substrate
/// has not been published.
#[inline]
pub fn preempt_enable() {
    if let Some(cpu) = current_cpu() {
        decrement_preempt_count(cpu);
    }
}

// Always-on lightweight spin-contention watchdog.
//
// The counter is CPU-local and is reset when a tracked lock acquisition starts
// or completes. With `sync-debug`, a threshold report looks up the current
// CPU's `acquiring` slot and then reports only `held` slots for the exact same
// lock address. The report path uses no heap allocation and bypasses the normal
// print lock, so it remains safe while the allocator or print infrastructure is
// the contended resource.
//
// This intentionally does not panic: a transient burst under heavy USB
// storage contention can legitimately spin for millions of iterations. The
// goal is observability, not a hard timeout that could turn a slow device
// into a kernel panic.

const SPIN_CONTENTION_REPORT_THRESHOLD: u64 = 1 << 22;

static SPIN_CONTENTION_COUNT: [AtomicU64; MAX_NUM_CPUS] =
    [const { AtomicU64::new(0) }; MAX_NUM_CPUS];

/// Note one busy-wait iteration in a lock's spin loop.
///
/// Lock implementations call this instead of `core::hint::spin_loop()` so that
/// a stuck owner becomes observable without enabling `sync-debug`.
#[inline(always)]
pub fn note_spin_contention() {
    let Some(cpu) = current_cpu() else {
        core::hint::spin_loop();
        return;
    };
    let count = SPIN_CONTENTION_COUNT[cpu].fetch_add(1, Ordering::Relaxed) + 1;
    if count == SPIN_CONTENTION_REPORT_THRESHOLD {
        report_spin_contention(cpu, count);
        SPIN_CONTENTION_COUNT[cpu].store(0, Ordering::Relaxed);
    }
    core::hint::spin_loop();
}

#[cfg(feature = "sync-debug")]
fn emergency_print_waiter(cpu: usize, snapshot: PreemptDebugSnapshot, spin_count: u64) {
    if snapshot.location.is_null() {
        crate::emergency_println!(
            "[sync-watchdog] waiter_cpu={} kind={} lock={:#x} task={} spins={} at <unknown>",
            cpu,
            snapshot.source.name(),
            snapshot.lock_address,
            snapshot.task_id,
            spin_count,
        );
        return;
    }

    // SAFETY: `Location::caller()` returns a reference with static lifetime.
    let location = unsafe { &*snapshot.location };
    crate::emergency_println!(
        "[sync-watchdog] waiter_cpu={} kind={} lock={:#x} task={} spins={} at {}:{}:{}",
        cpu,
        snapshot.source.name(),
        snapshot.lock_address,
        snapshot.task_id,
        spin_count,
        location.file(),
        location.line(),
        location.column(),
    );
}

#[cfg(feature = "sync-debug")]
fn emergency_print_holder(cpu: usize, snapshot: PreemptDebugSnapshot, now_ns: u64) {
    let held_ns = if snapshot.acquired_at_ns == 0 {
        0
    } else {
        now_ns.saturating_sub(snapshot.acquired_at_ns)
    };
    if snapshot.location.is_null() {
        crate::emergency_println!(
            "[sync-watchdog]   owner_cpu={} kind={} lock={:#x} task={} held_ns={} acquire_pc={:#x} acquire_lr={:#x} at <unknown>",
            cpu,
            snapshot.source.name(),
            snapshot.lock_address,
            snapshot.task_id,
            held_ns,
            snapshot.acquisition_pc,
            snapshot.acquisition_lr,
        );
        return;
    }

    // SAFETY: `Location::caller()` returns a reference with static lifetime.
    let location = unsafe { &*snapshot.location };
    crate::emergency_println!(
        "[sync-watchdog]   owner_cpu={} kind={} lock={:#x} task={} held_ns={} acquire_pc={:#x} acquire_lr={:#x} at {}:{}:{}",
        cpu,
        snapshot.source.name(),
        snapshot.lock_address,
        snapshot.task_id,
        held_ns,
        snapshot.acquisition_pc,
        snapshot.acquisition_lr,
        location.file(),
        location.line(),
        location.column(),
    );
}

#[cfg(feature = "sync-debug")]
fn emergency_print_peer_waiter(cpu: usize, snapshot: PreemptDebugSnapshot) {
    if snapshot.location.is_null() {
        crate::emergency_println!(
            "[sync-watchdog]   peer_waiter_cpu={} kind={} lock={:#x} task={} spins={} at <unknown>",
            cpu,
            snapshot.source.name(),
            snapshot.lock_address,
            snapshot.task_id,
            snapshot.spin_iterations,
        );
        return;
    }

    // SAFETY: `Location::caller()` returns a reference with static lifetime.
    let location = unsafe { &*snapshot.location };
    crate::emergency_println!(
        "[sync-watchdog]   peer_waiter_cpu={} kind={} lock={:#x} task={} spins={} at {}:{}:{}",
        cpu,
        snapshot.source.name(),
        snapshot.lock_address,
        snapshot.task_id,
        snapshot.spin_iterations,
        location.file(),
        location.line(),
        location.column(),
    );
}

#[cfg(feature = "sync-debug")]
fn report_tracked_lock_contention(
    waiter_cpu: usize,
    waiter_slot_index: usize,
    waiter: PreemptDebugSnapshot,
    spin_count: u64,
) {
    emergency_print_waiter(waiter_cpu, waiter, spin_count);

    let now_ns = crate::timer::get_time_ns();
    let mut holder_count = 0usize;
    let mut peer_waiter_count = 0usize;
    for target_cpu in 0..MAX_NUM_CPUS {
        for slot_index in 0..PREEMPT_DEBUG_SLOT_COUNT {
            let Some(snapshot) = snapshot_debug_slot(target_cpu, slot_index) else {
                continue;
            };
            if snapshot.lock_address != waiter.lock_address {
                continue;
            }
            match snapshot.phase {
                PreemptDebugPhase::Held => {
                    holder_count += 1;
                    emergency_print_holder(target_cpu, snapshot, now_ns);
                }
                PreemptDebugPhase::Acquiring
                    if target_cpu != waiter_cpu || slot_index != waiter_slot_index =>
                {
                    peer_waiter_count += 1;
                    emergency_print_peer_waiter(target_cpu, snapshot);
                }
                PreemptDebugPhase::Acquiring | PreemptDebugPhase::Released => {}
            }
        }
    }

    if holder_count == 0 {
        crate::emergency_println!(
            "[sync-watchdog]   owner unavailable for lock={:#x} (untracked, released during snapshot, or in handoff)",
            waiter.lock_address,
        );
    }
    if peer_waiter_count != 0 {
        crate::emergency_println!(
            "[sync-watchdog]   matching peer waiters={}",
            peer_waiter_count,
        );
    }
}

#[cfg(feature = "sync-debug")]
fn emergency_print_active_slot(cpu: usize, snapshot: PreemptDebugSnapshot) {
    if snapshot.location.is_null() {
        crate::emergency_println!(
            "[sync-watchdog]   active_cpu={} kind={} phase={} lock={:#x} task={} spins={} at <unknown>",
            cpu,
            snapshot.source.name(),
            snapshot.phase.name(),
            snapshot.lock_address,
            snapshot.task_id,
            snapshot.spin_iterations,
        );
        return;
    }

    // SAFETY: `Location::caller()` returns a reference with static lifetime.
    let location = unsafe { &*snapshot.location };
    crate::emergency_println!(
        "[sync-watchdog]   active_cpu={} kind={} phase={} lock={:#x} task={} spins={} at {}:{}:{}",
        cpu,
        snapshot.source.name(),
        snapshot.phase.name(),
        snapshot.lock_address,
        snapshot.task_id,
        snapshot.spin_iterations,
        location.file(),
        location.line(),
        location.column(),
    );
}

#[cold]
fn report_spin_contention(cpu: usize, spin_count: u64) {
    crate::emergency_println!(
        "[sync-watchdog] spin contention on cpu={} preempt_count={} spins={} (possible lock stall)",
        cpu,
        preempt_count(),
        spin_count,
    );

    #[cfg(feature = "sync-debug")]
    {
        let mut tracked_waiters = 0usize;
        for slot_index in 0..PREEMPT_DEBUG_SLOT_COUNT {
            let slot = &PREEMPT_DEBUG_SLOTS[cpu][slot_index];
            let Some(mut waiter) = snapshot_debug_slot(cpu, slot_index) else {
                continue;
            };
            if waiter.phase != PreemptDebugPhase::Acquiring || waiter.lock_address == 0 {
                continue;
            }
            slot.spin_iterations.store(spin_count, Ordering::Relaxed);
            waiter.spin_iterations = spin_count;
            tracked_waiters += 1;
            report_tracked_lock_contention(cpu, slot_index, waiter, spin_count);
        }

        if tracked_waiters == 0 {
            crate::emergency_println!(
                "[sync-watchdog] no tracked lock waiter on reporting CPU; active slots are diagnostic context, not owners"
            );
            for target_cpu in 0..MAX_NUM_CPUS {
                for slot_index in 0..PREEMPT_DEBUG_SLOT_COUNT {
                    if let Some(snapshot) = snapshot_debug_slot(target_cpu, slot_index) {
                        emergency_print_active_slot(target_cpu, snapshot);
                    }
                }
                let untracked = PREEMPT_DEBUG_UNTRACKED[target_cpu].load(Ordering::Relaxed);
                if untracked != 0 {
                    crate::emergency_println!(
                        "[sync-watchdog]   active_cpu={} untracked_guard_count={}",
                        target_cpu,
                        untracked,
                    );
                }
            }
        }
    }
}

/// Reset the always-on spin-contention counter for the current CPU.
#[cfg(test)]
pub(crate) fn reset_spin_contention_for_test() {
    if let Some(cpu) = current_cpu() {
        SPIN_CONTENTION_COUNT[cpu].store(0, Ordering::Relaxed);
    }
}

/// RAII guard that disables preemption while alive.
///
/// Dropping the guard restores the previous preempt state. The guard is
/// `!Send` because preemption state is per-CPU.
///
/// # Examples
///
/// ```
/// let _guard = PreemptGuard::new();
/// // Preemption is disabled for the duration of this scope.
/// ```
pub struct PreemptGuard {
    cpu: Option<usize>,
    #[cfg(feature = "sync-debug")]
    debug_slot: Option<u8>,
    _not_send: PhantomData<*mut ()>,
}

impl PreemptGuard {
    /// Disable preemption on the current CPU and return a guard that
    /// re-enables it on drop.
    ///
    /// # Returns
    ///
    /// A guard bound to the current CPU. If the per-CPU substrate has not
    /// been published yet, the guard remains unarmed and requires that state
    /// to stay uninitialized until it is dropped.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn new() -> Self {
        let guard = Self::new_with_source(PreemptSourceKind::Explicit, 0);
        guard.mark_acquired();
        guard
    }

    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub(crate) fn new_with_source(source: PreemptSourceKind, lock_address: usize) -> Self {
        let cpu = current_cpu();
        if let Some(cpu) = cpu {
            increment_preempt_count(cpu);
            if lock_address != 0 {
                SPIN_CONTENTION_COUNT[cpu].store(0, Ordering::Relaxed);
            }
        }
        #[cfg(feature = "sync-debug")]
        let caller = Location::caller();
        #[cfg(feature = "sync-debug")]
        let debug_slot =
            cpu.and_then(|cpu| register_preempt_source(cpu, source, lock_address, caller));
        #[cfg(not(feature = "sync-debug"))]
        let _ = (source, lock_address);
        Self {
            cpu,
            #[cfg(feature = "sync-debug")]
            debug_slot,
            _not_send: PhantomData,
        }
    }

    /// Publish that atomic acquisition succeeded. After this the slot reports
    /// the `held` phase, so a stall report distinguishes a live owner from a
    /// waiter.
    #[inline]
    pub(crate) fn mark_acquired(&self) {
        if let Some(cpu) = self.cpu {
            SPIN_CONTENTION_COUNT[cpu].store(0, Ordering::Relaxed);
        }
        #[cfg(feature = "sync-debug")]
        if let (Some(cpu), Some(slot_index)) = (self.cpu, self.debug_slot) {
            let acquired_at_ns = crate::timer::get_time_ns();
            let (acquisition_pc, acquisition_lr) =
                crate::arch::instruction::capture_execution_site();
            let slot = &PREEMPT_DEBUG_SLOTS[cpu][slot_index as usize];
            slot.acquired_at_ns.store(acquired_at_ns, Ordering::Relaxed);
            slot.acquisition_pc.store(acquisition_pc, Ordering::Relaxed);
            slot.acquisition_lr.store(acquisition_lr, Ordering::Relaxed);
            slot.phase.store(DEBUG_PHASE_HELD, Ordering::Release);
        }
    }
}

impl Default for PreemptGuard {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PreemptGuard {
    #[inline]
    fn drop(&mut self) {
        match self.cpu {
            Some(cpu) => {
                assert_eq!(
                    current_cpu(),
                    Some(cpu),
                    "PreemptGuard dropped on a different CPU"
                );
                #[cfg(feature = "sync-debug")]
                unregister_preempt_source(cpu, self.debug_slot.take());
                decrement_preempt_count(cpu);
            }
            None => {
                assert!(
                    current_cpu().is_none(),
                    "unarmed PreemptGuard crossed into initialized per-CPU state"
                );
            }
        }
    }
}

/// Print every live preemption guard on the current CPU.
///
/// With `sync-debug` disabled this compiles to a no-op. The diagnostic path
/// does not allocate or acquire the print lock, and tolerates guards being
/// dropped out of acquisition order.
#[inline]
pub fn dump_active_preempt_guards() {
    #[cfg(feature = "sync-debug")]
    {
        let Some(cpu) = current_cpu() else {
            crate::emergency_println!("[sync-debug] per-CPU state is not initialized");
            return;
        };
        let count = PREEMPT_COUNT[cpu].load(Ordering::Relaxed);
        let untracked = PREEMPT_DEBUG_UNTRACKED[cpu].load(Ordering::Relaxed);
        let now_ns = crate::timer::get_time_ns();
        crate::emergency_println!(
            "[sync-debug] cpu={} preempt_count={} active guard(s):",
            cpu,
            count
        );

        let mut tracked = 0u32;
        for slot_index in 0..PREEMPT_DEBUG_SLOT_COUNT {
            let Some(snapshot) = snapshot_debug_slot(cpu, slot_index) else {
                continue;
            };
            tracked = tracked.saturating_add(1);
            let held_ns =
                if snapshot.phase == PreemptDebugPhase::Held && snapshot.acquired_at_ns != 0 {
                    now_ns.saturating_sub(snapshot.acquired_at_ns)
                } else {
                    0
                };
            if snapshot.location.is_null() {
                crate::emergency_println!(
                    "[sync-debug]   slot={} kind={} phase={} lock={:#x} task={} spins={} held_ns={} acquire_pc={:#x} acquire_lr={:#x} at <unknown>",
                    slot_index,
                    snapshot.source.name(),
                    snapshot.phase.name(),
                    snapshot.lock_address,
                    snapshot.task_id,
                    snapshot.spin_iterations,
                    held_ns,
                    snapshot.acquisition_pc,
                    snapshot.acquisition_lr,
                );
                continue;
            }

            // SAFETY: `Location::caller()` returns a reference with static
            // lifetime.
            let location = unsafe { &*snapshot.location };
            crate::emergency_println!(
                "[sync-debug]   slot={} kind={} phase={} lock={:#x} task={} spins={} held_ns={} acquire_pc={:#x} acquire_lr={:#x} at {}:{}:{}",
                slot_index,
                snapshot.source.name(),
                snapshot.phase.name(),
                snapshot.lock_address,
                snapshot.task_id,
                snapshot.spin_iterations,
                held_ns,
                snapshot.acquisition_pc,
                snapshot.acquisition_lr,
                location.file(),
                location.line(),
                location.column(),
            );
        }

        if untracked != 0 || count != tracked.saturating_add(untracked) {
            crate::emergency_println!(
                "[sync-debug]   tracked={} untracked={} count_delta={}",
                tracked,
                untracked,
                count as i64 - tracked.saturating_add(untracked) as i64
            );
        }
    }
}

#[cfg(test)]
impl PreemptGuard {
    /// Test-only helper to reset the current CPU's preempt count.
    ///
    /// Callers must ensure no other `PreemptGuard` is live on this CPU.
    pub(crate) fn reset_count_for_test() {
        if let Some(cpu) = current_cpu() {
            #[cfg(feature = "sync-debug")]
            {
                for slot in &PREEMPT_DEBUG_SLOTS[cpu] {
                    assert_eq!(
                        slot.state.load(Ordering::Relaxed),
                        DEBUG_SLOT_EMPTY,
                        "cannot reset live preemption diagnostic state"
                    );
                }
                assert_eq!(
                    PREEMPT_DEBUG_UNTRACKED[cpu].load(Ordering::Relaxed),
                    0,
                    "cannot reset untracked live preemption guards"
                );
            }
            PREEMPT_COUNT[cpu].store(0, Ordering::Relaxed);
            SPIN_CONTENTION_COUNT[cpu].store(0, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_preempt_count_starts_zero() {
        PreemptGuard::reset_count_for_test();
        assert_eq!(preempt_count(), 0);
        assert!(preemptible());
    }

    #[test_case]
    fn test_guard_increments_and_decrements() {
        PreemptGuard::reset_count_for_test();
        assert_eq!(preempt_count(), 0);
        {
            let _g = PreemptGuard::new();
            assert_eq!(preempt_count(), 1);
            assert!(!preemptible());
        }
        assert_eq!(preempt_count(), 0);
        assert!(preemptible());
    }

    #[test_case]
    fn test_nested_guards_accumulate() {
        PreemptGuard::reset_count_for_test();
        {
            let _a = PreemptGuard::new();
            assert_eq!(preempt_count(), 1);
            {
                let _b = PreemptGuard::new();
                assert_eq!(preempt_count(), 2);
            }
            assert_eq!(preempt_count(), 1);
        }
        assert_eq!(preempt_count(), 0);
    }

    #[test_case]
    fn test_non_lifo_guards_decrement_their_acquisition_cpu() {
        PreemptGuard::reset_count_for_test();

        let first = PreemptGuard::new();
        let second = PreemptGuard::new();
        assert_eq!(preempt_count(), 2);

        drop(first);
        assert_eq!(preempt_count(), 1);

        drop(second);
        assert_eq!(preempt_count(), 0);
    }

    #[test_case]
    fn test_explicit_disable_enable_balance() {
        PreemptGuard::reset_count_for_test();
        preempt_disable();
        preempt_disable();
        assert_eq!(preempt_count(), 2);
        preempt_enable();
        preempt_enable();
        assert_eq!(preempt_count(), 0);
    }

    #[cfg(feature = "sync-debug")]
    #[test_case]
    fn test_lock_debug_slot_transitions_from_acquiring_to_held() {
        PreemptGuard::reset_count_for_test();
        let cpu = current_cpu().expect("test CPU identity must be initialized");
        let guard = PreemptGuard::new_with_source(PreemptSourceKind::SpinLock, 0x1234);
        let slot_index = guard
            .debug_slot
            .expect("sync-debug test must obtain a diagnostic slot");

        let acquiring = snapshot_debug_slot(cpu, slot_index as usize).unwrap();
        assert_eq!(acquiring.phase, PreemptDebugPhase::Acquiring);
        assert_eq!(acquiring.lock_address, 0x1234);

        guard.mark_acquired();
        let held = snapshot_debug_slot(cpu, slot_index as usize).unwrap();
        assert_eq!(held.phase, PreemptDebugPhase::Held);
        assert_ne!(held.acquisition_pc, 0);

        drop(guard);
        assert!(snapshot_debug_slot(cpu, slot_index as usize).is_none());
    }
}
