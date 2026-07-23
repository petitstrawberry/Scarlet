use crate::arch::vcpu::Vcpu;
use crate::arch::{Mode, Trapframe};
use alloc::boxed::Box;

use super::super::fpu::FpuContext;
use super::reg_index::reg;
use super::sysreg::GuestSystemRegs;

const PSR_MODE_EL0T: u64 = 0x0;
const PSR_MODE_EL1T: u64 = 0x4;
const PSR_MODE_EL1H: u64 = 0x5;
const PSR_MODE_MASK: u64 = 0xf;

fn mode_from_pstate(spsr: u64) -> Mode {
    match spsr & PSR_MODE_MASK {
        PSR_MODE_EL0T => Mode::GuestUser,
        PSR_MODE_EL1T | PSR_MODE_EL1H => Mode::GuestKernel,
        _ => Mode::GuestKernel,
    }
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct GuestVcpu {
    iregs: [u64; 31],
    pc: u64,
    spsr: u64,
    pub(crate) sysregs: GuestSystemRegs,
    fpu: FpuContext,
    fpu_used: bool,
    vector_used: bool,
    vector: Option<Box<[u8; 4096]>>,
    mode: Mode,
    vm_id: u32,
    vcpu_id: u32,
}

impl GuestVcpu {
    pub fn new(vm_id: u32, vcpu_id: u32) -> Self {
        Self {
            iregs: [0; 31],
            pc: 0,
            spsr: PSR_MODE_EL1H,
            sysregs: GuestSystemRegs::default(),
            fpu: FpuContext::new(),
            fpu_used: false,
            vector_used: false,
            vector: None,
            mode: Mode::GuestKernel,
            vm_id,
            vcpu_id,
        }
    }

    pub fn store(&mut self, vcpu: &Vcpu) {
        let _ = vcpu;
    }

    pub fn switch(&mut self, trapframe: &mut Trapframe) {
        for (i, reg) in self.iregs.iter().enumerate() {
            if i < 31 {
                trapframe.regs.reg[i] = *reg as usize;
            }
        }
        trapframe.elr = self.pc;
    }

    pub fn save(&mut self, trapframe: &Trapframe) {
        for (i, reg) in trapframe.regs.reg.iter().enumerate() {
            if i < 31 {
                self.iregs[i] = *reg as u64;
            }
        }
        self.pc = trapframe.elr;
        self.spsr = trapframe.spsr;
        self.mode = mode_from_pstate(self.spsr);
    }

    pub fn save_fpu(&mut self) {
        // SAFETY: CPTR_EL2 is configured to allow FP/SIMD access while the
        // hypervisor runs. The guest owns the physical FP/SIMD register file
        // until this snapshot is taken after guest exit.
        unsafe { self.fpu.save() };
        self.fpu_used = true;
    }

    pub fn restore_fpu(&self) {
        // SAFETY: CPTR_EL2 is configured to allow FP/SIMD access while the
        // hypervisor runs. This restores the vCPU's FP/SIMD state immediately
        // before guest entry.
        unsafe { self.fpu.restore() };
    }

    pub fn get_mmio_data(&self, reg_idx: usize, size: u8) -> u64 {
        if reg_idx >= self.iregs.len() {
            return 0;
        }
        let val = self.iregs[reg_idx];
        match size {
            1 => val & 0xFF,
            2 => val & 0xFFFF,
            4 => val & 0xFFFFFFFF,
            _ => val,
        }
    }

    pub fn set_mmio_data(&mut self, reg_idx: usize, size: u8, data: u64) {
        if reg_idx >= self.iregs.len() {
            return;
        }
        let mask = match size {
            1 => 0xFF,
            2 => 0xFFFF,
            4 => 0xFFFFFFFF,
            _ => !0,
        };
        let old = self.iregs[reg_idx];
        let new = (old & !mask) | (data & mask);
        self.iregs[reg_idx] = new;
    }

    pub fn get_reg(&self, index: usize) -> Result<u64, &'static str> {
        match index {
            reg::X0..=reg::X30 => Ok(self.iregs[index]),
            reg::SP => Ok(self.sysregs.sp_el1),
            reg::PC => Ok(self.pc),
            reg::PSTATE => Ok(self.spsr),
            reg::SCTLR_EL1 => Ok(self.sysregs.sctlr_el1),
            reg::VBAR_EL1 => Ok(self.sysregs.vbar_el1),
            reg::TCR_EL1 => Ok(self.sysregs.tcr_el1),
            reg::TTBR0_EL1 => Ok(self.sysregs.ttbr0_el1),
            reg::TTBR1_EL1 => Ok(self.sysregs.ttbr1_el1),
            reg::MAIR_EL1 => Ok(self.sysregs.mair_el1),
            reg::AMAIR_EL1 => Ok(self.sysregs.amair_el1),
            reg::ESR_EL1 => Ok(self.sysregs.esr_el1),
            reg::FAR_EL1 => Ok(self.sysregs.far_el1),
            reg::ELR_EL1 => Ok(self.sysregs.elr_el1),
            reg::SPSR_EL1 => Ok(self.sysregs.spsr_el1),
            reg::SP_EL1 => Ok(self.sysregs.sp_el1),
            reg::CPACR_EL1 => Ok(self.sysregs.cpacr_el1),
            reg::CONTEXTIDR_EL1 => Ok(self.sysregs.contextidr_el1),
            reg::CNTV_CTL_EL0 => Ok(self.sysregs.cntv_ctl_el0),
            reg::CNTV_CVAL_EL0 => Ok(self.sysregs.cntv_cval_el0),
            reg::CNTVOFF_EL2 => Ok(self.sysregs.cntvoff_el2),
            reg::CNTKCTL_EL1 => Ok(self.sysregs.cntkctl_el1),
            _ => Err("Invalid register index"),
        }
    }

    pub fn set_reg(&mut self, index: usize, value: u64) -> Result<(), &'static str> {
        match index {
            reg::X0..=reg::X30 => {
                self.iregs[index] = value;
                Ok(())
            }
            reg::SP => {
                self.sysregs.sp_el1 = value;
                Ok(())
            }
            reg::PC => {
                self.pc = value;
                Ok(())
            }
            reg::PSTATE => {
                self.spsr = value;
                self.mode = mode_from_pstate(value);
                Ok(())
            }
            reg::SCTLR_EL1 => {
                self.sysregs.sctlr_el1 = value;
                Ok(())
            }
            reg::VBAR_EL1 => {
                self.sysregs.vbar_el1 = value;
                Ok(())
            }
            reg::TCR_EL1 => {
                self.sysregs.tcr_el1 = value;
                Ok(())
            }
            reg::TTBR0_EL1 => {
                self.sysregs.ttbr0_el1 = value;
                Ok(())
            }
            reg::TTBR1_EL1 => {
                self.sysregs.ttbr1_el1 = value;
                Ok(())
            }
            reg::MAIR_EL1 => {
                self.sysregs.mair_el1 = value;
                Ok(())
            }
            reg::AMAIR_EL1 => {
                self.sysregs.amair_el1 = value;
                Ok(())
            }
            reg::ESR_EL1 => {
                self.sysregs.esr_el1 = value;
                Ok(())
            }
            reg::FAR_EL1 => {
                self.sysregs.far_el1 = value;
                Ok(())
            }
            reg::ELR_EL1 => {
                self.sysregs.elr_el1 = value;
                Ok(())
            }
            reg::SPSR_EL1 => {
                self.sysregs.spsr_el1 = value;
                Ok(())
            }
            reg::SP_EL1 => {
                self.sysregs.sp_el1 = value;
                Ok(())
            }
            reg::CPACR_EL1 => {
                self.sysregs.cpacr_el1 = value;
                Ok(())
            }
            reg::CONTEXTIDR_EL1 => {
                self.sysregs.contextidr_el1 = value;
                Ok(())
            }
            reg::CNTV_CTL_EL0 => {
                self.sysregs.cntv_ctl_el0 = value;
                Ok(())
            }
            reg::CNTV_CVAL_EL0 => {
                self.sysregs.cntv_cval_el0 = value;
                Ok(())
            }
            reg::CNTVOFF_EL2 => {
                self.sysregs.cntvoff_el2 = value;
                Ok(())
            }
            reg::CNTKCTL_EL1 => {
                self.sysregs.cntkctl_el1 = value;
                Ok(())
            }
            _ => Err("Invalid register index"),
        }
    }

    pub fn get_pc(&self) -> u64 {
        self.pc
    }

    pub fn set_pc(&mut self, pc: u64) {
        self.pc = pc;
    }

    pub fn get_mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub fn vm_id(&self) -> u32 {
        self.vm_id
    }

    pub fn vcpu_id(&self) -> u32 {
        self.vcpu_id
    }
}
