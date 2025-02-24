use core::arch::asm;

pub fn ecall(a0: usize, a1: usize, a2: usize, a3: usize, a7: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!(
            "ecall",
            inout("a0") a0 => ret,
            in("a1") a1,
            in("a2") a2,
            in("a3") a3,
            in("a7") a7,
            options(nostack),
        );
    }
    ret
}