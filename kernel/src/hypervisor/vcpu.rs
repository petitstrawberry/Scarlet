extern crate alloc;

use alloc::sync::Arc;
use alloc::sync::Weak;
use core::sync::atomic::Ordering;
use crate::sync::Mutex;

#[cfg(feature = "hypervisor")]
use crate::arch::hv::guest_vcpu::GuestVcpu;
use crate::hypervisor::types::InterruptType;
use crate::object::capability::ControlOps;

pub type VcpuId = u32;

const HV_VCPU_TIME_SLICE: u32 = 10;

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

pub fn mark_current_task_running_hv_vcpu() {
    let cpu_id = crate::arch::get_cpu().get_cpuid();
    if let Some(task) = crate::sched::scheduler::current_task(cpu_id) {
        task.default_time_slice
            .store(HV_VCPU_TIME_SLICE, Ordering::SeqCst);
        if task.time_slice.load(Ordering::SeqCst) < HV_VCPU_TIME_SLICE {
            task.time_slice.store(HV_VCPU_TIME_SLICE, Ordering::SeqCst);
        }
    }
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
    fn trigger_irq(&self, irq: u32) {
        self.set_irq_line(irq, true);
    }
    fn get_reg(&self, index: usize) -> Result<u64, &'static str>;
    fn set_reg(&self, index: usize, value: u64) -> Result<(), &'static str>;
    fn set_virtual_timer_next_event(&self, _next_event: u64) -> Result<(), &'static str> {
        Err("Virtual timer is not supported")
    }
    fn wait_for_interrupt(&self, trapframe: &mut crate::arch::Trapframe) {
        crate::sched::scheduler::schedule(trapframe);
    }
    fn run(&self) -> Result<crate::hypervisor::VmExit, &'static str>;
}
