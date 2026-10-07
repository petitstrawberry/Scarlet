//! A real Rust-generated shared object with a stable C entry point and data relocations.
#![no_std]

#[unsafe(no_mangle)]
pub static RUST_VALUE: i32 = 42;

#[unsafe(no_mangle)]
pub static mut RUST_VALUE_POINTER: *const i32 = &RUST_VALUE;

#[unsafe(no_mangle)]
pub extern "C" fn rust_answer() -> i32 {
    // Volatile reads keep both the global pointer and pointee loads in the
    // generated code, so the smoke call exercises their dynamic relocations.
    unsafe {
        let pointer = core::ptr::read_volatile(&raw const RUST_VALUE_POINTER);
        core::ptr::read_volatile(pointer)
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
