extern crate alloc;

use alloc::sync::Arc;
use alloc::sync::Weak;
use spin::Mutex;

#[cfg(feature = "hypervisor")]
use crate::arch::hv::guest_vcpu::GuestVcpu;
use crate::hypervisor::types::InterruptType;
use crate::object::capability::ControlOps;

pub type VcpuId = u32;

pub mod vcpu_ctl {
    pub const RUN: u32 = 0x01;
    pub const GET_ONE_REG: u32 = 0x02;
    pub const SET_ONE_REG: u32 = 0x03;
    pub const INJECT_INTERRUPT: u32 = 0x04;
    pub const CLEAR_INTERRUPT: u32 = 0x05;
}

#[repr(C)]
pub struct VcpuOneReg {
    pub index: u32,
    pub _padding: u32,
    pub value: u64,
}

pub trait VcpuObject: ControlOps + Send + Sync {
    fn id(&self) -> VcpuId;
    fn inject_interrupt(&self, irq_type: InterruptType);
    fn clear_interrupt(&self, irq_type: InterruptType);
    fn set_irq_line(&self, _irq: u32, level: bool) {
        if level {
            self.inject_interrupt(InterruptType::External);
        } else {
            self.clear_interrupt(InterruptType::External);
        }
    }
    fn get_reg(&self, index: u32) -> Result<u64, &'static str>;
    fn set_reg(&self, index: u32, value: u64) -> Result<(), &'static str>;
    fn wait_for_interrupt(&self, trapframe: &mut crate::arch::Trapframe) {
        crate::sched::scheduler::schedule(trapframe);
    }
    fn run(&self) -> Result<crate::hypervisor::VmExit, &'static str>;
}
