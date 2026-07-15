//! Per-CPU lock-free breadcrumb diagnostics for SMP hang localization.
//!
//! When a CPU hangs with FIQ masked it cannot print. Each CPU therefore records
//! its last execution phase in a lock-free atomic. Surviving CPUs periodically
//! dump all breadcrumbs alongside their timer heartbeat, revealing exactly
//! where each stuck CPU stopped and whether the state is still changing.
//!
//! Context fields use relaxed atomic stores and the phase is release-published
//! last. The path uses no locks or formatting, so it remains safe from FIQ
//! handlers and trap-vector prologues reached after `daifset`.

use core::sync::atomic::{AtomicU64, Ordering};

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
pub const SYSCALL_TASK_DONE: u64 = 0x5354; // 'ST' mytask returned (aux=task id, aux2=PC)
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
pub const PT_MAP_BEGIN: u64 = 0x4d42; // 'MB' entering single-page map
pub const PT_LEAF_DONE: u64 = 0x4d4c; // 'ML' leaf PTE installed and published
pub const PT_TLBI_BEGIN: u64 = 0x5442; // 'TB' broadcast stage-1 TLBI about to begin
pub const PT_TLBI_DONE: u64 = 0x5444; // 'TD' broadcast stage-1 TLBI completed
pub const PT_MAP_DONE: u64 = 0x4d44; // 'MD' single-page map returned
pub const PT_ALLOC_BEGIN: u64 = 0x4142; // 'AB' child page-table allocation about to begin
pub const PT_ALLOC_DONE: u64 = 0x4143; // 'AC' child page-table allocation completed
pub const PT_REGISTRY_WAIT: u64 = 0x5257; // 'RW' waiting for PAGE_TABLES registry write lock
pub const PT_REGISTRY_DONE: u64 = 0x5244; // 'RD' child page table recorded in registry
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

static PHASE: [AtomicU64; MAX_NUM_CPUS] = [const { AtomicU64::new(NONE) }; MAX_NUM_CPUS];
static AUX: [AtomicU64; MAX_NUM_CPUS] = [const { AtomicU64::new(0) }; MAX_NUM_CPUS];
static AUX2: [AtomicU64; MAX_NUM_CPUS] = [const { AtomicU64::new(0) }; MAX_NUM_CPUS];

/// Record the current execution phase for the running CPU.
///
/// Lock-free and FIQ-safe: only atomic stores, no allocation, no formatting.
/// `aux`/`aux2` carry context such as FAR, task id, or exception class. The
/// phase is release-published last so a dump that observes it also observes
/// the matching context fields.
#[inline(always)]
pub fn drop(phase: u64, aux: u64, aux2: u64) {
    let cpu = get_cpu().get_cpuid();
    if cpu < MAX_NUM_CPUS {
        AUX[cpu].store(aux, Ordering::Relaxed);
        AUX2[cpu].store(aux2, Ordering::Relaxed);
        PHASE[cpu].store(phase, Ordering::Release);
    }
}

/// Record a breadcrumb with an explicit CPU id, bypassing `get_cpu()`.
///
/// Use this after `set_arch()` has repointed `TPIDR_EL1` at the trampoline
/// Arch, where `get_cpu().get_cpuid()` may read an unexpected struct.
#[inline(always)]
pub fn drop_cpu(cpu_id: usize, phase: u64, aux: u64) {
    if cpu_id < MAX_NUM_CPUS {
        AUX[cpu_id].store(aux, Ordering::Relaxed);
        AUX2[cpu_id].store(0, Ordering::Relaxed);
        PHASE[cpu_id].store(phase, Ordering::Release);
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
            let phase = PHASE[cpu].load(Ordering::Acquire);
            if phase == NONE {
                continue;
            }
            let _ = write!(
                w,
                "[crumb] cpu={} phase={:#06x} aux={:#x} aux2={:#x}\n",
                cpu,
                phase,
                AUX[cpu].load(Ordering::Relaxed),
                AUX2[cpu].load(Ordering::Relaxed),
            );
        }
        len = w.pos;
    }

    let s = core::str::from_utf8(&buf[..len]).unwrap_or("");
    crate::earlyfb::write_raw(s);
}
