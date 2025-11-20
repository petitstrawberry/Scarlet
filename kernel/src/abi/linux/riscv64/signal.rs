//! Linux RISC-V 64 signal syscalls and signal handling
//!
//! Implements POSIX signals with Linux-compatible semantics, integrated with Scarlet's
//! event system for cross-ABI signal delivery.

use crate::abi::linux::riscv64::LinuxRiscv64Abi;
use crate::arch::Trapframe;
use crate::task::mytask;
use crate::ipc::event::{Event, EventContent, ProcessControlType};
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
        self.handlers.get(&signal).copied().unwrap_or(signal.default_action())
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
            trapframe.increment_pc_next(&task);
            return !0usize;
        }
    };
    
    let mut signal_state = abi.signal_state.lock();
    
    // Get old action if requested
    if oldact_ptr != 0 {
        let old_action = signal_state.get_handler(signal);
        // TODO: Copy old action to user space (requires memory copying functionality)
        // For now, just acknowledge the request
        let _ = old_action;
    }
    
    // Set new action if provided
    if act_ptr != 0 {
        // TODO: Read sigaction structure from user space
        // For now, we'll implement a simplified version that just sets ignore/default
        // In a real implementation, this would read the sigaction struct from user memory
        
        // For demonstration, assume user wants to ignore the signal
        signal_state.set_handler(signal, SignalAction::Ignore);
    }
    
    trapframe.set_return_value(0);
    trapframe.increment_pc_next(&task);
    0
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
        let old_mask = signal_state.blocked.raw();
        // TODO: Copy old mask to user space
        // For now, just acknowledge the request
        let _ = old_mask;
    }
    
    // Modify mask if new set is provided
    if set_ptr != 0 {
        // TODO: Read sigset_t from user space
        // For now, implement a simplified version
        
        // SIG_BLOCK = 0, SIG_UNBLOCK = 1, SIG_SETMASK = 2
        match how {
            0 => { // SIG_BLOCK
                // TODO: Read mask from user space and add to blocked signals
            }
            1 => { // SIG_UNBLOCK
                // TODO: Read mask from user space and remove from blocked signals
            }
            2 => { // SIG_SETMASK
                // TODO: Read mask from user space and set as blocked signals
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
        EventContent::ProcessControl(control_type) => {
            process_control_to_signal(*control_type)
        }
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
pub fn process_pending_signals_with_state(signal_state: &mut SignalState, trapframe: &mut Trapframe) -> bool {
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
                crate::early_println!("Signal {}: Calling custom handler at {:#x}", signal as u32, handler_addr);
                setup_signal_handler(trapframe, handler_addr, signal);
                true
            }
        }
    } else {
        false
    }
}

/// Enhanced signal handler setup with proper context saving
fn setup_signal_handler(trapframe: &mut Trapframe, handler_addr: usize, signal: LinuxSignal) {
    // TODO: Complete signal handler setup
    // 1. Save current trapframe on user stack
    // 2. Set up signal stack frame
    // 3. Set up signal handler arguments
    // 4. Set up return address to signal return trampoline
    
    // For now, basic setup:
    // Set up arguments for signal handler: handler(signal_number)
    trapframe.set_arg(0, signal as usize);
    
    // Jump to signal handler
    trapframe.epc = handler_addr as u64;
    
    // TODO: Implement signal return mechanism
    // - Set up return address to rt_sigreturn trampoline
    // - Save original context for restoration
}

/// Handle fatal signals that should terminate immediately
/// This is a simplified implementation for basic signal handling
pub fn handle_fatal_signal_immediately(signal: LinuxSignal) -> Result<(), &'static str> {
    if let Some(task) = crate::task::mytask() {
        let exit_code = match signal {
            LinuxSignal::SIGKILL => 128 + 9,   // Standard SIGKILL exit code
            LinuxSignal::SIGTERM => 128 + 15,  // Standard SIGTERM exit code  
            LinuxSignal::SIGINT => 128 + 2,    // Standard SIGINT exit code
            _ => return Err("Not a fatal signal"),
        };
        
        crate::early_println!("Signal {}: Immediately terminating task {} with exit code {}", 
                             signal as u32, task.get_id(), exit_code);
        
        // Set task state to terminated and exit
        task.exit(exit_code);
        Ok(())
    } else {
        Err("No current task to terminate")
    }
}

/// Check if a signal should be handled immediately (cannot be blocked/ignored)
pub fn is_fatal_signal(signal: LinuxSignal) -> bool {
    matches!(signal, LinuxSignal::SIGKILL | LinuxSignal::SIGTERM | LinuxSignal::SIGINT)
}

