//! Deterministic guest-only allocator failure, scoped to one native thread.
//! This tests a null backend result, not physical-memory exhaustion in the OS.
use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

static DENIED_THREAD: AtomicUsize = AtomicUsize::new(0);
static DENIED_CALLS: AtomicUsize = AtomicUsize::new(0);
static CONSTRUCTOR_RESULT: AtomicUsize = AtomicUsize::new(usize::MAX);
static CONSTRUCTOR_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static DESTRUCTOR_RESULT: AtomicUsize = AtomicUsize::new(usize::MAX);
static DESTRUCTOR_ADDRESS: AtomicUsize = AtomicUsize::new(0);

fn thread_pointer() -> usize {
    let base;
    unsafe {
        #[cfg(target_arch = "aarch64")]
        std::arch::asm!("mrs {}, tpidr_el0", out(reg) base, options(nostack, readonly));
        #[cfg(target_arch = "riscv64")]
        std::arch::asm!("mv {}, tp", out(reg) base, options(nostack, readonly));
    }
    base
}

struct FaultAllocator;

unsafe impl GlobalAlloc for FaultAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let denied = DENIED_THREAD.load(Ordering::SeqCst);
        if denied != 0 && denied == thread_pointer() {
            DENIED_CALLS.fetch_add(1, Ordering::SeqCst);
            return std::ptr::null_mut();
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: FaultAllocator = FaultAllocator;

fn deny<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    let base = thread_pointer();
    assert_ne!(base, 0, "CRT did not initialize native TLS");
    assert_eq!(DENIED_THREAD.swap(base, Ordering::SeqCst), 0);
    DENIED_CALLS.store(0, Ordering::SeqCst);
    let result = operation();
    DENIED_THREAD.store(0, Ordering::SeqCst);
    (result, DENIED_CALLS.load(Ordering::SeqCst))
}

// Avoid assertions/formatting while allocation is denied: preserve failures
// as bits and report them after normal allocation has been restored.
fn first_errno_failure() -> (usize, usize) {
    let errno = scarlet_c::__errno_location();
    let mut failures = 0;
    unsafe {
        if *errno != 0 {
            failures |= 1;
        }
        if !scarlet_c::allocation::calloc(usize::MAX, 2).is_null() || *errno != 12 {
            failures |= 2;
        }
        if !scarlet_c::allocation::malloc(64).is_null() || *errno != 12 {
            failures |= 4;
        }
        if !scarlet_c::allocation::calloc(3, 17).is_null() || *errno != 12 {
            failures |= 8;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(12) {
            failures |= 16;
        }
        *errno = 73;
    }
    (failures, errno as usize)
}

extern "C" fn constructor() {
    let ((mut failures, address), attempts) = deny(first_errno_failure);
    if attempts != 2 {
        failures |= 32;
    }
    CONSTRUCTOR_ADDRESS.store(address, Ordering::SeqCst);
    CONSTRUCTOR_RESULT.store(failures, Ordering::SeqCst);
}

#[used]
#[unsafe(link_section = ".init_array")]
static CHECK_BEFORE_MAIN: extern "C" fn() = constructor;

struct Destructor;

impl Drop for Destructor {
    fn drop(&mut self) {
        let ((failures, address), attempts) = deny(|| {
            let errno = scarlet_c::__errno_location();
            unsafe { *errno = 0 };
            first_errno_failure()
        });
        DESTRUCTOR_ADDRESS.store(address, Ordering::SeqCst);
        DESTRUCTOR_RESULT.store(
            failures | if attempts == 2 { 0 } else { 32 },
            Ordering::SeqCst,
        );
    }
}

std::thread_local! {
    static CLEANUP: Destructor = const { Destructor };
}

pub fn check() {
    assert_eq!(CONSTRUCTOR_RESULT.load(Ordering::SeqCst), 0);
    let parent_errno = scarlet_c::__errno_location();
    assert_eq!(
        parent_errno as usize,
        CONSTRUCTOR_ADDRESS.load(Ordering::SeqCst)
    );
    assert_eq!(unsafe { *parent_errno }, 73);
    // A stable pointer and no allocation even before the first successful C
    // allocation, on a fresh native thread and during its TLS destructors.
    let child_address = std::thread::spawn(|| {
        CLEANUP.with(|_| {});
        let ((failures, address), attempts) = deny(first_errno_failure);
        assert_eq!(failures, 0);
        assert_eq!(attempts, 2);
        address
    })
    .join()
    .unwrap();
    assert_ne!(child_address, parent_errno as usize);
    assert_eq!(DESTRUCTOR_ADDRESS.load(Ordering::SeqCst), child_address);
    assert_eq!(DESTRUCTOR_RESULT.load(Ordering::SeqCst), 0);
    assert_eq!(unsafe { *parent_errno }, 73);

    // Real backend failure, distinct from a request rejected for arithmetic
    // overflow, must preserve realloc's input and posix_memalign's output.
    let allocation = scarlet_c::allocation::malloc(32).cast::<u8>();
    assert!(!allocation.is_null());
    unsafe { allocation.write(42) };
    let (failures, attempts) = deny(|| {
        let mut failures = 0;
        unsafe {
            if !scarlet_c::allocation::realloc(allocation.cast(), 128).is_null() {
                failures |= 1;
            }
            if allocation.read() != 42 || *parent_errno != 12 {
                failures |= 2;
            }
            let mut output = allocation.cast::<c_void>();
            *parent_errno = 73;
            if scarlet_c::allocation::posix_memalign(&mut output, 64, 128) != 12
                || output != allocation.cast()
                || *parent_errno != 73
            {
                failures |= 4;
            }
            if !scarlet_c::allocation::aligned_alloc(64, 128).is_null() || *parent_errno != 12 {
                failures |= 8;
            }
            if !scarlet_c::allocation::reallocarray(allocation.cast(), 4, 64).is_null()
                || allocation.read() != 42
                || *parent_errno != 12
            {
                failures |= 16;
            }
        }
        failures
    });
    assert_eq!(failures, 0);
    assert_eq!(attempts, 4);
    unsafe { scarlet_c::allocation::free(allocation.cast()) };
    println!(
        "NATIVE_RUSTC ERRNO_RUNTIME PASS constructor + first child access + destructor + backend ENOMEM"
    );
}
