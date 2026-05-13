//! Interrupt guard for SMP-safe per-CPU data access.

pub struct IrqGuard {
    was_enabled: bool,
    _not_send: core::marker::PhantomData<*mut ()>,
}

impl IrqGuard {
    #[inline]
    pub fn new() -> Self {
        let was_enabled = crate::arch::interrupt::are_interrupts_enabled();
        if was_enabled {
            crate::arch::interrupt::disable_interrupts();
        }
        Self {
            was_enabled,
            _not_send: core::marker::PhantomData,
        }
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        if self.was_enabled {
            crate::arch::interrupt::enable_interrupts();
        }
    }
}
