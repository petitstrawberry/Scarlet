//! Guest VCPU state for Type-2 hypervisor

use crate::arch::hv::csr::{GuestCsrState, write_hstatus, write_vsatp};
use crate::arch::riscv64::fpu::{FpuContext, VectorContext};
use crate::arch::riscv64::{IntRegisters, Mode, Trapframe};
use crate::arch::vcpu::Vcpu;
use alloc::boxed::Box;

use super::csr;
use super::reg_index::reg;

#[repr(C)]
#[derive(Debug, Clone)]
pub struct GuestVcpu {
    iregs: IntRegisters,
    csrs: GuestCsrState,
    pc: u64,
    fpu: FpuContext,
    fpu_used: bool,
    vector: Option<Box<VectorContext>>,
    vector_used: bool,
    asid: usize,
    mode: Mode,
    vm_id: u32,
    vcpu_id: u32,
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
            mode: Mode::GuestKernel,
            csrs: GuestCsrState::default(),
            vm_id,
            vcpu_id,
        }
    }

    pub fn store(&mut self, vcpu: &Vcpu) {
        self.iregs = vcpu.iregs;
        #[cfg(feature = "user-fpu")]
        {
            self.fpu_used = vcpu.fpu_used;
            if vcpu.fpu_used {
                self.fpu = vcpu.fpu.clone();
            }
        }
        #[cfg(feature = "user-vector")]
        {
            self.vector_used = vcpu.vector_used;
            if vcpu.vector_used {
                self.vector = vcpu.vector.clone();
            }
        }
        self.pc = vcpu.get_pc();
        self.mode = vcpu.get_mode();
    }

    pub fn switch(&mut self, trapframe: &mut Trapframe) {
        trapframe.regs = self.iregs;
        trapframe.epc = self.pc;
    }

    pub fn save(&mut self, trapframe: &Trapframe) {
        self.iregs = trapframe.regs;
        self.pc = trapframe.epc;
        self.csrs = GuestCsrState::save();
    }

    pub fn get_mmio_data(&self, reg: u8, size: u8) -> u64 {
        if reg == 0 {
            return 0;
        }
        let val = self.get_gpr(reg as usize);
        match size {
            1 => val & 0xFF,
            2 => val & 0xFFFF,
            4 => val & 0xFFFFFFFF,
            _ => val,
        }
    }

    pub fn set_mmio_data(&mut self, reg: u8, size: u8, data: u64) {
        if reg == 0 {
            return;
        }
        let mask = match size {
            1 => 0xFF,
            2 => 0xFFFF,
            4 => 0xFFFFFFFF,
            _ => !0,
        };
        let old = self.get_gpr(reg as usize);
        let new = (old & !mask) | (data & mask);
        self.set_gpr(reg as usize, new);
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
            reg::STVEC => Ok(self.csrs.stvec),
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
        // crate::early_println!("[GuestVcpu::set_reg] index={} value={:#x}", index, value);
        match index {
            reg::PC => {
                self.pc = value;
                // crate::early_println!("[GuestVcpu::set_reg] PC set to {:#x}", self.pc);
                Ok(())
            }
            i if i < 32 => {
                self.iregs.reg[i as usize] = value as usize;
                // crate::early_println!(
                //     "[GuestVcpu::set_reg] reg[{}] = {:#x}",
                //     i,
                //     self.iregs.reg[i as usize]
                // );
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
            reg::STVEC => {
                self.csrs.stvec = value;
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

    pub fn init_csrs(&self) {
        self.csrs.restore();
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
