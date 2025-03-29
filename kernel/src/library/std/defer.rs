//! # Defer Module
//!
//! This module provides a mechanism to execute a function when the current scope is exited,
//! similar to Go's `defer` statement or C++'s RAII pattern.
//!
//! The defer mechanism ensures that cleanup code is executed regardless of how a scope is exited,
//! whether through normal control flow, early returns, or error propagation.
//!
//! ## Usage
//!
//! The module provides two ways to defer execution:
//!
//! 1. The `defer` function, which takes a closure and returns an object that executes 
//!    the closure when dropped:
//!
//! ```
//! let mut resource = acquire_resource();
//! let _defer = defer(|| {
//!     release_resource(&mut resource);
//! });
//!
//! // Work with resource...
//! // When scope ends, the defer will automatically release the resource
//! ```
//!
//! 2. The `defer!` macro, which offers a more concise syntax:
//!
//! ```
//! let mut resource = acquire_resource();
//! defer! {
//!     release_resource(&mut resource);
//! }
//!
//! // Work with resource...
//! // Resource will be released when scope ends
//! ```
//!
//! ## Features
//!
//! - Executes deferred functions in reverse order of declaration (LIFO)
//! - Works with early returns and panics
//! - Helps prevent resource leaks
//! - Simplifies cleanup logic
//!
//! ## Implementation Details
//!
//! The implementation uses Rust's RAII (Resource Acquisition Is Initialization) pattern
//! through the `Drop` trait to ensure the deferred function is called when the returned
//! object goes out of scope.
//! Defer module.
//! 
//! This module provides a mechanism to execute a function when the current scope is exited.

/// Defer a function to be executed when the current scope is exited.
/// This function takes a closure and returns an object that will execute the closure
/// when it is dropped.
/// This is useful for cleanup tasks, such as releasing resources or performing
/// finalization steps.
///
/// # Examples
/// ```
/// let mut resource = acquire_resource();
/// let _defer = defer(|| {
///     release_resource(&mut resource);
/// });
///
#[must_use]
#[inline]
pub fn defer<F>(f: F) -> impl Drop
where 
    F: FnOnce(),
{
    struct Defer<F: FnOnce()> {
        f: Option<F>,
    }

    impl<F: FnOnce()> Defer<F> {
        fn new(f: F) -> Self {
            Defer { f: Some(f) }
        }
    }
    
    impl<F: FnOnce()> Drop for Defer<F> {
        fn drop(&mut self) {
            if let Some(f) = self.f.take() {
                f();
            }
        }
    }
    
    Defer::new(f)
}

/// Macro to defer execution of a block of code.
/// This macro allows you to specify a block of code that will be executed when the
/// current scope is exited.
/// It is similar to the `defer` function but provides a more concise syntax.
///
#[macro_export]
macro_rules! defer {
    ($($data: tt)*) => (
        let _defer = $crate::library::std::defer::defer(|| {
            $($data)*
        });
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_simple_defer() {
        let mut called = false;
        {
            let _defer = defer(|| {
                called = true;
            });
        }
        assert!(called, "Defer function was not called");
    }

    #[test_case]
    fn test_multiple_defer() {
        let mut called1 = false;
        let mut called2 = false;
        {
            let _defer1 = defer(|| {
                called1 = true;
            });
            let _defer2 = defer(|| {
                called2 = true;
            });
        }
        assert!(called1, "First defer function was not called");
        assert!(called2, "Second defer function was not called");
    }

    #[test_case]
    fn test_defer_with_return() {
        let mut called = false;
        {
            let _defer = defer(|| {
                called = true;
            });
            return; // Early return
        }
        assert!(called, "Defer function was not called");
    }

    #[test_case]
    fn test_defer_with_error() {
        fn might_fail(called: &mut bool) -> Result<(), &'static str> {
            *called = false;

            let _defer = defer(|| {
                *called = true;
            });

            Err("Error occurred")
        }

        let mut called = false;
        let result = might_fail(&mut called);
        assert!(result.is_err(), "Expected an error");
        assert!(called, "Defer function was not called");
    }

    fn test_defer_macro() {
        let mut called = false;
        {
            defer!{
                called = true;
            }
        }
        assert!(called, "Defer macro function was not called");
    }
}