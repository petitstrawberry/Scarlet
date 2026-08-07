//! Basic synchronization primitives
//!
//! This module provides minimal synchronization primitives for multi-threaded applications.

use crate::syscall::{Syscall, syscall2};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

extern crate alloc;
pub use alloc::sync::Arc;
pub use spin::RwLock;

// reexport other sync primitives if needed
mod export {
    pub use core::sync::*;
}

pub use export::*;

/// Simple spin-lock based Mutex
pub struct Mutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// Create a new mutex
    pub const fn new(data: T) -> Self {
        Mutex {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Lock the mutex and return a guard
    pub fn lock(&self) -> MutexGuard<'_, T> {
        // Spin until we acquire the lock
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Yield to other threads while spinning
            // Sleep for 1ms to avoid busy waiting
            let _ = syscall2(Syscall::Sleep, 1_000_000, 0); // 1ms in nanoseconds
        }

        MutexGuard { mutex: self }
    }

    /// Try to lock the mutex without blocking
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(MutexGuard { mutex: self })
        } else {
            None
        }
    }
}

/// RAII guard for Mutex
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<'a, T> Deref for MutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
    }
}

/// A synchronization primitive which can be written to only once.
///
/// This is equivalent to `std::sync::OnceLock` but for no_std environments.
const ONCE_UNINITIALIZED: u8 = 0;
const ONCE_INITIALIZING: u8 = 1;
const ONCE_READY: u8 = 2;

struct OnceInitGuard<'a> {
    state: &'a AtomicU8,
    committed: bool,
}

impl Drop for OnceInitGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            // Allow a later caller to retry if the initializer unwinds before
            // publishing a value.
            self.state
                .store(ONCE_UNINITIALIZED, Ordering::Release);
        }
    }
}

pub struct OnceLock<T> {
    state: AtomicU8,
    data: UnsafeCell<Option<T>>,
}

unsafe impl<T: Send> Send for OnceLock<T> {}
unsafe impl<T: Send + Sync> Sync for OnceLock<T> {}

impl<T> OnceLock<T> {
    /// Creates a new `OnceLock`.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(ONCE_UNINITIALIZED),
            data: UnsafeCell::new(None),
        }
    }

    /// Gets the reference to the contained value, initializing it if needed.
    ///
    /// This method will block if another thread is currently initializing.
    pub fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        loop {
            match self.state.load(Ordering::Acquire) {
                ONCE_READY => {
                    // SAFETY: ONCE_READY is published with Release after the
                    // value has been written, so this acquire observes it.
                    return unsafe { (*self.data.get()).as_ref().unwrap() };
                }
                ONCE_UNINITIALIZED => {
                    if self
                        .state
                        .compare_exchange(
                            ONCE_UNINITIALIZED,
                            ONCE_INITIALIZING,
                            Ordering::Acquire,
                            Ordering::Acquire,
                        )
                        .is_err()
                    {
                        continue;
                    }

                    let mut init_guard = OnceInitGuard {
                        state: &self.state,
                        committed: false,
                    };
                    let value = f();
                    // SAFETY: this CPU exclusively owns the INITIALIZING
                    // state, so no other thread can access or write `data`.
                    unsafe { *self.data.get() = Some(value) };
                    self.state.store(ONCE_READY, Ordering::Release);
                    init_guard.committed = true;
                    return unsafe { (*self.data.get()).as_ref().unwrap() };
                }
                ONCE_INITIALIZING => {
                    // Do not burn a core while another thread performs the
                    // initializer (which may include filesystem or IPC I/O).
                    let _ = syscall2(Syscall::Sleep, 1_000_000, 0);
                }
                _ => unreachable!("invalid OnceLock state"),
            }
        }
    }

    /// Gets the reference to the contained value if already initialized.
    pub fn get(&self) -> Option<&T> {
        if self.state.load(Ordering::Acquire) == ONCE_READY {
            unsafe { &*self.data.get() }.as_ref()
        } else {
            None
        }
    }

    /// Sets the value if not already initialized.
    pub fn set(&self, value: T) -> Result<(), T> {
        let mut value = Some(value);
        loop {
            match self.state.load(Ordering::Acquire) {
                ONCE_READY => return Err(value.take().unwrap()),
                ONCE_INITIALIZING => {
                    let _ = syscall2(Syscall::Sleep, 1_000_000, 0);
                }
                ONCE_UNINITIALIZED => {
                    if self
                        .state
                        .compare_exchange(
                            ONCE_UNINITIALIZED,
                            ONCE_INITIALIZING,
                            Ordering::Acquire,
                            Ordering::Acquire,
                        )
                        .is_err()
                    {
                        continue;
                    }
                    // SAFETY: this CPU exclusively owns the INITIALIZING
                    // state after the successful CAS.
                    unsafe { *self.data.get() = value.take() };
                    self.state.store(ONCE_READY, Ordering::Release);
                    return Ok(());
                }
                _ => unreachable!("invalid OnceLock state"),
            }
        }
    }
}
