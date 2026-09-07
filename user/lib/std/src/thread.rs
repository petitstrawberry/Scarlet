//! Legacy thread creation, scheduling, and runtime TLS management.
//!
//! Spawned threads keep their stack/TLS cleanup record in a dedicated TLS
//! mapping. The TLS pointer and mapping helpers remain part of this runtime;
//! they do not provide storage management for typed thread-local variables.
//!
//! # Removed legacy TLS API
//!
//! The old `scarlet_std::thread_local!` and `LocalKey` API was removed because
//! it did not enforce initialization, storage layout, or exclusive borrowing.
//! Normal Rust `std` applications should use `std::thread_local!`, supplied by
//! the Scarlet Rust toolchain. This legacy `no_std` facade has no replacement
//! typed TLS API; pass per-thread state into the closure given to [`spawn`].
//!
//! The old macro and both key exports are intentionally unavailable:
//!
//! ```compile_fail,E0432
//! # #![no_std]
//! # #![no_main]
//! use scarlet_std::thread_local;
//! ```
//!
//! ```compile_fail,E0432
//! # #![no_std]
//! # #![no_main]
//! use scarlet_std::LocalKey;
//! ```
//!
//! ```compile_fail,E0432
//! # #![no_std]
//! # #![no_main]
//! use scarlet_std::thread::LocalKey;
//! ```

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
    // SAFETY: This allocates a fresh non-fixed range owned by the threading runtime, without replacing live memory.
    let mapping_base = unsafe { mmap_anonymous(0, mapping_len, prot::NONE, mmap_flags::PRIVATE) }
        .map_err(|_| "Failed to allocate thread stack guard")?;
    let stack_base = mapping_base + PAGE_SIZE;

    // SAFETY: This replaces only the unused interior of the guard reservation just allocated above; no stack or reference has been placed there.
    if unsafe {
        mmap_anonymous(
            stack_base,
            STACK_SIZE,
            prot::READ | prot::WRITE,
            mmap_flags::PRIVATE | mmap_flags::FIXED,
        )
    }
    .is_err()
    {
        // SAFETY: This teardown/rollback path owns the exact mapping; its borrowed CPU views have ended before releasing the virtual range.
        let _ = unsafe { munmap(mapping_base, mapping_len) };
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
    // SAFETY: This allocates a fresh non-fixed range owned by the threading runtime, without replacing live memory.
    unsafe {
        mmap_anonymous(
            0,
            TLS_MAPPING_SIZE,
            prot::READ | prot::WRITE,
            mmap_flags::PRIVATE,
        )
    }
    .map_err(|_| "Failed to allocate thread TLS")
}

fn cleanup_thread_mappings(
    stack_mapping_base: usize,
    stack_mapping_len: usize,
    tls_mapping_base: usize,
    tls_mapping_len: usize,
) {
    // SAFETY: This teardown/rollback path owns the exact mapping; its borrowed CPU views have ended before releasing the virtual range.
    let _ = unsafe { munmap(stack_mapping_base, stack_mapping_len) };
    // SAFETY: This teardown/rollback path owns the exact mapping; its borrowed CPU views have ended before releasing the virtual range.
    let _ = unsafe { munmap(tls_mapping_base, tls_mapping_len) };
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
/// This function allocates a zero-filled fallback TLS area for the main thread.
/// [`tls_pointer`] returns this area when the architecture's TLS register is zero.
/// It does not set that register, allocate typed TLS slots, or initialize values.
/// The fallback allocation is retained for the lifetime of the process.
///
/// # Returns
///
/// Nothing. If the fallback area already exists, this leaves it unchanged.
///
/// # Safety
///
/// This function should only be called once, typically at program startup.
/// Call it from the main thread before other threads can use the fallback area.
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
///
/// # Arguments
///
/// * `ptr` - Base address of the new TLS block.
///
/// # Returns
///
/// Nothing.
///
/// # Safety
///
/// The block must have the runtime's initialized TLS layout and remain valid
/// until replacement or thread exit. Replacement must preserve all live
/// thread-local references and the runtime's thread cleanup state.
pub unsafe fn set_tls_pointer(ptr: usize) {
    // SAFETY: The caller guarantees initialized live TLS storage and safe replacement of the current thread's runtime state.
    unsafe { crate::arch::arch_set_tls_pointer(ptr) };
}

/// Thread sleep
pub fn sleep(dur: Duration) -> i32 {
    let nanosecs = dur.as_nanos() as usize;
    // SAFETY: This fixed sleep operation takes only a scalar duration and has no userspace pointer arguments.
    (unsafe { syscall1(Syscall::Sleep, nanosecs) }) as i32
}

/// Yield execution to the scheduler.
pub fn yield_now() {
    // SAFETY: This fixed scheduling operation takes no arguments or userspace pointers.
    let _ = unsafe { syscall0(Syscall::Yield) };
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
    /// Check whether the thread has finished without blocking.
    ///
    /// A successful `true` result also reaps the thread. After that, the
    /// handle is detached and must not be passed to [`JoinHandle::join`].
    /// A `false` result leaves the handle joinable so callers can poll again
    /// or eventually perform a blocking join.
    pub fn try_join(&mut self) -> Result<bool, &'static str> {
        let Some(thread_id) = self.thread_id else {
            return Err("Thread handle is detached");
        };

        let (pid, _status) = crate::task::waitpid(thread_id as i32, crate::task::WAIT_NOHANG);
        if pid == 0 {
            Ok(false)
        } else if pid == thread_id as i32 {
            self.thread_id = None;
            Ok(true)
        } else {
            Err("Failed to join thread")
        }
    }

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
            // SAFETY: The owned join handle is disarmed before detaching this scalar thread ID.
            let _ = unsafe { syscall1(Syscall::ThreadDetach, thread_id as usize) };
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
            // SAFETY: This teardown/rollback path owns the exact mapping; its borrowed CPU views have ended before releasing the virtual range.
            let _ = unsafe { munmap(stack.mapping_base, stack.mapping_len) };
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
    // SAFETY: The child receives its own stack/TLS and a Send + 'static closure packet; ownership transfers to the typed trampoline only on success.
    let result = unsafe {
        syscall5(
            Syscall::Clone,
            flags.get_raw() as usize,
            stack_top,
            thread_typed_trampoline::<F> as *const () as usize,
            start_ptr,
            tls_ptr, // TLS pointer as 5th argument
        )
    };

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
    // SAFETY: Only the current thread's runtime-owned stack/TLS ranges reach this path; successful kernel cleanup exits without resuming them.
    unsafe {
        syscall5(
            Syscall::ThreadExitCleanup,
            code as usize,
            stack_mapping_base,
            stack_mapping_len,
            tls_mapping_base,
            tls_mapping_len,
        )
    };
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

    // SAFETY: This exits the current thread without resuming any of its live references.
    unsafe { syscall1(Syscall::Exit, code as usize) };
    unreachable!("exit_thread syscall should not return");
}
