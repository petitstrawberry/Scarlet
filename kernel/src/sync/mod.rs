//! Synchronization primitives module
//!
//! This module provides various synchronization primitives for the Scarlet kernel.
//! External modules should use these re-exports instead of depending on `spin` directly,
//! so that the kernel can control the underlying lock implementation.

pub mod cpu_local;
pub mod irq_guard;
pub mod mutex;
pub mod once;
pub mod preempt;
pub mod rwlock;
pub mod waker;

pub use cpu_local::CpuLocal;
pub use irq_guard::IrqGuard;
pub use waker::Waker;

pub use mutex::{
    IrqSafeMutex, IrqSafeMutexGuard, Mutex as NativeMutex, MutexGuard as NativeMutexGuard,
};
pub use once::Once as NativeOnce;
pub use preempt::{preempt_count, preempt_disable, preempt_enable, preemptible, PreemptGuard};
pub use rwlock::{
    IrqSafeRwLock, IrqSafeRwLockReadGuard, IrqSafeRwLockWriteGuard, RwLock as NativeRwLock,
    RwLockReadGuard as NativeRwLockReadGuard, RwLockWriteGuard as NativeRwLockWriteGuard,
};

pub use spin::{Mutex, MutexGuard, Once, RwLock, RwLockReadGuard, RwLockWriteGuard};
