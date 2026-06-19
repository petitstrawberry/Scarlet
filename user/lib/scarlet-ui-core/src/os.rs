//! OS compatibility layer for ScarletUI.

#[cfg(feature = "std")]
pub use std::fs::File;
#[cfg(feature = "std")]
pub use std::io::Read;

#[cfg(feature = "std")]
pub struct Mutex<T> {
    inner: std::sync::Mutex<T>,
}

#[cfg(feature = "std")]
impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: std::sync::Mutex::new(value),
        }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.inner.lock().expect("scarlet-ui mutex poisoned")
    }
}

#[cfg(not(feature = "std"))]
pub use scarlet_std::fs::File;
#[cfg(not(feature = "std"))]
pub use scarlet_std::sync::Mutex;
