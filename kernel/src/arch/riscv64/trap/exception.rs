use crate::arch::Arch;
use crate::println;
use crate::print;

pub fn arch_exception_handler(arch: &mut Arch, cause: usize) {
    loop {}
}