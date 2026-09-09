//! Non-reused 64-bit identities with capability-selected synchronization.
//!
//! This synchronizes identity allocation only. It neither publishes associated
//! objects nor supplies the memory-ordering interface of a core atomic type.
//! Without native 64-bit atomics, allocation can wait for an IRQ-safe lock and
//! requires initialized per-CPU state. Do not use it in emergency diagnostics.

use core::num::NonZeroU64;

#[cfg(target_has_atomic = "64")]
use native::Sequence;
#[cfg(not(target_has_atomic = "64"))]
use protected::Sequence;

/// A sequence of nonzero identities that stops permanently at exhaustion.
/// Native 64-bit atomic targets allocate without locks. Other targets serialize
/// the complete 64-bit update with the kernel's existing IRQ-safe spin lock.
pub(crate) struct IdSequence {
    inner: Sequence,
}

impl IdSequence {
    pub const fn new() -> Self {
        Self {
            inner: Sequence::new(1),
        }
    }

    /// Reserve one identity. `None` means every nonzero value was issued.
    /// No ordering of memory outside this sequence is guaranteed.
    pub fn reserve(&self) -> Option<NonZeroU64> {
        self.inner.reserve()
    }
}

fn successor(next: u64) -> Option<u64> {
    // Zero is a terminal exhausted state, never an issued identity. Wrapping
    // MAX to zero lets the last nonzero identity be issued exactly once.
    (next != 0).then(|| next.wrapping_add(1))
}

#[cfg(target_has_atomic = "64")]
mod native {
    use super::{NonZeroU64, successor};
    use core::sync::atomic::{AtomicU64, Ordering};

    pub(super) struct Sequence {
        next: AtomicU64,
    }

    impl Sequence {
        pub const fn new(first: u64) -> Self {
            Self {
                next: AtomicU64::new(first),
            }
        }

        pub fn reserve(&self) -> Option<NonZeroU64> {
            self.next
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, successor)
                .ok()
                .and_then(NonZeroU64::new)
        }
    }
}

// Compile the protected implementation in kernel tests even on native targets,
// so both implementations receive the same boundary and exhaustion checks.
#[cfg(any(test, not(target_has_atomic = "64")))]
mod protected {
    use super::{NonZeroU64, successor};
    use crate::sync::IrqSpinLock;

    pub(super) struct Sequence {
        next: IrqSpinLock<u64>,
    }

    impl Sequence {
        pub const fn new(first: u64) -> Self {
            Self {
                next: IrqSpinLock::new(first),
            }
        }

        pub fn reserve(&self) -> Option<NonZeroU64> {
            let mut next = self.next.lock();
            let following = successor(*next)?;
            let id = NonZeroU64::new(*next);
            *next = following;
            id
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_pointer_boundary(reserve: impl Fn() -> Option<NonZeroU64>) {
        for expected in [u32::MAX as u64, 0x1_0000_0000, 0x1_0000_0001] {
            assert_eq!(reserve().map(NonZeroU64::get), Some(expected));
        }
    }

    fn check_exhaustion(reserve: impl Fn() -> Option<NonZeroU64>) {
        assert_eq!(reserve().map(NonZeroU64::get), Some(u64::MAX - 1));
        assert_eq!(reserve().map(NonZeroU64::get), Some(u64::MAX));
        assert_eq!(reserve(), None);
        assert_eq!(reserve(), None);
    }

    #[test_case]
    fn protected_ids_preserve_width_and_never_wrap_back_to_live_values() {
        let sequence = protected::Sequence::new(u32::MAX as u64);
        check_pointer_boundary(|| sequence.reserve());
        let sequence = protected::Sequence::new(u64::MAX - 1);
        check_exhaustion(|| sequence.reserve());
    }

    #[cfg(target_has_atomic = "64")]
    #[test_case]
    fn native_ids_preserve_width_and_never_wrap_back_to_live_values() {
        let sequence = native::Sequence::new(u32::MAX as u64);
        check_pointer_boundary(|| sequence.reserve());
        let sequence = native::Sequence::new(u64::MAX - 1);
        check_exhaustion(|| sequence.reserve());
    }
}
