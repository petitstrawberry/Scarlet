//! World switch for guest execution

use core::arch::naked_asm;

use crate::arch::{
    Trapframe,
    hv::{
        csr::{GuestCsrState, HypervisorCsrState},
        guest_vcpu::GuestVcpu,
    },
};

mod offset {
    pub const IREGS: usize = 0;
    pub const CSRS: usize = 256;
    pub const CSRS_SSCRATCH: usize = CSRS + 0;
    pub const CSRS_SEPC: usize = CSRS + 8;
    pub const CSRS_SATP: usize = CSRS + 32;
    pub const CSRS_SSTATUS: usize = CSRS + 40;
    pub const PC: usize = CSRS + 48;
    pub const RISCV64_KERNEL_STACK: usize = 24;
}

mod tf_offset {
    pub const X1: usize = 8;
    pub const X2: usize = 16;
    pub const X3: usize = 24;
    pub const X31: usize = 248;
    pub const EPC: usize = 256;
}

#[unsafe(naked)]
pub unsafe extern "C" fn run_guest_loop(_vcpu: *const GuestVcpu, _arch: *mut u8) {
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

        "sd sp, {kernel_stack}(a1)",

        "ld t0, {csrs_sscratch}(a0)",
        "csrw vsscratch, t0",
        "ld t0, {csrs_sepc}(a0)",
        "csrw vsepc, t0",
        "ld t0, {csrs_satp}(a0)",
        "csrw vsatp, t0",
        "ld t0, {csrs_sstatus}(a0)",
        "csrw vsstatus, t0",

        "ld x1, 8(a0)",
        "ld x2, 16(a0)",
        "ld x3, 24(a0)",
        "ld x4, 32(a0)",
        "ld x5, 40(a0)",
        "ld x6, 48(a0)",
        "ld x7, 56(a0)",
        "ld x8, 64(a0)",
        "ld x9, 72(a0)",
        "ld x10, 80(a0)",
        "ld x11, 88(a0)",
        "ld x12, 96(a0)",
        "ld x13, 104(a0)",
        "ld x14, 112(a0)",
        "ld x15, 120(a0)",
        "ld x16, 128(a0)",
        "ld x17, 136(a0)",
        "ld x18, 144(a0)",
        "ld x19, 152(a0)",
        "ld x20, 160(a0)",
        "ld x21, 168(a0)",
        "ld x22, 176(a0)",
        "ld x23, 184(a0)",
        "ld x24, 192(a0)",
        "ld x25, 200(a0)",
        "ld x26, 208(a0)",
        "ld x27, 216(a0)",
        "ld x28, 224(a0)",
        "ld x29, 232(a0)",
        "ld x30, 240(a0)",
        "ld x31, 248(a0)",

        "ld t0, {pc}(a0)",
        "csrw sepc, t0",

        "li t0, 0x100",
        "csrs hstatus, t0",
        "sret",

        kernel_stack = const offset::RISCV64_KERNEL_STACK,
        csrs_sscratch = const offset::CSRS_SSCRATCH,
        csrs_sepc = const offset::CSRS_SEPC,
        csrs_satp = const offset::CSRS_SATP,
        csrs_sstatus = const offset::CSRS_SSTATUS,
        pc = const offset::PC,
    );
}

#[unsafe(naked)]
pub unsafe extern "C" fn resume_guest_loop(_trapframe: *mut Trapframe) {
    naked_asm!(
        "ld x1, {x1}(a0)",
        "ld x3, 24(a0)",
        "ld x4, 32(a0)",
        "ld x5, 40(a0)",
        "ld x6, 48(a0)",
        "ld x7, 56(a0)",
        "ld x8, 64(a0)",
        "ld x9, 72(a0)",
        "ld x10, 80(a0)",
        "ld x11, 88(a0)",
        "ld x12, 96(a0)",
        "ld x13, 104(a0)",
        "ld x14, 112(a0)",
        "ld x15, 120(a0)",
        "ld x16, 128(a0)",
        "ld x17, 136(a0)",
        "ld x18, 144(a0)",
        "ld x19, 152(a0)",
        "ld x20, 160(a0)",
        "ld x21, 168(a0)",
        "ld x22, 176(a0)",
        "ld x23, 184(a0)",
        "ld x24, 192(a0)",
        "ld x25, 200(a0)",
        "ld x26, 208(a0)",
        "ld x27, 216(a0)",
        "ld x28, 224(a0)",
        "ld x29, 232(a0)",
        "ld x30, 240(a0)",
        "ld x31, 248(a0)",
        "ld t0, {epc}(a0)",
        "csrw sepc, t0",
        "sret",

        x1 = const tf_offset::X1,
        epc = const tf_offset::EPC,
    );
}

#[unsafe(naked)]
pub unsafe extern "C" fn arch_guest_trap_exit(_trapframe: *mut u8) {
    naked_asm!(
        "addi sp, a0, 272",
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

pub struct HypervisorSwitchData {
    hypervisor_csrs: HypervisorCsrState,
}

impl HypervisorSwitchData {
    pub fn save() -> Self {
        HypervisorSwitchData {
            hypervisor_csrs: HypervisorCsrState::save(),
        }
    }

    pub fn restore(&self) {
        self.hypervisor_csrs.restore();
    }
}

pub struct VcpuSwitchData {
    guest_csrs: GuestCsrState,
}

impl VcpuSwitchData {
    pub fn save() -> Self {
        VcpuSwitchData {
            guest_csrs: GuestCsrState::save(),
        }
    }

    pub fn restore(&self) {
        self.guest_csrs.restore();
    }
}
