//! Linux signal syscalls and signal handling
//!
//! Implements POSIX signals with Linux-compatible semantics, integrated with Scarlet's
//! event system for cross-ABI signal delivery.

use crate::abi::linux::generic::LinuxAbi;
use crate::abi::linux::generic::errno;
use crate::arch::Trapframe;
use crate::ipc::event::{Event, EventContent, ProcessControlType};
use crate::task::mytask;
use alloc::collections::BTreeMap;

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
            Self::SIGKILL | Self::SIGSTOP => SignalAction::ForceTerminate,
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

/// Signal handler state for a task
#[derive(Debug, Clone)]
pub struct SignalState {
    /// Signal handlers (signal number -> handler action)
    pub handlers: BTreeMap<LinuxSignal, SignalAction>,
    /// Blocked signals mask
    pub blocked: SignalMask,
    /// Pending signals that are blocked
    pub pending: SignalMask,
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
            handlers,
            blocked: SignalMask::new(),
            pending: SignalMask::new(),
        }
    }
}

impl SignalState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set signal handler
    pub fn set_handler(&mut self, signal: LinuxSignal, action: SignalAction) {
        // SIGKILL and SIGSTOP cannot be caught or ignored
        if signal != LinuxSignal::SIGKILL && signal != LinuxSignal::SIGSTOP {
            self.handlers.insert(signal, action);
        }
    }

    /// Get signal handler
    pub fn get_handler(&self, signal: LinuxSignal) -> SignalAction {
        self.handlers
            .get(&signal)
            .copied()
            .unwrap_or(signal.default_action())
    }

    /// Add pending signal
    pub fn add_pending(&mut self, signal: LinuxSignal) {
        self.pending.block_signal(signal);
    }

    /// Remove pending signal
    pub fn remove_pending(&mut self, signal: LinuxSignal) {
        self.pending.unblock_signal(signal);
    }

    /// Check if signal is pending
    pub fn is_pending(&self, signal: LinuxSignal) -> bool {
        self.pending.is_blocked(signal)
    }

    /// Get next deliverable signal (not blocked and pending)
    pub fn next_deliverable_signal(&self) -> Option<LinuxSignal> {
        for signal_num in 1..=31 {
            if let Some(signal) = LinuxSignal::from_u32(signal_num) {
                if self.is_pending(signal) && !self.blocked.is_blocked(signal) {
                    return Some(signal);
                }
            }
        }
        None
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
    const MINSIGSTKSZ: usize = 2048;

    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::EPERM),
    };

    let new_stack_ptr = trapframe.get_arg(0);
    let old_stack_ptr = trapframe.get_arg(1);
    trapframe.increment_pc_next(task);

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
    let task = mytask().unwrap();

    let signum = trapframe.get_arg(0) as u32;
    let act_ptr = trapframe.get_arg(1);
    let oldact_ptr = trapframe.get_arg(2);
    let _sigsetsize = trapframe.get_arg(3);

    // Convert signal number to LinuxSignal
    let signal = match LinuxSignal::from_u32(signum) {
        Some(sig) => sig,
        None => {
            trapframe.set_return_value(!0usize); // -1 (EINVAL)
            trapframe.increment_pc_next(task);
            return !0usize;
        }
    };

    let mut signal_state = abi.signal_state.lock();

    // Get old action if requested
    if oldact_ptr != 0 {
        let Some(paddr) = task.vm_manager.translate_to_kva(oldact_ptr) else {
            // Invalid user pointer for oldact: return EFAULT
            trapframe.set_return_value(!0usize);
            trapframe.increment_pc_next(task);
            return !0usize; // -EFAULT
        };
        let old_action = signal_state.get_handler(signal);
        let old_sigaction = sigaction_to_linux(old_action);
        unsafe {
            core::ptr::write(paddr as *mut Sigaction, old_sigaction);
        }
    }

    // Set new action if provided
    if act_ptr != 0 {
        let Some(paddr) = task.vm_manager.translate_to_kva(act_ptr) else {
            // Invalid user pointer for act: return EFAULT
            trapframe.set_return_value(!0usize);
            trapframe.increment_pc_next(task);
            return !0usize; // -EFAULT
        };
        let new_sigaction = unsafe { core::ptr::read(paddr as *const Sigaction) };
        let new_action = linux_to_sigaction(new_sigaction, signal);
        signal_state.set_handler(signal, new_action);
    }

    trapframe.set_return_value(0);
    trapframe.increment_pc_next(task);
    0
}

/// Convert internal SignalAction to Linux sigaction
fn sigaction_to_linux(action: SignalAction) -> Sigaction {
    match action {
        SignalAction::Ignore => Sigaction {
            handler: SIG_IGN,
            flags: 0,
            mask: 0,
        },
        SignalAction::Custom(addr) => Sigaction {
            handler: addr,
            flags: 0,
            mask: 0,
        },
        _ => Sigaction {
            handler: SIG_DFL,
            flags: 0,
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
    let task = mytask().unwrap();

    let how = trapframe.get_arg(0);
    let set_ptr = trapframe.get_arg(1);
    let oldset_ptr = trapframe.get_arg(2);
    let _sigsetsize = trapframe.get_arg(3);

    let mut signal_state = abi.signal_state.lock();

    // Save old mask if requested
    if oldset_ptr != 0 {
        let Some(paddr) = task.vm_manager.translate_to_kva(oldset_ptr) else {
            // Invalid user pointer for oldset: return EFAULT
            trapframe.set_return_value(!0usize);
            trapframe.increment_pc_next(task);
            return !0usize; // -EFAULT
        };
        let old_mask = signal_state.blocked.raw();
        unsafe {
            core::ptr::write(paddr as *mut u64, old_mask);
        }
    }

    // Modify mask if new set is provided
    if set_ptr != 0 {
        let Some(paddr) = task.vm_manager.translate_to_kva(set_ptr) else {
            // Invalid user pointer for set: return EFAULT
            trapframe.set_return_value(!0usize);
            trapframe.increment_pc_next(task);
            return !0usize; // -EFAULT
        };
        let new_mask = unsafe { core::ptr::read(paddr as *const u64) };
        let mut new_signal_mask = SignalMask::new();
        new_signal_mask.set_raw(new_mask);

        // SIG_BLOCK = 0, SIG_UNBLOCK = 1, SIG_SETMASK = 2
        match how {
            0 => {
                // SIG_BLOCK: Add new_mask to current blocked signals
                let current = signal_state.blocked.raw();
                signal_state.blocked.set_raw(current | new_mask);
            }
            1 => {
                // SIG_UNBLOCK: Remove new_mask from current blocked signals
                let current = signal_state.blocked.raw();
                signal_state.blocked.set_raw(current & !new_mask);
            }
            2 => {
                // SIG_SETMASK: Replace blocked signals with new_mask
                signal_state.blocked = new_signal_mask;
            }
            _ => {
                trapframe.set_return_value(!0usize); // -1 (EINVAL)
                trapframe.increment_pc_next(task);
                return !0usize;
            }
        }
    }

    trapframe.set_return_value(0);
    trapframe.increment_pc_next(task);
    0
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
        ProcessControlType::Hangup => Some(LinuxSignal::SIGHUP),
        ProcessControlType::ChildExit => Some(LinuxSignal::SIGCHLD),
        ProcessControlType::PipeBroken => Some(LinuxSignal::SIGPIPE),
        ProcessControlType::Alarm => Some(LinuxSignal::SIGALRM),
        ProcessControlType::IoReady => Some(LinuxSignal::SIGIO),
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

/// Process pending signals and dispatch to arch-specific handler.
/// Returns true if execution should be interrupted.
/// Arch modules must provide `arch_setup_signal_handler`.
pub fn process_pending_signals_with_state(
    signal_state: &mut SignalState,
    trapframe: &mut Trapframe,
) -> bool {
    if let Some(signal) = signal_state.next_deliverable_signal() {
        let action = signal_state.get_handler(signal);
        signal_state.remove_pending(signal);

        match action {
            SignalAction::Terminate | SignalAction::ForceTerminate => {
                crate::early_println!("Signal {}: Terminating task", signal as u32);
                true
            }
            SignalAction::Ignore => false,
            SignalAction::Stop => {
                crate::early_println!("Signal {}: Stopping task", signal as u32);
                true
            }
            SignalAction::Continue => {
                crate::early_println!("Signal {}: Continuing task", signal as u32);
                false
            }
            SignalAction::Custom(handler_addr) => {
                crate::early_println!(
                    "Signal {}: Calling custom handler at {:#x}",
                    signal as u32,
                    handler_addr
                );
                arch_setup_signal_handler(trapframe, handler_addr, signal);
                true
            }
        }
    } else {
        false
    }
}

/// Arch-specific signal handler setup. Must be implemented by each arch module.
pub fn arch_setup_signal_handler(
    _trapframe: &mut Trapframe,
    _handler_addr: usize,
    _signal: LinuxSignal,
) {
    // Default: no-op. Arch modules should override via cfg.
    #[cfg(target_arch = "riscv64")]
    crate::abi::linux::riscv64::signal::setup_signal_handler(_trapframe, _handler_addr, _signal);
    #[cfg(target_arch = "aarch64")]
    crate::abi::linux::aarch64::signal::setup_signal_handler(_trapframe, _handler_addr, _signal);
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

        crate::early_println!(
            "Signal {}: Immediately terminating task {} with exit code {}",
            signal as u32,
            task.get_id(),
            exit_code
        );

        task.exit(exit_code);
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

/// Linux sys_tkill - Send a signal to a specific thread
///
/// tkill() sends a signal to a specific thread within the same thread group.
/// This is a simplified implementation that mainly prevents crashes.
///
/// Arguments:
/// - abi: LinuxAbi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: tid (thread ID)
///   - arg1: sig (signal number)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) on error
pub fn sys_tkill(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let tid = trapframe.get_arg(0) as i32;
    let sig = trapframe.get_arg(1) as i32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // For now, just log and return success
    // A proper implementation would:
    // 1. Find the target task by TID
    // 2. Add the signal to its pending signal queue
    // 3. Wake the task if it's sleeping
    crate::early_println!(
        "[sys_tkill] tid={} sig={} - NOOP (signal delivery not implemented)",
        tid,
        sig
    );

    // Return success to avoid crashing applications
    // Many applications use tkill for thread management
    0
}

/// Linux sys_tgkill - Send a signal to a thread in a specific thread group.
///
/// This currently mirrors `tkill`'s permissive behavior. Go's runtime uses
/// `tgkill` for internal signal delivery, so returning success is enough for
/// runtimes that install handlers but do not require full signal semantics yet.
pub fn sys_tgkill(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let _tgid = trapframe.get_arg(0) as i32;
    let _tid = trapframe.get_arg(1) as i32;
    let sig = trapframe.get_arg(2) as i32;

    trapframe.increment_pc_next(task);

    if sig < 0 {
        return errno::to_result(errno::EINVAL);
    }

    0
}
