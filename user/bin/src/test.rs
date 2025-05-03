#![no_std]
#![no_main]

use core::arch::naked_asm;

#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry")]
#[unsafe(naked)]
pub extern "C" fn _entry() {
    unsafe {
        naked_asm!("
        .option norvc
        .option norelax
        .align 8
                ecall
                j main
        ",
        );
    }
}

#[panic_handler]
#[unsafe(link_section = ".text.panic")]
pub fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[allow(static_mut_refs)]
#[unsafe(link_section = ".text")]
#[unsafe(export_name = "main")]
pub extern "C" fn main() {
    let a = HELLO;
    let b = DUMMY;
    let c = unsafe { &mut DUMMY_BSS };
    for i in 0..a.len() {
        *c = a[i] as usize + b;
    }
    loop {}
}

pub static HELLO: &[u8] = b"Hello, world!\n\0";

#[unsafe(link_section = ".data")]
pub static DUMMY: usize = 0xdeadbeef;

#[unsafe(link_section = ".bss")]
pub static mut DUMMY_BSS: usize = 0;