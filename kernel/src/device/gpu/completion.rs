//! Authoritative, read-only observation of one accepted GPU submission.
//!
//! Only the kernel producer can settle a completion. Observers are selectable
//! capabilities, not user-signallable timelines. Observation lifetime is separate
//! from driver ownership of commands and backing: failure or closing every
//! observer does not establish hardware quiescence or permit resource reuse.

use alloc::sync::Arc;

use super::connection::{read_user_value, write_user_value};
use super::{
    GPU_ABI_VERSION, GPU_COMPLETION_COMPLETE, GPU_COMPLETION_FAILED,
    GPU_COMPLETION_FAILURE_ABANDONED, GPU_COMPLETION_FAILURE_DEVICE_LOST,
    GPU_COMPLETION_FAILURE_EXECUTION, GPU_COMPLETION_PENDING, GPU_COMPLETION_QUERY,
    GPU_RESULT_INVALID_ABI, GPU_RESULT_INVALID_ARGUMENT, GpuCompletionInfo, GpuObject,
};
use crate::object::capability::ControlOps;
use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::sync::IrqSpinLock;
use crate::sync::waker::{WaitResult, Waker};

/// Reason an accepted submission cannot report successful retirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum GpuCompletionFailure {
    /// A hardware fault, timeout, or reset made the device unusable.
    DeviceLost = GPU_COMPLETION_FAILURE_DEVICE_LOST,
    /// The producer was dropped without publishing a terminal observation.
    Abandoned = GPU_COMPLETION_FAILURE_ABANDONED,
    /// The accepted submission encountered another execution failure.
    Execution = GPU_COMPLETION_FAILURE_EXECUTION,
}

/// Kernel observation of one submission; terminal states never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuCompletionState {
    /// Covered work has not yet been observed to retire.
    Pending,
    /// All covered GPU accesses have retired successfully.
    Complete,
    /// Completion failed; this does not certify that GPU access has stopped.
    Failed(GpuCompletionFailure),
}

struct CompletionShared {
    state: IrqSpinLock<GpuCompletionState>,
    waker: Waker,
}

impl CompletionShared {
    fn settle(&self, state: GpuCompletionState) {
        {
            let mut current = self.state.lock();
            if *current != GpuCompletionState::Pending {
                return;
            }
            *current = state;
        }
        // Publish the terminal level before the broadcast. Waiters recheck it
        // after registration, including those arriving after another observer
        // has consumed the coalesced notification.
        self.waker.wake_all();
    }
}

/// Read-only GPU completion capability, independent of the submitting handle.
#[derive(Clone)]
pub struct GpuCompletion {
    shared: Arc<CompletionShared>,
}

impl GpuCompletion {
    /// Create a pending observer and its unique kernel-only producer.
    ///
    /// # Returns
    ///
    /// An observer that can be installed in a user handle table, and a producer
    /// that must remain owned by the driver until completion or failure. Neither
    /// object takes ownership of GPU resources; the in-flight submission must
    /// retain those separately, including after failure until access has stopped.
    pub fn pair() -> (Self, GpuCompletionSignal) {
        let shared = Arc::new(CompletionShared {
            state: IrqSpinLock::new(GpuCompletionState::Pending),
            waker: Waker::new_interruptible("gpu_completion"),
        });
        (
            Self {
                shared: Arc::clone(&shared),
            },
            GpuCompletionSignal { shared },
        )
    }

    /// Read the current observation without waiting for hardware.
    ///
    /// # Returns
    ///
    /// Pending, successful retirement, or a terminal failure.
    pub fn state(&self) -> GpuCompletionState {
        *self.shared.state.lock()
    }

    fn is_ready(&self) -> bool {
        self.state() != GpuCompletionState::Pending
    }

    fn fill_query(&self, info: &mut GpuCompletionInfo) {
        let abi_version = info.abi_version;
        let reserved = info.reserved;
        let reserved2 = info.reserved2;
        *info = GpuCompletionInfo::new();
        info.abi_version = abi_version;
        if abi_version != GPU_ABI_VERSION {
            info.result = GPU_RESULT_INVALID_ABI;
            return;
        }
        if reserved != 0 || reserved2 != 0 {
            info.result = GPU_RESULT_INVALID_ARGUMENT;
            return;
        }
        match self.state() {
            GpuCompletionState::Pending => info.state = GPU_COMPLETION_PENDING,
            GpuCompletionState::Complete => info.state = GPU_COMPLETION_COMPLETE,
            GpuCompletionState::Failed(reason) => {
                info.state = GPU_COMPLETION_FAILED;
                info.failure = reason as u32;
            }
        }
    }
}

impl ControlOps for GpuCompletion {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        if command != GPU_COMPLETION_QUERY {
            return Err("Unsupported GPU completion control command");
        }
        let mut info: GpuCompletionInfo = read_user_value(arg)?;
        self.fill_query(&mut info);
        write_user_value(arg, &info)?;
        Ok(0)
    }
}

impl GpuObject for GpuCompletion {
    fn as_control_ops(&self) -> Option<&dyn ControlOps> {
        Some(self)
    }

    fn as_selectable(&self) -> Option<&dyn Selectable> {
        Some(self)
    }
}

impl Selectable for GpuCompletion {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let ready = self.is_ready();
        ReadySet {
            read: ready && interest.read,
            write: false,
            except: ready && interest.except,
        }
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        timeout_ns: Option<u64>,
        min_wait_ns: u64,
    ) -> SelectWaitOutcome {
        if !interest.read && !interest.except {
            return SelectWaitOutcome::TimedOut;
        }
        if min_wait_ns == 0 && self.is_ready() {
            return SelectWaitOutcome::Ready;
        }
        if timeout_ns == Some(0) {
            return SelectWaitOutcome::TimedOut;
        }
        let cpu_id = crate::arch::get_cpu().get_cpuid();
        let Some(task_id) = crate::sched::scheduler::current_task_id(cpu_id) else {
            return SelectWaitOutcome::TimedOut;
        };
        let deadline = timeout_ns.map(|ns| crate::timer::get_time_ns().saturating_add(ns));
        let mut minimum_ns = min_wait_ns;
        loop {
            let remaining_ns = deadline.map(|ns| ns.saturating_sub(crate::timer::get_time_ns()));
            let result = self.shared.waker.wait_with_condition(
                task_id,
                trapframe,
                remaining_ns,
                minimum_ns,
                || self.is_ready(),
            );
            minimum_ns = 0;
            if self.is_ready() {
                return SelectWaitOutcome::Ready;
            }
            match result {
                WaitResult::Woken => continue,
                WaitResult::TimedOut => return SelectWaitOutcome::TimedOut,
                // Selectable has no Interrupted variant. Return to the syscall
                // so its readiness rescan and process-control delivery can run.
                WaitResult::Interrupted => return SelectWaitOutcome::Ready,
            }
        }
    }
}

/// Unique kernel-only authority for settling a GPU completion.
///
/// This is deliberately not a `GpuObject`, cloneable, or user-signallable.
/// Dropping it while pending reports abandonment, never successful retirement.
pub struct GpuCompletionSignal {
    shared: Arc<CompletionShared>,
}

impl GpuCompletionSignal {
    /// Publish that every GPU access covered by this submission has retired.
    ///
    /// The driver must establish retirement before calling this method. This
    /// does not perform cache maintenance, readback, presentation, or SWS release.
    ///
    /// # Returns
    ///
    /// Nothing; wakes all observers and consumes the producer authority.
    pub fn complete(self) {
        self.shared.settle(GpuCompletionState::Complete);
    }

    /// Publish a terminal failure without claiming hardware quiescence.
    ///
    /// # Arguments
    ///
    /// * `reason` - Failure observed by the driver. Resource ownership must still
    ///   be retained independently until GPU access has stopped.
    ///
    /// # Returns
    ///
    /// Nothing; wakes all observers and consumes the producer authority.
    pub fn fail(self, reason: GpuCompletionFailure) {
        self.shared.settle(GpuCompletionState::Failed(reason));
    }
}

impl Drop for GpuCompletionSignal {
    fn drop(&mut self) {
        self.shared
            .settle(GpuCompletionState::Failed(GpuCompletionFailure::Abandoned));
    }
}

#[cfg(test)]
mod tests {
    use super::{GpuCompletion, GpuCompletionFailure, GpuCompletionState};
    use crate::device::gpu::{
        GPU_ABI_VERSION, GPU_COMPLETION_COMPLETE, GPU_COMPLETION_FAILED,
        GPU_COMPLETION_FAILURE_ABANDONED, GPU_RESULT_INVALID_ABI, GPU_RESULT_INVALID_ARGUMENT,
        GpuCompletionInfo, GpuObject,
    };
    use crate::object::capability::ControlOps;
    use crate::object::capability::selectable::{ReadyInterest, SelectWaitOutcome, Selectable};

    #[test_case]
    fn completion_observers_own_state_and_cannot_signal_it() {
        let (observer, signal) = GpuCompletion::pair();
        let surviving = observer.clone();
        assert!(observer.as_timeline().is_none());
        assert!(observer.as_memory_mappable().is_none());
        for command in [
            super::super::GPU_TIMELINE_SIGNAL,
            super::super::GPU_TIMELINE_FAIL,
        ] {
            assert!(observer.control(command, 0).is_err());
        }
        assert_eq!(observer.state(), GpuCompletionState::Pending);
        drop(observer);
        signal.complete();
        assert_eq!(surviving.state(), GpuCompletionState::Complete);
        let mut info = GpuCompletionInfo::new();
        surviving.fill_query(&mut info);
        assert_eq!(info.state, GPU_COMPLETION_COMPLETE);
        assert_eq!(info.failure, 0);
    }

    #[test_case]
    fn abandoned_producer_is_failed_not_complete() {
        let (observer, signal) = GpuCompletion::pair();
        drop(signal);
        let mut info = GpuCompletionInfo::new();
        observer.fill_query(&mut info);
        assert_eq!(info.state, GPU_COMPLETION_FAILED);
        assert_eq!(info.failure, GPU_COMPLETION_FAILURE_ABANDONED);
    }

    #[test_case]
    fn successful_and_failed_completions_are_readable_but_never_writable() {
        for failure in [None, Some(GpuCompletionFailure::DeviceLost)] {
            let (observer, signal) = GpuCompletion::pair();
            assert!(!observer.current_ready(ReadyInterest::rw()).read);
            match failure {
                Some(reason) => signal.fail(reason),
                None => signal.complete(),
            }
            let ready = observer.current_ready(ReadyInterest {
                read: true,
                write: true,
                except: true,
            });
            assert!(ready.read);
            assert!(ready.except);
            assert!(!ready.write);
            assert_eq!(
                observer.wait_until_ready(
                    ReadyInterest::read(),
                    &mut crate::arch::Trapframe::new(),
                    Some(0),
                    0
                ),
                SelectWaitOutcome::Ready
            );
            assert_eq!(
                observer.wait_until_ready(
                    ReadyInterest::write(),
                    &mut crate::arch::Trapframe::new(),
                    Some(0),
                    0
                ),
                SelectWaitOutcome::TimedOut
            );
        }
    }

    #[test_case]
    fn completion_query_rejects_reserved_fields_and_unsupported_abi() {
        let (observer, signal) = GpuCompletion::pair();
        signal.fail(GpuCompletionFailure::Execution);
        let mut info = GpuCompletionInfo::new();
        info.reserved = 1;
        observer.fill_query(&mut info);
        assert_eq!(info.result, GPU_RESULT_INVALID_ARGUMENT);
        assert_eq!(info.reserved, 0);
        info.reserved2 = 1;
        observer.fill_query(&mut info);
        assert_eq!(info.result, GPU_RESULT_INVALID_ARGUMENT);
        assert_eq!(info.reserved2, 0);
        info.abi_version += 1;
        observer.fill_query(&mut info);
        assert_eq!(info.result, GPU_RESULT_INVALID_ABI);
        assert_eq!(info.abi_version, GPU_ABI_VERSION + 1);
    }

    #[test_case]
    fn closing_all_observers_does_not_settle_the_producer() {
        let (observer, signal) = GpuCompletion::pair();
        drop(observer);
        assert_eq!(*signal.shared.state.lock(), GpuCompletionState::Pending);
        signal.complete();
    }
}
