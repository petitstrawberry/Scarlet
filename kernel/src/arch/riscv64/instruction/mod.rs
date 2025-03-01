use core::arch::asm;

pub mod sbi;

pub fn idle() {
    loop {
        unsafe {
            asm!("wfi", options(nostack));
        }
    }
}

pub fn syscall(num: usize, arg0: usize, arg1: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize, arg6: usize) -> usize {
    ecall(num, arg0, arg1, arg2, arg3, arg4, arg5, arg6)
}

pub fn ecall(a0: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize, a6: usize, a7: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!(
            "ecall",
            inout("a0") a0 => ret,
            in("a1") a1,
            in("a2") a2,
            in("a3") a3,
            in("a4") a4,
            in("a5") a5,
            in("a6") a6,
            in("a7") a7,
            options(nostack),
        );
    }
    ret
}