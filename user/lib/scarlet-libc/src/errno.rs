//! C errno uses the runtime's preallocated native TLS header.

use std::ffi::c_int;

#[cfg(not(target_os = "scarlet"))]
std::thread_local! {
    static ERRNO: std::cell::UnsafeCell<c_int> = const { std::cell::UnsafeCell::new(0) };
}

/// errno belongs to the calling thread. C must not retain it past thread exit.
///
/// On Scarlet the matching CRT/thread creator prepares this storage before
/// user code starts. Access does not use Rust TLS keys, allocate, or issue a
/// syscall, including the first access and access during TLS destruction.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn __errno_location() -> *mut c_int {
    #[cfg(not(target_os = "scarlet"))]
    {
        ERRNO.with(std::cell::UnsafeCell::get)
    }
    #[cfg(target_os = "scarlet")]
    {
        use scarlet_abi::tls::{NATIVE_TLS_MAGIC, NativeTlsHeader};
        let base: usize;
        // SAFETY: Read the current thread's architectural runtime pointer.
        unsafe {
            #[cfg(target_arch = "aarch64")]
            std::arch::asm!("mrs {}, tpidr_el0", out(reg) base, options(nostack, readonly));
            #[cfg(target_arch = "riscv64")]
            std::arch::asm!("mv {}, tp", out(reg) base, options(nostack, readonly));
        }
        if base == 0 {
            // An incompatible/custom entry point skipped runtime startup.
            // Do not turn an error-path accessor into a fallible allocator.
            std::process::abort();
        }
        let header = std::ptr::with_exposed_provenance_mut::<NativeTlsHeader>(base);
        // SAFETY: A native runtime owns the mapping at TP for this thread's
        // lifetime. Reject older layouts rather than corrupting Rust TLS keys.
        unsafe {
            if (*header).magic != NATIVE_TLS_MAGIC {
                std::process::abort();
            }
            &raw mut (*header).errno
        }
    }
}
