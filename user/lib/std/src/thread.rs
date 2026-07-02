use crate::boxed::Box;
use crate::handle::capability::memory_mapping::{
    flags as mmap_flags, mmap_anonymous, munmap, prot,
};
use crate::syscall::{Syscall, syscall0, syscall1, syscall5};
use crate::task::{CloneFlags, CloneFlagsDef, SCHED_UTIL_SCALE};
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

/// Main thread TLS area
///
/// This is the TLS area for the main thread. It is initialized
/// when `init_main_thread_tls` is called.
static MAIN_THREAD_TLS: AtomicUsize = AtomicUsize::new(0);

const PAGE_SIZE: usize = 4096;
const STACK_SIZE: usize = 256 * 1024;
const STACK_ALIGN: usize = 16;
const TLS_MAPPING_SIZE: usize = PAGE_SIZE;
const TLS_SIZE: usize = TLS_MAPPING_SIZE;
const TLS_CLEANUP_OFFSET: usize = 2048;
const THREAD_CLEANUP_MAGIC: usize = 0x5343_5448_5244_0001;

#[repr(C)]
#[derive(Clone, Copy)]
struct ThreadCleanupRecord {
    magic: usize,
    stack_mapping_base: usize,
    stack_mapping_len: usize,
    tls_mapping_base: usize,
    tls_mapping_len: usize,
}

struct ThreadStart<F>
where
    F: FnOnce() + Send + 'static,
{
    closure: Option<F>,
    stack_mapping_base: usize,
    stack_mapping_len: usize,
    tls_mapping_base: usize,
    tls_mapping_len: usize,
}

struct ThreadStackMapping {
    mapping_base: usize,
    mapping_len: usize,
    stack_base: usize,
    stack_len: usize,
}

fn allocate_thread_stack() -> Result<ThreadStackMapping, &'static str> {
    let mapping_len = STACK_SIZE + PAGE_SIZE;
    let mapping_base = mmap_anonymous(0, mapping_len, prot::NONE, mmap_flags::PRIVATE)
        .map_err(|_| "Failed to allocate thread stack guard")?;
    let stack_base = mapping_base + PAGE_SIZE;

    if mmap_anonymous(
        stack_base,
        STACK_SIZE,
        prot::READ | prot::WRITE,
        mmap_flags::PRIVATE | mmap_flags::FIXED,
    )
    .is_err()
    {
        let _ = munmap(mapping_base, mapping_len);
        return Err("Failed to allocate thread stack");
    }

    Ok(ThreadStackMapping {
        mapping_base,
        mapping_len,
        stack_base,
        stack_len: STACK_SIZE,
    })
}

fn allocate_thread_tls() -> Result<usize, &'static str> {
    mmap_anonymous(
        0,
        TLS_MAPPING_SIZE,
        prot::READ | prot::WRITE,
        mmap_flags::PRIVATE,
    )
    .map_err(|_| "Failed to allocate thread TLS")
}

fn cleanup_thread_mappings(
    stack_mapping_base: usize,
    stack_mapping_len: usize,
    tls_mapping_base: usize,
    tls_mapping_len: usize,
) {
    let _ = munmap(stack_mapping_base, stack_mapping_len);
    let _ = munmap(tls_mapping_base, tls_mapping_len);
}

fn write_thread_cleanup_record(
    tls_mapping_base: usize,
    stack_mapping_base: usize,
    stack_mapping_len: usize,
) {
    let record = ThreadCleanupRecord {
        magic: THREAD_CLEANUP_MAGIC,
        stack_mapping_base,
        stack_mapping_len,
        tls_mapping_base,
        tls_mapping_len: TLS_MAPPING_SIZE,
    };
    unsafe {
        let ptr = (tls_mapping_base + TLS_CLEANUP_OFFSET) as *mut ThreadCleanupRecord;
        ptr.write(record);
    }
}

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
        if main_tls != 0 { main_tls } else { 0 }
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

    // Allocate TLS area for main thread (same usable size as spawned threads)
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
    util_min: Option<u32>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        Builder {
            name: None,
            util_min: None,
        }
    }

    pub fn name(mut self, name: &'static str) -> Self {
        self.name = Some(name);
        self
    }

    /// Set the minimum scheduler utilization inherited by the new thread.
    ///
    /// # Arguments
    ///
    /// * `util_min` - Minimum utilization in scheduler capacity units.
    pub fn util_min(mut self, util_min: u32) -> Self {
        self.util_min = Some(util_min);
        self
    }

    /// Request a full-capacity CPU for the new thread.
    pub fn performance(mut self) -> Self {
        self.util_min = Some(SCHED_UTIL_SCALE);
        self
    }

    pub fn spawn<F>(self, f: F) -> Result<JoinHandle, &'static str>
    where
        F: FnOnce() + Send + 'static,
    {
        spawn_impl(f, self.name, self.util_min)
    }
}

/// Spawn a new thread
pub fn spawn<F>(f: F) -> JoinHandle
where
    F: FnOnce() + Send + 'static,
{
    spawn_impl(f, None, None).expect("Failed to spawn thread")
}

/// Join handle for waiting on thread completion
pub struct JoinHandle {
    thread_id: Option<u32>,
}

impl JoinHandle {
    /// Wait for the thread to finish.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the thread was reaped successfully, otherwise an error.
    pub fn join(mut self) -> Result<(), &'static str> {
        let Some(thread_id) = self.thread_id.take() else {
            return Err("Thread handle is detached");
        };

        // Wait for child thread to exit
        let (pid, _status) = crate::task::waitpid(thread_id as i32, 0);
        if pid < 0 {
            Err("Failed to join thread")
        } else {
            Ok(())
        }
    }
}

impl Drop for JoinHandle {
    fn drop(&mut self) {
        if let Some(thread_id) = self.thread_id.take() {
            let _ = syscall1(Syscall::ThreadDetach, thread_id as usize);
        }
    }
}

// Internal implementation
fn spawn_impl<F>(
    f: F,
    _name: Option<&'static str>,
    util_min: Option<u32>,
) -> Result<JoinHandle, &'static str>
where
    F: FnOnce() + Send + 'static,
{
    // Allocate the thread stack like libc does: as an anonymous mapping owned by
    // the threading runtime, not by the general heap allocator.
    let stack = allocate_thread_stack()?;
    let stack_end = stack.stack_base + stack.stack_len;

    // Stack grows downward, so set SP to top of stack minus 16 bytes.
    // Keep it 16-byte aligned (AArch64/RISC-V ABI friendly).
    let stack_top = (stack_end - 16) & !(STACK_ALIGN - 1);

    let tls_ptr = match allocate_thread_tls() {
        Ok(ptr) => ptr,
        Err(e) => {
            let _ = munmap(stack.mapping_base, stack.mapping_len);
            return Err(e);
        }
    };
    write_thread_cleanup_record(tls_ptr, stack.mapping_base, stack.mapping_len);

    let start: Box<ThreadStart<F>> = Box::new(ThreadStart {
        closure: Some(f),
        stack_mapping_base: stack.mapping_base,
        stack_mapping_len: stack.mapping_len,
        tls_mapping_base: tls_ptr,
        tls_mapping_len: TLS_MAPPING_SIZE,
    });
    let start_ptr = Box::into_raw(start) as usize;

    // Set up clone flags for thread creation (share VM, FS, Files, Thread group)
    let mut flags = CloneFlags::new();
    flags.set(CloneFlagsDef::Thread); // Join parent's thread group (share TGID)
    flags.set(CloneFlagsDef::SetTls); // Set TLS pointer for new thread
    flags.set(CloneFlagsDef::Vm); // Share address space
    flags.set(CloneFlagsDef::Fs); // Share filesystem context
    flags.set(CloneFlagsDef::Files); // Share file descriptors

    let previous_util_min = if let Some(util_min) = util_min {
        let previous = match crate::task::sched_util_min() {
            Ok(value) => value,
            Err(_) => {
                unsafe {
                    let start = Box::from_raw(start_ptr as *mut ThreadStart<F>);
                    cleanup_thread_mappings(
                        start.stack_mapping_base,
                        start.stack_mapping_len,
                        start.tls_mapping_base,
                        start.tls_mapping_len,
                    );
                }
                return Err("Failed to read scheduler hint");
            }
        };
        if crate::task::set_sched_util_min(util_min).is_err() {
            unsafe {
                let start = Box::from_raw(start_ptr as *mut ThreadStart<F>);
                cleanup_thread_mappings(
                    start.stack_mapping_base,
                    start.stack_mapping_len,
                    start.tls_mapping_base,
                    start.tls_mapping_len,
                );
            }
            return Err("Invalid scheduler utilization hint");
        }
        Some(previous)
    } else {
        None
    };

    // Use a typed trampoline that knows about F.
    // Clone with: flags, stack, trampoline function, start packet, TLS pointer.
    let result = syscall5(
        Syscall::Clone,
        flags.get_raw() as usize,
        stack_top,
        thread_typed_trampoline::<F> as *const () as usize,
        start_ptr,
        tls_ptr, // TLS pointer as 5th argument
    );

    if let Some(previous) = previous_util_min {
        let _ = crate::task::set_sched_util_min(previous);
    }

    if result == usize::MAX {
        // Failed to create thread
        unsafe {
            let start = Box::from_raw(start_ptr as *mut ThreadStart<F>);
            cleanup_thread_mappings(
                start.stack_mapping_base,
                start.stack_mapping_len,
                start.tls_mapping_base,
                start.tls_mapping_len,
            );
        }
        return Err("Failed to create thread");
    }

    // Parent: return join handle
    Ok(JoinHandle {
        thread_id: Some(result as u32),
    })
}

/// Typed thread trampoline - knows the closure type F
#[inline(never)]
extern "C" fn thread_typed_trampoline<F>(start_ptr: usize) -> !
where
    F: FnOnce() + Send + 'static,
{
    let mut start = unsafe { Box::from_raw(start_ptr as *mut ThreadStart<F>) };
    let Some(closure) = start.closure.take() else {
        drop(start);
        exit_current_thread(-1);
    };

    closure();
    drop(start);

    exit_current_thread(0);
}

fn exit_thread_with_cleanup(
    code: i32,
    stack_mapping_base: usize,
    stack_mapping_len: usize,
    tls_mapping_base: usize,
    tls_mapping_len: usize,
) -> ! {
    syscall5(
        Syscall::ThreadExitCleanup,
        code as usize,
        stack_mapping_base,
        stack_mapping_len,
        tls_mapping_base,
        tls_mapping_len,
    );
    unreachable!("thread cleanup exit syscall should not return");
}

pub(crate) fn exit_current_thread(code: i32) -> ! {
    let tls_base = tls_pointer();
    if tls_base != 0 {
        let record = unsafe {
            let ptr = (tls_base + TLS_CLEANUP_OFFSET) as *const ThreadCleanupRecord;
            ptr.read()
        };
        if record.magic == THREAD_CLEANUP_MAGIC {
            exit_thread_with_cleanup(
                code,
                record.stack_mapping_base,
                record.stack_mapping_len,
                record.tls_mapping_base,
                record.tls_mapping_len,
            );
        }
    }

    syscall1(Syscall::Exit, code as usize);
    unreachable!("exit_thread syscall should not return");
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

// SAFETY: `LocalKey<T>` is `Sync` for all `T` because accessing thread-local
// storage through `LocalKey` is thread-safe: each thread has its own copy of
// the TLS value, and `LocalKey` is just a handle to access that storage.
// The `offset` and `align` fields are constant and can be safely shared.
unsafe impl<T> Sync for LocalKey<T> {}

// SAFETY: `LocalKey<T>` is `Send` for all `T` because it only contains constant
// data (`offset` and `align`) and can be safely moved between threads.
unsafe impl<T> Send for LocalKey<T> {}

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
    pub fn with<F, R>(&self, f: F) -> R
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
    pub fn with_mut<F, R>(&self, f: F) -> R
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
    pub fn try_with<F, R>(&self, f: F) -> Option<R>
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
    pub fn try_with_mut<F, R>(&self, f: F) -> Option<R>
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
    // Entry point - with attributes, single item
    ($(#[$attr:meta])+ $vis:vis static $name:ident: $ty:ty = $init:expr;) => {
        #[allow(non_upper_case_globals)]
        #[$(#[$attr])*]
        $vis static $name: $crate::thread::LocalKey<$ty> = {
            const __TLS_NAME__: &[u8] = stringify!($name).as_bytes();
            const __TLS_OFFSET__: usize = $crate::thread::__tls_offset_from_hash(__TLS_NAME__);
            const __TLS_ALIGN__: usize = $crate::thread::__TLS_ALIGN;
            $crate::thread::LocalKey::new(__TLS_OFFSET__, __TLS_ALIGN__)
        };
    };

    // Entry point - without attributes, single item
    ($vis:vis static $name:ident: $ty:ty = $init:expr;) => {
        #[allow(non_upper_case_globals)]
        $vis static $name: $crate::thread::LocalKey<$ty> = {
            const __TLS_NAME__: &[u8] = stringify!($name).as_bytes();
            const __TLS_OFFSET__: usize = $crate::thread::__tls_offset_from_hash(__TLS_NAME__);
            const __TLS_ALIGN__: usize = $crate::thread::__TLS_ALIGN;
            $crate::thread::LocalKey::new(__TLS_OFFSET__, __TLS_ALIGN__)
        };
    };

    // Entry point - with attributes, multiple items
    ($(#[$attr:meta])+ $vis:vis static $name:ident: $ty:ty = $init:expr; $($rest:tt)*) => {
        #[allow(non_upper_case_globals)]
        #[$(#[$attr])*]
        $vis static $name: $crate::thread::LocalKey<$ty> = {
            const __TLS_NAME__: &[u8] = stringify!($name).as_bytes();
            const __TLS_OFFSET__: usize = $crate::thread::__tls_offset_from_hash(__TLS_NAME__);
            const __TLS_ALIGN__: usize = $crate::thread::__TLS_ALIGN;
            $crate::thread::LocalKey::new(__TLS_OFFSET__, __TLS_ALIGN__)
        };
        $crate::thread_local!($($rest)*);
    };

    // Entry point - without attributes, multiple items
    ($vis:vis static $name:ident: $ty:ty = $init:expr; $($rest:tt)*) => {
        #[allow(non_upper_case_globals)]
        $vis static $name: $crate::thread::LocalKey<$ty> = {
            const __TLS_NAME__: &[u8] = stringify!($name).as_bytes();
            const __TLS_OFFSET__: usize = $crate::thread::__tls_offset_from_hash(__TLS_NAME__);
            const __TLS_ALIGN__: usize = $crate::thread::__TLS_ALIGN;
            $crate::thread::LocalKey::new(__TLS_OFFSET__, __TLS_ALIGN__)
        };
        $crate::thread_local!($($rest)*);
    };
}
