#![no_std]
#![no_main]
#![feature(naked_functions)]

mod arch;
pub mod mem;
pub mod syscall;

#[panic_handler]
pub fn panic(_info: &core::panic::PanicInfo) -> ! {
    // This is the panic handler.
    // You can put your code here.
    loop {}
}
