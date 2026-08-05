//! Synchronization primitives module.
//!
//! All primitives are kernel-native. `SpinLock`/`IrqSpinLock` and
//! `RwSpinLock`/`IrqRwSpinLock` integrate with the per-CPU `preempt_count` so
//! that lock-held sections are non-preemptible. [`Mutex`] is the sleepable
//! task-context primitive and leaves preemption enabled while held. External
//! modules import these types from `crate::sync`; the `spin` crate is no
//! longer re-exported.

pub mod cpu_local;
pub mod irq_guard;
pub mod lazy;
pub mod mutex;
pub mod once;
pub mod preempt;
pub mod rw_spinlock;
pub mod spinlock;
pub mod waker;

pub use cpu_local::CpuLocal;
pub use irq_guard::IrqGuard;
pub use lazy::Lazy;
pub use mutex::{Mutex, MutexGuard};
pub use once::Once;
pub use preempt::{
    PreemptGuard, dump_active_preempt_guards, preempt_count, preempt_disable, preempt_enable,
    preemptible,
};
pub use rw_spinlock::{
    IrqRwSpinLock, IrqRwSpinLockReadGuard, IrqRwSpinLockWriteGuard, RwSpinLock,
    RwSpinLockReadGuard, RwSpinLockWriteGuard,
};
pub use spinlock::{IrqSpinLock, IrqSpinLockGuard, RawIrqSpinLock, SpinLock, SpinLockGuard};
pub use waker::Waker;
