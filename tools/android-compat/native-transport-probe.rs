//! Link the real Android-target SDK into a Bionic executable.

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_sdk_pid() -> usize {
    // SAFETY: Getpid has no pointer arguments and transfers no ownership.
    unsafe { scarlet_sys::syscall0(scarlet_sys::Syscall::Getpid) }
}

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_sdk_pid_number() -> usize {
    scarlet_abi::syscall_transport_number(scarlet_abi::Syscall::Getpid)
}

