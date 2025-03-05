use core::{arch::asm, mem::transmute};

use exception::arch_exception_handler;
use interrupt::arch_interrupt_handler;

use super::Trapframe;

pub mod interrupt;
pub mod exception;
pub mod kernel;
pub mod user;

#[unsafe(export_name = "arch_trap_handler")]
pub extern "C" fn arch_trap_handler(addr: usize) -> usize {
    let trapframe: &mut Trapframe = unsafe { transmute(addr) };

    let cause: usize;
    unsafe {
        asm!(
            "csrr {0}, scause",
            out(reg) cause,
        );
    }

    let interrupt = cause & 0x8000000000000000 != 0;
    if interrupt {
        arch_interrupt_handler(trapframe, cause & !0x8000000000000000);
    } else {
        arch_exception_handler(trapframe, cause);
    }
    addr
}