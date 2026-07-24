//! Native one-time initialization primitive.
//!
//! `Once<T>` initializes a value at most once and grants long-lived shared
//! references afterwards. The first caller to invoke [`Once::get_or_init`]
//! runs the supplied closure; concurrent callers spin until initialization
//! completes.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, Ordering};

const UNINIT: u8 = 0;
const INITIALIZING: u8 = 1;
const COMPLETE: u8 = 2;

pub struct Once<T = ()> {
    state: AtomicU8,
    data: UnsafeCell<MaybeUninit<T>>,
}

// SAFETY: `Once` synchronizes initialization via `state`. Once COMPLETE,
// the inner `T` is immutable and may be shared across CPUs. Before
// COMPLETE, only the initializing CPU has access.
unsafe impl<T: Send + Sync> Sync for Once<T> {}
unsafe impl<T: Send> Send for Once<T> {}

impl<T> Once<T> {
    /// Create an uninitialized `Once`.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(UNINIT),
            data: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Return whether initialization has completed.
    ///
    /// # Returns
    ///
    /// `true` once a prior [`Once::get_or_init`] call has finished.
    #[inline]
    pub fn is_completed(&self) -> bool {
        self.state.load(Ordering::Acquire) == COMPLETE
    }

    /// Return a shared reference if initialization has completed.
    ///
    /// # Returns
    ///
    /// `Some(&T)` once initialized, otherwise `None`.
    #[inline]
    pub fn get(&self) -> Option<&T> {
        if self.is_completed() {
            // SAFETY: `state == COMPLETE` means initialization has finished
            // and the inner value is immutable thereafter.
            Some(unsafe { (*self.data.get()).assume_init_ref() })
        } else {
            None
        }
    }

    /// Initialize with `f` if needed and return a shared reference.
    ///
    /// The closure runs at most once across all callers. Concurrent
    /// callers spin until the first invocation finishes.
    ///
    /// # Returns
    ///
    /// A shared reference to the initialized value.
    pub fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        match self.state.load(Ordering::Acquire) {
            COMPLETE => {
                // SAFETY: COMPLETE means the inner value is initialized and
                // immutable.
                unsafe { (*self.data.get()).assume_init_ref() }
            }
            _ => self.init_slow(f),
        }
    }

    /// Initialize with `f` if needed and return a shared reference.
    ///
    /// Alias for [`Once::get_or_init`] matching the `spin::Once::call_once`
    /// surface that existing callers depend on.
    ///
    /// # Returns
    ///
    /// A shared reference to the initialized value.
    pub fn call_once<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        self.get_or_init(f)
    }

    #[cold]
    fn init_slow<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        loop {
            match self.state.compare_exchange(
                UNINIT,
                INITIALIZING,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // SAFETY: We hold INITIALIZING; no other writer can
                    // touch `data` until we publish COMPLETE.
                    unsafe { (*self.data.get()).write(f()) };
                    self.state.store(COMPLETE, Ordering::Release);
                    // SAFETY: We just published COMPLETE.
                    return unsafe { (*self.data.get()).assume_init_ref() };
                }
                Err(COMPLETE) => {
                    // SAFETY: Another CPU finished initialization.
                    return unsafe { (*self.data.get()).assume_init_ref() };
                }
                Err(_) => {
                    while self.state.load(Ordering::Relaxed) != COMPLETE {
                        core::hint::spin_loop();
                    }
                }
            }
        }
    }

    /// Return a shared reference without checking initialization.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that `get_or_init` has already returned on
    /// some CPU. Use [`Once::get`] for the checked alternative.
    #[inline]
    pub unsafe fn get_unchecked(&self) -> &T {
        // SAFETY: Caller guarantees COMPLETE.
        unsafe { (*self.data.get()).assume_init_ref() }
    }
}

impl<T> core::fmt::Debug for Once<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Once")
            .field("state", &self.state.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<T> Drop for Once<T> {
    fn drop(&mut self) {
        if self.state.load(Ordering::Relaxed) == COMPLETE {
            // SAFETY: COMPLETE means the inner value is initialized; we
            // have exclusive access via `&mut self`.
            unsafe { self.data.get_mut().assume_init_drop() };
        }
    }
}

impl<T> Default for Once<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering};

    #[test_case]
    fn test_once_starts_uninitialized() {
        let once: Once<u32> = Once::new();
        assert!(!once.is_completed());
        assert!(once.get().is_none());
    }

    #[test_case]
    fn test_once_initializes_once() {
        let once = Once::new();
        let counter = AtomicU32::new(0);
        let v1 = once.get_or_init(|| {
            counter.fetch_add(1, Ordering::Relaxed);
            42u32
        });
        let v2 = once.get_or_init(|| {
            counter.fetch_add(1, Ordering::Relaxed);
            99u32
        });
        assert_eq!(*v1, 42);
        assert_eq!(*v2, 42);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
        assert!(once.is_completed());
        assert_eq!(*once.get().unwrap(), 42);
    }

    #[test_case]
    fn test_once_default_matches_new() {
        let a: Once<u32> = Once::new();
        let b: Once<u32> = Once::default();
        assert!(!a.is_completed());
        assert!(!b.is_completed());
    }
}
