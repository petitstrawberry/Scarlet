//! Guest VCPU state for Type-2 hypervisor

use crate::arch::riscv64::fpu::{FpuContext, VectorContext};
use crate::arch::riscv64::{IntRegisters, Mode, Trapframe};
use alloc::boxed::Box;

use super::csr;
use super::reg_index::reg;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GuestCsrState {
    pub sscratch: u64,
    pub sepc: u64,
    pub scause: u64,
    pub stval: u64,
    pub satp: u64,
    pub sstatus: u64,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct GuestVcpu {
    pub iregs: IntRegisters,
    pub csrs: GuestCsrState,
    pub pc: u64,
    pub fpu: FpuContext,
    pub fpu_used: bool,
    pub vector: Option<Box<VectorContext>>,
    pub vector_used: bool,
    pub asid: usize,
    pub mode: Mode,
    pub vm_id: u32,
    pub vcpu_id: u32,
}

impl GuestVcpu {
    pub fn new(vm_id: u32, vcpu_id: u32) -> Self {
        Self {
            iregs: IntRegisters::new(),
            fpu: FpuContext::new(),
            fpu_used: false,
            vector: None,
            vector_used: false,
            pc: 0,
            asid: 0,
            mode: Mode::GuestUser,
            csrs: GuestCsrState::default(),
            vm_id,
            vcpu_id,
        }
    }

    pub fn store(&mut self, trapframe: &Trapframe) {
        self.iregs = trapframe.regs;
        self.pc = trapframe.epc;
    }

    pub fn switch(&mut self, trapframe: &mut Trapframe) {
        trapframe.regs = self.iregs;
        trapframe.epc = self.pc;
    }

    pub fn set_pc(&mut self, pc: u64) {
        self.pc = pc;
    }
    pub fn get_pc(&self) -> u64 {
        self.pc
    }
    pub fn set_asid(&mut self, asid: usize) {
        self.asid = asid;
    }
    pub fn get_asid(&self) -> usize {
        self.asid
    }
    pub fn get_mode(&self) -> Mode {
        self.mode
    }
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub fn get_gpr(&self, index: usize) -> u64 {
        if index < 32 {
            self.iregs.reg[index] as u64
        } else {
            0
        }
    }

    pub fn set_gpr(&mut self, index: usize, value: u64) {
        if index < 32 {
            self.iregs.reg[index] = value as usize;
        }
    }

    pub fn get_reg(&self, index: u32) -> Result<u64, &'static str> {
        match index {
            reg::PC => Ok(self.pc),
            i if i < 32 => Ok(self.iregs.reg[i as usize] as u64),
            reg::SSTATUS => Ok(self.csrs.sstatus),
            reg::SEPC => Ok(self.csrs.sepc),
            reg::SCAUSE => Ok(self.csrs.scause),
            reg::STVAL => Ok(self.csrs.stval),
            reg::SATP => Ok(self.csrs.satp),
            reg::SSCRATCH => Ok(self.csrs.sscratch),
            i if reg::IS_FREG(i) => {
                let fidx = (i - reg::FREG_BASE) as usize;
                if fidx < 32 {
                    Ok(self.fpu.f[fidx])
                } else {
                    Err("Invalid FPR index")
                }
            }
            reg::FCSR => Ok(self.fpu.fcsr as u64),
            _ => Err("Invalid register index"),
        }
    }

    pub fn set_reg(&mut self, index: u32, value: u64) -> Result<(), &'static str> {
        match index {
            reg::PC => {
                self.pc = value;
                Ok(())
            }
            i if i < 32 => {
                self.iregs.reg[i as usize] = value as usize;
                Ok(())
            }
            reg::SSTATUS => {
                self.csrs.sstatus = value;
                Ok(())
            }
            reg::SEPC => {
                self.csrs.sepc = value;
                Ok(())
            }
            reg::SCAUSE => {
                self.csrs.scause = value;
                Ok(())
            }
            reg::STVAL => {
                self.csrs.stval = value;
                Ok(())
            }
            reg::SATP => {
                self.csrs.satp = value;
                Ok(())
            }
            reg::SSCRATCH => {
                self.csrs.sscratch = value;
                Ok(())
            }
            i if reg::IS_FREG(i) => {
                let fidx = (i - reg::FREG_BASE) as usize;
                if fidx < 32 {
                    self.fpu.f[fidx] = value;
                    self.fpu_used = true;
                    Ok(())
                } else {
                    Err("Invalid FPR index")
                }
            }
            reg::FCSR => {
                self.fpu.fcsr = value as u32;
                Ok(())
            }
            _ => Err("Invalid register index"),
        }
    }

    pub fn save_csrs(&mut self) {
        self.csrs.sscratch = csr::read_vsscratch();
        self.csrs.sepc = csr::read_vsepc();
        self.csrs.scause = csr::read_vscause();
        self.csrs.stval = csr::read_vstval();
        self.csrs.satp = csr::read_vsatp();
        self.csrs.sstatus = csr::read_vsstatus();
    }

    pub fn restore_csrs(&self) {
        csr::write_vsscratch(self.csrs.sscratch);
        csr::write_vsepc(self.csrs.sepc);
        csr::write_vscause(self.csrs.scause);
        csr::write_vstval(self.csrs.stval);
        csr::write_vsatp(self.csrs.satp);
        csr::write_vsstatus(self.csrs.sstatus);
    }

    pub fn clone_to(&self, other: &mut GuestVcpu) {
        other.iregs = self.iregs;
        other.fpu = self.fpu.clone();
        other.fpu_used = self.fpu_used;
        other.vector = self.vector.clone();
        other.vector_used = self.vector_used;
        other.pc = self.pc;
        other.asid = self.asid;
        other.mode = self.mode;
        other.csrs = self.csrs;
    }
}

static mut CURRENT_GUEST_VCPU: *mut GuestVcpu = core::ptr::null_mut();

pub unsafe fn current_guest_vcpu() -> &'static mut GuestVcpu {
    &mut *CURRENT_GUEST_VCPU
}

pub unsafe fn set_current_guest_vcpu(vcpu: *mut GuestVcpu) {
    CURRENT_GUEST_VCPU = vcpu;
}

pub unsafe fn clear_current_guest_vcpu() {
    CURRENT_GUEST_VCPU = core::ptr::null_mut();
}
