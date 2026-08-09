//! Basic synchronization primitives
//!
//! This module provides minimal synchronization primitives for multi-threaded applications.

use crate::syscall::{Syscall, syscall1, syscall2, syscall3};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};

extern crate alloc;
pub use alloc::sync::Arc;
pub use spin::RwLock;

// reexport other sync primitives if needed
mod export {
    pub use core::sync::*;
}

pub use export::*;

const MUTEX_UNLOCKED: u32 = 0;
const MUTEX_LOCKED: u32 = 1;
const MUTEX_CONTENDED: u32 = 2;

fn futex_wait(word: &AtomicU32, expected: u32) {
    let result = syscall3(
        Syscall::FutexWait,
        word as *const AtomicU32 as usize,
        expected as usize,
        usize::MAX,
    );
    if result == usize::MAX {
        // Keep mixed old-kernel/new-userland images from turning lock
        // contention into a tight syscall loop.
        let _ = syscall1(Syscall::Sleep, 10_000_000);
    }
}

fn futex_wake(word: &AtomicU32, count: usize) {
    let _ = syscall2(Syscall::FutexWake, word as *const AtomicU32 as usize, count);
}

/// Process-private sleeping mutex.
pub struct Mutex<T> {
    state: AtomicU32,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// Create a new mutex
    pub const fn new(data: T) -> Self {
        Mutex {
            state: AtomicU32::new(MUTEX_UNLOCKED),
            data: UnsafeCell::new(data),
        }
    }

    /// Lock the mutex and return a guard
    pub fn lock(&self) -> MutexGuard<'_, T> {
        if self
            .state
            .compare_exchange(
                MUTEX_UNLOCKED,
                MUTEX_LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_err()
        {
            loop {
                if self.state.swap(MUTEX_CONTENDED, Ordering::Acquire) == MUTEX_UNLOCKED {
                    break;
                }
                futex_wait(&self.state, MUTEX_CONTENDED);
            }
        }

        MutexGuard { mutex: self }
    }

    /// Try to lock the mutex without blocking
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self
            .state
            .compare_exchange(
                MUTEX_UNLOCKED,
                MUTEX_LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
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
        if self.mutex.state.swap(MUTEX_UNLOCKED, Ordering::Release) == MUTEX_CONTENDED {
            futex_wake(&self.mutex.state, 1);
        }
    }
}

/// A synchronization primitive which can be written to only once.
///
/// This is equivalent to `std::sync::OnceLock` but for no_std environments.
const ONCE_UNINITIALIZED: u32 = 0;
const ONCE_INITIALIZING: u32 = 1;
const ONCE_READY: u32 = 2;

struct OnceInitGuard<'a> {
    state: &'a AtomicU32,
    committed: bool,
}

impl Drop for OnceInitGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            // Allow a later caller to retry if the initializer unwinds before
            // publishing a value.
            self.state.store(ONCE_UNINITIALIZED, Ordering::Release);
            futex_wake(self.state, usize::MAX);
        }
    }
}

pub struct OnceLock<T> {
    state: AtomicU32,
    data: UnsafeCell<Option<T>>,
}

unsafe impl<T: Send> Send for OnceLock<T> {}
unsafe impl<T: Send + Sync> Sync for OnceLock<T> {}

impl<T> OnceLock<T> {
    /// Creates a new `OnceLock`.
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(ONCE_UNINITIALIZED),
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
                    futex_wake(&self.state, usize::MAX);
                    init_guard.committed = true;
                    return unsafe { (*self.data.get()).as_ref().unwrap() };
                }
                ONCE_INITIALIZING => {
                    futex_wait(&self.state, ONCE_INITIALIZING);
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
                    futex_wait(&self.state, ONCE_INITIALIZING);
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
                    futex_wake(&self.state, usize::MAX);
                    return Ok(());
                }
                _ => unreachable!("invalid OnceLock state"),
            }
        }
    }
}
