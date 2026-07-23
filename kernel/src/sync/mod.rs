//! Synchronization primitives module.
//!
//! All primitives are kernel-native: `Mutex`/`IrqSafeMutex`,
//! `RwLock`/`IrqSafeRwLock`, `Once`, and `Lazy` integrate with the per-CPU
//! `preempt_count` so that lock-held sections are non-preemptible. External
//! modules import these types from `crate::sync`; the `spin` crate is no
//! longer re-exported.

pub mod cpu_local;
pub mod irq_guard;
pub mod lazy;
pub mod mutex;
pub mod once;
pub mod preempt;
pub mod rwlock;
pub mod waker;

pub use cpu_local::CpuLocal;
pub use irq_guard::IrqGuard;
pub use lazy::Lazy;
pub use mutex::{IrqSafeMutex, IrqSafeMutexGuard, Mutex, MutexGuard};
pub use once::Once;
pub use preempt::{preempt_count, preempt_disable, preempt_enable, preemptible, PreemptGuard};
pub use rwlock::{
    IrqSafeRwLock, IrqSafeRwLockReadGuard, IrqSafeRwLockWriteGuard, RwLock, RwLockReadGuard,
    RwLockWriteGuard,
};
pub use waker::Waker;
