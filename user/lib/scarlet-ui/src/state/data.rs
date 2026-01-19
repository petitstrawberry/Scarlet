//! DataContext - Core reactive data storage
//!
//! DataContext<T> provides thread-safe reactive storage with change notification.
//! This is the foundation for all state management in ScarletUI.

extern crate alloc;
use alloc::sync::Arc;
use scarlet_std::sync::Mutex;

/// Data context for managing reactive state
///
/// DataContext<T> stores a value and notifies subscribers when it changes.
pub struct DataContext<T> {
    inner: Arc<Mutex<DataInner<T>>>,
}

struct DataInner<T> {
    data: T,
    version: u64,
}

impl<T> DataContext<T> {
    /// Create a new data context with an initial value
    pub fn new(data: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DataInner {
                data,
                version: 0,
            })),
        }
    }

    /// Get the current value (clones for value types)
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.inner.lock().data.clone()
    }

    /// Read the value with a closure (more efficient)
    pub fn read<U, F>(&self, f: F) -> U
    where
        F: FnOnce(&T) -> U,
    {
        let inner = self.inner.lock();
        f(&inner.data)
    }

    /// Set a new value
    pub fn set(&self, data: T)
    where
        T: PartialEq,
    {
        let mut inner = self.inner.lock();
        if inner.data != data {
            inner.data = data;
            inner.version = inner.version.wrapping_add(1);
        }
    }

    /// Modify the value
    pub fn modify<U, F>(&self, f: F) -> U
    where
        F: FnOnce(&mut T) -> U,
    {
        let mut inner = self.inner.lock();
        let result = f(&mut inner.data);
        inner.version = inner.version.wrapping_add(1);
        result
    }

    /// Get the current version
    pub fn version(&self) -> u64 {
        self.inner.lock().version
    }
}

impl<T: Clone> Clone for DataContext<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_context() {
        let ctx = DataContext::new(42);
        assert_eq!(ctx.get(), 42);
        assert_eq!(ctx.version(), 0);

        ctx.set(100);
        assert_eq!(ctx.get(), 100);
        assert_eq!(ctx.version(), 1);
    }

    #[test]
    fn test_data_context_modify() {
        let ctx = DataContext::new(42);
        ctx.modify(|v| *v += 1);
        assert_eq!(ctx.get(), 43);
        assert_eq!(ctx.version(), 1);
    }

    #[test]
    fn test_data_context_read() {
        let ctx = DataContext::new(42);
        let result = ctx.read(|v| *v * 2);
        assert_eq!(result, 84);
        // Version should not change on read
        assert_eq!(ctx.version(), 0);
    }
}
