//! Linux RISC-V 64 signal syscalls and signal handling
//!
//! Implements POSIX signals with Linux-compatible semantics, integrated with Scarlet's
//! event system for cross-ABI signal delivery.

use crate::abi::linux::riscv64::LinuxRiscv64Abi;
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
        for signal_num in 1..=31 {
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
/// This matches the Linux sigaction layout for RISC-V
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

/// Linux rt_sigaction system call implementation
///
/// int rt_sigaction(int signum, const struct sigaction *act, struct sigaction *oldact, size_t sigsetsize);
pub fn sys_rt_sigaction(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
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
        let Some(paddr) = task.vm_manager.translate_vaddr(oldact_ptr) else {
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
        let Some(paddr) = task.vm_manager.translate_vaddr(act_ptr) else {
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
pub fn sys_rt_sigprocmask(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();

    let how = trapframe.get_arg(0);
    let set_ptr = trapframe.get_arg(1);
    let oldset_ptr = trapframe.get_arg(2);
    let _sigsetsize = trapframe.get_arg(3);

    let mut signal_state = abi.signal_state.lock();

    // Save old mask if requested
    if oldset_ptr != 0 {
        let Some(paddr) = task.vm_manager.translate_vaddr(oldset_ptr) else {
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
        let Some(paddr) = task.vm_manager.translate_vaddr(set_ptr) else {
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
pub fn deliver_signal_to_task(abi: &LinuxRiscv64Abi, signal: LinuxSignal) {
    // Add signal to pending if it's not already pending
    let mut signal_state = abi.signal_state.lock();
    if !signal_state.is_pending(signal) {
        signal_state.add_pending(signal);
    }
}

/// Check if task has pending signals and return the next one to handle
pub fn get_next_pending_signal(abi: &LinuxRiscv64Abi) -> Option<LinuxSignal> {
    let signal_state = abi.signal_state.lock();
    signal_state.next_deliverable_signal()
}

/// Process pending signals for a task with explicit signal state
/// Returns true if a signal was handled and execution should be interrupted
pub fn process_pending_signals_with_state(
    signal_state: &mut SignalState,
    trapframe: &mut Trapframe,
) -> bool {
    if let Some(signal) = signal_state.next_deliverable_signal() {
        let action = signal_state.get_handler(signal);

        // Remove signal from pending
        signal_state.remove_pending(signal);

        match action {
            SignalAction::Terminate | SignalAction::ForceTerminate => {
                // TODO: Implement actual task termination
                // This should call task.set_state(TaskState::Terminated)
                // and set exit code based on signal
                crate::early_println!("Signal {}: Terminating task", signal as u32);
                true
            }
            SignalAction::Ignore => {
                // Signal ignored, continue execution
                false
            }
            SignalAction::Stop => {
                // TODO: Implement actual task stopping
                // This should call task.set_state(TaskState::Stopped)
                crate::early_println!("Signal {}: Stopping task", signal as u32);
                true
            }
            SignalAction::Continue => {
                // TODO: Implement actual task continuation
                // This should call task.set_state(TaskState::Ready) if stopped
                crate::early_println!("Signal {}: Continuing task", signal as u32);
                false
            }
            SignalAction::Custom(handler_addr) => {
                // Set up user-space signal handler execution
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

/// Set up user-space signal handler execution with context save/restore.
///
/// Signal frame layout on user stack (RISC-V, simplified `rt_sigframe`):
/// ```text
///   [frame_base + 0]:   trampoline (li a7, 139; ecall) — rt_sigreturn
///   [frame_base + 8]:   signal number
///   [frame_base + 16]:  saved regs[0..31] (256 bytes)
///   [frame_base + 272]: saved epc (8 bytes)
/// ```
pub fn setup_signal_handler(trapframe: &mut Trapframe, handler_addr: usize, signal: LinuxSignal) {
    let mut sp = trapframe.regs.reg[2];

    // trampoline(8) + signo(8) + regs(32×8) + epc(8) = 280
    const SIGNAL_FRAME_SIZE: usize = 8 + 8 + (32 * 8) + 8;
    sp -= SIGNAL_FRAME_SIZE;
    sp &= !0xF;

    let frame_base = sp;

    let task = match mytask() {
        Some(t) => t,
        None => return,
    };

    unsafe {
        // Trampoline: addi a7, x0, 139 (0x08b00893) + ecall (0x00000073)
        let paddr = match task.vm_manager.translate_vaddr(frame_base) {
            Some(p) => p,
            None => return,
        };
        *(paddr as *mut u32) = 0x08b00893; // addi a7, x0, 139
        *((paddr as *mut u32).add(1)) = 0x00000073; // ecall

        let paddr = match task.vm_manager.translate_vaddr(frame_base + 8) {
            Some(p) => p,
            None => return,
        };
        *(paddr as *mut usize) = signal as usize;

        for i in 0..32 {
            let paddr = match task.vm_manager.translate_vaddr(frame_base + 16 + i * 8) {
                Some(p) => p,
                None => return,
            };
            *(paddr as *mut usize) = trapframe.regs.reg[i];
        }

        let paddr = match task.vm_manager.translate_vaddr(frame_base + 272) {
            Some(p) => p,
            None => return,
        };
        *(paddr as *mut u64) = trapframe.epc;
    }

    trapframe.epc = handler_addr as u64;
    trapframe.regs.reg[2] = sp;
    trapframe.regs.reg[10] = signal as usize; // a0 = signal number
    trapframe.regs.reg[1] = frame_base; // ra = trampoline (rt_sigreturn)
}

/// Handle fatal signals that should terminate immediately
/// This is a simplified implementation for basic signal handling
pub fn handle_fatal_signal_immediately(signal: LinuxSignal) -> Result<(), &'static str> {
    if let Some(task) = crate::task::mytask() {
        let exit_code = match signal {
            LinuxSignal::SIGKILL => 128 + 9,  // Standard SIGKILL exit code
            LinuxSignal::SIGTERM => 128 + 15, // Standard SIGTERM exit code
            LinuxSignal::SIGINT => 128 + 2,   // Standard SIGINT exit code
            _ => return Err("Not a fatal signal"),
        };

        crate::early_println!(
            "Signal {}: Immediately terminating task {} with exit code {}",
            signal as u32,
            task.get_id(),
            exit_code
        );

        // Set task state to terminated and exit
        task.exit(exit_code);
        Ok(())
    } else {
        Err("No current task to terminate")
    }
}

/// Check if a signal should be handled immediately (cannot be blocked/ignored)
pub fn is_fatal_signal(signal: LinuxSignal) -> bool {
    matches!(
        signal,
        LinuxSignal::SIGKILL | LinuxSignal::SIGTERM | LinuxSignal::SIGINT
    )
}

/// Linux sys_rt_sigreturn — Restore context saved by `setup_signal_handler`.
///
/// Reads the signal frame from the current user stack pointer and restores
/// all 32 general-purpose registers plus `epc`.  The frame layout matches
/// what `setup_signal_handler` wrote:
///
/// ```text
///   [sp + 0]:   trampoline (8 bytes, ignored here)
///   [sp + 8]:   signal number (8 bytes, ignored here)
///   [sp + 16]:  saved regs[0..31] (256 bytes)
///   [sp + 272]: saved epc (8 bytes)
/// ```
///
/// After restoration the task resumes at the instruction that was
/// interrupted by the signal.
pub fn sys_rt_sigreturn(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let frame_base = trapframe.regs.reg[2]; // SP points to signal frame

    unsafe {
        // Restore all 32 general-purpose registers
        for i in 0..32 {
            let paddr = match task.vm_manager.translate_vaddr(frame_base + 16 + i * 8) {
                Some(p) => p,
                None => return usize::MAX,
            };
            trapframe.regs.reg[i] = *(paddr as *const usize);
        }

        // Restore epc (program counter at time of signal)
        let paddr = match task.vm_manager.translate_vaddr(frame_base + 272) {
            Some(p) => p,
            None => return usize::MAX,
        };
        trapframe.epc = *(paddr as *const u64);
    }

    // Return value is irrelevant — a0 was already restored from the frame.
    0
}

/// Linux sys_tkill - Send a signal to a specific thread
///
/// tkill() sends a signal to a specific thread within the same thread group.
/// This is a simplified implementation that mainly prevents crashes.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: tid (thread ID)
///   - arg1: sig (signal number)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) on error
pub fn sys_tkill(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
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
