//! Global allocator using the talc crate.
//!
//! This module provides a global allocator based on `talc`, a high-performance
//! no_std allocator. It uses a custom OOM handler that claims additional heap
//! regions via the `sbrk` system call when memory runs out.

use core::alloc::Layout;
use core::cell::UnsafeCell;
use scarlet_sys::{Syscall, syscall1};
use talc::{OomHandler, Span, Talc, Talck};

/// Minimum extension size (4 KiB).
const MIN_EXTEND_SIZE: usize = 4096;

/// Custom OOM handler that claims heap regions using the sbrk syscall.
pub struct SbrkOomHandler;

impl SbrkOomHandler {
    /// Create a new uninitialized handler.
    pub const fn new() -> Self {
        Self
    }
}

impl Default for SbrkOomHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl OomHandler for SbrkOomHandler {
    fn handle_oom(talc: &mut Talc<Self>, layout: Layout) -> Result<(), ()> {
        let required = layout.size().max(layout.align().checked_mul(2).ok_or(())?);
        let extend_size = required.checked_add(MIN_EXTEND_SIZE).ok_or(())?;
        let aligned_size =
            extend_size.checked_add(MIN_EXTEND_SIZE - 1).ok_or(())? & !(MIN_EXTEND_SIZE - 1);

        // Talc automatically merges adjacent claimed spans through `extend`.
        // Reserve one unclaimed page after each heap so independently claimed
        // sbrk regions can never take that path.
        let reservation_size = aligned_size.checked_add(MIN_EXTEND_SIZE).ok_or(())?;
        let result = sbrk(reservation_size);
        if result == usize::MAX {
            // sbrk failed
            return Err(());
        }

        let new_end = result.checked_add(aligned_size).ok_or(())?;
        let new_base = result as *mut u8;
        let new_span = Span::new(new_base, new_end as *mut u8);

        // Treat every sbrk allocation as an independent Talc heap. `extend`
        // requires Talc boundary metadata at the old span's acme, but the
        // program break is also managed by the kernel and cannot provide that
        // invariant reliably.
        // SAFETY: A successful sbrk call grants this process exclusive,
        // readable and writable access to the returned contiguous range. The
        // claimed portion cannot overlap an earlier claimed range. The final
        // page in this sbrk reservation intentionally remains unclaimed.
        unsafe { talc.claim(new_span) }.map_err(|_| ())?;

        Ok(())
    }
}

/// The global allocator type.
pub type GlobalAllocator = Talck<spin::Mutex<()>, SbrkOomHandler>;

/// The global allocator instance.
///
/// Note: The allocator starts with an empty heap and claims memory on first OOM
/// (which happens on first allocation).
#[global_allocator]
pub static ALLOCATOR: GlobalAllocator = {
    let talc = Talc::new(SbrkOomHandler::new());
    talc.lock::<spin::Mutex<()>>()
};

type AllocatorForkGuard = lock_api::MutexGuard<'static, spin::Mutex<()>, Talc<SbrkOomHandler>>;

struct ForkGuardSlot(UnsafeCell<Option<AllocatorForkGuard>>);

// SAFETY: `FORK_GUARD` is only touched by the thread currently executing
// `fork()`. Concurrent fork callers serialize on `ALLOCATOR.lock()`, and the
// child receives a private copy of this slot after the kernel clone returns.
unsafe impl Sync for ForkGuardSlot {}

static FORK_GUARD: ForkGuardSlot = ForkGuardSlot(UnsafeCell::new(None));

/// Lock the global allocator before `fork`.
///
/// In a multi-threaded process, another thread can be mutating allocator
/// metadata while the calling thread forks. The child only keeps the calling
/// thread, so it must not inherit allocator metadata in the middle of a
/// mutation. Scarlet's `fork` wrapper follows libc practice and holds the
/// allocator lock across the kernel clone operation.
pub fn fork_prepare() {
    let guard = ALLOCATOR.lock();
    // SAFETY: Holding `ALLOCATOR` serializes all parent-side access to this
    // slot. The guard is stored without allocating so it can span the raw
    // clone syscall.
    unsafe {
        *FORK_GUARD.0.get() = Some(guard);
    }
}

/// Release the allocator lock in the parent after `fork`.
pub fn fork_parent() {
    // SAFETY: This is paired with `fork_prepare` in the same parent process.
    // Taking the guard drops it and unlocks the allocator.
    unsafe {
        drop((*FORK_GUARD.0.get()).take());
    }
}

/// Release the copied allocator lock state in the child after `fork`.
pub fn fork_child() {
    // SAFETY: After fork the child has its own copied address space and only
    // the calling thread exists. Dropping the copied guard unlocks the child's
    // copied allocator mutex state before any normal Rust code allocates.
    unsafe {
        drop((*FORK_GUARD.0.get()).take());
    }
}

/// Increase the program break by `size` bytes.
/// Returns the previous break address, or `usize::MAX` on failure.
pub fn sbrk(size: usize) -> usize {
    syscall1(Syscall::Sbrk, size)
}

/// Set the program break to `addr`.
/// Returns the new break address, or `usize::MAX` on failure.
#[allow(dead_code)]
pub fn brk(addr: usize) -> usize {
    syscall1(Syscall::Brk, addr)
}
