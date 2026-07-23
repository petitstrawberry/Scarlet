//! Native lazy-initialized value.
//!
//! `Lazy<T, F>` stores an initializer closure and runs it the first time the
//! value is accessed via `Deref`. Subsequent accesses return the cached
//! value. This mirrors `spin::Lazy` so call sites migrate by changing the
//! import.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::Deref;

use crate::sync::once::Once;

pub struct Lazy<T, F = fn() -> T> {
    once: Once<T>,
    init: UnsafeCell<Option<F>>,
    _not_send_sync_marker: PhantomData<*mut ()>,
}

unsafe impl<T: Send + Sync, F: Send> Sync for Lazy<T, F> {}
unsafe impl<T: Send, F: Send> Send for Lazy<T, F> {}

impl<T, F> Lazy<T, F> {
    /// Create a new `Lazy` initialized by `init`.
    pub const fn new(init: F) -> Self {
        Self {
            once: Once::new(),
            init: UnsafeCell::new(Some(init)),
            _not_send_sync_marker: PhantomData,
        }
    }
}

impl<T, F: FnOnce() -> T> Lazy<T, F> {
    /// Force initialization and return a shared reference.
    ///
    /// # Returns
    ///
    /// A shared reference to the initialized value. The initializer runs at
    /// most once across all callers.
    pub fn force(this: &Self) -> &T {
        this.once.get_or_init(|| {
            // SAFETY: `Once::get_or_init` guarantees the closure runs on at
            // most one CPU. Other callers spin until `COMPLETE` and never
            // execute this body, so the `take()` cannot race.
            let init = unsafe { (*this.init.get()).take() }
                .expect("Lazy initializer consumed while uninitialized");
            init()
        })
    }
}

impl<T, F: FnOnce() -> T> Deref for Lazy<T, F> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        Self::force(self)
    }
}

impl<T, F> core::fmt::Debug for Lazy<T, F>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.once.get() {
            Some(value) => f.debug_struct("Lazy").field("value", value).finish(),
            None => f.debug_struct("Lazy").finish_non_exhaustive(),
        }
    }
}
