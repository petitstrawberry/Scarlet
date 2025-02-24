use core::arch::asm;

pub mod sbi;
pub mod ecall;

pub fn idle() {
    loop {
        unsafe {
            asm!("wfi", options(nostack));
        }
    }
}