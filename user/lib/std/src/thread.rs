use crate::boxed::Box;
use crate::syscall::{Syscall, syscall1, syscall4};
use crate::task::{CloneFlags, CloneFlagsDef};
use crate::vec::Vec;
use core::time::Duration;

/// Get the current thread's TLS base pointer
pub fn tls_pointer() -> usize {
    crate::syscall::syscall1(Syscall::GetTls, 0)
}

/// Set the current thread's TLS base pointer
pub fn set_tls_pointer(ptr: usize) {
    crate::syscall::syscall1(Syscall::SetTls, ptr);
}

/// Thread sleep
pub fn sleep(dur: Duration) -> i32 {
    let nanosecs = dur.as_nanos() as usize;
    syscall1(Syscall::Sleep, nanosecs) as i32
}

/// Thread builder (simplified)
pub struct Builder {
    name: Option<&'static str>,
}

impl Builder {
    pub fn new() -> Self {
        Builder { name: None }
    }

    pub fn name(mut self, name: &'static str) -> Self {
        self.name = Some(name);
        self
    }

    pub fn spawn<F>(self, f: F) -> Result<JoinHandle, &'static str>
    where
        F: FnOnce() + Send + 'static,
    {
        spawn_impl(f, self.name)
    }
}

/// Spawn a new thread
pub fn spawn<F>(f: F) -> JoinHandle
where
    F: FnOnce() + Send + 'static,
{
    spawn_impl(f, None).expect("Failed to spawn thread")
}

/// Join handle for waiting on thread completion
pub struct JoinHandle {
    thread_id: u32,
}

impl JoinHandle {
    /// Wait for the thread to finish
    pub fn join(self) -> Result<(), &'static str> {
        // Wait for child thread to exit
        let (pid, _status) = crate::task::waitpid(self.thread_id as i32, 0);
        if pid < 0 {
            Err("Failed to join thread")
        } else {
            Ok(())
        }
    }
}

// Internal implementation
fn spawn_impl<F>(f: F, _name: Option<&'static str>) -> Result<JoinHandle, &'static str>
where
    F: FnOnce() + Send + 'static,
{
    // Keep the allocation type consistent with the typed trampoline.
    // Using a trait object here and reconstructing it as `Box<F>` is UB.
    let closure: Box<F> = Box::new(f);
    let closure_ptr = Box::into_raw(closure) as usize;

    // Allocate thread stack (256KB - reasonable size for embedded system).
    // Keep this as `u8` so we don't require high-alignment allocations from the allocator.
    const STACK_SIZE: usize = 256 * 1024;
    const STACK_ALIGN: usize = 16;
    let stack: Vec<u8> = crate::vec![0u8; STACK_SIZE];
    let stack_base = stack.as_ptr() as usize;
    let stack_end = stack_base + STACK_SIZE;

    // Stack grows downward, so set SP to top of stack minus 16 bytes.
    // Keep it 16-byte aligned (AArch64/RISC-V ABI friendly).
    let stack_top = (stack_end - 16) & !(STACK_ALIGN - 1);

    // Leak stack (child thread will use it, freed on thread exit)
    core::mem::forget(stack);

    // Set up clone flags for thread creation (share VM, FS, Files)
    let mut flags = CloneFlags::new();
    flags.set(CloneFlagsDef::Vm);
    flags.set(CloneFlagsDef::Fs);
    flags.set(CloneFlagsDef::Files);

    // Use a typed trampoline that knows about F
    // Clone with: flags, stack, trampoline function, closure pointer
    let result = syscall4(
        Syscall::Clone,
        flags.get_raw() as usize,
        stack_top,
        thread_typed_trampoline::<F> as *const () as usize,
        closure_ptr,
    );

    if result == usize::MAX {
        // Failed to create thread
        unsafe {
            let _ = Box::from_raw(closure_ptr as *mut F);
            // TODO: free stack
        }
        return Err("Failed to create thread");
    }

    // Parent: return join handle
    Ok(JoinHandle {
        thread_id: result as u32,
    })
}

/// Typed thread trampoline - knows the closure type F
#[inline(never)]
extern "C" fn thread_typed_trampoline<F>(closure_ptr: usize) -> !
where
    F: FnOnce() + Send + 'static,
{
    // Reconstruct the Box<F> from the pointer
    unsafe {
        let closure = Box::from_raw(closure_ptr as *mut F);
        closure();
    }

    // Exit the thread
    crate::task::exit(0);
}

/// Thread Local Storage key
///
/// This type represents a key for thread-local storage. Each key
/// can be associated with a value that is unique to each thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalKey {
    /// Offset into the TLS segment
    offset: usize,
}

impl LocalKey {
    /// Create a new LocalKey with the given offset
    #[inline]
    pub const fn new(offset: usize) -> Self {
        Self { offset }
    }

    /// Get the offset into the TLS segment
    #[inline]
    pub const fn offset(self) -> usize {
        self.offset
    }
}

/// Thread local storage variable
///
/// This macro defines a thread-local variable with the given type and
/// initializer. The variable is unique to each thread and is initialized
/// lazily on first access.
///
/// # Example
///
/// ```rust
/// thread_local! {
///     static FOO: Cell<i32> = Cell::new(5);
///     static BAR: &str = "hello";
/// }
/// ```
#[macro_export]
macro_rules! thread_local {
    // Static with initialization expression
    (@$cnt:vis $name:ident: $ty:ty = $init:expr) => {
        #[inline]
        fn $name() -> $ty {
            // Get the TLS base pointer
            let tls_base = $crate::thread::tls_pointer();

            // Calculate the address of this thread-local variable
            // For now, we use a simple scheme where each TLS variable gets a fixed offset
            // In a real implementation, you'd have a more sophisticated TLS layout
            const OFFSET: usize = 0; // Placeholder - would be calculated properly

            unsafe {
                let ptr = (tls_base + OFFSET) as *mut $ty;

                // Initialize if not already initialized
                // This is a simplified version - real TLS needs more sophisticated logic
                if (*ptr as *const u8 as usize) == 0 {
                    *ptr = $init;
                }

                &*ptr
            }
        }
    };

    // Multiple items
    ($(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty = $init:expr; $($rest:tt)*) => {
        $crate::thread_local! {
            @#[$attr] $name: $ty = $init
        }
        $crate::thread_local!($($rest)*);
    };

    // Single item
    ($(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty = $init:expr) => {
        $crate::thread_local! {
            @#[$attr] $name: $ty = $init
        }
    };
}

// Internal macro for calculating TLS offsets
#[doc(hidden)]
#[macro_export]
macro_rules! __tls_offset {
    ($name:ident) => {{
        // Use a simple hash of the identifier to generate an offset
        // In a real implementation, this would be done at link time
        const OFFSET: usize = 0; // Placeholder - would be calculated properly
        OFFSET
    }};
}

// Provide a simpler implementation using sys_set_tls
// This is a basic version that works with the kernel's TLS support

/// Basic thread-local storage implementation
///
/// This is a simplified TLS implementation that works with Scarlet's
/// kernel TLS support. For each thread, the kernel maintains a TLS pointer
/// that can be set via the SetTls syscall.
pub mod local {
    use super::tls_pointer;

    /// A thread-local variable using the kernel's TLS support
    ///
    /// This is a simplified implementation that stores a single pointer
    /// in the kernel's TLS area. For more complex TLS needs, use the
    /// thread_local! macro instead.
    #[repr(transparent)]
    pub struct LocalVariable<T> {
        _phantom: core::marker::PhantomData<T>,
    }

    impl<T: 'static> LocalVariable<T> {
        /// Create a new thread-local variable
        pub const fn new() -> Self {
            Self {
                _phantom: core::marker::PhantomData,
            }
        }

        /// Get the value of this thread-local variable
        ///
        /// This implementation uses a simple approach where the TLS pointer
        /// points directly to the value. For production use, you'd want
        /// a more sophisticated TLS layout.
        #[inline]
        pub fn get(&self) -> Option<T>
        where
            T: Copy,
        {
            let tls_ptr = tls_pointer();
            if tls_ptr == 0 {
                None
            } else {
                unsafe { Some(*(tls_ptr as *const T)) }
            }
        }

        /// Set the value of this thread-local variable
        #[inline]
        pub fn set(&self, value: T)
        where
            T: Copy,
        {
            let tls_ptr = tls_pointer();
            if tls_ptr != 0 {
                unsafe {
                    *(tls_ptr as *mut T) = value;
                }
            }
        }
    }

    impl<T: 'static> Default for LocalVariable<T> {
        fn default() -> Self {
            Self::new()
        }
    }

    unsafe impl<T: Send> Send for LocalVariable<T> {}
    unsafe impl<T: Sync> Sync for LocalVariable<T> {}
}
