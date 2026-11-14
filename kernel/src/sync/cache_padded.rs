//! Separate independently written per-CPU state across cache lines.

/// 128 bytes also covers AArch64 hosts with 128-byte data cache lines. Array
/// elements remain disjoint without changing the atomics' ordering semantics.
#[repr(align(128))]
pub(crate) struct CachePadded<T>(T);

impl<T> CachePadded<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self(value)
    }
}

impl<T> core::ops::Deref for CachePadded<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}
