use crate::arch::vcpu::Vcpu;
use crate::arch::{Mode, Trapframe};
use alloc::boxed::Box;

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

    pub fn get_mmio_data(&self, reg_idx: u8, size: u8) -> u64 {
        if reg_idx == 0 || reg_idx as usize > 30 {
            return 0;
        }
        let val = self.iregs[reg_idx as usize - 1];
        match size {
            1 => val & 0xFF,
            2 => val & 0xFFFF,
            4 => val & 0xFFFFFFFF,
            _ => val,
        }
    }

    pub fn set_mmio_data(&mut self, reg_idx: u8, size: u8, data: u64) {
        if reg_idx == 0 || reg_idx as usize > 30 {
            return;
        }
        let mask = match size {
            1 => 0xFF,
            2 => 0xFFFF,
            4 => 0xFFFFFFFF,
            _ => !0,
        };
        let old = self.iregs[reg_idx as usize - 1];
        let new = (old & !mask) | (data & mask);
        self.iregs[reg_idx as usize - 1] = new;
    }

    pub fn get_reg(&self, index: u32) -> Result<u64, &'static str> {
        match index {
            reg::X0..=reg::X30 => Ok(self.iregs[index as usize]),
            reg::SP => Ok(self.sysregs.sp_el1),
            reg::PC => Ok(self.pc),
            reg::PSTATE => Ok(self.spsr),
            _ => Err("Invalid register index"),
        }
    }

    pub fn set_reg(&mut self, index: u32, value: u64) -> Result<(), &'static str> {
        match index {
            reg::X0..=reg::X30 => {
                self.iregs[index as usize] = value;
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
