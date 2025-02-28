use crate::arch::Arch;
use crate::println;
use crate::print;

pub fn arch_interrupt_handler(arch: &mut Arch, cause: usize) {
    loop {}
}