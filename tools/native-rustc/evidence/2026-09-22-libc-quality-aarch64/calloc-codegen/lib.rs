#![no_std]
#![cfg_attr(disable_builtins, no_builtins)]

// Even this always-null implementation is ignored by LLVM's libc recognition
// when the consuming crate calls the exported calloc without no_builtins.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn calloc(_count: usize, _size: usize) -> *mut u8 {
    core::ptr::null_mut()
}
