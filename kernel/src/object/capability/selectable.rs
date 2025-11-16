//! Selectable capability for readiness and blocking waits used by select/pselect.
//!
//! This capability provides a minimal, ABI-agnostic interface that kernel objects
//! can implement to participate in select-like syscalls. It is intentionally
//! simple: callers provide an interest (read/write/except), can query the
//! current readiness, and can block the current task until the interest
//! becomes ready or an optional timeout expires.
//!
//! Timeout semantics use kernel ticks to avoid coupling to any specific time
//! unit representation at the ABI layer. See `crate::timer` for conversion
//! helpers (e.g., `ns_to_ticks`).

use crate::arch::Trapframe;

/// Interest mask for readiness queries and waits.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadyInterest {
    pub read: bool,
    pub write: bool,
    pub except: bool,
}

impl ReadyInterest {
    pub const fn read() -> Self { Self { read: true, write: false, except: false } }
    pub const fn write() -> Self { Self { read: false, write: true, except: false } }
    pub const fn rw() -> Self { Self { read: true, write: true, except: false } }
}

/// Result mask for readiness queries.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadySet {
    pub read: bool,
    pub write: bool,
    pub except: bool,
}

impl ReadySet {
    pub const fn none() -> Self { Self { read: false, write: false, except: false } }
}

/// Outcome of a wait operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectWaitOutcome {
    Ready,
    TimedOut,
}

/// Objects that can be waited on by select/pselect.
pub trait Selectable {
    /// Return current readiness for the given interest set.
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet;

    /// Block the current task using the provided trapframe until the interest
    /// becomes ready or the optional timeout (in ticks) expires.
    ///
    /// Return `SelectWaitOutcome::TimedOut` if the timeout elapsed before
    /// readiness, otherwise `SelectWaitOutcome::Ready`.
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut Trapframe,
        timeout_ticks: Option<u64>,
    ) -> SelectWaitOutcome;
}
