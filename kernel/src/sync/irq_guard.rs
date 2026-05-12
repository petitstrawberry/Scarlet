//! Interrupt guard for SMP-safe per-CPU data access.
//!
//! `IrqGuard` serves as a proof that local interrupts are disabled.
//! Currently a **stub** — the kernel does not handle interrupts internally yet,
//! so this is a no-op. When interrupt support is added, this will actually
//! disable/restore local interrupts using architecture-specific instructions.
//!
//! # Usage
//!
//! Wrap per-CPU data mutations in `IrqGuard` to ensure no interrupt
//! (and thus no re-entrant schedule) can occur while the data is being modified:
//!
//! ```ignore
//! let guard = IrqGuard::new();
//! let local_data = cpu_local.lock(&guard);
//! // ... mutate data ...
//! // guard dropped here, interrupts would be restored
//! ```

/// A guard that represents a period where local interrupts are disabled.
///
/// Currently a no-op stub. When SMP interrupt support is implemented,
/// `new()` will disable local interrupts and `drop()` will restore them.
///
/// This type is !Send — it must not be moved across CPUs.
pub struct IrqGuard {
    /// Whether interrupts were enabled before we disabled them.
    /// Used to restore the previous state on drop.
    _was_enabled: bool,
    // Mark as !Send and !Sync
    _not_send: core::marker::PhantomData<*mut ()>,
}

impl IrqGuard {
    /// Create a new IrqGuard, disabling local interrupts.
    ///
    /// # Safety (stub)
    /// Currently a no-op. When implemented, this will:
    /// - Read the current interrupt enable state
    /// - Disable local interrupts
    /// - Store the previous state for restoration on drop
    #[inline]
    pub fn new() -> Self {
        // TODO: Actually disable interrupts when SMP is ready
        // For now, this is a no-op stub
        Self {
            _was_enabled: false,
            _not_send: core::marker::PhantomData,
        }
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        // TODO: Restore interrupt state when SMP is ready
        // if self._was_enabled {
        //     enable_local_irq();
        // }
    }
}
