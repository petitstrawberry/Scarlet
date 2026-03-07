//! Linux KVM ioctl compatibility layer
//!
//! Maps Linux KVM ioctl commands onto Scarlet's ABI-agnostic hypervisor
//! subsystem. All KVM-specific constants, struct layouts, and ioctl
//! dispatch logic live here — the hypervisor module itself knows nothing
//! about KVM.

extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use core::any::Any;

#[cfg(target_arch = "riscv64")]
use crate::abi::linux::riscv64::LinuxRiscv64Abi;
use crate::device::manager::DeviceManager;
use crate::device::{Device, DeviceType};
use crate::hypervisor::memory::MemorySlotFlags;
use crate::hypervisor::{VcpuRef, VmObject, VmRef};
use crate::object::capability::selectable::{SelectWaitOutcome, Selectable};
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::object::KernelObject;
use crate::task::mytask;

// ---------------------------------------------------------------------------
// Arch-specific register module (re-export pattern)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "riscv64")]
pub mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64 as arch;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64 as arch;

#[cfg(target_arch = "aarch64")]
pub use aarch64::KvmRegs;
#[cfg(target_arch = "riscv64")]
pub use riscv64::KvmRegs;

// ---------------------------------------------------------------------------
// Linux KVM ioctl numbers (include/uapi/linux/kvm.h)
// ---------------------------------------------------------------------------

const KVMIO: u32 = 0xAE;

const fn io_none(ty: u32, nr: u32) -> u32 {
    (ty << 8) | nr
}

pub const KVM_GET_API_VERSION: u32 = io_none(KVMIO, 0x00);
pub const KVM_CREATE_VM: u32 = io_none(KVMIO, 0x01);
pub const KVM_CHECK_EXTENSION: u32 = io_none(KVMIO, 0x03);
pub const KVM_GET_VCPU_MMAP_SIZE: u32 = io_none(KVMIO, 0x04);

pub const KVM_SET_USER_MEMORY_REGION: u32 = io_none(KVMIO, 0x46);
pub const KVM_CREATE_VCPU: u32 = io_none(KVMIO, 0x41);

pub const KVM_RUN: u32 = io_none(KVMIO, 0x80);
pub const KVM_GET_REGS: u32 = io_none(KVMIO, 0x81);
pub const KVM_SET_REGS: u32 = io_none(KVMIO, 0x82);
pub const KVM_GET_ONE_REG: u32 = io_none(KVMIO, 0xAB);
pub const KVM_SET_ONE_REG: u32 = io_none(KVMIO, 0xAC);

const KVM_API_VERSION: usize = 12;

// ---------------------------------------------------------------------------
// KVM exit reasons (include/uapi/linux/kvm.h)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub const KVM_EXIT_UNKNOWN: u32 = 0;
#[allow(dead_code)]
pub const KVM_EXIT_DEBUG: u32 = 4;
#[allow(dead_code)]
pub const KVM_EXIT_HLT: u32 = 5;
pub const KVM_EXIT_MMIO: u32 = 6;
pub const KVM_EXIT_SHUTDOWN: u32 = 8;
pub const KVM_EXIT_FAIL_ENTRY: u32 = 9;
pub const KVM_EXIT_INTERNAL_ERROR: u32 = 17;
pub const KVM_EXIT_SYSTEM_EVENT: u32 = 24;
#[allow(dead_code)]
pub const KVM_EXIT_RISCV_SBI: u32 = 35;

pub const KVM_MEM_READONLY: u32 = 1 << 1;

// ---------------------------------------------------------------------------
// Linux KVM userspace struct layouts (C-compatible repr)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmUserspaceMemoryRegion {
    pub slot: u32,
    pub flags: u32,
    pub guest_phys_addr: u64,
    pub memory_size: u64,
    pub userspace_addr: u64,
}

#[repr(C)]
pub struct KvmRun {
    pub request_interrupt_window: u8,
    pub immediate_exit: u8,
    pub _padding1: [u8; 6],
    pub exit_reason: u32,
    pub ready_for_interrupt_injection: u8,
    pub if_flag: u8,
    pub flags: u16,
    pub cr8: u64,
    pub apic_base: u64,
    pub exit_data: KvmRunExitData,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union KvmRunExitData {
    pub mmio: KvmRunMmio,
    pub system_event: KvmRunSystemEvent,
    pub fail_entry: KvmRunFailEntry,
    pub _padding: [u8; 256],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmRunMmio {
    pub phys_addr: u64,
    pub data: [u8; 8],
    pub len: u32,
    pub is_write: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmRunSystemEvent {
    pub event_type: u32,
    pub ndata: u32,
    pub data: [u64; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmRunFailEntry {
    pub hardware_entry_failure_reason: u64,
}

// ---------------------------------------------------------------------------
// System-level (/dev/kvm) ioctl dispatcher
// ---------------------------------------------------------------------------

#[cfg(target_arch = "riscv64")]
pub fn handle_system_ioctl(
    request: u32,
    _arg: usize,
    abi: &mut LinuxRiscv64Abi,
) -> Result<Option<usize>, ()> {
    match request {
        KVM_GET_API_VERSION => Ok(Some(KVM_API_VERSION)),

        KVM_CREATE_VM => {
            let task = mytask().ok_or(())?;
            let vm = crate::hypervisor::vm::GLOBAL_VM_MANAGER
                .create_vm()
                .map_err(|_| ())?;
            let kernel_obj = KernelObject::HypervisorVm(vm);
            let handle = task.handle_table.insert(kernel_obj).map_err(|_| ())?;
            let fd = abi.allocate_fd(handle).map_err(|_| ())?;
            Ok(Some(fd))
        }

        KVM_CHECK_EXTENSION => Ok(Some(0)),

        KVM_GET_VCPU_MMAP_SIZE => Ok(Some(core::mem::size_of::<KvmRun>())),

        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// VM-level ioctl dispatcher
// ---------------------------------------------------------------------------

#[cfg(target_arch = "riscv64")]
pub fn handle_vm_ioctl(
    request: u32,
    arg: usize,
    vm: &VmRef,
    abi: &mut LinuxRiscv64Abi,
) -> Result<Option<usize>, ()> {
    match request {
        KVM_CREATE_VCPU => {
            let vcpu_id = arg as u32;
            let task = mytask().ok_or(())?;
            let vcpu = vm.create_vcpu(vcpu_id).map_err(|_| ())?;
            let kernel_obj = KernelObject::HypervisorVcpu(vcpu);
            let handle = task.handle_table.insert(kernel_obj).map_err(|_| ())?;
            let fd = abi.allocate_fd(handle).map_err(|_| ())?;
            Ok(Some(fd))
        }

        KVM_SET_USER_MEMORY_REGION => {
            let task = mytask().ok_or(())?;
            let paddr = task.vm_manager.translate_to_phys(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmUserspaceMemoryRegion
            let region = unsafe { &*(paddr as *const KvmUserspaceMemoryRegion) };

            let flags = MemorySlotFlags {
                readonly: (region.flags & KVM_MEM_READONLY) != 0,
            };

            let host_phys = task
                .vm_manager.translate_to_phys(region.userspace_addr as usize)
                .ok_or(())? as u64;

            vm.set_memory_region(
                region.slot,
                region.guest_phys_addr,
                region.memory_size,
                host_phys,
                flags,
            )
            .map_err(|_| ())?;

            Ok(Some(0))
        }

        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// VCPU-level ioctl dispatcher
// ---------------------------------------------------------------------------

pub fn handle_vcpu_ioctl(request: u32, arg: usize, vcpu: &VcpuRef) -> Result<Option<usize>, ()> {
    match request {
        KVM_RUN => {
            let exit = vcpu.run().map_err(|_| ())?;

            if arg != 0 {
                let task = mytask().ok_or(())?;
                let paddr = task.vm_manager.translate_to_phys(arg).ok_or(())?;
                // SAFETY: caller guarantees arg points to a valid KvmRun
                let kvm_run = unsafe { &mut *(paddr as *mut KvmRun) };
                write_vm_exit(kvm_run, &exit);
            }

            Ok(Some(0))
        }

        KVM_GET_REGS => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let paddr = task.vm_manager.translate_to_phys(arg).ok_or(())?;
            let kvm_regs = unsafe { &mut *(paddr as *mut KvmRegs) };
            *kvm_regs = arch::read_regs_to_kvm(vcpu);
            Ok(Some(0))
        }

        KVM_SET_REGS => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let paddr = task.vm_manager.translate_to_phys(arg).ok_or(())?;
            let kvm_regs = unsafe { &*(paddr as *const KvmRegs) };
            arch::write_kvm_to_regs(vcpu, kvm_regs);
            Ok(Some(0))
        }

        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// VmExit → kvm_run conversion
// ---------------------------------------------------------------------------

fn write_vm_exit(kvm_run: &mut KvmRun, exit: &crate::hypervisor::VmExit) {
    use crate::hypervisor::VmExit;

    kvm_run.exit_data = KvmRunExitData {
        _padding: [0u8; 256],
    };

    match exit {
        VmExit::MmioRead {
            epc: _,
            addr,
            size,
            reg: _,
        } => {
            kvm_run.exit_reason = KVM_EXIT_MMIO;
            let mmio = unsafe { &mut kvm_run.exit_data.mmio };
            mmio.phys_addr = *addr;
            mmio.len = *size as u32;
            mmio.is_write = 0;
        }
        VmExit::MmioWrite {
            epc: _,
            addr,
            size,
            reg: _,
            data,
        } => {
            kvm_run.exit_reason = KVM_EXIT_MMIO;
            let mmio = unsafe { &mut kvm_run.exit_data.mmio };
            mmio.phys_addr = *addr;
            mmio.len = *size as u32;
            mmio.is_write = 1;
            mmio.data[..core::mem::size_of::<u64>()].copy_from_slice(&data.to_le_bytes());
        }
        VmExit::Hlt => {
            kvm_run.exit_reason = KVM_EXIT_HLT;
        }
        VmExit::FirmwareCall { epc: _ } => {
            kvm_run.exit_reason = KVM_EXIT_RISCV_SBI;
        }
        VmExit::Shutdown => {
            kvm_run.exit_reason = KVM_EXIT_SHUTDOWN;
        }
        VmExit::FailEntry {
            hardware_entry_failure_reason,
        } => {
            kvm_run.exit_reason = KVM_EXIT_FAIL_ENTRY;
            let fe = unsafe { &mut kvm_run.exit_data.fail_entry };
            fe.hardware_entry_failure_reason = *hardware_entry_failure_reason;
        }
        VmExit::InternalError => {
            kvm_run.exit_reason = KVM_EXIT_INTERNAL_ERROR;
        }
        VmExit::Unknown(_) => {
            kvm_run.exit_reason = KVM_EXIT_UNKNOWN;
        }
        VmExit::VirtualInstruction {
            epc: _,
            inst: _,
            inst_len: _,
        } => {
            kvm_run.exit_reason = KVM_EXIT_RISCV_SBI;
        }
        VmExit::IllegalInstruction {
            epc: _,
            inst: _,
            inst_len: _,
        } => {
            kvm_run.exit_reason = KVM_EXIT_INTERNAL_ERROR;
        }
        VmExit::Breakpoint { epc: _ } => {
            kvm_run.exit_reason = KVM_EXIT_DEBUG;
        }
    }
}

// ---------------------------------------------------------------------------
// /dev/kvm device (registered via driver_initcall)
// ---------------------------------------------------------------------------

/// Name used to identify the KVM system device in DeviceManager / DevFS.
pub const KVM_DEVICE_NAME: &str = "kvm";

struct KvmSystemDevice;

impl Device for KvmSystemDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        KVM_DEVICE_NAME
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Selectable for KvmSystemDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}

impl ControlOps for KvmSystemDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Use ioctl dispatch, not ControlOps")
    }
}

impl MemoryMappingOps for KvmSystemDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by KVM system device")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

fn register_kvm_device() {
    let dm = DeviceManager::get_manager();
    let dev: Arc<dyn Device> = Arc::new(KvmSystemDevice);
    let id = dm.register_device_with_name(String::from(KVM_DEVICE_NAME), dev);
    crate::early_println!(
        "KVM device registered as '{}' with ID: {}",
        KVM_DEVICE_NAME,
        id
    );
}

crate::driver_initcall!(register_kvm_device);
