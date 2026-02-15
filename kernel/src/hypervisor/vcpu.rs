extern crate alloc;

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::hv::guest_vcpu::{self, GuestVcpu};
use crate::arch::hv::switch::run_guest_loop;
use crate::arch::{get_cpu, set_next_mode, set_trapvector};
use crate::hypervisor::memory::MemorySlot;
use crate::hypervisor::trap::{AccessType, TrapType, VmTrapInfo};
use crate::hypervisor::types::{InterruptType, VmExit};
use crate::object::capability::ControlOps;
use crate::task::mytask;
use crate::vm::get_trampoline_trap_vector;

pub type VcpuId = u32;

pub mod vcpu_ctl {
    pub const RUN: u32 = 0x01;
    pub const GET_ONE_REG: u32 = 0x02;
    pub const SET_ONE_REG: u32 = 0x03;
    pub const INJECT_INTERRUPT: u32 = 0x04;
}

#[repr(C)]
pub struct VcpuOneReg {
    pub index: u32,
    pub _padding: u32,
    pub value: u64,
}

struct VcpuState {
    guest: GuestVcpu,
    pending_timer_irq: bool,
    pending_external_irq: bool,
}

pub struct VcpuObject {
    id: VcpuId,
    state: Mutex<VcpuState>,
    vm: Weak<super::vm::VmObject>,
}

impl VcpuObject {
    pub fn new(id: VcpuId, vm: Weak<super::vm::VmObject>) -> Result<Arc<Self>, &'static str> {
        Ok(Arc::new(Self {
            id,
            state: Mutex::new(VcpuState {
                guest: GuestVcpu::new(0, id),
                pending_timer_irq: false,
                pending_external_irq: false,
            }),
            vm,
        }))
    }

    pub fn id(&self) -> VcpuId {
        self.id
    }

    pub fn inject_interrupt(&self, irq_type: InterruptType) {
        let mut state = self.state.lock();
        match irq_type {
            InterruptType::Timer => state.pending_timer_irq = true,
            InterruptType::External => state.pending_external_irq = true,
        }
    }

    pub fn run(&self) -> Result<VmExit, &'static str> {
        let _vm = self.vm.upgrade().ok_or("VM no longer exists")?;

        let task = mytask().ok_or("No current task")?;
        let mode = self.state.lock().guest.get_mode();
        task.vcpu.lock().set_mode(mode);
        // Set up next mode and trap vector for guest execution
        set_next_mode(mode);
        set_trapvector(get_trampoline_trap_vector());

        // Run the guest loop, which will return on VM exit
        let arch = get_cpu();
        unsafe {
            run_guest_loop(
                &self.state.lock().guest as *const GuestVcpu,
                arch as *const _ as *mut u8,
            )
        };

        // After returning from run_guest_loop, we can read the exit reason from the guest state
        let exit_reason = self
            .state
            .lock()
            .guest
            .get_exit_reason()
            .ok_or("Failed to get exit reason")?;
        Ok(exit_reason)
    }

    pub fn get_reg(&self, index: u32) -> Result<u64, &'static str> {
        self.state.lock().guest.get_reg(index)
    }

    pub fn set_reg(&self, index: u32, value: u64) -> Result<(), &'static str> {
        self.state.lock().guest.set_reg(index, value)
    }
}

fn translate_user_ptr(arg: usize) -> Result<usize, &'static str> {
    if arg == 0 {
        return Err("Invalid argument pointer");
    }
    mytask()
        .ok_or("No current task")?
        .vm_manager
        .translate_vaddr(arg)
        .ok_or("Invalid user pointer")
}

impl ControlOps for VcpuObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            vcpu_ctl::RUN => Err("Use sys_vcpu_run"),
            vcpu_ctl::GET_ONE_REG => {
                let value = self.get_reg(arg as u32)?;
                Ok(value as i32)
            }
            vcpu_ctl::SET_ONE_REG => {
                let target_ptr = translate_user_ptr(arg)?;
                let one_reg = unsafe { core::ptr::read(target_ptr as *const VcpuOneReg) };
                self.set_reg(one_reg.index, one_reg.value)?;
                Ok(0)
            }
            _ => Err("Unsupported vCPU control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![
            (vcpu_ctl::RUN, "Run vCPU"),
            (vcpu_ctl::GET_ONE_REG, "Get one register"),
            (vcpu_ctl::SET_ONE_REG, "Set one register"),
        ]
    }
}
