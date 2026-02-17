extern crate alloc;

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use spin::Mutex;

#[cfg(all(feature = "hypervisor", target_arch = "riscv64"))]
use crate::arch::hv::guest_vcpu::GuestVcpu;
#[cfg(all(feature = "hypervisor", target_arch = "riscv64"))]
use crate::arch::hv::switch::{resume_guest_loop, run_guest_loop};
#[cfg(all(feature = "hypervisor", target_arch = "riscv64"))]
use crate::arch::hv::trap::{arch_guest_trap_handler, clear_guest_mode};
#[cfg(not(target_arch = "riscv64"))]
use crate::arch::{Mode, Trapframe};
#[cfg(target_arch = "riscv64")]
use crate::arch::{get_cpu, set_arch, set_next_mode, set_trapvector};
#[cfg(all(feature = "hypervisor", target_arch = "riscv64"))]
use crate::hypervisor::memory::MemorySlot;
use crate::hypervisor::types::{InterruptType, VmExit};
use crate::object::capability::ControlOps;
use crate::task::mytask;
#[cfg(target_arch = "riscv64")]
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

#[cfg(not(target_arch = "riscv64"))]
#[derive(Debug, Clone)]
struct GuestVcpu {
    mode: Mode,
}

#[cfg(not(target_arch = "riscv64"))]
impl GuestVcpu {
    fn new(_vm_id: u32, _vcpu_id: u32) -> Self {
        Self {
            mode: Mode::GuestKernel,
        }
    }

    fn get_mode(&self) -> Mode {
        self.mode
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    fn save(&mut self, _trapframe: &Trapframe) {}

    fn get_mmio_data(&self, _reg: u8, _size: u8) -> u64 {
        0
    }

    fn set_mmio_data(&mut self, _reg: u8, _size: u8, _data: u64) {}

    fn get_reg(&self, _index: u32) -> Result<u64, &'static str> {
        Err("Guest registers are not supported on this architecture")
    }

    fn set_reg(&mut self, _index: u32, _value: u64) -> Result<(), &'static str> {
        Err("Guest registers are not supported on this architecture")
    }
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
        #[cfg(not(target_arch = "riscv64"))]
        {
            return Err("Hypervisor guest run is only supported on riscv64");
        }

        #[cfg(target_arch = "riscv64")]
        {
            crate::early_println!("[VcpuObject::run] starting");

            let vm = self.vm.upgrade().ok_or("VM no longer exists")?;

            let task = mytask().ok_or("No current task")?;
            let cpu = get_cpu();
            let cpu_id = cpu.get_cpuid();
            let arch_vaddr = crate::vm::get_trampoline_arch(cpu_id);
            set_arch(arch_vaddr);

            let (kstack_slot, kstack_base) = task
                .get_kernel_stack_window_base()
                .ok_or("Task has no kernel stack window")?;
            let kernel_sp = (kstack_base
                + crate::environment::PAGE_SIZE
                + crate::environment::TASK_KERNEL_STACK_SIZE) as u64;

            crate::early_println!(
                "[VcpuObject::run] arch_vaddr={:#x} kstack_slot={} kstack_base={:#x} kernel_sp={:#x}",
                arch_vaddr,
                kstack_slot,
                kstack_base,
                kernel_sp
            );

            cpu.set_next_address_space(crate::vm::get_kernel_vm_manager().get_asid());
            let mode = self.state.lock().guest.get_mode();
            crate::early_println!("[VcpuObject::run] guest mode={:?}", mode);
            task.vcpu.lock().set_mode(mode);
            set_next_mode(mode);
            let guest_tv = get_guest_trapvector_trampoline();
            crate::early_println!("[VcpuObject::run] guest trap vector={:#x}", guest_tv);
            set_trapvector(guest_tv);

            crate::early_println!("[VcpuObject::run] setting guest root pagetable");
            vm.set_guest_root_pagetable();

            crate::early_println!("[VcpuObject::run] calling run_guest_loop");
            {
                let state = self.state.lock();
                crate::early_println!("[VcpuObject::run] guest PC={:#x}", state.guest.get_pc());
                let guest_ptr = &state.guest as *const GuestVcpu;
                crate::early_println!("[VcpuObject::run] guest_ptr={:#x}", guest_ptr as usize);
                unsafe {
                    let pc_at_offset = core::ptr::read((guest_ptr as *const u64).add(40));
                    crate::early_println!(
                        "[VcpuObject::run] PC at offset 320 = {:#x}",
                        pc_at_offset
                    );
                }
            }
            let arch_vaddr: usize;
            unsafe {
                core::arch::asm!("csrr {0}, sscratch", out(reg) arch_vaddr);
            }
            let guest_ptr_for_run = &self.state.lock().guest as *const GuestVcpu;
            crate::early_println!(
                "[VcpuObject::run] guest_ptr_for_run={:#x}",
                guest_ptr_for_run as usize
            );
            unsafe {
                let pc_at_320 =
                    core::ptr::read((guest_ptr_for_run as *const u8).add(320) as *const u64);
                crate::early_println!("[VcpuObject::run] PC at byte 320 = {:#x}", pc_at_320);
            }
            let guest_tf_ptr: *mut crate::arch::Trapframe =
                unsafe { run_guest_loop(guest_ptr_for_run, arch_vaddr) };

            let mut trapframe_copy = unsafe { (*guest_tf_ptr).clone() };

            crate::early_println!(
                "[VcpuObject::run] returned from run_guest_loop, trapframe={:#x}",
                guest_tf_ptr as usize
            );

            loop {
                match arch_guest_trap_handler(&mut trapframe_copy, &vm) {
                    Some(exit) => {
                        crate::arch::set_trapvector(crate::vm::get_trampoline_trap_vector());
                        crate::early_println!("[VcpuObject::run] got exit: {:?}", exit);
                        clear_guest_mode();
                        self.state.lock().guest.save(&mut trapframe_copy);

                        if let VmExit::MmioWrite {
                            addr,
                            size,
                            reg,
                            data: _,
                        } = exit
                        {
                            let data = self.state.lock().guest.get_mmio_data(reg, size);
                            return Ok(VmExit::MmioWrite {
                                addr,
                                size,
                                reg,
                                data,
                            });
                        }

                        return Ok(exit);
                    }
                    None => unsafe {
                        // Write back to original location before resuming
                        core::ptr::write(guest_tf_ptr, trapframe_copy.clone());
                        let new_tf_ptr = run_guest_loop(guest_ptr_for_run, arch_vaddr);
                        trapframe_copy = (*new_tf_ptr).clone();
                    },
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
                crate::early_println!("[VcpuObject::control] GET_ONE_REG index={}", arg);
                let value = self.get_reg(arg as u32)?;
                Ok(value as i32)
            }
            vcpu_ctl::SET_ONE_REG => {
                let target_ptr = translate_user_ptr(arg)?;
                let one_reg = unsafe { core::ptr::read(target_ptr as *const VcpuOneReg) };
                crate::early_println!(
                    "[SET_ONE_REG] index={} value={:#x}",
                    one_reg.index,
                    one_reg.value
                );
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
