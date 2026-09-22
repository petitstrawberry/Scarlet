#![no_std]
extern crate demo;
#[unsafe(no_mangle)]
pub extern "C" fn overflow_check() -> bool {
    demo::calloc(usize::MAX, 2).is_null()
}
