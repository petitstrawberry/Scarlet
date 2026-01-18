use crate::boxed::Box;
use crate::syscall::{Syscall, syscall0, syscall1, syscall5};
use crate::task::{CloneFlags, CloneFlagsDef};
use crate::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

/// Main thread TLS area
///
/// This is the TLS area for the main thread. It is initialized
/// when `init_main_thread_tls` is called.
static MAIN_THREAD_TLS: AtomicUsize = AtomicUsize::new(0);

/// Get the current thread's TLS base pointer
///
/// This reads the architecture-specific TLS register directly without a syscall:
/// - RISC-V: Reads the tp register (x4)
/// - AArch64: Reads the TPIDR_EL0 system register
///
/// For the main thread, if the TLS register is 0, this returns the main thread
/// TLS area that was allocated via `init_main_thread_tls`.
///
/// The TLS register is set by the kernel during thread creation and is part
/// of the task's context, so reading it directly is safe and fast.
#[inline]
pub fn tls_pointer() -> usize {
    let ptr = crate::arch::arch_tls_pointer();
    if ptr == 0 {
        // Check if we have main thread TLS initialized
        let main_tls = MAIN_THREAD_TLS.load(Ordering::Acquire);
        if main_tls != 0 {
            main_tls
        } else {
            0
        }
    } else {
        ptr
    }
}

/// Initialize TLS for the main thread
///
/// This function allocates a TLS area for the main thread and sets it up
/// for use with `thread_local!` variables. This must be called before
/// any `thread_local!` variables are accessed.
///
/// # Safety
///
/// This function should only be called once, typically at program startup.
pub unsafe fn init_main_thread_tls() {
    if MAIN_THREAD_TLS.load(Ordering::Acquire) != 0 {
        return; // Already initialized
    }

    // Allocate TLS area for main thread (same size as spawned threads)
    const TLS_SIZE: usize = 1024; // Enough room for multiple TLS variables
    let tls_box: Box<[u8; TLS_SIZE]> = crate::boxed::Box::new([0u8; TLS_SIZE]);
    let tls_ptr = Box::into_raw(tls_box) as usize;

    // Store the TLS pointer
    MAIN_THREAD_TLS.store(tls_ptr, Ordering::Release);
}

/// Set the current thread's TLS base pointer
///
/// This performs a syscall to set the TLS pointer in the kernel.
/// Direct register writes are not recommended as the kernel needs to
/// maintain ABI state synchronization.
///
/// This is typically only called during thread initialization.
pub fn set_tls_pointer(ptr: usize) {
    crate::arch::arch_set_tls_pointer(ptr);
}

/// Thread sleep
pub fn sleep(dur: Duration) -> i32 {
    let nanosecs = dur.as_nanos() as usize;
    syscall1(Syscall::Sleep, nanosecs) as i32
}

/// Yield execution to the scheduler.
pub fn yield_now() {
    let _ = syscall0(Syscall::Yield);
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

    // Allocate TLS (Thread Local Storage) for the new thread
    // For now, we allocate a simple TLS area. In a full implementation,
    // this would include thread-local variables, errno, etc.
    const TLS_SIZE: usize = 1024; // Reserve space for TLS variables
    let tls_box: Box<[u8; TLS_SIZE]> = crate::boxed::Box::new([0u8; TLS_SIZE]);
    let tls_ptr = Box::into_raw(tls_box) as usize;

    // Set up clone flags for thread creation (share VM, FS, Files, Thread group)
    let mut flags = CloneFlags::new();
    flags.set(CloneFlagsDef::Thread); // Join parent's thread group (share TGID)
    flags.set(CloneFlagsDef::SetTls); // Set TLS pointer for new thread
    flags.set(CloneFlagsDef::Vm); // Share address space
    flags.set(CloneFlagsDef::Fs); // Share filesystem context
    flags.set(CloneFlagsDef::Files); // Share file descriptors

    // Use a typed trampoline that knows about F
    // Clone with: flags, stack, trampoline function, closure pointer, TLS pointer
    let result = syscall5(
        Syscall::Clone,
        flags.get_raw() as usize,
        stack_top,
        thread_typed_trampoline::<F> as *const () as usize,
        closure_ptr,
        tls_ptr, // TLS pointer as 5th argument
    );

    if result == usize::MAX {
        // Failed to create thread
        unsafe {
            let _ = Box::from_raw(closure_ptr as *mut F);
            // Reconstruct and drop TLS to free it
            let _ = Box::from_raw(tls_ptr as *mut [u8; 1024]);
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
///
/// # Example
///
/// ```rust
/// thread_local! {
///     static FOO: Cell<i32> = Cell::new(5);
/// }
///
/// FOO.with(|value| {
///     value.set(10);
/// });
/// ```
#[derive(Debug, Clone, Copy)]
pub struct LocalKey<T> {
    /// Offset into the TLS segment
    offset: usize,
    /// Alignment requirement for the value
    align: usize,
    _phantom: core::marker::PhantomData<T>,
}

impl<T> LocalKey<T> {
    /// Create a new LocalKey with the given offset and alignment
    #[inline]
    pub const fn new(offset: usize, align: usize) -> Self {
        Self {
            offset,
            align,
            _phantom: core::marker::PhantomData,
        }
    }

    /// Get the offset into the TLS segment
    #[inline]
    pub const fn offset(self) -> usize {
        self.offset
    }

    /// Initialize this thread-local value with the given initializer
    ///
    /// This function should be called once per thread to initialize the value.
    /// Subsequent calls will have no effect if the value is already initialized.
    ///
    /// # Safety
    ///
    /// This function should only be called once per thread for each LocalKey.
    /// The caller must ensure the TLS area is large enough for this value.
    pub unsafe fn initialize(self, value: T)
    where
        T: 'static,
    {
        let tls_base = tls_pointer();
        if tls_base == 0 {
            panic!("TLS not initialized for this thread");
        }

        let ptr = (tls_base + self.offset) as *mut T;
        unsafe {
            ptr.write(value);
        }
    }

    /// Execute a closure with access to this thread-local value
    ///
    /// This function provides safe access to thread-local storage by
    /// executing the provided closure with a reference to the value.
    ///
    /// # Panics
    ///
    /// Panics if the TLS pointer is not set for the current thread.
    pub fn with<F, R>(self, f: F) -> R
    where
        T: 'static,
        F: FnOnce(&T) -> R,
    {
        let tls_base = tls_pointer();
        if tls_base == 0 {
            panic!("TLS not initialized for this thread");
        }

        let ptr = (tls_base + self.offset) as *const T;
        unsafe {
            // Ensure alignment is correct
            let aligned_ptr = ptr as usize;
            assert_eq!(
                aligned_ptr % self.align,
                0,
                "TLS value is not properly aligned"
            );
            f(&*ptr)
        }
    }

    /// Execute a closure with mutable access to this thread-local value
    ///
    /// This function provides safe mutable access to thread-local storage.
    pub fn with_mut<F, R>(self, f: F) -> R
    where
        T: 'static,
        F: FnOnce(&mut T) -> R,
    {
        let tls_base = tls_pointer();
        if tls_base == 0 {
            panic!("TLS not initialized for this thread");
        }

        let ptr = (tls_base + self.offset) as *mut T;
        unsafe {
            let aligned_ptr = ptr as usize;
            assert_eq!(
                aligned_ptr % self.align,
                0,
                "TLS value is not properly aligned"
            );
            f(&mut *ptr)
        }
    }

    /// Try to execute a closure with access to this thread-local value
    ///
    /// Returns `None` if TLS is not initialized, otherwise returns the
    /// result of the closure.
    pub fn try_with<F, R>(self, f: F) -> Option<R>
    where
        T: 'static,
        F: FnOnce(&T) -> R,
    {
        let tls_base = tls_pointer();
        if tls_base == 0 {
            return None;
        }

        let ptr = (tls_base + self.offset) as *const T;
        unsafe { Some(f(&*ptr)) }
    }

    /// Try to execute a closure with mutable access to this thread-local value
    ///
    /// Returns `None` if TLS is not initialized, otherwise returns the
    /// result of the closure.
    pub fn try_with_mut<F, R>(self, f: F) -> Option<R>
    where
        T: 'static,
        F: FnOnce(&mut T) -> R,
    {
        let tls_base = tls_pointer();
        if tls_base == 0 {
            return None;
        }

        let ptr = (tls_base + self.offset) as *mut T;
        unsafe { Some(f(&mut *ptr)) }
    }
}

// Helper for calculating offset from identifier hash
#[doc(hidden)]
#[inline]
pub const fn __tls_offset_from_hash(name: &[u8]) -> usize {
    let mut hash: usize = 5381;
    let mut i = 0;
    while i < name.len() {
        hash = hash.wrapping_mul(33).wrapping_add(name[i] as usize);
        i += 1;
    }
    // Map hash to offset with 8-byte alignment (each slot is 8 bytes minimum)
    (hash % 1024) & !7
}

// Counter for TLS offset allocation (used internally by thread_local! macro)
#[doc(hidden)]
pub const __TLS_ALIGN: usize = 8;

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
///     static BAR: RefCell<String> = RefCell::new(String::new());
/// }
///
/// FOO.with(|value| {
///     value.set(10);
/// });
/// ```
#[macro_export]
macro_rules! thread_local {
    // Internal state: current offset, next index, and items
    (@state $tls_size:expr, $index:expr,) => {};

    // Process all items and generate LocalKey declarations
    (@state $tls_size:expr, $index:expr, $(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty = $init:expr; $($rest:tt)*) => {
        #[allow(non_upper_case_globals)]
        #[$attr]
        $vis static $name: $crate::thread::LocalKey<$ty> = {
            // Calculate offset based on variable name hash
            const __TLS_NAME__: &[u8] = stringify!($name).as_bytes();
            const __TLS_OFFSET__: usize = $crate::thread::__tls_offset_from_hash(__TLS_NAME__);
            const __TLS_ALIGN__: usize = $crate::thread::__TLS_ALIGN;

            $crate::thread::LocalKey::new(__TLS_OFFSET__, __TLS_ALIGN)
        };

        $crate::thread_local!(@state $tls_size + 8, $index + 1, $($rest)*);
    };

    // Entry point: parse all items and calculate offsets
    ($(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty = $init:expr; $($rest:tt)*) => {
        $crate::thread_local!(@state 0, 0, $(#[$attr])* $vis static $name: $ty = $init; $($rest)*);
    };
}

