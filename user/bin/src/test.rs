#![no_std]
#![no_main]
#![feature(naked_functions)]

use core::arch::naked_asm;

#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry")]
#[naked]
pub extern "C" fn _entry() {
    unsafe {
        naked_asm!("
        .option norvc
        .option norelax
        .align 8
                ecall
        ",
        );
    }
}

#[panic_handler]
pub fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}