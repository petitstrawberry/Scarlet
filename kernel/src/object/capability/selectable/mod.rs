//! Selectable capability for readiness and blocking waits used by select/pselect.
//!
//! This capability provides a minimal, ABI-agnostic interface that kernel objects
//! can implement to participate in select-like syscalls. It is intentionally
//! simple: callers provide an interest (read/write/except), can query the
//! current readiness, and can block the current task until the interest
//! becomes ready or an optional timeout expires.
//!
//! Timeout semantics use relative nanoseconds at the ABI boundary and absolute
//! monotonic nanosecond deadlines in the timer core.

pub mod syscall;

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::arch::Trapframe;
use crate::sync::{ReadinessRegistration, Waker};

/// Maximum one-shot delay between readiness scans for objects without native
/// subscriptions. The delay is always clipped to the caller's absolute deadline.
pub const MULTI_READINESS_RECHECK_NS: u64 = crate::timer::NANOSECONDS_PER_MILLISECOND;

/// Return the next deadline-clipped readiness recheck delay.
///
/// Objects that implement [`Selectable::subscribe_readiness`] use native
/// notification. For each object that returns `None`, callers retain a bounded
/// one-shot recheck so a readiness change cannot be missed indefinitely.
///
/// # Arguments
///
/// * `deadline_ns` - Optional absolute monotonic timeout deadline.
/// * `now_ns` - Current monotonic time in nanoseconds.
///
/// # Returns
///
/// The next relative recheck delay, or `None` when the deadline elapsed.
pub fn multi_readiness_recheck_delay(deadline_ns: Option<u64>, now_ns: u64) -> Option<u64> {
    match deadline_ns {
        Some(deadline_ns) => {
            let remaining_ns = deadline_ns.saturating_sub(now_ns);
            (remaining_ns > 0).then_some(remaining_ns.min(MULTI_READINESS_RECHECK_NS))
        }
        None => Some(MULTI_READINESS_RECHECK_NS),
    }
}

/// Interest mask for readiness queries and waits.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadyInterest {
    pub read: bool,
    pub write: bool,
    pub except: bool,
}

impl ReadyInterest {
    pub const fn read() -> Self {
        Self {
            read: true,
            write: false,
            except: false,
        }
    }
    pub const fn write() -> Self {
        Self {
            read: false,
            write: true,
            except: false,
        }
    }
    pub const fn rw() -> Self {
        Self {
            read: true,
            write: true,
            except: false,
        }
    }
}

/// Result mask for readiness queries.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadySet {
    pub read: bool,
    pub write: bool,
    pub except: bool,
}

impl ReadySet {
    pub const fn none() -> Self {
        Self {
            read: false,
            write: false,
            except: false,
        }
    }
}

/// Outcome of a wait operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectWaitOutcome {
    Ready,
    TimedOut,
    /// Return to the syscall boundary to dispatch pending process control.
    Interrupted,
}

/// Objects that can be waited on by select/pselect.
pub trait Selectable {
    /// Whether the peer endpoint has disconnected, independent of requested readiness.
    fn is_hung_up(&self) -> bool {
        false
    }

    /// Return current readiness for the given interest set.
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        // Default: treat as always-ready for read/write interests, except is false
        let mut set = ReadySet::none();
        if interest.read {
            set.read = true;
        }
        if interest.write {
            set.write = true;
        }
        if interest.except {
            set.except = false;
        }
        set
    }

    /// Register native readiness notification for this object.
    ///
    /// `None` means the object cannot provide native notification and the
    /// caller must retain its bounded readiness recheck fallback. `Some(empty)`
    /// means readiness is constant for this interest and needs no further
    /// polling after the caller's current readiness check. A non-empty result
    /// contains persistent registrations that the caller should retain across
    /// waits. Sample their generations immediately before each readiness scan
    /// so a notification during the scan is visible on the next iteration.
    fn subscribe_readiness(
        &self,
        _interest: ReadyInterest,
        _waiter: &Arc<Waker>,
    ) -> Option<Vec<ReadinessRegistration>> {
        None
    }

    /// Block the current task using the provided trapframe until the interest
    /// becomes ready or the optional timeout (in nanoseconds) expires.
    ///
    /// If `min_wait_ns > 0`, the task will remain blocked for at least that
    /// duration even if the interest becomes ready earlier. After the minimum
    /// wait elapses, normal readiness-check + timeout semantics apply.
    ///
    /// Return `SelectWaitOutcome::TimedOut` if the timeout elapsed before
    /// readiness, `Interrupted` for a signal wake, otherwise `Ready`.
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut Trapframe,
        timeout_ns: Option<u64>,
        min_wait_ns: u64,
    ) -> SelectWaitOutcome;

    /// Enable or disable non-blocking I/O semantics on this object.
    ///
    /// When enabled, operations that would otherwise block (e.g., reads with
    /// no data available, writes to a full buffer) should avoid internal waits
    /// and instead report a WouldBlock condition to the caller.
    fn set_nonblocking(&self, _enabled: bool) {}

    /// Query whether non-blocking I/O semantics are enabled on this object.
    fn is_nonblocking(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn multi_readiness_recheck_clips_to_remaining_deadline() {
        assert_eq!(multi_readiness_recheck_delay(Some(1_500), 1_000), Some(500));
        assert_eq!(
            multi_readiness_recheck_delay(Some(MULTI_READINESS_RECHECK_NS.saturating_mul(3)), 0,),
            Some(MULTI_READINESS_RECHECK_NS)
        );
    }

    #[test_case]
    fn multi_readiness_recheck_stops_at_elapsed_deadline() {
        assert_eq!(multi_readiness_recheck_delay(Some(1_000), 1_000), None);
        assert_eq!(
            multi_readiness_recheck_delay(None, 0),
            Some(MULTI_READINESS_RECHECK_NS)
        );
    }
}
