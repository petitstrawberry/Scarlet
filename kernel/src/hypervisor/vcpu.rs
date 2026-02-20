extern crate alloc;

use alloc::sync::{Arc, Weak};
use alloc::task;
use alloc::vec::Vec;
use spin::Mutex;

#[cfg(feature = "hypervisor")]
use crate::arch::hv::guest_vcpu::GuestVcpu;
#[cfg(feature = "hypervisor")]
use crate::arch::hv::switch::arch_run_guest_loop;
#[cfg(feature = "hypervisor")]
use crate::arch::hv::trap::{arch_guest_trap_handler, clear_guest_mode};
use crate::arch::{Arch, Trapframe};
use crate::arch::{Mode, set_next_mode, set_trapvector};
#[cfg(feature = "hypervisor")]
use crate::hypervisor::memory::MemorySlot;
use crate::hypervisor::types::{InterruptType, VmExit};
use crate::object::capability::ControlOps;
use crate::task::mytask;
use crate::vm::{get_guest_trapvector_trampoline, get_trampoline_trap_vector};

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
    pending_software_irq: bool,
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
                pending_software_irq: false,
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
            InterruptType::Software => state.pending_software_irq = true,
            InterruptType::Timer => state.pending_timer_irq = true,
            InterruptType::External => state.pending_external_irq = true,
        }
    }

    pub fn run(&self) -> Result<VmExit, &'static str> {
        let vm = self.vm.upgrade().ok_or("VM no longer exists")?;
        let mut vcpu = self.state.lock();

        let arch = crate::arch::get_cpu();
        let task = mytask().ok_or("No current task")?;

        let mut guest_tf = Trapframe::new();

        setup_for_guest(task, &vcpu, &vm);
        unsafe { arch_run_guest_loop(&mut guest_tf, &vcpu.guest, arch) };

        loop {
            match arch_guest_trap_handler(&mut guest_tf, &vm) {
                Some(exit) => {
                    prepare_normal_task_and_save_guest(task, &mut vcpu, &mut guest_tf);

                    if let VmExit::MmioWrite {
                        epc,
                        addr,
                        size,
                        reg,
                        data: _,
                    } = exit
                    {
                        let data = vcpu.guest.get_mmio_data(reg, size);
                        return Ok(VmExit::MmioWrite {
                            epc,
                            addr,
                            size,
                            reg,
                            data,
                        });
                    }

                    return Ok(exit);
                }
                None => {
                    setup_for_guest(task, &vcpu, &vm);
                    unsafe { arch_run_guest_loop(&mut guest_tf, &vcpu.guest, arch) };
                }
            }
        }
    }

    pub fn complete_mmio_read(&self, reg: u8, size: u8, data: u64) {
        self.state.lock().guest.set_mmio_data(reg, size, data);
    }

    pub fn get_reg(&self, index: u32) -> Result<u64, &'static str> {
        self.state.lock().guest.get_reg(index)
    }

    pub fn set_reg(&self, index: u32, value: u64) -> Result<(), &'static str> {
        self.state.lock().guest.set_reg(index, value)
    }
}

fn setup_for_guest(task: &crate::task::Task, vcpu: &VcpuState, vm: &super::vm::VmObject) {
    let mode = vcpu.guest.get_mode();
    set_next_mode(mode);

    let guest_tv = get_guest_trapvector_trampoline();
    set_trapvector(guest_tv);
    task.vcpu.lock().set_mode(mode);

    vm.set_guest_root_pagetable();
}

fn prepare_normal_task_and_save_guest(
    task: &crate::task::Task,
    vcpu: &mut VcpuState,
    guest_tf: &Trapframe,
) {
    let mut task_vcpu = task.vcpu.lock();
    vcpu.guest.save(guest_tf);
    task_vcpu.set_mode(crate::arch::Mode::User);
    set_next_mode(task_vcpu.get_mode());
    set_trapvector(crate::vm::get_trampoline_trap_vector());
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
            vcpu_ctl::RUN => Err("Use sys_shv_vcpu_run"),
            vcpu_ctl::GET_ONE_REG => {
                // crate::early_println!("[VcpuObject::control] GET_ONE_REG index={}", arg);
                let value = self.get_reg(arg as u32)?;
                Ok(value as i32)
            }
            vcpu_ctl::SET_ONE_REG => {
                let target_ptr = translate_user_ptr(arg)?;
                let one_reg = unsafe { core::ptr::read(target_ptr as *const VcpuOneReg) };
                // crate::early_println!(
                //     "[SET_ONE_REG] index={} value={:#x}",
                //     one_reg.index,
                //     one_reg.value
                // );
                self.set_reg(one_reg.index, one_reg.value)?;
                Ok(0)
            }
            vcpu_ctl::INJECT_INTERRUPT => {
                let irq_type = match arg {
                    0 => InterruptType::Software,
                    1 => InterruptType::Timer,
                    2 => InterruptType::External,
                    _ => return Err("Invalid interrupt type"),
                };
                self.inject_interrupt(irq_type);
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
            (vcpu_ctl::INJECT_INTERRUPT, "Inject interrupt"),
        ]
    }
}
