//! Linux signal syscalls and signal handling
//!
//! Implements POSIX signals with Linux-compatible semantics, integrated with Scarlet's
//! event system for cross-ABI signal delivery.

use crate::abi::EventProcessOutcome;
use crate::abi::linux::generic::LinuxAbi;
use crate::abi::linux::generic::errno;
use crate::arch::Trapframe;
use crate::ipc::event::{Event, EventContent, EventPayload, ProcessControlType};
use crate::task::mytask;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;

use crate::sched::scheduler::{
    get_task_by_id, mark_blocked, push_ready_task, remove_from_ready_queues, unmark_blocked,
};
use crate::task::{Task, TaskType};

/// Linux signal numbers (POSIX standard)
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LinuxSignal {
    SIGHUP = 1,
    SIGINT = 2,
    SIGQUIT = 3,
    SIGILL = 4,
    SIGTRAP = 5,
    SIGABRT = 6,
    SIGBUS = 7,
    SIGFPE = 8,
    SIGKILL = 9,
    SIGUSR1 = 10,
    SIGSEGV = 11,
    SIGUSR2 = 12,
    SIGPIPE = 13,
    SIGALRM = 14,
    SIGTERM = 15,
    SIGSTKFLT = 16,
    SIGCHLD = 17,
    SIGCONT = 18,
    SIGSTOP = 19,
    SIGTSTP = 20,
    SIGTTIN = 21,
    SIGTTOU = 22,
    SIGURG = 23,
    SIGXCPU = 24,
    SIGXFSZ = 25,
    SIGVTALRM = 26,
    SIGPROF = 27,
    SIGWINCH = 28,
    SIGIO = 29,
    SIGPWR = 30,
    SIGSYS = 31,
    SIGRT0 = 32,
    SIGRT1 = 33,
    SIGRT2 = 34,
    SIGRT3 = 35,
    SIGRT4 = 36,
    SIGRT5 = 37,
    SIGRT6 = 38,
    SIGRT7 = 39,
    SIGRT8 = 40,
    SIGRT9 = 41,
    SIGRT10 = 42,
    SIGRT11 = 43,
    SIGRT12 = 44,
    SIGRT13 = 45,
    SIGRT14 = 46,
    SIGRT15 = 47,
    SIGRT16 = 48,
    SIGRT17 = 49,
    SIGRT18 = 50,
    SIGRT19 = 51,
    SIGRT20 = 52,
    SIGRT21 = 53,
    SIGRT22 = 54,
    SIGRT23 = 55,
    SIGRT24 = 56,
    SIGRT25 = 57,
    SIGRT26 = 58,
    SIGRT27 = 59,
    SIGRT28 = 60,
    SIGRT29 = 61,
    SIGRT30 = 62,
    SIGRT31 = 63,
    SIGRT32 = 64,
}

impl LinuxSignal {
    /// Convert from u32 to LinuxSignal
    pub fn from_u32(signal: u32) -> Option<Self> {
        match signal {
            1 => Some(Self::SIGHUP),
            2 => Some(Self::SIGINT),
            3 => Some(Self::SIGQUIT),
            4 => Some(Self::SIGILL),
            5 => Some(Self::SIGTRAP),
            6 => Some(Self::SIGABRT),
            7 => Some(Self::SIGBUS),
            8 => Some(Self::SIGFPE),
            9 => Some(Self::SIGKILL),
            10 => Some(Self::SIGUSR1),
            11 => Some(Self::SIGSEGV),
            12 => Some(Self::SIGUSR2),
            13 => Some(Self::SIGPIPE),
            14 => Some(Self::SIGALRM),
            15 => Some(Self::SIGTERM),
            16 => Some(Self::SIGSTKFLT),
            17 => Some(Self::SIGCHLD),
            18 => Some(Self::SIGCONT),
            19 => Some(Self::SIGSTOP),
            20 => Some(Self::SIGTSTP),
            21 => Some(Self::SIGTTIN),
            22 => Some(Self::SIGTTOU),
            23 => Some(Self::SIGURG),
            24 => Some(Self::SIGXCPU),
            25 => Some(Self::SIGXFSZ),
            26 => Some(Self::SIGVTALRM),
            27 => Some(Self::SIGPROF),
            28 => Some(Self::SIGWINCH),
            29 => Some(Self::SIGIO),
            30 => Some(Self::SIGPWR),
            31 => Some(Self::SIGSYS),
            32 => Some(Self::SIGRT0),
            33 => Some(Self::SIGRT1),
            34 => Some(Self::SIGRT2),
            35 => Some(Self::SIGRT3),
            36 => Some(Self::SIGRT4),
            37 => Some(Self::SIGRT5),
            38 => Some(Self::SIGRT6),
            39 => Some(Self::SIGRT7),
            40 => Some(Self::SIGRT8),
            41 => Some(Self::SIGRT9),
            42 => Some(Self::SIGRT10),
            43 => Some(Self::SIGRT11),
            44 => Some(Self::SIGRT12),
            45 => Some(Self::SIGRT13),
            46 => Some(Self::SIGRT14),
            47 => Some(Self::SIGRT15),
            48 => Some(Self::SIGRT16),
            49 => Some(Self::SIGRT17),
            50 => Some(Self::SIGRT18),
            51 => Some(Self::SIGRT19),
            52 => Some(Self::SIGRT20),
            53 => Some(Self::SIGRT21),
            54 => Some(Self::SIGRT22),
            55 => Some(Self::SIGRT23),
            56 => Some(Self::SIGRT24),
            57 => Some(Self::SIGRT25),
            58 => Some(Self::SIGRT26),
            59 => Some(Self::SIGRT27),
            60 => Some(Self::SIGRT28),
            61 => Some(Self::SIGRT29),
            62 => Some(Self::SIGRT30),
            63 => Some(Self::SIGRT31),
            64 => Some(Self::SIGRT32),
            _ => None,
        }
    }

    /// Get default action for this signal
    pub fn default_action(&self) -> SignalAction {
        match self {
            Self::SIGKILL => SignalAction::ForceTerminate,
            Self::SIGSTOP => SignalAction::Stop,
            Self::SIGCHLD | Self::SIGURG | Self::SIGWINCH => SignalAction::Ignore,
            Self::SIGCONT => SignalAction::Continue,
            Self::SIGTSTP | Self::SIGTTIN | Self::SIGTTOU => SignalAction::Stop,
            _ => SignalAction::Terminate,
        }
    }
}

/// Signal action types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalAction {
    /// Default action: terminate process
    Terminate,
    /// Force terminate (cannot be caught/ignored)
    ForceTerminate,
    /// Ignore signal
    Ignore,
    /// Stop process
    Stop,
    /// Continue process
    Continue,
    /// Custom handler
    Custom(usize), // Handler function address
}

/// Signal mask for blocking signals
#[derive(Debug, Clone, Copy, Default)]
pub struct SignalMask {
    mask: u64, // Bit mask for signals 1-64
}

impl SignalMask {
    pub fn new() -> Self {
        Self { mask: 0 }
    }

    pub fn block_signal(&mut self, signal: LinuxSignal) {
        self.mask |= 1u64 << (signal as u32 - 1);
    }

    pub fn unblock_signal(&mut self, signal: LinuxSignal) {
        self.mask &= !(1u64 << (signal as u32 - 1));
    }

    pub fn is_blocked(&self, signal: LinuxSignal) -> bool {
        (self.mask & (1u64 << (signal as u32 - 1))) != 0
    }

    pub fn raw(&self) -> u64 {
        self.mask
    }

    pub fn set_raw(&mut self, mask: u64) {
        self.mask = mask;
    }
}

#[derive(Clone)]
struct SignalDispositions {
    handlers: BTreeMap<LinuxSignal, SignalAction>,
    actions: BTreeMap<LinuxSignal, Sigaction>,
}

/// The ABI's fixed-width siginfo record. Native events carry these bytes
/// without interpreting Linux signal numbers or union layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SignalInfo {
    pub(crate) bytes: [u8; 128],
}

impl SignalInfo {
    pub(crate) fn kernel(signal: LinuxSignal) -> Self {
        let mut bytes = [0; 128];
        bytes[..4].copy_from_slice(&(signal as u32).to_ne_bytes());
        bytes[8..12].copy_from_slice(&128i32.to_ne_bytes());
        Self { bytes }
    }
}

const MAX_REALTIME_PENDING: usize = 1024;
const PENDING_WAKE: &[u8] = b"linux.pending-signal.v1";

#[derive(Default)]
struct PendingSignals {
    records: BTreeMap<LinuxSignal, VecDeque<SignalInfo>>,
    realtime_count: usize,
    control_epoch: u64,
}

impl Clone for PendingSignals {
    fn clone(&self) -> Self {
        let records = self.records.clone();
        let realtime_count = records.iter().filter(|(signal, _)| **signal as u32 >= 32)
            .map(|(_, records)| records.len()).sum();
        Self { records, realtime_count, control_epoch: self.control_epoch }
    }
}

impl PendingSignals {
    fn accept_job_control(&mut self, signal: LinuxSignal) {
        if is_job_signal(signal) { self.control_epoch = self.control_epoch.wrapping_add(1); }
        self.discard_job_opposite(signal);
    }
    fn discard_job_opposite(&mut self, signal: LinuxSignal) {
        if signal == LinuxSignal::SIGCONT {
            for stop in [LinuxSignal::SIGSTOP, LinuxSignal::SIGTSTP, LinuxSignal::SIGTTIN, LinuxSignal::SIGTTOU] {
                self.records.remove(&stop);
            }
        } else if is_stop_signal(signal) { self.records.remove(&LinuxSignal::SIGCONT); }
    }
    fn mask(&self) -> u64 {
        self.records.keys().fold(0, |mask, signal| mask | (1u64 << (*signal as u32 - 1)))
    }

    fn push(&mut self, signal: LinuxSignal, info: SignalInfo) -> Result<(), usize> {
        let realtime = signal as u32 >= 32;
        // Standard signals retain the first record while pending. Realtime
        // signals retain every admitted record and preserve FIFO per number.
        if !realtime && self.records.contains_key(&signal) { return Ok(()); }
        if realtime && self.realtime_count == MAX_REALTIME_PENDING { return Err(errno::EAGAIN); }
        self.records.entry(signal).or_default().push_back(info);
        if realtime { self.realtime_count += 1; }
        Ok(())
    }

    fn peek(&self, signal: LinuxSignal) -> Option<SignalInfo> {
        self.records.get(&signal)?.front().copied()
    }

    fn take(&mut self, signal: LinuxSignal) -> Option<SignalInfo> {
        let queue = self.records.get_mut(&signal)?;
        let info = queue.pop_front()?;
        if queue.is_empty() { self.records.remove(&signal); }
        if signal as u32 >= 32 { self.realtime_count -= 1; }
        Some(info)
    }

    fn reserve(&mut self, signal: LinuxSignal) -> Option<SignalInfo> {
        let info = self.take(signal)?;
        // A user copy may allocate or resolve COW. Keep its realtime slot
        // charged while the record is outside the spin lock.
        if signal as u32 >= 32 { self.realtime_count += 1; }
        Some(info)
    }

    fn finish_reservation(&mut self, signal: LinuxSignal, info: SignalInfo, restore: bool) {
        if signal as u32 >= 32 { self.realtime_count -= 1; }
        if restore {
            if signal as u32 >= 32 {
                self.records.entry(signal).or_default().push_front(info);
                self.realtime_count += 1;
            } else {
                // Preserve the original standard record when an arrival raced
                // a failed copy; it coalesces with the now-pending record.
                let queue = self.records.entry(signal).or_default();
                queue.clear();
                queue.push_back(info);
            }
        }
    }
}

enum PendingOrigin {
    Thread(Arc<crate::sync::IrqSpinLock<SignalState>>),
    Process(Arc<crate::sync::IrqSpinLock<PendingSignals>>),
}

struct PendingReservation {
    origin: PendingOrigin,
    signal: LinuxSignal,
    info: SignalInfo,
    epoch: u64,
    active: bool,
    waker: Arc<crate::sync::waker::Waker>,
}

impl PendingReservation {
    fn finish(&mut self, restore: bool) {
        if !self.active { return; }
        match &self.origin {
            PendingOrigin::Thread(state) => {
                let mut state = state.lock();
                let epoch = state.shared_pending.lock().control_epoch;
                state.pending.finish_reservation(self.signal, self.info,
                    restore && (!is_job_signal(self.signal) || epoch == self.epoch));
            }
            PendingOrigin::Process(pending) => {
                let mut pending = pending.lock();
                let restore = restore && (!is_job_signal(self.signal) || pending.control_epoch == self.epoch);
                pending.finish_reservation(self.signal, self.info, restore);
            }
        }
        if restore { self.waker.wake_all(); }
        self.active = false;
    }
}

impl Drop for PendingReservation {
    fn drop(&mut self) { self.finish(true); }
}

fn reserve_pending(state: &Arc<crate::sync::IrqSpinLock<SignalState>>, mask: u64) -> Option<PendingReservation> {
    let mut locked = state.lock();
    let process = locked.shared_pending.clone();
    let mut shared = process.lock();
    let mask = (locked.pending.mask() | shared.mask()) & mask;
    if mask == 0 { return None; }
    let signal = LinuxSignal::from_u32(mask.trailing_zeros() + 1).unwrap();
    let (origin, info, epoch) = if let Some(info) = locked.pending.reserve(signal) {
        (PendingOrigin::Thread(state.clone()), info, locked.pending.control_epoch)
    } else {
        (PendingOrigin::Process(process.clone()), shared.reserve(signal).unwrap(), shared.control_epoch)
    };
    Some(PendingReservation { origin, signal, info, epoch, active: true, waker: locked.pending_waker.clone() })
}

/// Per-thread mask and pending signals, with separately shareable dispositions.
pub struct SignalState {
    dispositions: Arc<crate::sync::IrqSpinLock<SignalDispositions>>,
    /// Blocked signals mask
    pub blocked: SignalMask,
    /// Pending signals that are blocked
    pending: PendingSignals,
    shared_pending: Arc<crate::sync::IrqSpinLock<PendingSignals>>,
    pub(crate) pending_waker: Arc<crate::sync::waker::Waker>,
    /// The pre-sigsuspend mask is restored by sigreturn, not before delivery.
    pub(crate) suspend_restore_mask: Option<u64>,
}

// A snapshot (fork/exec) must not mutate the source's dispositions. Thread
// creation explicitly opts into sharing them via clone_for_child().
impl Clone for SignalState {
    fn clone(&self) -> Self {
        Self {
            dispositions: Arc::new(crate::sync::IrqSpinLock::new(
                self.dispositions.lock().clone(),
            )),
            blocked: self.blocked,
            pending: self.pending.clone(),
            shared_pending: Arc::new(crate::sync::IrqSpinLock::new(self.shared_pending.lock().clone())),
            pending_waker: Arc::new(crate::sync::waker::Waker::new_interruptible("linux_signal")),
            suspend_restore_mask: None,
        }
    }
}

impl core::fmt::Debug for SignalState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SignalState")
            .field("handlers", &self.dispositions.lock().handlers)
            .field("blocked", &self.blocked)
            .field("pending", &self.pending_mask())
            .finish()
    }
}

impl Default for SignalState {
    fn default() -> Self {
        let mut handlers = BTreeMap::new();
        // Set default actions for all signals
        for signal_num in 1..=64 {
            if let Some(signal) = LinuxSignal::from_u32(signal_num) {
                handlers.insert(signal, signal.default_action());
            }
        }

        Self {
            dispositions: Arc::new(crate::sync::IrqSpinLock::new(SignalDispositions {
                handlers,
                actions: BTreeMap::new(),
            })),
            blocked: SignalMask::new(),
            pending: PendingSignals::default(),
            shared_pending: Arc::new(crate::sync::IrqSpinLock::new(PendingSignals::default())),
            pending_waker: Arc::new(crate::sync::waker::Waker::new_interruptible("linux_signal")),
            suspend_restore_mask: None,
        }
    }
}

impl SignalState {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn clone_for_child(&self, share_handlers: bool, share_pending: bool) -> Self {
        let dispositions = if share_handlers {
            self.dispositions.clone()
        } else {
            Arc::new(crate::sync::IrqSpinLock::new(
                self.dispositions.lock().clone(),
            ))
        };
        Self {
            dispositions,
            blocked: self.blocked,
            pending: PendingSignals::default(),
            shared_pending: if share_pending { self.shared_pending.clone() }
                else { Arc::new(crate::sync::IrqSpinLock::new(PendingSignals::default())) },
            pending_waker: if share_pending { self.pending_waker.clone() }
                else { Arc::new(crate::sync::waker::Waker::new_interruptible("linux_signal")) },
            suspend_restore_mask: None,
        }
    }

    /// Set signal handler
    pub fn set_handler(&mut self, signal: LinuxSignal, action: SignalAction) {
        // SIGKILL and SIGSTOP cannot be caught or ignored
        if signal != LinuxSignal::SIGKILL && signal != LinuxSignal::SIGSTOP {
            let mut dispositions = self.dispositions.lock();
            dispositions.handlers.insert(signal, action);
            dispositions.actions.remove(&signal);
        }
    }

    pub fn get_sigaction(&self, signal: LinuxSignal) -> Sigaction {
        let dispositions = self.dispositions.lock();
        dispositions
            .actions
            .get(&signal)
            .copied()
            .unwrap_or_else(|| {
                sigaction_to_linux(
                    dispositions
                        .handlers
                        .get(&signal)
                        .copied()
                        .unwrap_or(signal.default_action()),
                )
            })
    }

    fn install_sigaction(&mut self, signal: LinuxSignal, action: Sigaction) {
        if !matches!(signal, LinuxSignal::SIGKILL | LinuxSignal::SIGSTOP) {
            let mut dispositions = self.dispositions.lock();
            dispositions
                .handlers
                .insert(signal, linux_to_sigaction(action, signal));
            dispositions.actions.insert(signal, action);
        }
    }

    pub(crate) fn reset_caught_handlers(&mut self) {
        for number in 1..=64 {
            if let Some(signal) = LinuxSignal::from_u32(number) {
                if matches!(self.get_handler(signal), SignalAction::Custom(_)) {
                    self.set_handler(signal, signal.default_action());
                }
            }
        }
    }

    /// Get signal handler
    pub fn get_handler(&self, signal: LinuxSignal) -> SignalAction {
        self.dispositions
            .lock()
            .handlers
            .get(&signal)
            .copied()
            .unwrap_or(signal.default_action())
    }

    /// Add pending signal
    pub fn add_pending(&mut self, signal: LinuxSignal) {
        let _ = self.queue_pending_info(signal, SignalInfo::kernel(signal));
    }

    pub(crate) fn queue_pending_info(&mut self, signal: LinuxSignal, info: SignalInfo) -> Result<(), usize> {
        if is_job_signal(signal) { self.pending.control_epoch = self.shared_pending.lock().control_epoch; }
        self.pending.discard_job_opposite(signal);
        self.pending.push(signal, info)?;
        self.pending_waker.wake_all();
        Ok(())
    }

    pub(crate) fn add_process_pending(&self, signal: LinuxSignal, info: SignalInfo) -> Result<(), usize> {
        self.publish_process_signal(signal, info).map(|_| ())
    }

    fn publish_process_signal(&self, signal: LinuxSignal, info: SignalInfo) -> Result<u64, usize> {
        let epoch = {
            let mut pending = self.shared_pending.lock();
            pending.accept_job_control(signal);
            pending.push(signal, info)?;
            pending.control_epoch
        };
        self.pending_waker.wake_all();
        Ok(epoch)
    }

    pub(crate) fn pending_mask(&self) -> u64 {
        self.pending.mask() | self.shared_pending.lock().mask()
    }

    pub(crate) fn take_pending_info(&mut self, signal: LinuxSignal) -> Option<SignalInfo> {
        self.take_pending_with_epoch(signal).map(|(info, _)| info)
    }

    fn take_pending_with_epoch(&mut self, signal: LinuxSignal) -> Option<(SignalInfo, u64)> {
        if let Some(info) = self.pending.take(signal) { return Some((info, self.pending.control_epoch)); }
        let mut shared = self.shared_pending.lock();
        shared.take(signal).map(|info| (info, shared.control_epoch))
    }

    pub(crate) fn take_pending_matching(&mut self, mask: u64) -> Option<(LinuxSignal, SignalInfo)> {
        let mut shared = self.shared_pending.lock();
        let matching = (self.pending.mask() | shared.mask()) & mask;
        if matching == 0 { return None; }
        let signal = LinuxSignal::from_u32(matching.trailing_zeros() + 1).unwrap();
        let info = self.pending.take(signal).or_else(|| shared.take(signal)).unwrap();
        Some((signal, info))
    }

    /// Remove pending signal
    pub fn remove_pending(&mut self, signal: LinuxSignal) {
        self.take_pending_info(signal);
    }

    /// Check if signal is pending
    pub fn is_pending(&self, signal: LinuxSignal) -> bool {
        self.pending_mask() & (1u64 << (signal as u32 - 1)) != 0
    }

    /// Get next deliverable signal (not blocked and pending)
    pub fn next_deliverable_signal(&self) -> Option<LinuxSignal> {
        let deliverable = self.pending_mask() & !self.blocked.raw();
        (deliverable != 0).then(|| LinuxSignal::from_u32(deliverable.trailing_zeros() + 1).unwrap())
    }
}

/// Linux sigaction structure (simplified)
/// This matches the Linux sigaction layout
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Sigaction {
    /// Signal handler address (or SIG_DFL, SIG_IGN)
    pub handler: usize,
    /// Signal flags
    pub flags: u64,
    /// AArch64's kernel sigaction includes a restorer slot; RISC-V does not.
    #[cfg(target_arch = "aarch64")]
    pub restorer: usize,
    /// Signal mask to apply during handler execution
    pub mask: u64,
}

/// Special handler values
pub const SIG_DFL: usize = 0; // Default action
pub const SIG_IGN: usize = 1; // Ignore signal

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SigAltStack {
    pub ss_sp: usize,
    pub ss_flags: u32,
    _pad: u32,
    pub ss_size: usize,
}

/// Linux sigaltstack system call implementation.
///
/// int sigaltstack(const stack_t *ss, stack_t *old_ss);
pub fn sys_sigaltstack(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    const SS_ONSTACK: u32 = 1;
    const SS_DISABLE: u32 = 2;
    const SS_AUTODISARM: u32 = 1 << 31;
    #[cfg(target_arch = "aarch64")]
    const MINSIGSTKSZ: usize = 5120;
    #[cfg(not(target_arch = "aarch64"))]
    const MINSIGSTKSZ: usize = 2048;

    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::EPERM),
    };

    let new_stack_ptr = trapframe.get_arg(0);
    let old_stack_ptr = trapframe.get_arg(1);
    trapframe.increment_pc_next(&task);

    if old_stack_ptr != 0 {
        let Some(kva) = task.vm_manager.translate_to_kva(old_stack_ptr) else {
            return errno::to_result(errno::EFAULT);
        };
        let state = abi.thread_state();
        let flags = if state.sigaltstack_size == 0 {
            SS_DISABLE
        } else {
            state.sigaltstack_flags
        };
        let old_stack = SigAltStack {
            ss_sp: state.sigaltstack_sp,
            ss_flags: flags,
            _pad: 0,
            ss_size: state.sigaltstack_size,
        };
        // SAFETY: old_stack_ptr was translated from the current task address space.
        unsafe {
            core::ptr::write(kva as *mut SigAltStack, old_stack);
        }
    }

    if new_stack_ptr != 0 {
        let Some(kva) = task.vm_manager.translate_to_kva(new_stack_ptr) else {
            return errno::to_result(errno::EFAULT);
        };
        // SAFETY: new_stack_ptr was translated from the current task address space.
        let new_stack = unsafe { core::ptr::read(kva as *const SigAltStack) };

        if new_stack.ss_flags & !(SS_DISABLE | SS_AUTODISARM) != 0
            || new_stack.ss_flags & SS_ONSTACK != 0
        {
            return errno::to_result(errno::EINVAL);
        }

        let state = abi.thread_state_mut();
        if new_stack.ss_flags & SS_DISABLE != 0 {
            state.sigaltstack_sp = 0;
            state.sigaltstack_size = 0;
            state.sigaltstack_flags = SS_DISABLE;
        } else {
            if new_stack.ss_size < MINSIGSTKSZ {
                return errno::to_result(errno::ENOMEM);
            }
            state.sigaltstack_sp = new_stack.ss_sp;
            state.sigaltstack_size = new_stack.ss_size;
            state.sigaltstack_flags = new_stack.ss_flags & SS_AUTODISARM;
        }
    }

    0
}

/// Linux rt_sigaction system call implementation
///
/// int rt_sigaction(int signum, const struct sigaction *act, struct sigaction *oldact, size_t sigsetsize);
pub fn sys_rt_sigaction(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    use crate::library::std::usercopy::{copy_from_user, copy_to_user};
    let task = mytask().unwrap();
    let signum = trapframe.get_arg(0) as u32;
    let act_ptr = trapframe.get_arg(1);
    let oldact_ptr = trapframe.get_arg(2);
    let sigsetsize = trapframe.get_arg(3);
    trapframe.increment_pc_next(&task);
    let Some(signal) = LinuxSignal::from_u32(signum) else {
        return errno::to_result(errno::EINVAL);
    };
    if sigsetsize != 8
        || (act_ptr != 0 && matches!(signal, LinuxSignal::SIGKILL | LinuxSignal::SIGSTOP))
    {
        return errno::to_result(errno::EINVAL);
    }
    let new_action = if act_ptr != 0 {
        let mut bytes = [0u8; core::mem::size_of::<Sigaction>()];
        if copy_from_user(&task, act_ptr, &mut bytes).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        // All fields are integers; the copied buffer need not be aligned.
        Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<Sigaction>()) })
    } else {
        None
    };
    if oldact_ptr != 0 {
        let old = abi.signal_state.lock().get_sigaction(signal);
        let bytes = unsafe {
            core::slice::from_raw_parts(
                (&old as *const Sigaction).cast::<u8>(),
                core::mem::size_of::<Sigaction>(),
            )
        };
        if copy_to_user(&task, oldact_ptr, bytes).is_err() {
            return errno::to_result(errno::EFAULT);
        }
    }
    if let Some(action) = new_action {
        abi.signal_state.lock().install_sigaction(signal, action);
    }
    0
}

/// Convert internal SignalAction to Linux sigaction
fn sigaction_to_linux(action: SignalAction) -> Sigaction {
    match action {
        SignalAction::Ignore => Sigaction {
            handler: SIG_IGN,
            flags: 0,
            #[cfg(target_arch = "aarch64")]
            restorer: 0,
            mask: 0,
        },
        SignalAction::Custom(addr) => Sigaction {
            handler: addr,
            flags: 0,
            #[cfg(target_arch = "aarch64")]
            restorer: 0,
            mask: 0,
        },
        _ => Sigaction {
            handler: SIG_DFL,
            flags: 0,
            #[cfg(target_arch = "aarch64")]
            restorer: 0,
            mask: 0,
        },
    }
}

/// Convert Linux sigaction to internal SignalAction
fn linux_to_sigaction(sigaction: Sigaction, signal: LinuxSignal) -> SignalAction {
    match sigaction.handler {
        SIG_DFL => signal.default_action(), // Restore per-signal default action
        SIG_IGN => SignalAction::Ignore,
        addr => SignalAction::Custom(addr),
    }
}

/// Linux rt_sigprocmask system call implementation
///
/// int rt_sigprocmask(int how, const sigset_t *set, sigset_t *oldset, size_t sigsetsize);
pub fn sys_rt_sigprocmask(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    use crate::library::std::usercopy::{copy_from_user, copy_to_user};
    let task = mytask().unwrap();
    let how = trapframe.get_arg(0);
    let set_ptr = trapframe.get_arg(1);
    let oldset_ptr = trapframe.get_arg(2);
    let sigsetsize = trapframe.get_arg(3);
    trapframe.increment_pc_next(&task);
    if sigsetsize != 8 || (set_ptr != 0 && how > 2) {
        return errno::to_result(errno::EINVAL);
    }
    // Read first: set and oldset may point to the same userspace object.
    let new_mask = if set_ptr != 0 {
        let mut bytes = [0u8; 8];
        if copy_from_user(&task, set_ptr, &mut bytes).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        Some(u64::from_ne_bytes(bytes))
    } else {
        None
    };
    let old_mask = abi.signal_state.lock().blocked.raw();
    // This must break COW after fork instead of writing the shared physical page.
    if oldset_ptr != 0 && copy_to_user(&task, oldset_ptr, &old_mask.to_ne_bytes()).is_err() {
        return errno::to_result(errno::EFAULT);
    }
    if let Some(new_mask) = new_mask {
        let mask = match how {
            0 => old_mask | new_mask,
            1 => old_mask & !new_mask,
            _ => new_mask,
        };
        let unmaskable = (1u64 << (LinuxSignal::SIGKILL as u32 - 1))
            | (1u64 << (LinuxSignal::SIGSTOP as u32 - 1));
        abi.signal_state.lock().blocked.set_raw(mask & !unmaskable);
    }
    0
}

/// Atomically replace the signal mask and wait for a caught or fatal signal.
/// The architecture must save suspend_restore_mask in its signal frame before
/// exposing this syscall; only AArch64 currently has that frame contract.
#[cfg(target_arch = "aarch64")]
pub fn sys_rt_sigsuspend(abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    use crate::library::std::usercopy::copy_from_user;
    let Some(task) = mytask() else { return errno::to_result(errno::ESRCH); };
    let mask_address = frame.get_arg(0);
    let size = frame.get_arg(1);
    frame.increment_pc_next(&task);
    if size != 8 { return errno::to_result(errno::EINVAL); }
    let mut bytes = [0; 8];
    if copy_from_user(&task, mask_address, &mut bytes).is_err() {
        return errno::to_result(errno::EFAULT);
    }
    let unmaskable = (1u64 << (LinuxSignal::SIGKILL as u32 - 1))
        | (1u64 << (LinuxSignal::SIGSTOP as u32 - 1));
    abi.bind_task_signals(&task);
    let state = abi.signal_state.clone();
    let waker = {
        let mut state = state.lock();
        state.suspend_restore_mask = Some(state.blocked.raw());
        state.blocked.set_raw(u64::from_ne_bytes(bytes) & !unmaskable);
        state.pending_waker.clone()
    };
    loop {
        // Native child-exit and other process-control events normally become
        // signals at user return. Import them while suspended so masked and
        // ignored signals cannot incorrectly complete this wait. Non-signal
        // events remain queued for their own consumers.
        loop {
            let event = task.event_queue.lock()
                .dequeue_matching(|event| handle_event_to_signal(event).is_some());
            let Some(event) = event else { break; };
            if !is_pending_wake(&event) {
                state.lock().add_pending(handle_event_to_signal(&event).unwrap());
            }
        }
        if task.termination_signal().is_some() {
            return errno::to_result(errno::EINTR);
        }
        let action = {
            let mut state = state.lock();
            state.next_deliverable_signal().map(|signal| {
                let action = state.get_handler(signal);
                if matches!(action, SignalAction::Ignore | SignalAction::Continue | SignalAction::Stop) {
                    state.remove_pending(signal);
                }
                action
            })
        };
        match action {
            Some(SignalAction::Custom(_) | SignalAction::Terminate | SignalAction::ForceTerminate) => {
                // Leave the signal pending for the ordinary delivery path.
                // Its frame saves -EINTR and the mask from before sigsuspend.
                return errno::to_result(errno::EINTR);
            }
            Some(SignalAction::Ignore | SignalAction::Continue) => continue,
            Some(SignalAction::Stop) => {
                // Keep Continue's enqueue/wake atomic with entering Stop.
                let events = task.event_queue.lock();
                if events.has_pending_continue() { continue; }
                stop_target_for_signal(&task);
                drop(events);
                crate::sched::scheduler::schedule(frame);
                continue;
            }
            None => {}
        }
        // Registration rechecks both queues, closing the signal-arrival race.
        // Spurious wakes do not return to userspace or restore the old mask.
        waker.wait_with_condition(task.get_id(), frame, None, 0, || {
            state.lock().next_deliverable_signal().is_some()
                || task.has_pending_process_control()
                || task.termination_signal().is_some()
        });
    }
}

/// Consume one matching record only after its optional userspace copy succeeds.
fn take_waited_signal(
    task: &Task,
    state: &Arc<crate::sync::IrqSpinLock<SignalState>>,
    mask: u64,
    info: usize,
) -> Result<Option<usize>, usize> {
    let Some(mut record) = reserve_pending(state, mask) else { return Ok(None); };
    if info != 0 {
        crate::library::std::usercopy::copy_to_user(task, info, &record.info.bytes)
            .map_err(|_| errno::EFAULT)?;
    }
    record.finish(false);
    Ok(Some(record.signal as usize))
}

/// Wait for a signal in a userspace set, with a relative optional timeout.
/// Thread-directed records take precedence over process-directed records for
/// the same number, matching the handler and signalfd consumers.
pub fn sys_rt_sigtimedwait(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    use crate::library::std::usercopy::copy_from_user;
    use crate::sync::waker::WaitResult;
    let Some(task) = mytask() else {
        return errno::to_result(errno::ESRCH);
    };
    let mask_ptr = trapframe.get_arg(0);
    let info = trapframe.get_arg(1);
    let timeout = trapframe.get_arg(2);
    let size = trapframe.get_arg(3);
    trapframe.increment_pc_next(&task);
    if size != 8 {
        return errno::to_result(errno::EINVAL);
    }
    let mut bytes = [0; 8];
    if copy_from_user(&task, mask_ptr, &mut bytes).is_err() {
        return errno::to_result(errno::EFAULT);
    }
    let unmaskable =
        (1u64 << (LinuxSignal::SIGKILL as u32 - 1)) | (1u64 << (LinuxSignal::SIGSTOP as u32 - 1));
    let mask = u64::from_ne_bytes(bytes) & !unmaskable;
    let deadline = if timeout != 0 {
        let mut bytes = [0; 16];
        if copy_from_user(&task, timeout, &mut bytes).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        let seconds = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
        let nanos = i64::from_ne_bytes(bytes[8..].try_into().unwrap());
        if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
            return errno::to_result(errno::EINVAL);
        }
        let duration =
            (seconds as u128 * 1_000_000_000 + nanos as u128).min(u64::MAX as u128) as u64;
        Some(crate::timer::get_time_ns().saturating_add(duration))
    } else {
        None
    };
    abi.bind_task_signals(&task);
    let state = abi.signal_state.clone();
    let waker = state.lock().pending_waker.clone();
    let mut interrupted = false;
    loop {
        {
            match take_waited_signal(&task, &state, mask, info) {
                Ok(Some(number)) => return number,
                Err(error) => return errno::to_result(error),
                Ok(None) => {}
            }
            if interrupted || state.lock().next_deliverable_signal().is_some() {
                return errno::to_result(errno::EINTR);
            }
        }
        let remaining = deadline.map(|end| end.saturating_sub(crate::timer::get_time_ns()));
        if remaining == Some(0) {
            return errno::to_result(errno::EAGAIN);
        }
        // The native Waker checks readiness again after registration, closing
        // the signal-arrives-before-sleep window without holding SignalState
        // while scheduling. Spurious/unrelated wakes retain the original deadline.
        interrupted = waker.wait_with_condition(task.get_id(), trapframe, remaining, 0, || {
            let state = state.lock();
            state.pending_mask() & mask != 0 || state.next_deliverable_signal().is_some()
        }) == WaitResult::Interrupted;
    }
}

fn process_signal_members(target: &Task) -> alloc::vec::Vec<Arc<Task>> {
    let group = target.get_thread_group_id();
    crate::sched::scheduler::get_task_pool().task_ids_snapshot()
        .into_iter().filter_map(get_task_by_id)
        .filter(|member| member.get_thread_group_id() == group
            && !matches!(member.get_state(), crate::task::TaskState::Zombie | crate::task::TaskState::Terminated))
        .collect()
}

fn is_stop_signal(signal: LinuxSignal) -> bool {
    matches!(signal, LinuxSignal::SIGSTOP | LinuxSignal::SIGTSTP | LinuxSignal::SIGTTIN | LinuxSignal::SIGTTOU)
}

fn is_job_signal(signal: LinuxSignal) -> bool { signal == LinuxSignal::SIGCONT || is_stop_signal(signal) }

fn queue_process_signal(current: &Task, target: &Task, signal: LinuxSignal,
    info: SignalInfo) -> Result<(), usize> {
    let members = process_signal_members(target);
    let states: alloc::vec::Vec<_> = members.iter().filter_map(|member| {
        member.linux_signal_state.lock().as_ref().and_then(|state| state.upgrade())
            .map(|state| (member, state))
    }).collect();
    let Some((_, state)) = states.first() else { return Err(errno::ESRCH); };
    let epoch = state.lock().publish_process_signal(signal, info)?;
    if is_job_signal(signal) {
        for (member, state) in &states {
            let shared = {
                let mut state = state.lock();
                let shared = state.shared_pending.clone();
                if shared.lock().control_epoch == epoch { state.pending.discard_job_opposite(signal); }
                shared
            };
            if signal == LinuxSignal::SIGCONT {
                // Serialize the resume effect with a later accepted stop.
                // No event-queue lock is held while acquiring the shared lock.
                let mut events = member.event_queue.lock();
                let shared = shared.lock();
                if shared.control_epoch == epoch {
                    events.enqueue(pending_wake_event(member, signal, info, Some(current.get_id() as u32)));
                    wake_target_for_signal(member);
                }
            }
        }
    }
    // sigwait/signalfd use the shared waker. An unblocked handler may instead
    // be sleeping in another interruptible native operation: route a native
    // control event to one eligible thread to interrupt that operation. The
    // event carries data but never requeues a record already consumed elsewhere.
    if signal != LinuxSignal::SIGCONT {
        notify_process_signal(target, signal, info, Some(current.get_id() as u32), None);
    }
    Ok(())
}

fn notify_process_signal(target: &Task, signal: LinuxSignal, info: SignalInfo,
    sender: Option<u32>, exclude: Option<usize>) {
    for member in process_signal_members(target) {
        if exclude == Some(member.get_id()) { continue; }
        let state = member.linux_signal_state.lock().as_ref().and_then(|state| state.upgrade());
        let Some(state) = state else { continue; };
        let (eligible, shared) = {
            let state = state.lock();
            (!state.blocked.is_blocked(signal), state.shared_pending.clone())
        };
        if eligible {
            let mut events = member.event_queue.lock();
            let shared = shared.lock();
            if shared.peek(signal).is_none() { return; }
            events.enqueue(pending_wake_event(&member, signal, info, sender));
            crate::sched::scheduler::wake_task(member.get_id());
            break;
        }
    }
}

fn pending_wake_event(target: &Task, signal: LinuxSignal, info: SignalInfo, sender: Option<u32>) -> Event {
    let mut event = Event::immediate_process_control(target.get_id() as u32, signal_process_control(signal).unwrap());
    let mut payload = PENDING_WAKE.to_vec();
    payload.extend_from_slice(&info.bytes);
    event.payload = EventPayload::Custom(payload);
    event.metadata.sender = sender;
    event
}

pub(super) fn deliver_process_signal(abi: &LinuxAbi, current: &Task, target: &Task,
    signal: LinuxSignal) -> usize {
    if matches!(signal, LinuxSignal::SIGKILL | LinuxSignal::SIGSTOP)
        || target.linux_signal_state.lock().as_ref().and_then(|state| state.upgrade()).is_none() {
        return deliver_signal(abi, current, target, signal);
    }
    let mut info = SignalInfo::kernel(signal);
    info.bytes[8..12].copy_from_slice(&0i32.to_ne_bytes()); // SI_USER
    let sender = target.get_namespace().resolve_local_id(current.get_thread_group_id()).unwrap_or(0);
    info.bytes[16..20].copy_from_slice(&(sender as i32).to_ne_bytes());
    info.bytes[20..24].copy_from_slice(&current.credentials.read().principals().base.0.to_ne_bytes());
    queue_process_signal(current, target, signal, info).map_or_else(errno::to_result, |_| 0)
}

/// Send a queued process signal using the existing native event transport.
/// Process-directed records remain available to every thread in the group,
/// including a blocked SignalCatcher using sigwait. PID/UID for SI_QUEUE are
/// derived from the sender and translated into the receiver's PID namespace.
pub fn sys_rt_sigqueueinfo(abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    let Some(caller) = mytask() else { return errno::to_result(errno::ESRCH); };
    let pid = frame.get_arg(0) as i32;
    let number = frame.get_arg(1) as i32;
    let address = frame.get_arg(2);
    frame.increment_pc_next(&caller);
    if pid <= 0 || !(0..=64).contains(&number) { return errno::to_result(errno::EINVAL); }
    let mut info = SignalInfo { bytes: [0; 128] };
    if crate::library::std::usercopy::copy_from_user(&caller, address, &mut info.bytes).is_err() {
        return errno::to_result(errno::EFAULT);
    }
    let Some(target) = resolve_task_by_linux_tid(&caller, pid) else { return errno::to_result(errno::ESRCH); };
    let code = i32::from_ne_bytes(info.bytes[8..12].try_into().unwrap());
    if code >= 0 || code == -6 && target.get_thread_group_id() != caller.get_thread_group_id() {
        return errno::to_result(errno::EPERM);
    }
    if number == 0 { return 0; }
    let signal = LinuxSignal::from_u32(number as u32).unwrap();
    info.bytes[..4].copy_from_slice(&number.to_ne_bytes());
    if code == -1 {
        let sender = target.get_namespace().resolve_local_id(caller.get_thread_group_id()).unwrap_or(0);
        info.bytes[16..20].copy_from_slice(&(sender as i32).to_ne_bytes());
        info.bytes[20..24].copy_from_slice(&caller.credentials.read().principals().base.0.to_ne_bytes());
    }
    if matches!(signal, LinuxSignal::SIGKILL | LinuxSignal::SIGSTOP) {
        return deliver_signal(abi, &caller, &target, signal);
    }
    queue_process_signal(&caller, &target, signal, info).map_or_else(errno::to_result, |_| 0)
}

/// Convert Scarlet ProcessControlType to Linux signal
pub fn process_control_to_signal(control_type: ProcessControlType) -> Option<LinuxSignal> {
    match control_type {
        ProcessControlType::Terminate => Some(LinuxSignal::SIGTERM),
        ProcessControlType::Kill => Some(LinuxSignal::SIGKILL),
        ProcessControlType::Stop => Some(LinuxSignal::SIGSTOP),
        ProcessControlType::Continue => Some(LinuxSignal::SIGCONT),
        ProcessControlType::Interrupt => Some(LinuxSignal::SIGINT),
        ProcessControlType::Quit => Some(LinuxSignal::SIGQUIT),
        ProcessControlType::TerminalStop => Some(LinuxSignal::SIGTSTP),
        ProcessControlType::TerminalInput => Some(LinuxSignal::SIGTTIN),
        ProcessControlType::TerminalOutput => Some(LinuxSignal::SIGTTOU),
        ProcessControlType::WindowChange => Some(LinuxSignal::SIGWINCH),
        ProcessControlType::Hangup => Some(LinuxSignal::SIGHUP),
        ProcessControlType::ChildExit => Some(LinuxSignal::SIGCHLD),
        ProcessControlType::PipeBroken => Some(LinuxSignal::SIGPIPE),
        ProcessControlType::Alarm => Some(LinuxSignal::SIGALRM),
        ProcessControlType::IoReady => Some(LinuxSignal::SIGIO),
        ProcessControlType::User(sig) if (257..288).contains(&sig) => LinuxSignal::from_u32(sig - 256),
        ProcessControlType::User(sig) => LinuxSignal::from_u32(sig + 32), // Map to RT signals
    }
}

/// Handle incoming event and convert to signal if needed
pub fn handle_event_to_signal(event: &Event) -> Option<LinuxSignal> {
    match &event.content {
        EventContent::ProcessControl(control_type) => process_control_to_signal(*control_type),
        _ => None, // Non-signal events are ignored in Linux ABI
    }
}

pub(crate) fn is_pending_wake(event: &Event) -> bool {
    matches!(&event.payload, EventPayload::Custom(bytes)
        if bytes.len() == PENDING_WAKE.len() + 128 && bytes.starts_with(PENDING_WAKE))
}

/// Handle an incoming Scarlet event as a Linux signal for a target task.
///
/// # Arguments
///
/// * `abi` - Linux ABI state that owns the signal handler table
/// * `event` - Scarlet event to translate
/// * `target_task_id` - Kernel task ID that receives the event
/// * `setup_signal_handler` - Architecture-specific signal-frame setup callback
///
/// # Returns
///
/// `Ok(outcome)` describing how event processing should proceed, otherwise an
/// error if the target task could not be found.
pub(crate) fn handle_event_for_task(
    abi: &LinuxAbi,
    event: &Event,
    target_task_id: usize,
    setup_signal_handler: impl Fn(
        &LinuxAbi,
        &Task,
        &mut Trapframe,
        usize,
        LinuxSignal,
        SignalInfo,
    ) -> Result<(), &'static str>,
) -> Result<EventProcessOutcome, &'static str> {
    let Some(signal) = handle_event_to_signal(event) else {
        return Ok(EventProcessOutcome::Continue);
    };
    let (action, info, epoch, shared) = {
        let mut signal_state = abi.signal_state.lock();
        if signal != LinuxSignal::SIGKILL
            && signal != LinuxSignal::SIGSTOP
            && signal_state.blocked.is_blocked(signal)
        {
            if !is_pending_wake(event) { signal_state.add_pending(signal); }
            else {
                drop(signal_state);
                if let Some(target) = get_task_by_id(target_task_id) {
                    notify_process_signal(&target, signal, SignalInfo::kernel(signal),
                        event.metadata.sender, Some(target_task_id));
                }
            }
            return Ok(EventProcessOutcome::Continue);
        }
        let (info, epoch) = if is_pending_wake(event) {
            // Another thread (including sigwait) can already have consumed
            // this process record. The wake event must not recreate it.
            let Some(record) = signal_state.take_pending_with_epoch(signal) else {
                return Ok(EventProcessOutcome::Continue);
            };
            record
        } else { (SignalInfo::kernel(signal), signal_state.shared_pending.lock().control_epoch) };
        (signal_state.get_handler(signal), info, epoch, signal_state.shared_pending.clone())
    };

    // Preserve owned lookup semantics for nonfatal and remote targets. Fatal
    // delivery returns an exit request; Task performs the actual exit after its
    // ABI mutable borrow has been released.
    let target_task =
        crate::sched::scheduler::get_task_by_id(target_task_id).ok_or("Target task not found")?;

    let outcome = match action {
        SignalAction::Custom(handler_addr) => {
            let trapframe = target_task.get_trapframe();
            match setup_signal_handler(abi, &target_task, trapframe, handler_addr, signal, info) {
                Ok(()) => EventProcessOutcome::UserHandlerArmed,
                Err(error) => {
                    crate::println!("[linux] signal frame failed: {}", error);
                    target_task.mark_signal_termination(LinuxSignal::SIGSEGV as u8);
                    EventProcessOutcome::Exited(128 + LinuxSignal::SIGSEGV as i32)
                }
            }
        }
        SignalAction::Ignore => EventProcessOutcome::Continue,
        SignalAction::ForceTerminate | SignalAction::Terminate => {
            crate::println!(
                "[linux] fatal signal={} sender={:?} target={} name={}",
                signal as u32,
                event.metadata.sender,
                target_task.get_id(),
                target_task.name.read().as_str()
            );
            target_task.mark_signal_termination(signal as u8);
            let exit_code = 128 + (signal as i32);
            EventProcessOutcome::Exited(exit_code)
        }
        SignalAction::Stop => {
            if apply_signal_stop(&target_task, &shared, epoch) { EventProcessOutcome::NeedReschedule }
                else { EventProcessOutcome::Continue }
        }
        SignalAction::Continue => {
            let shared = shared.lock();
            if shared.control_epoch != epoch { return Ok(EventProcessOutcome::Continue); }
            let current_state = target_task.get_state();
            if matches!(current_state, crate::task::TaskState::Blocked(_)) {
                target_task.set_state(crate::task::TaskState::Ready);
                crate::sched::scheduler::unmark_blocked(target_task.get_id());
                crate::sched::scheduler::push_ready_task(
                    crate::arch::get_cpu().get_cpuid(),
                    target_task.get_id(),
                );
            }
            EventProcessOutcome::Continue
        }
    };

    Ok(outcome)
}

/// Deliver a signal to a task's signal state
pub fn deliver_signal_to_task(abi: &LinuxAbi, signal: LinuxSignal) {
    // Add signal to pending if it's not already pending
    let mut signal_state = abi.signal_state.lock();
    if !signal_state.is_pending(signal) {
        signal_state.add_pending(signal);
    }
}

/// Check if task has pending signals and return the next one to handle
pub fn get_next_pending_signal(abi: &LinuxAbi) -> Option<LinuxSignal> {
    let signal_state = abi.signal_state.lock();
    signal_state.next_deliverable_signal()
}

/// Process pending signals and dispatch to the supplied handler-frame setup.
/// Returns true if execution should be interrupted.
pub fn process_pending_signals_with_setup(
    signal_state: &mut SignalState,
    trapframe: &mut Trapframe,
    setup_signal_handler: fn(&mut Trapframe, usize, LinuxSignal),
) -> bool {
    if let Some(signal) = signal_state.next_deliverable_signal() {
        let action = signal_state.get_handler(signal);
        signal_state.remove_pending(signal);

        match action {
            SignalAction::Terminate | SignalAction::ForceTerminate => {
                crate::println!("Signal {}: Terminating task", signal as u32);
                true
            }
            SignalAction::Ignore => false,
            SignalAction::Stop => {
                crate::println!("Signal {}: Stopping task", signal as u32);
                true
            }
            SignalAction::Continue => {
                crate::println!("Signal {}: Continuing task", signal as u32);
                false
            }
            SignalAction::Custom(handler_addr) => {
                crate::println!(
                    "Signal {}: Calling custom handler at {:#x}",
                    signal as u32,
                    handler_addr
                );
                setup_signal_handler(trapframe, handler_addr, signal);
                true
            }
        }
    } else {
        false
    }
}

/// Process pending signals without installing a custom handler frame.
///
/// Arch-specific ABI modules should prefer `process_pending_signals_with_setup`
/// and pass their own signal-frame builder. This fallback exists for generic
/// callers that only need default actions.
pub fn process_pending_signals_with_state(
    signal_state: &mut SignalState,
    trapframe: &mut Trapframe,
) -> bool {
    process_pending_signals_with_setup(signal_state, trapframe, |_trapframe, _handler, _signal| {})
}

/// Handle fatal signals that should terminate immediately
pub fn handle_fatal_signal_immediately(signal: LinuxSignal) -> Result<(), &'static str> {
    if let Some(task) = crate::task::mytask() {
        let exit_code = match signal {
            LinuxSignal::SIGKILL => 128 + 9,
            LinuxSignal::SIGTERM => 128 + 15,
            LinuxSignal::SIGINT => 128 + 2,
            _ => return Err("Not a fatal signal"),
        };

        crate::println!(
            "Signal {}: Immediately terminating task {} with exit code {}",
            signal as u32,
            task.get_id(),
            exit_code
        );

        task.mark_signal_termination(signal as u8);
        task.exit_group(exit_code);
        Ok(())
    } else {
        Err("No current task to terminate")
    }
}

pub fn is_fatal_signal(signal: LinuxSignal) -> bool {
    matches!(
        signal,
        LinuxSignal::SIGKILL | LinuxSignal::SIGTERM | LinuxSignal::SIGINT
    )
}

/// Linux wait status reported when a task is killed by `signal`.
///
/// Scarlet currently stores shell-style process statuses, so signal deaths use
/// `128 + signal`. Task records the signal separately, and the Linux wait ABI
/// encodes it without confusing a normal exit(128 + signal) with signal death.
///
/// # Arguments
///
/// * `signal` - Fatal signal being delivered
///
/// # Returns
///
/// Exit status encoding the signal death.
fn signal_death_status(signal: LinuxSignal) -> i32 {
    128 + signal as i32
}

/// Find the task currently owning a Linux TID (namespace ID).
///
/// # Arguments
///
/// * `current` - Calling task whose PID namespace defines `tid`
/// * `tid` - Linux thread ID as seen by the caller
///
/// # Returns
///
/// The task owning the TID, or `None` if no live task matches.
fn resolve_task_by_linux_tid(current: &Task, tid: i32) -> Option<Arc<Task>> {
    if tid <= 0 {
        return None;
    }
    let global_id = current.get_namespace().resolve_global_id(tid as usize)?;
    get_task_by_id(global_id).filter(|task| matches!(task.task_type, TaskType::User))
}

/// Wake a task that may be blocked so it can observe kernel state changes.
///
/// # Arguments
///
/// * `target` - Task to make runnable again
fn wake_target_for_signal(target: &Task) {
    if matches!(target.get_state(), crate::task::TaskState::Blocked(_)) {
        target.set_state(crate::task::TaskState::Ready);
        unmark_blocked(target.get_id());
        push_ready_task(crate::arch::get_cpu().get_cpuid(), target.get_id());
    }
}

/// Move a task into the stopped state.
///
/// # Arguments
///
/// * `target` - Task to stop
fn stop_target_for_signal(target: &Task) {
    target.set_state(crate::task::TaskState::Blocked(
        crate::task::BlockedType::Interruptible,
    ));
    mark_blocked(target.get_id());
    remove_from_ready_queues(target.get_id());
}

fn apply_signal_stop(target: &Task, shared: &crate::sync::IrqSpinLock<PendingSignals>, epoch: u64) -> bool {
    let events = target.event_queue.lock();
    let shared = shared.lock();
    if shared.control_epoch != epoch || events.has_pending_continue() { return false; }
    stop_target_for_signal(target);
    true
}

/// Deliver a signal to the calling task itself.
///
/// The action is resolved through the caller's installed handler table, so
/// applications that installed handlers keep the historical permissive
/// behavior (the syscall still succeeds), while unhandled fatal signals now
/// terminate the thread group with a signal wait status. This is what musl's
/// `abort()` path depends on: it restores the default SIGABRT disposition,
/// unblocks the signal, and raises it.
///
/// # Arguments
///
/// * `abi` - Linux ABI state that owns the signal handler table
/// * `task` - Currently running task
/// * `signal` - Signal to deliver
fn deliver_signal_to_self(abi: &LinuxAbi, task: &Task, signal: LinuxSignal,
    job: Option<&(Arc<crate::sync::IrqSpinLock<PendingSignals>>, u64)>) {
    let action = {
        let mut signal_state = abi.signal_state.lock();
        if signal != LinuxSignal::SIGKILL && signal_state.blocked.is_blocked(signal) {
            signal_state.add_pending(signal);
            return;
        }
        signal_state.get_handler(signal)
    };

    match action {
        SignalAction::Terminate | SignalAction::ForceTerminate => {
            let status = signal_death_status(signal);
            crate::println!(
                "[linux] signal {} terminating task {} (PID {}) with status {}",
                signal as u32,
                task.get_id(),
                task.try_get_namespace_id().unwrap_or(0),
                status
            );
            task.mark_signal_termination(signal as u8);
            task.request_deferred_exit_group(status);
        }
        SignalAction::Stop => {
            if let Some((shared, epoch)) = job { apply_signal_stop(task, shared, *epoch); }
            else { stop_target_for_signal(task); }
        }
        SignalAction::Ignore | SignalAction::Continue | SignalAction::Custom(_) => {}
    }
}

/// Reverse the process-control mapping for delivery on the receiver's own
/// user-return boundary. Never build a frame in a remotely running thread.
fn signal_process_control(signal: LinuxSignal) -> Option<ProcessControlType> {
    [
        ProcessControlType::Terminate,
        ProcessControlType::Kill,
        ProcessControlType::Stop,
        ProcessControlType::Continue,
        ProcessControlType::Interrupt,
        ProcessControlType::Quit,
        ProcessControlType::TerminalStop,
        ProcessControlType::TerminalInput,
        ProcessControlType::TerminalOutput,
        ProcessControlType::WindowChange,
        ProcessControlType::Hangup,
        ProcessControlType::ChildExit,
        ProcessControlType::PipeBroken,
        ProcessControlType::Alarm,
        ProcessControlType::IoReady,
    ]
    .into_iter()
    .find(|control| process_control_to_signal(*control) == Some(signal))
    .or_else(|| Some(if signal as u32 >= 32 {
        ProcessControlType::User(signal as u32 - 32)
    } else { ProcessControlType::User(256 + signal as u32) }))
}

/// Honor the receiver's disposition. Wine registers SIGQUIT to terminate a
/// Windows thread; applying SIGQUIT's default here killed its whole process.
fn deliver_signal_to_remote(current: &Task, target: &Task, signal: LinuxSignal) -> usize {
    deliver_signal_to_remote_with_job(current, target, signal, None)
}

fn deliver_signal_to_remote_with_job(current: &Task, target: &Task, signal: LinuxSignal,
    job: Option<&(Arc<crate::sync::IrqSpinLock<PendingSignals>>, u64)>) -> usize {
    // An unreaped task still exists for kill(), but cannot receive a signal.
    // In particular, Wine's delayed SIGKILL after a process has exited must
    // not replace the recorded cause of death observed by wait4()/waitid().
    if matches!(
        target.get_state(),
        crate::task::TaskState::Zombie | crate::task::TaskState::Terminated
    ) {
        return 0;
    }
    let state = target
        .linux_signal_state
        .lock()
        .as_ref()
        .and_then(|state| state.upgrade());
    let action = if let Some(state) = state {
        let mut state = state.lock();
        if signal != LinuxSignal::SIGKILL
            && signal != LinuxSignal::SIGSTOP
            && state.blocked.is_blocked(signal)
        {
            state.add_pending(signal);
            return 0;
        }
        state.get_handler(signal)
    } else {
        signal.default_action()
    };
    match action {
        SignalAction::Custom(_) => {
            let Some(control) = signal_process_control(signal) else {
                // Unsupported asynchronous signal delivery must not silently
                // use the default fatal action or falsely report success.
                return errno::to_result(errno::ENOSYS);
            };
            let mut event = Event::immediate_process_control(target.get_id() as u32, control);
            event.metadata.sender = Some(current.get_id() as u32);
            target.event_queue.lock().enqueue(event);
            crate::sched::scheduler::wake_task(target.get_id());
        }
        SignalAction::Terminate | SignalAction::ForceTerminate => {
            let status = signal_death_status(signal);
            if !target.request_fatal_signal_exit_group(signal as u8, status) {
                return 0;
            }
            crate::println!(
                "[linux] fatal signal={} sender={} target={} name={} status={}",
                signal as u32,
                current.get_id(),
                target.get_id(),
                target.name.read().as_str(),
                status
            );
            wake_target_for_signal(target);
        }
        SignalAction::Stop => {
            if let Some((shared, epoch)) = job { apply_signal_stop(target, shared, *epoch); }
            else { stop_target_for_signal(target); }
        }
        SignalAction::Continue => wake_target_for_signal(target),
        SignalAction::Ignore => {}
    }
    0
}

/// Route signal delivery between the calling task and a remote target.
///
/// # Arguments
///
/// * `abi` - Linux ABI state of the calling task
/// * `current` - Currently running task
/// * `target` - Task the signal is addressed to
/// * `signal` - Signal to deliver
///
/// Returns a Linux syscall result. Supported process-control signals are
/// dispatched on the remote target; unsupported custom signals return ENOSYS.
/// AArch64 self-signals are dispatched by its pending-handler return hook.
pub fn deliver_signal(abi: &LinuxAbi, current: &Task, target: &Task, signal: LinuxSignal) -> usize {
    let mut job = None;
    if is_job_signal(signal) {
        let state = target.linux_signal_state.lock().as_ref().and_then(|state| state.upgrade());
        if let Some(state) = state {
            if signal == LinuxSignal::SIGCONT {
                return deliver_process_signal(abi, current, target, signal);
            }
            // Thread-addressed stop controls also supersede a previously
            // accepted process continuation, before they can stop the task.
            let shared = state.lock().shared_pending.clone();
            let epoch = {
                let mut pending = shared.lock();
                pending.accept_job_control(signal);
                pending.control_epoch
            };
            job = Some((shared, epoch));
            for member in process_signal_members(target) {
                if let Some(state) = member.linux_signal_state.lock().as_ref().and_then(|state| state.upgrade()) {
                    state.lock().pending.discard_job_opposite(signal);
                }
            }
        }
    }
    // tgkill/tkill address a particular thread, even within our thread group.
    let is_self = target.get_id() == current.get_id();
    if is_self {
        let state = abi.signal_state.lock();
        let unsupported_handler = matches!(state.get_handler(signal), SignalAction::Custom(_))
            && !state.blocked.is_blocked(signal);
        drop(state);
        if unsupported_handler {
            #[cfg(target_arch = "aarch64")]
            {
                abi.signal_state.lock().add_pending(signal);
                return 0; // aarch64's user-return hook installs the frame.
            }
            #[cfg(not(target_arch = "aarch64"))]
            return errno::to_result(errno::ENOSYS);
        }
        deliver_signal_to_self(abi, current, signal, job.as_ref());
    } else {
        return deliver_signal_to_remote_with_job(current, target, signal, job.as_ref());
    }
    0
}

/// Deliver unblocked pending signals when returning from a syscall.
///
/// Linux delivers a pending signal as soon as it becomes unblocked and the
/// task returns to user space. musl's `abort()` relies on exactly this: it
/// raises SIGABRT while the signal may still be blocked, unblocks it, and
/// expects the kernel to perform the default termination.
///
/// # Arguments
///
/// * `abi` - Linux ABI state of the calling task
pub fn deliver_pending_signals(abi: &mut LinuxAbi) {
    let Some(task) = mytask() else {
        return;
    };

    loop {
        let deliverable = {
            let signal_state = abi.signal_state.lock();
            match signal_state.next_deliverable_signal() {
                Some(signal) => match signal_state.get_handler(signal) {
                    SignalAction::Custom(_) => None,
                    _ => Some(signal),
                },
                None => None,
            }
        };
        let Some(signal) = deliverable else {
            return;
        };

        let (action, epoch, shared) = {
            let mut signal_state = abi.signal_state.lock();
            let Some((_, epoch)) = signal_state.take_pending_with_epoch(signal) else { continue; };
            (signal_state.get_handler(signal), epoch, signal_state.shared_pending.clone())
        };

        match action {
            SignalAction::Terminate | SignalAction::ForceTerminate => {
                let status = signal_death_status(signal);
                crate::println!(
                    "[linux] pending signal {} terminating task {} (PID {}) with status {}",
                    signal as u32,
                    task.get_id(),
                    task.try_get_namespace_id().unwrap_or(0),
                    status
                );
                task.mark_signal_termination(signal as u8);
                task.request_deferred_exit_group(status);
                return;
            }
            SignalAction::Stop => {
                if apply_signal_stop(&task, &shared, epoch) { return; }
            }
            SignalAction::Ignore | SignalAction::Continue | SignalAction::Custom(_) => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test_case]
    fn waited_signal_validates_output_before_consuming_and_reports_known_info() {
        use crate::environment::{PAGE_SIZE, USER_STACK_END};
        use crate::library::std::usercopy::copy_from_user;
        let task = crate::task::new_user_task("waited-signal".into(), 0);
        let info = USER_STACK_END - PAGE_SIZE;
        task.allocate_stack_pages(info, 1).unwrap();
        let state = Arc::new(crate::sync::IrqSpinLock::new(SignalState::new()));
        state.lock().add_pending(LinuxSignal::SIGUSR1);
        state.lock().add_pending(LinuxSignal::SIGUSR2);
        let mask = 1u64 << (LinuxSignal::SIGUSR1 as u32 - 1);
        task.vm_manager
            .protect_linux_memory_map_range(
                crate::vm::vmem::MemoryArea::new(info, USER_STACK_END - 1),
                1,
            )
            .unwrap();
        assert_eq!(
            take_waited_signal(&task, &state, mask, info),
            Err(errno::EFAULT)
        );
        assert!(state.lock().is_pending(LinuxSignal::SIGUSR1));
        task.vm_manager
            .protect_linux_memory_map_range(
                crate::vm::vmem::MemoryArea::new(info, USER_STACK_END - 1),
                3,
            )
            .unwrap();
        assert_eq!(
            take_waited_signal(&task, &state, mask, info),
            Ok(Some(10))
        );
        let mut record = [0xff; 128];
        copy_from_user(&task, info, &mut record).unwrap();
        assert_eq!(u32::from_ne_bytes(record[..4].try_into().unwrap()), 10);
        assert_eq!(i32::from_ne_bytes(record[8..12].try_into().unwrap()), 128);
        assert_eq!(&record[16..], &[0; 112]);
        assert!(!state.lock().is_pending(LinuxSignal::SIGUSR1));
        assert!(state.lock().is_pending(LinuxSignal::SIGUSR2));
        assert_eq!(take_waited_signal(&task, &state, mask, 0), Ok(None));
    }

    #[test_case]
    fn sigtimedwait_validates_inputs_zero_timeout_and_unmaskable_signals() {
        use crate::environment::{PAGE_SIZE, USER_STACK_END};
        use crate::library::std::usercopy::copy_to_user;
        use crate::task::{clear_mock_current_task, set_mock_current_task};
        let task = Arc::new(crate::task::new_user_task("sigtimedwait-input".into(), 0));
        let base = USER_STACK_END - PAGE_SIZE;
        task.allocate_stack_pages(base, 1).unwrap();
        copy_to_user(&task, base, &0x73u32.to_ne_bytes()).unwrap();
        let mask = base + 16;
        let timeout = base + 32;
        copy_to_user(
            &task,
            mask,
            &((1u64 << 9) | (1u64 << 8) | (1u64 << 18)).to_ne_bytes(),
        )
        .unwrap();
        set_mock_current_task(task.clone());
        let mut abi = LinuxAbi::default();
        abi.signal_state
            .lock()
            .blocked
            .block_signal(LinuxSignal::SIGUSR1);
        for (size, mask_ptr, seconds, nanos, expected) in [
            (16, mask, 0, 0, errno::EINVAL),
            (8, 0, 0, 0, errno::EFAULT),
            (8, mask, -1, 0, errno::EINVAL),
            (8, mask, 0, -1, errno::EINVAL),
            (8, mask, 0, 1_000_000_000, errno::EINVAL),
            (8, mask, 0, 0, errno::EAGAIN),
        ] {
            copy_to_user(&task, timeout, &(seconds as i64).to_ne_bytes()).unwrap();
            copy_to_user(&task, timeout + 8, &(nanos as i64).to_ne_bytes()).unwrap();
            let mut frame = Trapframe::new();
            frame.set_pc(base as u64);
            frame.set_arg(0, mask_ptr);
            frame.set_arg(1, 0);
            frame.set_arg(2, timeout);
            frame.set_arg(3, size);
            assert_eq!(
                sys_rt_sigtimedwait(&mut abi, &mut frame),
                errno::to_result(expected)
            );
        }
        let mut frame = Trapframe::new();
        frame.set_pc(base as u64);
        frame.set_arg(0, mask);
        frame.set_arg(1, 0);
        frame.set_arg(2, timeout);
        frame.set_arg(3, 8);
        abi.signal_state.lock().add_pending(LinuxSignal::SIGUSR1);
        assert_eq!(sys_rt_sigtimedwait(&mut abi, &mut frame), 10);
        abi.signal_state.lock().add_pending(LinuxSignal::SIGKILL);
        frame.set_pc(base as u64);
        assert_eq!(
            sys_rt_sigtimedwait(&mut abi, &mut frame),
            errno::to_result(errno::EINTR)
        );
        assert!(abi.signal_state.lock().is_pending(LinuxSignal::SIGKILL));
        clear_mock_current_task();
    }

    #[test_case]
    fn signals_to_exited_tasks_preserve_wait_status_and_state() {
        use crate::task::TaskState;

        let mut sender = Task::new("signal-sender".into(), 0, TaskType::User);
        sender.set_id(usize::MAX);
        for state in [TaskState::Zombie, TaskState::Terminated] {
            for previous_signal in [None, Some(LinuxSignal::SIGINT)] {
                let mut receiver = Task::new("exited-receiver".into(), 0, TaskType::User);
                receiver.set_id(usize::MAX - 1);
                let abi = LinuxAbi::default();
                abi.bind_task_signals(&receiver);
                abi.signal_state
                    .lock()
                    .set_handler(LinuxSignal::SIGQUIT, SignalAction::Custom(0x400000));
                let status = previous_signal.map_or(7, signal_death_status);
                if let Some(signal) = previous_signal {
                    receiver.mark_signal_termination(signal as u8);
                }
                receiver.set_exit_status(status);
                receiver.set_state(state);
                for signal in [
                    LinuxSignal::SIGKILL,
                    LinuxSignal::SIGSTOP,
                    LinuxSignal::SIGCONT,
                    LinuxSignal::SIGQUIT,
                ] {
                    assert_eq!(deliver_signal_to_remote(&sender, &receiver, signal), 0);
                    assert_eq!(receiver.get_exit_status(), Some(status));
                    assert_eq!(
                        receiver.termination_signal(),
                        previous_signal.map(|signal| signal as u8)
                    );
                    assert_eq!(receiver.get_state(), state);
                    assert!(receiver.event_queue.lock().dequeue().is_none());
                }
            }
        }
    }

    #[test_case]
    fn remote_quit_uses_registered_handler_and_preserves_ignored_signals() {
        // Unscheduled fixtures still need IDs for signal event metadata.
        let mut sender = Task::new("signal-sender".into(), 0, TaskType::User);
        sender.set_id(usize::MAX);
        let mut receiver = Task::new("signal-receiver".into(), 0, TaskType::User);
        receiver.set_id(usize::MAX - 1);
        let abi = LinuxAbi::default();
        abi.bind_task_signals(&receiver);
        abi.signal_state
            .lock()
            .set_handler(LinuxSignal::SIGQUIT, SignalAction::Custom(0x400000));
        assert_eq!(
            deliver_signal_to_remote(&sender, &receiver, LinuxSignal::SIGQUIT),
            0
        );
        assert_eq!(receiver.termination_signal(), None);
        let event = receiver.event_queue.lock().dequeue().unwrap();
        assert_eq!(event.metadata.sender, Some(sender.get_id() as u32));
        assert_eq!(handle_event_to_signal(&event), Some(LinuxSignal::SIGQUIT));
        abi.signal_state
            .lock()
            .set_handler(LinuxSignal::SIGTERM, SignalAction::Ignore);
        assert_eq!(
            deliver_signal_to_remote(&sender, &receiver, LinuxSignal::SIGTERM),
            0
        );
        assert_eq!(receiver.termination_signal(), None);
        assert!(receiver.event_queue.lock().dequeue().is_none());
        abi.signal_state
            .lock()
            .blocked
            .block_signal(LinuxSignal::SIGQUIT);
        assert_eq!(
            deliver_signal_to_remote(&sender, &receiver, LinuxSignal::SIGQUIT),
            0
        );
        assert!(abi.signal_state.lock().is_pending(LinuxSignal::SIGQUIT));
        assert!(receiver.event_queue.lock().dequeue().is_none());
    }

    use super::*;

    #[test_case]
    fn thread_server_call_masks_do_not_block_other_threads() {
        let parent = LinuxAbi::default();
        let action = Sigaction {
            handler: 0x400000,
            flags: 0x14000000,
            #[cfg(target_arch = "aarch64")]
            restorer: 0x410000,
            mask: 1 << (LinuxSignal::SIGTERM as u32 - 1),
        };
        parent
            .signal_state
            .lock()
            .install_sigaction(LinuxSignal::SIGINT, action);
        parent
            .signal_state
            .lock()
            .blocked
            .block_signal(LinuxSignal::SIGCHLD);
        parent.signal_state.lock().add_pending(LinuxSignal::SIGCHLD);
        let mut worker = parent.clone();
        worker.clone_signal_state(true);
        {
            let mut state = worker.signal_state.lock();
            assert!(state.blocked.is_blocked(LinuxSignal::SIGCHLD));
            assert!(!state.is_pending(LinuxSignal::SIGCHLD));
            // Wine blocks SIGINT during a server call. Another thread must
            // still be able to receive it while this worker awaits its reply.
            state.blocked.block_signal(LinuxSignal::SIGINT);
            state.add_pending(LinuxSignal::SIGINT);
            assert_eq!(state.next_deliverable_signal(), None);
        }
        {
            let mut state = parent.signal_state.lock();
            assert!(!state.blocked.is_blocked(LinuxSignal::SIGINT));
            assert!(!state.is_pending(LinuxSignal::SIGINT));
            state.add_pending(LinuxSignal::SIGINT);
            assert_eq!(state.next_deliverable_signal(), Some(LinuxSignal::SIGINT));
            state.remove_pending(LinuxSignal::SIGINT);
        }
        assert!(worker.signal_state.lock().is_pending(LinuxSignal::SIGINT));
        // Dispositions, including flags and handler masks, remain shared.
        worker
            .signal_state
            .lock()
            .install_sigaction(LinuxSignal::SIGTERM, action);
        let shared = parent
            .signal_state
            .lock()
            .get_sigaction(LinuxSignal::SIGTERM);
        assert_eq!(shared.handler, action.handler);
        assert_eq!(shared.flags, action.flags);
        assert_eq!(shared.mask, action.mask);
        let mut child = worker.clone();
        child.fork_signal_state();
        child
            .signal_state
            .lock()
            .set_handler(LinuxSignal::SIGTERM, SignalAction::Ignore);
        assert_eq!(
            parent.signal_state.lock().get_handler(LinuxSignal::SIGTERM),
            SignalAction::Custom(action.handler)
        );
        assert!(!child.signal_state.lock().is_pending(LinuxSignal::SIGINT));
        assert!(
            child
                .signal_state
                .lock()
                .blocked
                .is_blocked(LinuxSignal::SIGINT)
        );
    }

    #[test_case]
    fn sigaction_keeps_flags_mask_and_fork_dispositions_separate() {
        let mut parent = SignalState::new();
        let action = Sigaction {
            handler: 0x400000,
            flags: 0x14000000,
            #[cfg(target_arch = "aarch64")]
            restorer: 0x410000,
            mask: 0x102,
        };
        parent.install_sigaction(LinuxSignal::SIGINT, action);
        let saved = parent.get_sigaction(LinuxSignal::SIGINT);
        assert_eq!(saved.flags, action.flags);
        assert_eq!(saved.mask, action.mask);
        #[cfg(target_arch = "aarch64")]
        {
            assert_eq!(core::mem::size_of::<Sigaction>(), 32);
            assert_eq!(core::mem::offset_of!(Sigaction, mask), 24);
            assert_eq!(saved.restorer, action.restorer);
        }
        let mut child = parent.clone();
        child.reset_caught_handlers();
        assert_eq!(
            child.get_handler(LinuxSignal::SIGINT),
            LinuxSignal::SIGINT.default_action()
        );
        assert_eq!(child.get_sigaction(LinuxSignal::SIGINT).flags, 0);
        assert_eq!(
            parent.get_handler(LinuxSignal::SIGINT),
            SignalAction::Custom(action.handler)
        );
    }

    #[test_case]
    fn test_sigstop_default_action_stops() {
        assert_eq!(LinuxSignal::SIGSTOP.default_action(), SignalAction::Stop);
    }

    #[test_case]
    fn test_process_control_job_control_signal_mapping() {
        assert_eq!(
            process_control_to_signal(ProcessControlType::Stop),
            Some(LinuxSignal::SIGSTOP)
        );
        assert_eq!(
            process_control_to_signal(ProcessControlType::Continue),
            Some(LinuxSignal::SIGCONT)
        );
    }
}

#[cfg(test)]
mod signal_delivery_tests {
    use super::*;

    #[test_case]
    fn reserved_realtime_record_holds_capacity_and_failed_copy_restores_fifo() {
        let state = Arc::new(crate::sync::IrqSpinLock::new(SignalState::new()));
        let signal = LinuxSignal::SIGRT0;
        for n in 0..MAX_REALTIME_PENDING {
            let mut info = SignalInfo::kernel(signal);
            info.bytes[24..32].copy_from_slice(&(n as u64).to_ne_bytes());
            state.lock().add_process_pending(signal, info).unwrap();
        }
        let record = reserve_pending(&state, 1u64 << (signal as u32 - 1)).unwrap();
        assert_eq!(u64::from_ne_bytes(record.info.bytes[24..32].try_into().unwrap()), 0);
        assert_eq!(state.lock().add_process_pending(signal, SignalInfo::kernel(signal)), Err(errno::EAGAIN));
        // Cloning a process snapshot cannot inherit a slot charged to another
        // thread's in-progress user copy without inheriting that reservation.
        let snapshot = state.lock().clone();
        assert_eq!(snapshot.shared_pending.lock().realtime_count, MAX_REALTIME_PENDING - 1);
        drop(record);
        let mut state = state.lock();
        assert_eq!(u64::from_ne_bytes(state.take_pending_info(signal).unwrap().bytes[24..32].try_into().unwrap()), 0);
        assert_eq!(u64::from_ne_bytes(state.take_pending_info(signal).unwrap().bytes[24..32].try_into().unwrap()), 1);
        state.add_process_pending(signal, SignalInfo::kernel(signal)).unwrap();
    }

    #[test_case]
    fn continuation_invalidates_an_inflight_stop_and_later_stop_supersedes_continue() {
        let mut state = SignalState::new();
        let stop = LinuxSignal::SIGTSTP;
        state.publish_process_signal(stop, SignalInfo::kernel(stop)).unwrap();
        let (_, epoch) = state.take_pending_with_epoch(stop).unwrap();
        state.publish_process_signal(LinuxSignal::SIGCONT, SignalInfo::kernel(LinuxSignal::SIGCONT)).unwrap();
        let task = Task::new("stale-stop".into(), 0, TaskType::Kernel);
        let before = task.get_state();
        assert!(!apply_signal_stop(&task, &state.shared_pending, epoch));
        assert_eq!(task.get_state(), before);
        assert!(!state.is_pending(stop));
        assert!(state.is_pending(LinuxSignal::SIGCONT));
        state.publish_process_signal(LinuxSignal::SIGTTOU, SignalInfo::kernel(LinuxSignal::SIGTTOU)).unwrap();
        assert!(!state.is_pending(LinuxSignal::SIGCONT));
        assert!(state.is_pending(LinuxSignal::SIGTTOU));
    }

    #[test_case]
    fn a_process_marker_for_a_newly_blocked_thread_is_forwarded_without_duplicate_delivery() {
        use crate::sched::scheduler::{register_task, reset};
        reset();
        let leader_id = register_task(Task::new("marker-leader".into(), 0, TaskType::User));
        let leader = get_task_by_id(leader_id).unwrap();
        let mut member = Task::new("marker-sibling".into(), 0, TaskType::User);
        member.set_namespace(leader.get_namespace());
        member.set_thread_group_id(leader_id);
        member.init();
        let member_id = register_task(member);
        let member = get_task_by_id(member_id).unwrap();
        let leader_abi = LinuxAbi::default();
        leader_abi.bind_task_signals(&leader);
        let mut member_abi = leader_abi.clone();
        member_abi.clone_signal_state_with_group(true, true);
        member_abi.bind_task_signals(&member);
        let signal = LinuxSignal::SIGUSR1;
        leader_abi.signal_state.lock().set_handler(signal, SignalAction::Custom(0x400000));
        let mut info = SignalInfo::kernel(signal);
        info.bytes[24] = 42;
        queue_process_signal(&leader, &leader, signal, info).unwrap();
        let event = leader.event_queue.lock().dequeue().unwrap();
        assert!(is_pending_wake(&event));
        leader_abi.signal_state.lock().blocked.block_signal(signal);
        assert!(matches!(handle_event_for_task(&leader_abi, &event, leader_id,
            |_,_,_,_,_,_| panic!("masked thread must not handle signal")).unwrap(), EventProcessOutcome::Continue));
        let event = member.event_queue.lock().dequeue().unwrap();
        assert!(matches!(handle_event_for_task(&member_abi, &event, member_id,
            |_,_,_,_,received,record| { assert_eq!(received, signal); assert_eq!(record, info); Ok(()) }).unwrap(),
            EventProcessOutcome::UserHandlerArmed));
        assert!(!leader_abi.signal_state.lock().is_pending(signal));
        assert!(matches!(handle_event_for_task(&member_abi, &event, member_id,
            |_,_,_,_,_,_| panic!("stale marker must not deliver twice")).unwrap(), EventProcessOutcome::Continue));
        drop(member);
        drop(leader);
        reset();
    }

    #[test_case]
    fn process_pending_is_shared_only_by_threads_and_fork_starts_empty() {
        let parent = SignalState::new();
        let mut thread = parent.clone_for_child(true, true);
        let mut sighand_only = parent.clone_for_child(true, false);
        let mut fork = parent.clone_for_child(false, false);
        parent.add_process_pending(LinuxSignal::SIGQUIT, SignalInfo::kernel(LinuxSignal::SIGQUIT)).unwrap();
        assert!(thread.is_pending(LinuxSignal::SIGQUIT));
        assert!(!sighand_only.is_pending(LinuxSignal::SIGQUIT));
        assert!(!fork.is_pending(LinuxSignal::SIGQUIT));
        thread.take_pending_info(LinuxSignal::SIGQUIT).unwrap();
        assert!(!parent.is_pending(LinuxSignal::SIGQUIT));
        sighand_only.add_pending(LinuxSignal::SIGUSR1);
        fork.add_pending(LinuxSignal::SIGUSR1);
        assert!(!parent.is_pending(LinuxSignal::SIGUSR1));
    }

    #[test_case]
    fn queued_signals_coalesce_standard_records_and_bound_realtime_fifo() {
        let mut queue = PendingSignals::default();
        let first = SignalInfo::kernel(LinuxSignal::SIGUSR1);
        let mut second = first;
        second.bytes[24] = 7;
        queue.push(LinuxSignal::SIGUSR1, first).unwrap();
        queue.push(LinuxSignal::SIGUSR1, second).unwrap();
        assert_eq!(queue.take(LinuxSignal::SIGUSR1), Some(first));
        assert_eq!(queue.take(LinuxSignal::SIGUSR1), None);
        for n in 0..MAX_REALTIME_PENDING {
            let mut info = SignalInfo::kernel(LinuxSignal::SIGRT0);
            info.bytes[24..32].copy_from_slice(&(n as u64).to_ne_bytes());
            queue.push(LinuxSignal::SIGRT0, info).unwrap();
        }
        assert_eq!(queue.push(LinuxSignal::SIGRT1, SignalInfo::kernel(LinuxSignal::SIGRT1)), Err(errno::EAGAIN));
        // Realtime exhaustion must not prevent an ordinary control signal.
        queue.push(LinuxSignal::SIGQUIT, SignalInfo::kernel(LinuxSignal::SIGQUIT)).unwrap();
        for n in 0..MAX_REALTIME_PENDING {
            let info = queue.take(LinuxSignal::SIGRT0).unwrap();
            assert_eq!(u64::from_ne_bytes(info.bytes[24..32].try_into().unwrap()), n as u64);
        }
        assert_eq!(queue.realtime_count, 0);
        queue.push(LinuxSignal::SIGRT1, SignalInfo::kernel(LinuxSignal::SIGRT1)).unwrap();
    }

    #[test_case]
    fn sigqueue_reaches_a_blocked_sibling_and_preserves_info_after_bad_copy() {
        use crate::environment::{PAGE_SIZE, USER_STACK_END};
        use crate::library::std::usercopy::{copy_from_user, copy_to_user};
        use crate::sched::scheduler::{register_task, reset};
        use crate::task::{clear_mock_current_task, set_mock_current_task};
        reset();
        let sender_id = register_task(Task::new("queue-sender".into(), 0, TaskType::User));
        let sender = get_task_by_id(sender_id).unwrap();
        let leader_id = register_task(Task::new("queue-leader".into(), 0, TaskType::User));
        let leader = get_task_by_id(leader_id).unwrap();
        let mut catcher = Task::new("Signal Catcher".into(), 0, TaskType::User);
        catcher.set_namespace(leader.get_namespace());
        catcher.set_thread_group_id(leader_id);
        let catcher_id = register_task(catcher);
        let catcher = get_task_by_id(catcher_id).unwrap();
        let leader_abi = LinuxAbi::default();
        leader_abi.bind_task_signals(&leader);
        let mut catcher_abi = leader_abi.clone();
        catcher_abi.clone_signal_state_with_group(true, true);
        catcher_abi.bind_task_signals(&catcher);
        for state in [&leader_abi.signal_state, &catcher_abi.signal_state] {
            state.lock().blocked.block_signal(LinuxSignal::SIGQUIT);
        }
        let base = USER_STACK_END - PAGE_SIZE;
        sender.allocate_stack_pages(base, 1).unwrap();
        catcher.allocate_stack_pages(base, 1).unwrap();
        let mut info = SignalInfo::kernel(LinuxSignal::SIGQUIT);
        info.bytes[8..12].copy_from_slice(&(-1i32).to_ne_bytes());
        info.bytes[16..24].fill(0xff); // Caller cannot forge the recorded sender.
        info.bytes[24..32].copy_from_slice(&0x12345678u64.to_ne_bytes());
        copy_to_user(&sender, base, &info.bytes).unwrap();
        let mut frame = Trapframe::new();
        frame.set_pc(base as u64);
        frame.set_arg(0, leader.get_namespace_id());
        frame.set_arg(1, LinuxSignal::SIGQUIT as usize);
        frame.set_arg(2, base);
        set_mock_current_task(sender.clone());
        let mut sender_abi = LinuxAbi::default();
        assert_eq!(sys_rt_sigqueueinfo(&mut sender_abi, &mut frame), 0);
        let state = catcher_abi.signal_state.clone();
        let mask = 1u64 << (LinuxSignal::SIGQUIT as u32 - 1);
        assert_eq!(take_waited_signal(&catcher, &state, mask, 1), Err(errno::EFAULT));
        assert!(state.lock().is_pending(LinuxSignal::SIGQUIT));
        assert_eq!(take_waited_signal(&catcher, &state, mask, base), Ok(Some(3)));
        drop(state);
        let mut received = [0; 128];
        copy_from_user(&catcher, base, &mut received).unwrap();
        assert_eq!(i32::from_ne_bytes(received[8..12].try_into().unwrap()), -1);
        assert_eq!(i32::from_ne_bytes(received[16..20].try_into().unwrap()), sender.get_namespace_id() as i32);
        assert_eq!(u32::from_ne_bytes(received[20..24].try_into().unwrap()), sender.credentials.read().principals().base.0);
        assert_eq!(u64::from_ne_bytes(received[24..32].try_into().unwrap()), 0x12345678);
        assert!(!leader_abi.signal_state.lock().is_pending(LinuxSignal::SIGQUIT));
        // Invalid kernel-origin codes are rejected without creating a record.
        info.bytes[8..12].copy_from_slice(&128i32.to_ne_bytes());
        copy_to_user(&sender, base, &info.bytes).unwrap();
        assert_eq!(sys_rt_sigqueueinfo(&mut sender_abi, &mut frame), errno::to_result(errno::EPERM));
        assert!(!leader_abi.signal_state.lock().is_pending(LinuxSignal::SIGQUIT));
        clear_mock_current_task();
        drop(catcher);
        drop(leader);
        drop(sender);
        reset();
    }

    #[test_case]
    fn test_signal_death_status_matches_shell_status_convention() {
        assert_eq!(signal_death_status(LinuxSignal::SIGABRT), 134);
        assert_eq!(signal_death_status(LinuxSignal::SIGSEGV), 139);
        assert_eq!(signal_death_status(LinuxSignal::SIGKILL), 137);
    }

    #[test_case]
    fn test_blocked_fatal_signal_stays_pending_for_self() {
        let mut state = SignalState::new();
        state.blocked.block_signal(LinuxSignal::SIGABRT);
        state.add_pending(LinuxSignal::SIGABRT);
        assert!(state.is_pending(LinuxSignal::SIGABRT));
        assert_eq!(
            state.next_deliverable_signal(),
            None,
            "blocked signals must not be delivered while masked"
        );
    }
}

/// Send a signal to a specific thread.
///
/// The target TID is resolved in the caller's PID namespace. Signal zero only
/// checks that the target exists; other signals are delivered according to the
/// supported local or remote disposition semantics.
///
/// # Arguments
///
/// * `abi` - Linux ABI context used for the caller's signal state.
/// * `trapframe` - Trapframe containing the target TID and signal number.
///
/// # Returns
///
/// `0` on success, or a negative Linux errno encoded in `usize` on failure.
pub fn sys_tkill(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let tid = trapframe.get_arg(0) as i32;
    let sig = trapframe.get_arg(1) as i32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(&task);

    if !(0..=64).contains(&sig) {
        return errno::to_result(errno::EINVAL);
    }

    if tid <= 0 {
        return errno::to_result(errno::EINVAL);
    }

    let Some(target) = resolve_task_by_linux_tid(&task, tid) else {
        return errno::to_result(errno::ESRCH);
    };

    // Signal 0 only probes whether the target exists.
    if sig == 0 {
        return 0;
    }

    let Some(signal) = LinuxSignal::from_u32(sig as u32) else {
        return errno::to_result(errno::EINVAL);
    };

    if signal == LinuxSignal::SIGABRT && target.get_id() == task.get_id() {
        crate::println!(
            "[linux] task {} (PID {}) requested SIGABRT",
            task.get_id(),
            task.try_get_namespace_id().unwrap_or(0)
        );
        crate::arch::log_user_backtrace(&task, trapframe);
    }

    deliver_signal(abi, &task, &target, signal)
}

/// Send a signal to a thread in a specific thread group.
///
/// Both IDs are resolved in the caller's PID namespace, and the target TID must
/// belong to the supplied thread-group ID.
///
/// # Arguments
///
/// * `abi` - Linux ABI context used for the caller's signal state.
/// * `trapframe` - Trapframe containing the target TGID, TID, and signal number.
///
/// # Returns
///
/// `0` on success, or a negative Linux errno encoded in `usize` on failure.
pub fn sys_tgkill(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let tgid = trapframe.get_arg(0) as i32;
    let tid = trapframe.get_arg(1) as i32;
    let sig = trapframe.get_arg(2) as i32;

    trapframe.increment_pc_next(&task);

    if !(0..=64).contains(&sig) {
        return errno::to_result(errno::EINVAL);
    }

    if tgid <= 0 || tid <= 0 {
        return errno::to_result(errno::EINVAL);
    }

    let Some(target) = resolve_task_by_linux_tid(&task, tid) else {
        return errno::to_result(errno::ESRCH);
    };

    // The supplied tgid must name the target's thread group.
    let target_tgid = task
        .get_namespace()
        .resolve_local_id(target.get_thread_group_id());
    if target_tgid != Some(tgid as usize) {
        return errno::to_result(errno::ESRCH);
    }

    // Signal 0 only probes whether the target exists.
    if sig == 0 {
        return 0;
    }

    let Some(signal) = LinuxSignal::from_u32(sig as u32) else {
        return errno::to_result(errno::EINVAL);
    };

    if signal == LinuxSignal::SIGABRT && target.get_id() == task.get_id() {
        crate::println!(
            "[linux] task {} (PID {}) requested SIGABRT",
            task.get_id(),
            task.try_get_namespace_id().unwrap_or(0)
        );
        crate::arch::log_user_backtrace(&task, trapframe);
    }

    deliver_signal(abi, &task, &target, signal)
}
