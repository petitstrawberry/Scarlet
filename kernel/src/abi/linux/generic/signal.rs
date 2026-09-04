//! Linux signal syscalls and signal handling
//!
//! Implements POSIX signals with Linux-compatible semantics, integrated with Scarlet's
//! event system for cross-ABI signal delivery.

use crate::abi::EventProcessOutcome;
use crate::abi::linux::generic::LinuxAbi;
use crate::abi::linux::generic::errno;
use crate::arch::Trapframe;
use crate::ipc::event::{Event, EventContent, ProcessControlType};
use crate::task::mytask;
use alloc::collections::BTreeMap;
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
            trapframe.increment_pc_next(&task);
            return !0usize;
        }
    };

    let mut signal_state = abi.signal_state.lock();

    // Get old action if requested
    if oldact_ptr != 0 {
        let Some(paddr) = task.vm_manager.translate_to_kva(oldact_ptr) else {
            // Invalid user pointer for oldact: return EFAULT
            trapframe.set_return_value(!0usize);
            trapframe.increment_pc_next(&task);
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
            trapframe.increment_pc_next(&task);
            return !0usize; // -EFAULT
        };
        let new_sigaction = unsafe { core::ptr::read(paddr as *const Sigaction) };
        let new_action = linux_to_sigaction(new_sigaction, signal);
        signal_state.set_handler(signal, new_action);
    }

    trapframe.set_return_value(0);
    trapframe.increment_pc_next(&task);
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
            trapframe.increment_pc_next(&task);
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
            // Some compatibility workloads install signal masks while their
            // userspace stack is being reshaped. Keep this permissive until the
            // Linux ABI has copy_from_user semantics that can distinguish short
            // reads from genuinely invalid pointers.
            trapframe.set_return_value(0);
            trapframe.increment_pc_next(&task);
            return 0;
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
                trapframe.increment_pc_next(&task);
                return !0usize;
            }
        }
    }

    trapframe.set_return_value(0);
    trapframe.increment_pc_next(&task);
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
        ProcessControlType::TerminalStop => Some(LinuxSignal::SIGTSTP),
        ProcessControlType::TerminalInput => Some(LinuxSignal::SIGTTIN),
        ProcessControlType::TerminalOutput => Some(LinuxSignal::SIGTTOU),
        ProcessControlType::WindowChange => Some(LinuxSignal::SIGWINCH),
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
pub fn handle_event_for_task(
    abi: &LinuxAbi,
    event: &Event,
    target_task_id: usize,
    setup_signal_handler: fn(&mut Trapframe, usize, LinuxSignal),
) -> Result<EventProcessOutcome, &'static str> {
    let Some(signal) = handle_event_to_signal(event) else {
        return Ok(EventProcessOutcome::Continue);
    };
    let is_stop_class = matches!(
        &event.content,
        EventContent::ProcessControl(control_type) if control_type.is_stop_class()
    );

    let action = {
        let signal_state = abi.signal_state.lock();
        signal_state.get_handler(signal)
    };

    // Preserve owned lookup semantics for nonfatal and remote targets. Fatal
    // delivery returns an exit request; Task performs the actual exit after its
    // ABI mutable borrow has been released.
    let target_task =
        crate::sched::scheduler::get_task_by_id(target_task_id).ok_or("Target task not found")?;

    let outcome = match action {
        SignalAction::Custom(handler_addr) => {
            let trapframe = target_task.get_trapframe();
            setup_signal_handler(trapframe, handler_addr, signal);
            EventProcessOutcome::UserHandlerArmed
        }
        SignalAction::Ignore => EventProcessOutcome::Continue,
        SignalAction::ForceTerminate | SignalAction::Terminate => {
            let exit_code = 128 + (signal as i32);
            EventProcessOutcome::Exited(exit_code)
        }
        SignalAction::Stop => {
            let event_queue = target_task.event_queue.lock();
            if is_stop_class && event_queue.has_pending_continue() {
                drop(event_queue);
                EventProcessOutcome::Continue
            } else {
                target_task.set_state(crate::task::TaskState::Blocked(
                    crate::task::BlockedType::Interruptible,
                ));
                crate::sched::scheduler::mark_blocked(target_task.get_id());
                crate::sched::scheduler::remove_from_ready_queues(target_task.get_id());
                drop(event_queue);
                EventProcessOutcome::NeedReschedule
            }
        }
        SignalAction::Continue => {
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

        crate::early_println!(
            "Signal {}: Immediately terminating task {} with exit code {}",
            signal as u32,
            task.get_id(),
            exit_code
        );

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
/// `128 + signal`. Converting this to Linux's raw `wait(2)` bit layout belongs
/// at the wait ABI boundary once normal exit statuses are encoded there too.
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
fn deliver_signal_to_self(abi: &LinuxAbi, task: &Task, signal: LinuxSignal) {
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
            task.request_deferred_exit_group(status);
        }
        SignalAction::Stop => stop_target_for_signal(task),
        SignalAction::Ignore | SignalAction::Continue | SignalAction::Custom(_) => {}
    }
}

/// Deliver a signal to another task using its default disposition.
///
/// The remote task's installed handler table is not reachable from the
/// caller's syscall context, so the signal's default action is applied
/// instead. Fatal defaults terminate the target thread group; custom
/// handlers and ignored signals stay permissive to keep runtimes such as Go
/// working until cross-task userspace signal frames are available.
///
/// # Arguments
///
/// * `target` - Task receiving the signal
/// * `signal` - Signal to deliver
fn deliver_signal_to_remote(target: &Task, signal: LinuxSignal) {
    match signal.default_action() {
        SignalAction::Terminate | SignalAction::ForceTerminate => {
            let status = signal_death_status(signal);
            crate::println!(
                "[linux] signal {} terminating task {} (PID {}) with status {}",
                signal as u32,
                target.get_id(),
                target.try_get_namespace_id().unwrap_or(0),
                status
            );
            target.request_deferred_exit_group(status);
            wake_target_for_signal(target);
        }
        SignalAction::Stop => stop_target_for_signal(target),
        SignalAction::Continue => wake_target_for_signal(target),
        SignalAction::Ignore | SignalAction::Custom(_) => {}
    }
}

/// Route signal delivery between the calling task and a remote target.
///
/// # Arguments
///
/// * `abi` - Linux ABI state of the calling task
/// * `current` - Currently running task
/// * `target` - Task the signal is addressed to
/// * `signal` - Signal to deliver
pub fn deliver_signal(abi: &LinuxAbi, current: &Task, target: &Task, signal: LinuxSignal) {
    let is_self = target.get_id() == current.get_id()
        || target.get_thread_group_id() == current.get_thread_group_id();
    if is_self {
        deliver_signal_to_self(abi, current, signal);
    } else {
        deliver_signal_to_remote(target, signal);
    }
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

        let action = {
            let mut signal_state = abi.signal_state.lock();
            signal_state.remove_pending(signal);
            signal_state.get_handler(signal)
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
                task.request_deferred_exit_group(status);
                return;
            }
            SignalAction::Stop => {
                stop_target_for_signal(&task);
                return;
            }
            SignalAction::Ignore | SignalAction::Continue | SignalAction::Custom(_) => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    deliver_signal(abi, &task, &target, signal);

    0
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

    deliver_signal(abi, &task, &target, signal);

    0
}
