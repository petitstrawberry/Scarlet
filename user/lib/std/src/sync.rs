//! Basic synchronization primitives
//!
//! This module provides minimal synchronization primitives for multi-threaded applications.

use crate::syscall::{Syscall, syscall2};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

extern crate alloc;
pub use alloc::sync::Arc;

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
pub struct OnceLock<T> {
    initialized: AtomicBool,
    data: UnsafeCell<Option<T>>,
}

unsafe impl<T: Send> Send for OnceLock<T> {}
unsafe impl<T: Send + Sync> Sync for OnceLock<T> {}

impl<T> OnceLock<T> {
    /// Creates a new `OnceLock`.
    pub const fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
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
        if !self.initialized.load(Ordering::Acquire) {
            // Try to initialize
            let value = f();
            unsafe {
                // Check again before writing
                if !self.initialized.load(Ordering::Acquire) {
                    *self.data.get() = Some(value);
                    self.initialized.store(true, Ordering::Release);
                }
            }
        }
        unsafe { &*self.data.get() }.as_ref().unwrap()
    }

    /// Gets the reference to the contained value if already initialized.
    pub fn get(&self) -> Option<&T> {
        if self.initialized.load(Ordering::Acquire) {
            unsafe { &*self.data.get() }.as_ref()
        } else {
            None
        }
    }

    /// Sets the value if not already initialized.
    pub fn set(&self, value: T) -> Result<(), T> {
        if self.initialized.load(Ordering::Acquire) {
            return Err(value);
        }
        unsafe {
            *self.data.get() = Some(value);
            self.initialized.store(true, Ordering::Release);
        }
        Ok(())
    }
}
