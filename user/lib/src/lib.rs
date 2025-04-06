#![no_std]
#![no_main]
#![feature(naked_functions)]
#![feature(alloc_error_handler)]

extern crate alloc;

mod arch;
pub mod mem;
pub mod syscall;

#[panic_handler]
pub fn panic(_info: &core::panic::PanicInfo) -> ! {
    // This is the panic handler.
    // You can put your code here.
    loop {}
}

#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    loop {}
}
