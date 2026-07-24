//! Per-CPU lock-free breadcrumb diagnostics for SMP hang localization.
//!
//! When a CPU hangs with FIQ masked it cannot print. Each CPU therefore records
//! its last execution phase in a lock-free atomic. Surviving CPUs periodically
//! sample a stopped CPU's latest breadcrumb from a timer heartbeat, revealing
//! where that CPU stopped and whether the state is still changing.
//!
//! Each record is enclosed by an odd/even sequence generation. Readers accept
//! the phase and context fields only when the generation remains unchanged and
//! even. The path uses no locks or formatting, so it remains safe from FIQ
//! handlers and trap-vector prologues reached after `daifset`.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::get_cpu;
use crate::environment::MAX_NUM_CPUS;
// Human-readable hex phase codes (ASCII pairs) for quick log reading.
pub const NONE: u64 = 0x0000;
pub const FORK_RETURN: u64 = 0x4652; // 'FR' sys_clone returned to parent
pub const SWITCH_TO_USER: u64 = 0x5355; // 'SU' about to eret to userspace
pub const USER_TRAP_ENTER: u64 = 0x5554; // 'UT' entered user-trap handler
pub const DATA_FAULT_ENTER: u64 = 0x4446; // 'DF' COW/data fault handler entered
pub const DATA_FAULT_DONE: u64 = 0x4444; // 'DD' fault resolved
pub const SCHED_ENTER: u64 = 0x5343; // 'SC' schedule() entered
pub const PICK_NEXT_ENTER: u64 = 0x504e; // 'PN' pick_next() entered
pub const PICK_GUARD_DONE: u64 = 0x5047; // 'PG' pick_next IrqGuard acquired
pub const PICK_RELEASE_DONE: u64 = 0x5052; // 'PR' deferred previous task released
pub const PICK_OLD_DONE: u64 = 0x504f; // 'PO' current task lookup completed
pub const PICK_QUEUE_DONE: u64 = 0x5051; // 'PQ' local ready-queue pop completed
pub const PICK_STEAL_BEGIN: u64 = 0x5053; // 'PS' work stealing about to begin
pub const PICK_STEAL_DONE: u64 = 0x5044; // 'PD' work stealing completed
pub const KCTX_ENTER: u64 = 0x4b45; // 'KE' kernel_context_switch entered
pub const KCTX_SWITCH_TO: u64 = 0x4b53; // 'KS' about to switch_to
pub const KCTX_RESUME: u64 = 0x4b52; // 'KR' resumed after switch_to
pub const TIMER_TICK: u64 = 0x5454; // 'TT' timer FIQ serviced
pub const TIMER_SW_TIMERS: u64 = 0x5453; // 'TS' check_software_timers
pub const CLONE_ENTER: u64 = 0x434c; // 'CL' clone_task entered
pub const CLONE_KSTACK_DONE: u64 = 0x434b; // 'CK' child kstack window mapped
pub const CLONE_RETURN: u64 = 0x4352; // 'CR' clone_task returning
pub const SYSCALL_ENTER: u64 = 0x5359; // 'SY' syscall_dispatcher entered (aux=num)
pub const SYSCALL_TASK_DONE: u64 = 0x5354; // 'ST' entering ABI syscall (aux=task id, aux2=syscall)
pub const SYSCALL_ABI_DONE: u64 = 0x534f; // 'SO' ABI syscall returned (aux=task id, aux2=syscall)
pub const SYSCALL_EXIT: u64 = 0x5358; // 'SX' syscall_dispatcher returned
pub const EPOLL_ALLOC_BEGIN: u64 = 0x4542; // 'EB' epoll_create1 about to allocate an fd
pub const EPOLL_ALLOC_DONE: u64 = 0x4544; // 'ED' epoll_create1 fd allocation returned
pub const REL_PREV_DONE: u64 = 0x5250; // 'RP' release_deferred_prev done after resume
pub const GETTASK_DONE: u64 = 0x4754; // 'GT' get_task(from) done after resume
pub const SETUP_DONE: u64 = 0x5344; // 'SD' setup_task_cpu_state done after resume
pub const VCPU_LOCK_BEGIN: u64 = 0x564c; // 'VL' about to lock/restore resumed task vCPU
pub const VCPU_LOCK_DONE: u64 = 0x5644; // 'VD' resumed task vCPU restore completed
pub const TRAMP_GET: u64 = 0x5447; // 'TG' get_trampoline_arch returned (aux=trampoline addr)
pub const SET_ARCH_DONE: u64 = 0x5341; // 'SA' set_arch (TPIDR_EL1 write) done
pub const KFAULT: u64 = 0x4b46; // 'KF' kernel fault hit DataAbortSameEl/InstrAbortSameEl/Unhandled (aux=FAR aux2=ESR)
pub const LAZY_FOUND: u64 = 0x464e; // 'FN' lazy_map_page_with found the memmap
pub const LAZY_COWDONE: u64 = 0x4344; // 'CD' COW copy + add_memory_map_fixed done (pre root_pagetable.map)
pub const LAZY_RESOLVED: u64 = 0x5246; // 'RF' owner.resolve_fault returned Ok (aux=paddr_page_base)
pub const PMM_ALLOC_DONE: u64 = 0x5041; // 'PA' ContiguousPages::new (PMM) done (aux=new_paddr)
pub const COW_COPY_REQUIRED: u64 = 0x4343; // 'CC' COW store requires a private copy
pub const PMM_ALLOC_BEGIN: u64 = 0x504d; // 'PM' about to allocate the private COW page
pub const SETSP_DONE: u64 = 0x5350; // 'SP' set_kernel_stack done
pub const SETTH_DONE: u64 = 0x5448; // 'TH' set_trap_handler done
pub const SETAS_ASID_DONE: u64 = 0x4144; // 'AD' vm_manager.get_asid returned (aux=ASID)
pub const SETAS_ROOT_DONE: u64 = 0x4152; // 'AR' root page-table lookup done; cache clean next
pub const PT_LOCK_WAIT: u64 = 0x4c57; // 'LW' waiting for the ASID page-table lock
pub const PT_LOCK_DONE: u64 = 0x4c44; // 'LD' acquired the ASID page-table lock
pub const PT_LOCK_RELEASED: u64 = 0x4c52; // 'LR' released the ASID page-table lock
pub const PT_ASID_READ_WAIT: u64 = 0x4157; // 'AW' waiting for ASID bitmap read access
pub const PT_ASID_READ_DONE: u64 = 0x4145; // 'AE' ASID bitmap read completed
pub const PT_ASID_WRITE_WAIT: u64 = 0x5957; // 'YW' waiting for ASID bitmap write access
pub const PT_ASID_WRITE_HELD: u64 = 0x5948; // 'YH' acquired ASID bitmap write access
pub const PT_ASID_WRITE_RELEASED: u64 = 0x5958; // 'YX' released ASID bitmap write access
pub const PT_REGISTRY_READ_WAIT: u64 = 0x5157; // 'QW' waiting for PAGE_TABLES read access
pub const PT_REGISTRY_READ_DONE: u64 = 0x5144; // 'QD' PAGE_TABLES read completed
pub const PT_MAP_BEGIN: u64 = 0x4d42; // 'MB' entering single-page map
pub const PT_LEAF_DONE: u64 = 0x4d4c; // 'ML' leaf PTE installed and published
pub const PT_TLBI_BEGIN: u64 = 0x5442; // 'TB' broadcast stage-1 TLBI about to begin
pub const PT_TLBI_DONE: u64 = 0x5444; // 'TD' broadcast stage-1 TLBI completed
pub const PT_MAP_DONE: u64 = 0x4d44; // 'MD' single-page map returned
pub const PT_ALLOC_BEGIN: u64 = 0x4142; // 'AB' child page-table allocation about to begin
pub const PT_ALLOC_DONE: u64 = 0x4143; // 'AC' child page-table allocation completed
pub const PT_REGISTRY_WAIT: u64 = 0x5257; // 'RW' waiting for PAGE_TABLES registry write lock
pub const PT_REGISTRY_HELD: u64 = 0x5248; // 'RH' acquired PAGE_TABLES registry write lock
pub const PT_REGISTRY_DONE: u64 = 0x5244; // 'RD' child page table recorded in registry
pub const PT_REGISTRY_RELEASED: u64 = 0x5258; // 'RX' released PAGE_TABLES registry write lock
pub const VMM_READ_WAIT: u64 = 0x5652; // 'VR' get_asid blocked by an active inner writer (aux=writer count, aux2=lock address)
pub const VMM_WRITE_HELD: u64 = 0x5657; // 'VW' inner write lock acquired (aux=site code, aux2=lock address)
pub const KERNEL_IRQ_ENTER: u64 = 0x4951; // 'IQ' IRQ interrupted non-idle privileged code (aux=ELR, aux2=SPSR)
pub const KERNEL_FIQ_ENTER: u64 = 0x4651; // 'FQ' FIQ interrupted non-idle privileged code (aux=ELR, aux2=SPSR)
pub const FP_TRAP_ENTER: u64 = 0x4645; // 'FE' FP/SIMD trap entered (aux=ELR, aux2=SPSR)
pub const FP_TASK_LOOKUP: u64 = 0x464c; // 'FL' looking up current FP task (aux=CPU id)
pub const FP_TASK_FOUND: u64 = 0x4654; // 'FT' current FP task found (aux=task id)
pub const FP_VCPU_LOCKED: u64 = 0x4656; // 'FV' task vCPU locked and marked used (aux=task id)
pub const FP_ACCESS_ENABLED: u64 = 0x4641; // 'FA' FP access enabled (aux=task id, aux2=enabled)
pub const FP_RESTORE_BEGIN: u64 = 0x4642; // 'FB' FP context restore about to begin (aux=task id)
pub const FP_CONTROL_DONE: u64 = 0x4643; // 'FC' FPCR/FPSR restore completed (aux=task id)
pub const FP_RESTORE_DONE: u64 = 0x4644; // 'FD' FP context restore completed (aux=task id)
pub const FP_VECTOR_BEGIN: u64 = 0x5642; // 'VB' vector-register restore about to begin (aux=task id)
pub const FP_VECTOR_DONE: u64 = 0x5646; // 'VF' vector-register restore completed (aux=task id)
pub const IPI_SEND_DONE: u64 = 0x4944; // 'ID' send_ipi controller returned (aux=target CPU, aux2=status)
pub const ENQUEUE_IRQ_RESTORED: u64 = 0x4552; // 'ER' enqueue IrqGuard restored DAIF (aux=task, aux2=target CPU)
pub const FAST_CLAIM_DONE: u64 = 0x4653; // 'FS' claim_fast_interrupt controller returned (aux=CPU, aux2=status)
pub const EXIT_ENTER: u64 = 0x5845; // 'XE' task exit entered (aux=task id, aux2=status)
pub const EXIT_HANDLES_DONE: u64 = 0x5848; // 'XH' exit handle cleanup completed (aux=task id)
pub const EXIT_ABI_DONE: u64 = 0x5841; // 'XA' ABI exit hook completed (aux=task id)
pub const EXIT_VM_BEGIN: u64 = 0x5842; // 'XB' exit VM teardown entered (aux=task id, aux2=map count)
pub const EXIT_VM_UNMAPPED: u64 = 0x5855; // 'XU' exit page table unmap completed (aux=task id, aux2=map count)
pub const EXIT_VM_DONE: u64 = 0x5844; // 'XD' exit VM backing reclaim completed (aux=task id)
pub const EXIT_REPARENT_DONE: u64 = 0x5852; // 'XR' child reparenting completed (aux=task id)
pub const EXIT_STATE_DONE: u64 = 0x5853; // 'XS' terminal task state published (aux=task id, aux2=state code)
pub const EXIT_SCHEDULE: u64 = 0x5843; // 'XC' exiting current task about to schedule (aux=task id)
pub const RELEASE_PREV_ENTER: u64 = 0x5245; // 'RE' deferred previous-task release entered (aux=task id, aux2=CPU)
pub const RELEASE_PREV_DONE: u64 = 0x5259; // 'RY' deferred previous-task release completed (aux=task id, aux2=state code)
pub const ZOMBIE_FINALIZE: u64 = 0x5a46; // 'ZF' zombie finalization entered (aux=task id, aux2=parent id)
pub const ZOMBIE_CLEANUP: u64 = 0x5a43; // 'ZC' terminated-task cleanup entered (aux=task id)
pub const REAPER_DROP_BEGIN: u64 = 0x5242; // 'RB' reaper about to drop a retired task (aux=task id)
pub const REAPER_DROP_DONE: u64 = 0x5252; // 'RR' retired task drop completed (aux=task id)
pub const VMM_DROP_ENTER: u64 = 0x5645; // 'VE' sole-owner VMM drop entered (aux=ASID)
pub const VMM_DROP_MAPS_DONE: u64 = 0x564d; // 'VM' VMM mapping-owner cleanup completed (aux=ASID)
pub const VMM_DROP_ASID_BEGIN: u64 = 0x5658; // 'VX' VMM ASID teardown entered (aux=ASID)
pub const VMM_DROP_DONE: u64 = 0x565a; // 'VZ' VMM drop completed (aux=ASID)
pub const PUBLICATION_IN_PROGRESS: u64 = 0x4259; // 'BY' bounded snapshot found an in-flight writer

const SNAPSHOT_RETRY_LIMIT: usize = 4;
const STALL_REPORT_AFTER_SAMPLES: u64 = 2;
const STALL_REPEAT_SAMPLES: u64 = 6;
const STALL_SAMPLE_SLOTS: usize = MAX_NUM_CPUS * MAX_NUM_CPUS;
static STALL_SAMPLE_INITIALIZED: [AtomicBool; STALL_SAMPLE_SLOTS] =
    [const { AtomicBool::new(false) }; STALL_SAMPLE_SLOTS];
static STALL_LAST_TIMER_COUNT: [AtomicU64; STALL_SAMPLE_SLOTS] =
    [const { AtomicU64::new(0) }; STALL_SAMPLE_SLOTS];
static STALL_LAST_PHASE: [AtomicU64; STALL_SAMPLE_SLOTS] =
    [const { AtomicU64::new(0) }; STALL_SAMPLE_SLOTS];
static STALL_LAST_AUX: [AtomicU64; STALL_SAMPLE_SLOTS] =
    [const { AtomicU64::new(0) }; STALL_SAMPLE_SLOTS];
static STALL_LAST_AUX2: [AtomicU64; STALL_SAMPLE_SLOTS] =
    [const { AtomicU64::new(0) }; STALL_SAMPLE_SLOTS];
static STALL_STALE_SAMPLES: [AtomicU64; STALL_SAMPLE_SLOTS] =
    [const { AtomicU64::new(0) }; STALL_SAMPLE_SLOTS];

struct BreadcrumbSlot {
    sequence: AtomicU64,
    phase: AtomicU64,
    aux: AtomicU64,
    aux2: AtomicU64,
}

impl BreadcrumbSlot {
    const fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            phase: AtomicU64::new(NONE),
            aux: AtomicU64::new(0),
            aux2: AtomicU64::new(0),
        }
    }

    #[inline(always)]
    fn record(&self, phase: u64, aux: u64, aux2: u64) {
        let odd_sequence = self.sequence.load(Ordering::Relaxed).wrapping_add(1);
        self.sequence.store(odd_sequence, Ordering::SeqCst);
        self.phase.store(phase, Ordering::SeqCst);
        self.aux.store(aux, Ordering::SeqCst);
        self.aux2.store(aux2, Ordering::SeqCst);
        self.sequence
            .store(odd_sequence.wrapping_add(1), Ordering::SeqCst);
    }

    #[inline(always)]
    fn snapshot(&self) -> BreadcrumbSnapshot {
        self.snapshot_with_probe(|| {})
            .unwrap_or_else(|| BreadcrumbSnapshot {
                phase: PUBLICATION_IN_PROGRESS,
                aux: self.sequence.load(Ordering::SeqCst),
                aux2: 0,
            })
    }

    #[inline(always)]
    fn snapshot_with_probe(&self, mut after_aux: impl FnMut()) -> Option<BreadcrumbSnapshot> {
        for _ in 0..SNAPSHOT_RETRY_LIMIT {
            let sequence_before = self.sequence.load(Ordering::SeqCst);
            if sequence_before & 1 != 0 {
                continue;
            }

            let phase = self.phase.load(Ordering::SeqCst);
            let aux = self.aux.load(Ordering::SeqCst);
            after_aux();
            let aux2 = self.aux2.load(Ordering::SeqCst);
            let sequence_after = self.sequence.load(Ordering::SeqCst);
            if sequence_before == sequence_after {
                return Some(BreadcrumbSnapshot { phase, aux, aux2 });
            }
        }

        None
    }
}

static BREADCRUMBS: [BreadcrumbSlot; MAX_NUM_CPUS] =
    [const { BreadcrumbSlot::new() }; MAX_NUM_CPUS];

/// Lock-free breadcrumb state sampled from one CPU.
#[derive(Clone, Copy, Debug)]
pub struct BreadcrumbSnapshot {
    /// Execution phase from one committed generation, or an in-progress marker.
    pub phase: u64,
    /// First numeric context field recorded with the phase.
    pub aux: u64,
    /// Second numeric context field recorded with the phase.
    pub aux2: u64,
}

/// Return a lock-free snapshot of a CPU's last breadcrumb.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU whose breadcrumb should be sampled.
///
/// # Returns
///
/// The sampled phase and context fields, or `None` when `cpu_id` is outside
/// the supported CPU range. If all bounded attempts overlap a publication,
/// the returned phase is [`PUBLICATION_IN_PROGRESS`] and `aux` is the observed
/// sequence value instead of an inconsistent payload tuple.
///
/// The sequence is sampled before and after the context fields. A sample is
/// returned only when both sequence reads match and are even, so a following
/// writer cannot mix a previous phase with new context. Sampling is bounded;
/// a CPU stopped during publication cannot spin the observing CPU forever.
#[inline(always)]
pub fn snapshot(cpu_id: usize) -> Option<BreadcrumbSnapshot> {
    if cpu_id >= MAX_NUM_CPUS {
        return None;
    }

    Some(BREADCRUMBS[cpu_id].snapshot())
}

/// Sample other CPUs for an unchanged timer, breadcrumb, and scheduler state.
///
/// # Arguments
///
/// * `observer_cpu` - CPU collecting the diagnostic sample.
/// * `timer_irq_count` - Lock-free local timer IRQ counter accessor.
///
/// This diagnostic boundary keeps the timer core independent from scheduler
/// diagnostics while retaining the combined SMP liveness report.
pub fn sample_timer_stalls(observer_cpu: usize, timer_irq_count: fn(usize) -> Option<u64>) {
    if observer_cpu >= MAX_NUM_CPUS {
        return;
    }

    for target_cpu in 0..MAX_NUM_CPUS {
        if target_cpu == observer_cpu {
            continue;
        }

        let Some(timer_count) = timer_irq_count(target_cpu) else {
            continue;
        };
        let Some(breadcrumb) = snapshot(target_cpu) else {
            continue;
        };
        let Some(scheduler) = crate::sched::scheduler::diagnostic_snapshot(target_cpu) else {
            continue;
        };
        if timer_count == 0 && breadcrumb.phase == NONE && scheduler.current_task_id == 0 {
            continue;
        }

        let slot = observer_cpu * MAX_NUM_CPUS + target_cpu;
        let changed = !STALL_SAMPLE_INITIALIZED[slot].swap(true, Ordering::Relaxed)
            || STALL_LAST_TIMER_COUNT[slot].load(Ordering::Relaxed) != timer_count
            || STALL_LAST_PHASE[slot].load(Ordering::Relaxed) != breadcrumb.phase
            || STALL_LAST_AUX[slot].load(Ordering::Relaxed) != breadcrumb.aux
            || STALL_LAST_AUX2[slot].load(Ordering::Relaxed) != breadcrumb.aux2;
        if changed {
            STALL_LAST_TIMER_COUNT[slot].store(timer_count, Ordering::Relaxed);
            STALL_LAST_PHASE[slot].store(breadcrumb.phase, Ordering::Relaxed);
            STALL_LAST_AUX[slot].store(breadcrumb.aux, Ordering::Relaxed);
            STALL_LAST_AUX2[slot].store(breadcrumb.aux2, Ordering::Relaxed);
            STALL_STALE_SAMPLES[slot].store(0, Ordering::Relaxed);
            continue;
        }

        let stale_samples = STALL_STALE_SAMPLES[slot]
            .load(Ordering::Relaxed)
            .saturating_add(1);
        STALL_STALE_SAMPLES[slot].store(stale_samples, Ordering::Relaxed);
        let should_report = stale_samples == STALL_REPORT_AFTER_SAMPLES
            || (stale_samples > STALL_REPORT_AFTER_SAMPLES
                && (stale_samples - STALL_REPORT_AFTER_SAMPLES) % STALL_REPEAT_SAMPLES == 0);
        if should_report {
            crate::early_println!(
                "[timer] stall observer={} cpu={} count={} phase={:#06x} aux={:#x} aux2={:#x} task={} idle={} pending_reschedule={}",
                observer_cpu,
                target_cpu,
                timer_count,
                breadcrumb.phase,
                breadcrumb.aux,
                breadcrumb.aux2,
                scheduler.current_task_id,
                scheduler.is_idle,
                scheduler.pending_reschedule,
            );
        }
    }
}

/// Record the current execution phase for the running CPU.
///
/// Lock-free and FIQ-safe: only atomic operations, no allocation or formatting.
/// `aux`/`aux2` carry context such as FAR, task id, or exception class. An
/// odd/even sequence brackets the fields so readers can reject a concurrent
/// publication, including consecutive records that use the same phase code.
#[inline(always)]
pub fn drop(phase: u64, aux: u64, aux2: u64) {
    let cpu = get_cpu().get_cpuid();
    if cpu < MAX_NUM_CPUS {
        BREADCRUMBS[cpu].record(phase, aux, aux2);
    }
}

/// Record a breadcrumb with an explicit CPU id, bypassing `get_cpu()`.
///
/// Use this after `set_arch()` has repointed `TPIDR_EL1` at the trampoline
/// Arch, where `get_cpu().get_cpuid()` may read an unexpected struct.
/// The caller must name the currently executing CPU. The odd/even protocol has
/// exactly one writer per slot and does not support remote concurrent writers.
#[inline(always)]
pub fn drop_cpu(cpu_id: usize, phase: u64, aux: u64) {
    if cpu_id < MAX_NUM_CPUS {
        BREADCRUMBS[cpu_id].record(phase, aux, 0);
    }
}

/// Dump the latest breadcrumb from every CPU.
///
/// Formats the whole dump into a stack buffer and emits it through
/// `earlyfb::write_raw`, which holds `EARLY_CONSOLE` across the entire string.
/// This avoids the per-byte locking of `early_println!` that would interleave
/// the dump with concurrent heartbeat output from other CPUs. No allocation,
/// so it is safe from the timer-FIQ context.
pub fn dump_all() {
    use core::fmt::{self, Write};

    struct BufWriter<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }

    impl fmt::Write for BufWriter<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let bytes = s.as_bytes();
            if self.pos + bytes.len() > self.buf.len() {
                self.pos = self.buf.len();
                return Err(fmt::Error);
            }
            self.buf[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
            self.pos += bytes.len();
            Ok(())
        }
    }

    let mut buf = [0u8; 1024];
    let len;
    {
        let mut w = BufWriter {
            buf: &mut buf,
            pos: 0,
        };
        for cpu in 0..MAX_NUM_CPUS {
            let snapshot = BREADCRUMBS[cpu].snapshot();
            if snapshot.phase == NONE {
                continue;
            }
            let _ = write!(
                w,
                "[crumb] cpu={} phase={:#06x} aux={:#x} aux2={:#x}\n",
                cpu, snapshot.phase, snapshot.aux, snapshot.aux2,
            );
        }
        len = w.pos;
    }

    let s = core::str::from_utf8(&buf[..len]).unwrap_or("");
    if crate::earlyfb::is_redirection_enabled() {
        crate::earlyfb::write_raw(s);
    }
}

#[cfg(test)]
mod tests;
