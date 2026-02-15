//! Hypervisor support for userspace

pub mod types;

pub use types::{MmioInfo, VcpuExit, VcpuExitReason};

use crate::syscall::{Syscall, syscall2, syscall3};

/// Create a new VM, returns handle
pub fn vm_create() -> Result<u32, ()> {
    let ret = syscall2(Syscall::HypervisorVmCreate, 0, 0);
    if ret == usize::MAX {
        Err(())
    } else {
        Ok(ret as u32)
    }
}

/// Create a vCPU on an existing VM
pub fn vcpu_create(vm_handle: u32, vcpu_id: u32) -> Result<u32, ()> {
    let ret = syscall2(
        Syscall::HypervisorVcpuCreate,
        vm_handle as usize,
        vcpu_id as usize,
    );
    if ret == usize::MAX {
        Err(())
    } else {
        Ok(ret as u32)
    }
}

/// Run a vCPU until exit
pub fn vcpu_run(vcpu_handle: u32, exit: &mut VcpuExit) -> Result<(), ()> {
    let ret = syscall3(
        Syscall::VcpuRun,
        vcpu_handle as usize,
        exit as *mut VcpuExit as usize,
        0,
    );
    if ret == usize::MAX { Err(()) } else { Ok(()) }
}

/// VM control commands
pub mod vm_ctl {
    pub const SET_MEMORY_REGION: u32 = 0x01;
    pub const GET_VCPU_COUNT: u32 = 0x02;
}

/// vCPU control commands  
pub mod vcpu_ctl {
    pub const RUN: u32 = 0x01;
    pub const GET_REGS: u32 = 0x02;
    pub const SET_REGS: u32 = 0x03;
}

/// Memory region descriptor for VM setup
#[repr(C)]
pub struct VmMemoryRegion {
    pub slot_id: u32,
    pub flags: u32,
    pub guest_phys_addr: u64,
    pub memory_size: u64,
    pub host_phys_addr: u64,
}

/// Control a VM handle via HandleControl
pub fn vm_control(vm_handle: u32, command: u32, arg: usize) -> Result<i32, ()> {
    let ret = syscall3(
        Syscall::HandleControl,
        vm_handle as usize,
        command as usize,
        arg,
    );
    if ret == usize::MAX {
        Err(())
    } else {
        Ok(ret as i32)
    }
}

/// Control a vCPU handle via HandleControl
pub fn vcpu_control(vcpu_handle: u32, command: u32, arg: usize) -> Result<i32, ()> {
    let ret = syscall3(
        Syscall::HandleControl,
        vcpu_handle as usize,
        command as usize,
        arg,
    );
    if ret == usize::MAX {
        Err(())
    } else {
        Ok(ret as i32)
    }
}
