//! Per-CPU lock-free breadcrumb diagnostics for SMP hang localization.
//!
//! When a CPU hangs with FIQ masked it cannot print. Each CPU therefore records
//! its last execution phase in a lock-free atomic. A surviving CPU dumps all
//! breadcrumbs once, revealing exactly where each stuck CPU stopped.
//!
//! All writes are `Relaxed` atomic stores with no locks and no formatting, so
//! they are safe from any context including FIQ handlers and trap-vector
//! prologues reached after `daifset`.

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
pub const KCTX_ENTER: u64 = 0x4b45; // 'KE' kernel_context_switch entered
pub const KCTX_SWITCH_TO: u64 = 0x4b53; // 'KS' about to switch_to
pub const KCTX_RESUME: u64 = 0x4b52; // 'KR' resumed after switch_to
pub const TIMER_TICK: u64 = 0x5454; // 'TT' timer FIQ serviced
pub const TIMER_SW_TIMERS: u64 = 0x5453; // 'TS' check_software_timers
pub const CLONE_ENTER: u64 = 0x434c; // 'CL' clone_task entered
pub const CLONE_KSTACK_DONE: u64 = 0x434b; // 'CK' child kstack window mapped
pub const CLONE_RETURN: u64 = 0x4352; // 'CR' clone_task returning
pub const SYSCALL_ENTER: u64 = 0x5359; // 'SY' syscall_dispatcher entered (aux=num)
pub const SYSCALL_EXIT: u64 = 0x5358; // 'SX' syscall_dispatcher returned
pub const REL_PREV_DONE: u64 = 0x5250; // 'RP' release_deferred_prev done after resume
pub const GETTASK_DONE: u64 = 0x4754; // 'GT' get_task(from) done after resume
pub const SETUP_DONE: u64 = 0x5344; // 'SD' setup_task_cpu_state done after resume
pub const TRAMP_GET: u64 = 0x5447; // 'TG' get_trampoline_arch returned (aux=trampoline addr)
pub const SET_ARCH_DONE: u64 = 0x5341; // 'SA' set_arch (TPIDR_EL1 write) done
pub const KFAULT: u64 = 0x4b46; // 'KF' kernel fault hit DataAbortSameEl/InstrAbortSameEl/Unhandled (aux=FAR aux2=ESR)
pub const LAZY_FOUND: u64 = 0x464e; // 'FN' lazy_map_page_with found the memmap
pub const LAZY_COWDONE: u64 = 0x4344; // 'CD' COW copy + add_memory_map_fixed done (pre root_pagetable.map)
pub const LAZY_RESOLVED: u64 = 0x5246; // 'RF' owner.resolve_fault returned Ok (aux=paddr_page_base)
pub const PMM_ALLOC_DONE: u64 = 0x5041; // 'PA' ContiguousPages::new (PMM) done (aux=new_paddr)
pub const SETSP_DONE: u64 = 0x5350; // 'SP' set_kernel_stack done
pub const SETTH_DONE: u64 = 0x5448; // 'TH' set_trap_handler done

static PHASE: [AtomicU64; MAX_NUM_CPUS] = [const { AtomicU64::new(NONE) }; MAX_NUM_CPUS];
static AUX: [AtomicU64; MAX_NUM_CPUS] = [const { AtomicU64::new(0) }; MAX_NUM_CPUS];
static AUX2: [AtomicU64; MAX_NUM_CPUS] = [const { AtomicU64::new(0) }; MAX_NUM_CPUS];

static DUMPED: AtomicBool = AtomicBool::new(false);

/// Whether the one-shot breadcrumb dump has already fired.
#[inline]
pub fn dumped() -> bool {
    DUMPED.load(Ordering::SeqCst)
}

/// Record the current execution phase for the running CPU.
///
/// Lock-free and FIQ-safe: only relaxed atomic stores, no allocation, no
/// formatting. `aux`/`aux2` carry context such as FAR, task id, or exception
/// class.
#[inline(always)]
pub fn drop(phase: u64, aux: u64, aux2: u64) {
    let cpu = get_cpu().get_cpuid();
    if cpu < MAX_NUM_CPUS {
        PHASE[cpu].store(phase, Ordering::Relaxed);
        AUX[cpu].store(aux, Ordering::Relaxed);
        AUX2[cpu].store(aux2, Ordering::Relaxed);
    }
}

/// Record a breadcrumb with an explicit CPU id, bypassing `get_cpu()`.
///
/// Use this after `set_arch()` has repointed `TPIDR_EL1` at the trampoline
/// Arch, where `get_cpu().get_cpuid()` may read an unexpected struct.
#[inline(always)]
pub fn drop_cpu(cpu_id: usize, phase: u64, aux: u64) {
    if cpu_id < MAX_NUM_CPUS {
        PHASE[cpu_id].store(phase, Ordering::Relaxed);
        AUX[cpu_id].store(aux, Ordering::Relaxed);
        AUX2[cpu_id].store(0, Ordering::Relaxed);
    }
}

/// Dump all CPU breadcrumbs exactly once.
///
/// Formats the whole dump into a stack buffer and emits it through
/// `earlyfb::write_raw`, which holds `EARLY_CONSOLE` across the entire string.
/// This avoids the per-byte locking of `early_println!` that would interleave
/// the dump with concurrent heartbeat output from other CPUs. No allocation,
/// so it is safe from the timer-FIQ context.
pub fn dump_once() {
    if DUMPED.swap(true, Ordering::SeqCst) {
        return;
    }

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
        let mut w = BufWriter { buf: &mut buf, pos: 0 };
        for cpu in 0..MAX_NUM_CPUS {
            let phase = PHASE[cpu].load(Ordering::Relaxed);
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
