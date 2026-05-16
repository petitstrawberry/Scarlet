//! Per-CPU data abstraction for SMP support.
//!
//! `CpuLocal<T>` provides per-CPU copies of data, indexed by CPU ID.
//! Each CPU accesses only its own slot, avoiding cross-CPU data races.
//!
//! # Safety Model
//!
//! - `get()` returns `&T` — safe, multiple readers allowed
//! - `lock()` returns `CpuLocalGuard<'_, T>` holding `&mut T` + `IrqGuard`
//!   - The `IrqGuard` proves interrupts are disabled, preventing re-entrant access
//!   - Within the same task, calling `lock()` twice while the first guard lives
//!     is technically possible but documented as forbidden (kernel code convention)
//!
//! # Architecture
//!
//! Data is stored as `[UnsafeCell<T>; MAX_NUM_CPUS]`, indexed by `get_cpu().get_cpuid()`.
//! CPU ID comes from architecture-specific registers (sscratch on RISC-V, TPIDR_EL1 on AArch64).

use crate::arch::get_cpu;
use crate::environment::MAX_NUM_CPUS;
use crate::sync::IrqGuard;
use core::cell::UnsafeCell;

/// Per-CPU data storage. Each CPU has its own copy of `T`.
///
/// # Thread Safety
///
/// `CpuLocal<T>` is `Sync` because:
/// - Each CPU only accesses its own slot (by `get_cpuid()` index)
/// - `lock()` requires `IrqGuard` which prevents re-entrant access on the same CPU
/// - Cross-CPU access to the same slot is prevented by design
pub struct CpuLocal<T> {
    data: [UnsafeCell<T>; MAX_NUM_CPUS],
}

// SAFETY: Each CPU accesses only its own slot via get_cpuid().
// The IrqGuard requirement on lock() prevents re-entrant same-CPU access.
unsafe impl<T> Sync for CpuLocal<T> {}

/// Guard providing exclusive `&mut` access to per-CPU data.
///
/// Holds an `IrqGuard` ensuring interrupts remain disabled for the
/// duration of the exclusive access.
pub struct CpuLocalGuard<'a, T> {
    data: &'a mut T,
    _irq_guard: IrqGuard,
}

impl<'a, T> core::ops::Deref for CpuLocalGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.data
    }
}

impl<'a, T> core::ops::DerefMut for CpuLocalGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data
    }
}

impl<T> CpuLocal<T> {
    /// Create a new `CpuLocal` with the given initial value for all CPUs.
    ///
    /// This requires `T: Copy` to initialize all CPU slots uniformly.
    pub fn new(value: T) -> Self
    where
        T: Copy,
    {
        let data = core::array::from_fn(|_| UnsafeCell::new(value));
        Self { data }
    }

    /// Get a shared reference to the current CPU's data.
    ///
    /// This is safe because `&T` allows multiple readers.
    /// The caller must ensure no `lock()` guard is currently held
    /// for this CPU (otherwise it would be UB).
    ///
    /// In practice, this is safe because the kernel currently runs
    /// without interrupts internally.
    pub fn get(&self) -> &T {
        let cpu_id = get_cpu().get_cpuid();
        // SAFETY: cpu_id is valid (0..MAX_NUM_CPUS), and we return &T
        // which is safe for shared reads. The caller must not hold a
        // mutable guard for this CPU.
        unsafe { &*self.data[cpu_id].get() }
    }

    /// Get exclusive access to the current CPU's data with interrupt guard.
    ///
    /// Returns a `CpuLocalGuard` that:
    /// - Holds `&mut T` for the current CPU
    /// - Holds `IrqGuard` proving interrupts are disabled
    ///
    /// # Safety Invariant
    ///
    /// The caller MUST NOT hold another `CpuLocalGuard` for the same `CpuLocal`
    /// on the same CPU. Doing so would create two `&mut T` to the same data.
    /// This is enforced by kernel code convention, not runtime checks.
    pub fn lock(&self) -> CpuLocalGuard<'_, T> {
        let cpu_id = get_cpu().get_cpuid();
        // SAFETY: cpu_id is valid (0..MAX_NUM_CPUS).
        // The IrqGuard ensures interrupts are disabled, preventing
        // re-entrant access from interrupt handlers on this CPU.
        // The caller is responsible for not calling lock() twice
        // while a guard is held (kernel code convention).
        let data = unsafe { &mut *self.data[cpu_id].get() };
        CpuLocalGuard {
            data,
            _irq_guard: IrqGuard::new(),
        }
    }

    /// Get the raw index for a CPU ID. Useful for debug/assertions.
    pub fn cpu_count() -> usize {
        MAX_NUM_CPUS
    }
}
