//! Interrupt guard for SMP-safe per-CPU data access.

pub struct IrqGuard {
    saved_state: usize,
    _not_send: core::marker::PhantomData<*mut ()>,
}

impl IrqGuard {
    #[inline]
    pub fn new() -> Self {
        let saved_state = crate::arch::interrupt::save_and_disable_interrupts();
        Self {
            saved_state,
            _not_send: core::marker::PhantomData,
        }
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        crate::arch::interrupt::restore_interrupts(self.saved_state);
    }
}
