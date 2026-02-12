//! Virtual CPU management

extern crate alloc;

use alloc::sync::Weak;
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::hv::{ArchVcpu, ArchVm, GuestRegisters};
use crate::hypervisor::exit::VmExit;
use crate::object::capability::ControlOps;
use crate::task::mytask;

pub type VcpuId = u32;

/// Scarlet Native vCPU control commands (via HandleControl)
pub mod vcpu_ctl {
    pub const SCTL_VCPU_RUN: u32 = 0x01;
    pub const SCTL_VCPU_GET_REGS: u32 = 0x02;
    pub const SCTL_VCPU_SET_REGS: u32 = 0x03;
    pub const SCTL_VCPU_GET_ONE_REG: u32 = 0x04;
    pub const SCTL_VCPU_SET_ONE_REG: u32 = 0x05;
}

/// Exit reason codes returned to userspace in `ScarletVcpuRunResult`
pub mod exit_reason {
    pub const EXIT_MMIO_READ: u32 = 1;
    pub const EXIT_MMIO_WRITE: u32 = 2;
    pub const EXIT_HLT: u32 = 3;
    pub const EXIT_SHUTDOWN: u32 = 4;
    pub const EXIT_SYSTEM_EVENT: u32 = 5;
    pub const EXIT_FAIL_ENTRY: u32 = 6;
    pub const EXIT_INTERNAL_ERROR: u32 = 7;
    pub const EXIT_UNKNOWN: u32 = 0xFF;
}

/// Userspace-facing vCPU run result (C ABI)
#[repr(C)]
pub struct ScarletVcpuRunResult {
    pub exit_reason: u32,
    pub _padding: u32,
    pub mmio_addr: u64,
    pub mmio_size: u8,
    pub mmio_is_write: u8,
    pub _pad2: [u8; 6],
    pub mmio_data: u64,
    pub system_event_type: u64,
    pub fail_reason: u64,
}

/// Register index 32 refers to the program counter
pub const REG_INDEX_PC: usize = 32;

/// Max GPR index (0..31 are GPRs, 32 is PC)
const MAX_REG_INDEX: usize = 32;

/// Userspace-facing register set (C ABI): gprs\[0..31\] + pc
#[repr(C)]
pub struct ScarletVcpuRegisters {
    pub gprs: [u64; 32],
    pub pc: u64,
}

/// Userspace-facing single register write descriptor (C ABI)
#[repr(C)]
pub struct ScarletVcpuOneReg {
    pub index: u32,
    pub _padding: u32,
    pub value: u64,
}

struct VcpuState {
    arch: ArchVcpu,
}

/// Virtual CPU with internal mutability.
///
/// Mutable arch state is behind a `Mutex<VcpuState>`.
pub struct VcpuObject {
    id: VcpuId,
    state: Mutex<VcpuState>,
    _vm: Weak<super::vm::VmObject>,
}

impl VcpuObject {
    pub(crate) fn new(
        id: VcpuId,
        vm: Weak<super::vm::VmObject>,
        arch_vm: &ArchVm,
    ) -> Result<Self, &'static str> {
        let arch = ArchVcpu::new(arch_vm)?;
        Ok(Self {
            id,
            state: Mutex::new(VcpuState { arch }),
            _vm: vm,
        })
    }

    pub fn id(&self) -> VcpuId {
        self.id
    }

    pub fn run(&self) -> Result<VmExit, &'static str> {
        self.state.lock().arch.run()
    }

    pub fn get_regs(&self) -> GuestRegisters {
        self.state.lock().arch.get_regs()
    }

    pub fn set_regs(&self, regs: &GuestRegisters) {
        self.state.lock().arch.set_regs(regs);
    }

    pub fn get_pc(&self) -> u64 {
        self.state.lock().arch.get_pc()
    }

    pub fn set_pc(&self, pc: u64) {
        self.state.lock().arch.set_pc(pc);
    }

    pub fn get_gpr(&self, index: usize) -> u64 {
        self.state.lock().arch.get_gpr(index)
    }

    pub fn set_gpr(&self, index: usize, value: u64) {
        self.state.lock().arch.set_gpr(index, value);
    }
}

fn translate_user_ptr(arg: usize) -> Result<usize, &'static str> {
    if arg == 0 {
        return Err("Invalid argument pointer");
    }
    if let Some(current_task) = mytask() {
        current_task
            .vm_manager
            .translate_vaddr(arg)
            .ok_or("Invalid user pointer")
    } else {
        Ok(arg)
    }
}

fn fill_run_result(result: &mut ScarletVcpuRunResult, exit: &VmExit) {
    use exit_reason::*;

    result._padding = 0;
    result.mmio_addr = 0;
    result.mmio_size = 0;
    result.mmio_is_write = 0;
    result._pad2 = [0; 6];
    result.mmio_data = 0;
    result.system_event_type = 0;
    result.fail_reason = 0;

    match exit {
        VmExit::MmioRead { addr, size } => {
            result.exit_reason = EXIT_MMIO_READ;
            result.mmio_addr = *addr;
            result.mmio_size = *size;
            result.mmio_is_write = 0;
        }
        VmExit::MmioWrite { addr, size, data } => {
            result.exit_reason = EXIT_MMIO_WRITE;
            result.mmio_addr = *addr;
            result.mmio_size = *size;
            result.mmio_is_write = 1;
            result.mmio_data = *data;
        }
        VmExit::Hlt => {
            result.exit_reason = EXIT_HLT;
        }
        VmExit::Shutdown => {
            result.exit_reason = EXIT_SHUTDOWN;
        }
        VmExit::SystemEvent { event_type } => {
            result.exit_reason = EXIT_SYSTEM_EVENT;
            result.system_event_type = *event_type;
        }
        VmExit::FailEntry {
            hardware_entry_failure_reason,
        } => {
            result.exit_reason = EXIT_FAIL_ENTRY;
            result.fail_reason = *hardware_entry_failure_reason;
        }
        VmExit::InternalError => {
            result.exit_reason = EXIT_INTERNAL_ERROR;
        }
        VmExit::Unknown(code) => {
            result.exit_reason = EXIT_UNKNOWN;
            result.fail_reason = *code;
        }
    }
}

impl ControlOps for VcpuObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        use vcpu_ctl::*;

        match command {
            SCTL_VCPU_RUN => {
                let target_ptr = translate_user_ptr(arg)?;
                let exit = self.run()?;

                // SAFETY: pointer was translated from a valid user mapping
                unsafe {
                    let result_ptr = target_ptr as *mut ScarletVcpuRunResult;
                    fill_run_result(&mut *result_ptr, &exit);
                }
                Ok(0)
            }
            SCTL_VCPU_GET_REGS => {
                let target_ptr = translate_user_ptr(arg)?;
                let regs = self.get_regs();

                // SAFETY: pointer was translated from a valid user mapping
                unsafe {
                    let user_regs = target_ptr as *mut ScarletVcpuRegisters;
                    for i in 0..32 {
                        (*user_regs).gprs[i] = regs.get_gpr(i);
                    }
                    (*user_regs).pc = self.get_pc();
                }
                Ok(0)
            }
            SCTL_VCPU_SET_REGS => {
                let target_ptr = translate_user_ptr(arg)?;

                // SAFETY: pointer was translated from a valid user mapping
                let user_regs =
                    unsafe { core::ptr::read(target_ptr as *const ScarletVcpuRegisters) };

                let mut regs = GuestRegisters::new();
                for i in 0..32 {
                    regs.set_gpr(i, user_regs.gprs[i]);
                }
                self.set_regs(&regs);
                self.set_pc(user_regs.pc);
                Ok(0)
            }
            SCTL_VCPU_GET_ONE_REG => {
                let index = arg;
                if index == REG_INDEX_PC {
                    Ok(self.get_pc() as i32)
                } else if index <= MAX_REG_INDEX {
                    Ok(self.get_gpr(index) as i32)
                } else {
                    Err("Invalid register index")
                }
            }
            SCTL_VCPU_SET_ONE_REG => {
                let target_ptr = translate_user_ptr(arg)?;

                // SAFETY: pointer was translated from a valid user mapping
                let one_reg = unsafe { core::ptr::read(target_ptr as *const ScarletVcpuOneReg) };

                let index = one_reg.index as usize;
                if index == REG_INDEX_PC {
                    self.set_pc(one_reg.value);
                    Ok(0)
                } else if index <= MAX_REG_INDEX {
                    self.set_gpr(index, one_reg.value);
                    Ok(0)
                } else {
                    Err("Invalid register index")
                }
            }
            _ => Err("Unsupported vCPU control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        use vcpu_ctl::*;
        alloc::vec![
            (SCTL_VCPU_RUN, "Run vCPU"),
            (SCTL_VCPU_GET_REGS, "Get all registers"),
            (SCTL_VCPU_SET_REGS, "Set all registers"),
            (SCTL_VCPU_GET_ONE_REG, "Get single register"),
            (SCTL_VCPU_SET_ONE_REG, "Set single register"),
        ]
    }
}
