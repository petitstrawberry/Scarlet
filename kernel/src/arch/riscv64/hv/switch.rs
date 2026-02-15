//! World switch for guest execution

use core::arch::naked_asm;

use super::GuestVcpu;

unsafe extern "C" {
    fn run_guest_loop_return();
}

#[unsafe(naked)]
pub unsafe extern "C" fn run_guest_loop(_vcpu: *const GuestVcpu) {
    naked_asm!(
        "addi sp, sp, -104",
        "sd ra, 0(sp)",
        "sd s0, 8(sp)",
        "sd s1, 16(sp)",
        "sd s2, 24(sp)",
        "sd s3, 32(sp)",
        "sd s4, 40(sp)",
        "sd s5, 48(sp)",
        "sd s6, 56(sp)",
        "sd s7, 64(sp)",
        "sd s8, 72(sp)",
        "sd s9, 80(sp)",
        "sd s10, 88(sp)",
        "sd s11, 96(sp)",
        "ld t0, 0(a0)",
        "csrw vsscratch, t0",
        "ld t0, 8(a0)",
        "csrw vsepc, t0",
        "ld t0, 24(a0)",
        "csrw vsatp, t0",
        "ld t0, 32(a0)",
        "csrw vsstatus, t0",
        "ld t0, 40(a0)",
        "csrw hstatus, t0",
        "ld x1, 104(a0)",
        "ld x2, 112(a0)",
        "ld x3, 120(a0)",
        "ld x4, 128(a0)",
        "ld x5, 136(a0)",
        "ld x6, 144(a0)",
        "ld x7, 152(a0)",
        "ld x8, 160(a0)",
        "ld x9, 168(a0)",
        "ld x10, 176(a0)",
        "ld x11, 184(a0)",
        "ld x12, 192(a0)",
        "ld x13, 200(a0)",
        "ld x14, 208(a0)",
        "ld x15, 216(a0)",
        "ld x16, 224(a0)",
        "ld x17, 232(a0)",
        "ld x18, 240(a0)",
        "ld x19, 248(a0)",
        "ld x20, 256(a0)",
        "ld x21, 264(a0)",
        "ld x22, 272(a0)",
        "ld x23, 280(a0)",
        "ld x24, 288(a0)",
        "ld x25, 296(a0)",
        "ld x26, 304(a0)",
        "ld x27, 312(a0)",
        "ld x28, 320(a0)",
        "ld x29, 328(a0)",
        "ld x30, 336(a0)",
        "ld x31, 344(a0)",
        "ld t0, 352(a0)",
        "csrw sepc, t0",
        "li t0, 0x80",
        "csrs hstatus, t0",
        "sret",
        ".global run_guest_loop_return",
        "run_guest_loop_return:",
        "ld ra, 0(sp)",
        "ld s0, 8(sp)",
        "ld s1, 16(sp)",
        "ld s2, 24(sp)",
        "ld s3, 32(sp)",
        "ld s4, 40(sp)",
        "ld s5, 48(sp)",
        "ld s6, 56(sp)",
        "ld s7, 64(sp)",
        "ld s8, 72(sp)",
        "ld s9, 80(sp)",
        "ld s10, 88(sp)",
        "ld s11, 96(sp)",
        "addi sp, sp, 104",
        "ret",
    );
}

pub fn run_guest_loop_return_addr() -> usize {
    unsafe { run_guest_loop_return as usize }
}
