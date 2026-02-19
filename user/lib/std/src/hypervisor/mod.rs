//! Hypervisor support for userspace

pub mod types;

pub use types::{MmioInfo, VcpuExit, VcpuExitReason};

use crate::syscall::{Syscall, syscall2, syscall3};

pub fn vm_create() -> Result<u32, ()> {
    let ret = syscall2(Syscall::ShvVmCreate, 0, 0);
    if ret == usize::MAX {
        Err(())
    } else {
        Ok(ret as u32)
    }
}

pub fn vcpu_create(vm_handle: u32, vcpu_id: u32) -> Result<u32, ()> {
    let ret = syscall2(Syscall::ShvVcpuCreate, vm_handle as usize, vcpu_id as usize);
    if ret == usize::MAX {
        Err(())
    } else {
        Ok(ret as u32)
    }
}

pub fn vcpu_run(vcpu_handle: u32, exit: &mut VcpuExit) -> Result<(), ()> {
    let ret = syscall2(
        Syscall::ShvVcpuRun,
        vcpu_handle as usize,
        exit as *mut VcpuExit as usize,
    );
    if ret == usize::MAX { Err(()) } else { Ok(()) }
}

pub mod vm_ctl {
    pub const SET_MEMORY_REGION: u32 = 0x01;
    pub const GET_VCPU_COUNT: u32 = 0x02;
}

pub mod vcpu_ctl {
    pub const RUN: u32 = 0x01;
    pub const GET_ONE_REG: u32 = 0x02;
    pub const SET_ONE_REG: u32 = 0x03;
}

pub mod reg {
    pub const A0: u32 = 10;
    pub const A1: u32 = 11;
    pub const A2: u32 = 12;
    pub const A3: u32 = 13;
    pub const A4: u32 = 14;
    pub const A5: u32 = 15;
    pub const A6: u32 = 16;
    pub const A7: u32 = 17;
    pub const PC: u32 = 32;
}

#[repr(C)]
pub struct VmMemoryRegion {
    pub slot_id: u32,
    pub flags: u32,
    pub guest_phys_addr: u64,
    pub memory_size: u64,
    pub host_phys_addr: u64,
}

#[repr(C)]
pub struct VcpuOneReg {
    pub index: u32,
    pub _padding: u32,
    pub value: u64,
}

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

pub struct Vm {
    handle: u32,
}

impl Vm {
    pub fn create() -> Result<Self, ()> {
        let handle = vm_create()?;
        Ok(Self { handle })
    }

    pub fn create_vcpu(&self, vcpu_id: u32) -> Result<Vcpu, ()> {
        let handle = vcpu_create(self.handle, vcpu_id)?;
        Ok(Vcpu {
            handle,
            vm_handle: self.handle,
        })
    }

    pub fn add_memory_region(
        &self,
        slot_id: u32,
        guest_phys_addr: u64,
        size: u64,
        host_addr: u64,
    ) -> Result<(), ()> {
        let region = VmMemoryRegion {
            slot_id,
            flags: 0,
            guest_phys_addr,
            memory_size: size,
            host_phys_addr: host_addr,
        };
        vm_control(
            self.handle,
            vm_ctl::SET_MEMORY_REGION,
            &region as *const VmMemoryRegion as usize,
        )?;
        Ok(())
    }

    pub fn handle(&self) -> u32 {
        self.handle
    }
}

impl Drop for Vm {
    fn drop(&mut self) {
        // Handle will be closed by kernel when all references are dropped
    }
}

pub struct Vcpu {
    handle: u32,
    vm_handle: u32,
}

impl Vcpu {
    pub fn run(&mut self) -> Result<VcpuExit, ()> {
        let mut exit = VcpuExit::default();
        vcpu_run(self.handle, &mut exit)?;
        Ok(exit)
    }

    pub fn get_reg(&mut self, index: u32) -> Result<u64, ()> {
        let result = vcpu_control(self.handle, vcpu_ctl::GET_ONE_REG, index as usize)?;
        Ok(result as u64)
    }

    pub fn set_reg(&mut self, index: u32, value: u64) -> Result<(), ()> {
        let one_reg = VcpuOneReg {
            index,
            _padding: 0,
            value,
        };
        vcpu_control(
            self.handle,
            vcpu_ctl::SET_ONE_REG,
            &one_reg as *const VcpuOneReg as usize,
        )?;
        Ok(())
    }

    pub fn handle(&self) -> u32 {
        self.handle
    }

    pub fn vm_handle(&self) -> u32 {
        self.vm_handle
    }
}

impl Drop for Vcpu {
    fn drop(&mut self) {
        // Handle will be closed by kernel
    }
}
