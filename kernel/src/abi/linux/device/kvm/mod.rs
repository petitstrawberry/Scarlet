//! Linux KVM ioctl compatibility layer
//!
//! Maps Linux KVM ioctl commands onto Scarlet's ABI-agnostic hypervisor
//! subsystem. All KVM-specific constants, struct layouts, and ioctl
//! dispatch logic live here — the hypervisor module itself knows nothing
//! about KVM.

extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use spin::{Once, RwLock};

use crate::abi::linux::generic::LinuxAbi;
use crate::device::manager::DeviceManager;
use crate::device::{Device, DeviceType};
use crate::hypervisor::memory::MemorySlotFlags;
use crate::hypervisor::types::InterruptType;
use crate::hypervisor::{VcpuRef, VmObject, VmRef};
use crate::object::KernelObject;
use crate::object::capability::selectable::{SelectWaitOutcome, Selectable};
use crate::object::capability::{ControlOps, MemoryMappingOps};
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

const fn io_write(ty: u32, nr: u32, size: u32) -> u32 {
    (1 << 30) | (size << 16) | (ty << 8) | nr
}

const fn io_read(ty: u32, nr: u32, size: u32) -> u32 {
    (2 << 30) | (size << 16) | (ty << 8) | nr
}

const fn io_read_write(ty: u32, nr: u32, size: u32) -> u32 {
    (3 << 30) | (size << 16) | (ty << 8) | nr
}

// _IO(KVMIO, nr)
pub const KVM_GET_API_VERSION: u32 = io_none(KVMIO, 0x00);
pub const KVM_CREATE_VM: u32 = io_none(KVMIO, 0x01);
pub const KVM_CHECK_EXTENSION: u32 = io_none(KVMIO, 0x03);
pub const KVM_GET_VCPU_MMAP_SIZE: u32 = io_none(KVMIO, 0x04);
pub const KVM_CREATE_VCPU: u32 = io_none(KVMIO, 0x41);
pub const KVM_RUN: u32 = io_none(KVMIO, 0x80);
pub const KVM_CREATE_IRQCHIP: u32 = io_none(KVMIO, 0x60);

// _IOW(KVMIO, 0x86, struct kvm_interrupt)
pub const KVM_INTERRUPT: u32 = io_write(KVMIO, 0x86, 4);

const KVM_INTERRUPT_SET: u32 = u32::MAX;
const KVM_INTERRUPT_UNSET: u32 = u32::MAX - 1;

// _IOW(KVMIO, nr, struct)
pub const KVM_SET_USER_MEMORY_REGION: u32 = io_write(KVMIO, 0x46, 32);
pub const KVM_IRQ_LINE: u32 = io_write(KVMIO, 0x61, 8);
pub const KVM_SET_MP_STATE: u32 = io_write(KVMIO, 0x99, 4);
pub const KVM_SET_REGS: u32 = io_write(KVMIO, 0x82, 256);
pub const KVM_SET_ONE_REG: u32 = io_write(KVMIO, 0xAC, 16);

// _IOR(KVMIO, nr, struct)
pub const KVM_GET_MP_STATE: u32 = io_read(KVMIO, 0x98, 4);
pub const KVM_GET_REGS: u32 = io_read(KVMIO, 0x81, 256);

// _IOW(KVMIO, nr, struct) — both GET/SET_ONE_REG use _IOW in Linux
pub const KVM_GET_ONE_REG: u32 = io_write(KVMIO, 0xAB, 16);

pub const KVM_MP_STATE_RUNNABLE: u32 = 0;
pub const KVM_MP_STATE_STOPPED: u32 = 1;

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
#[derive(Clone, Copy)]
pub struct KvmIrqLevel {
    pub irq: u32,
    pub level: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmMpState {
    pub mp_state: u32,
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
    pub kvm_valid_regs: u64,
    pub kvm_dirty_regs: u64,
    pub sync_regs: [u8; 2048],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union KvmRunExitData {
    pub mmio: KvmRunMmio,
    pub system_event: KvmRunSystemEvent,
    pub fail_entry: KvmRunFailEntry,
    pub riscv_sbi: KvmRunRiscvSbi,
    pub _padding: [u8; 256],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmRunRiscvSbi {
    pub extension_id: u64,
    pub function_id: u64,
    pub args: [u64; 6],
    pub ret: [u64; 2],
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
// Per-vCPU shared kvm_run page management
// ---------------------------------------------------------------------------

struct KvmRunPage {
    vaddr: usize,
    paddr: usize,
}

struct KvmRunPageEntry {
    vcpu: VcpuRef,
    page: KvmRunPage,
}

static KVM_RUN_PAGES: Once<RwLock<Vec<KvmRunPageEntry>>> = Once::new();

fn get_run_pages() -> &'static RwLock<Vec<KvmRunPageEntry>> {
    KVM_RUN_PAGES.call_once(|| RwLock::new(Vec::new()))
}

/// Allocate a shared kvm_run page for the given vCPU and register it.
pub fn register_vcpu_run_page(vcpu: &VcpuRef) -> Result<(), ()> {
    use crate::mem::page::allocate_raw_pages;
    use crate::vm::addr::virt_to_phys;

    let page = allocate_raw_pages(1);
    if page.is_null() {
        return Err(());
    }
    let vaddr = page as usize;
    let paddr = virt_to_phys(vaddr);

    get_run_pages().write().push(KvmRunPageEntry {
        vcpu: Arc::clone(vcpu),
        page: KvmRunPage { vaddr, paddr },
    });
    Ok(())
}

/// Look up the physical address of a vCPU's shared kvm_run page.
/// Returns `None` if the vCPU was not created through the KVM compat layer
/// (e.g., U-SHV vcpus created via sys_shv_vcpu_create).
pub fn get_vcpu_run_paddr(vcpu: &VcpuRef) -> Option<usize> {
    let pages = get_run_pages().read();
    for entry in pages.iter() {
        if Arc::ptr_eq(&entry.vcpu, vcpu) {
            return Some(entry.page.paddr);
        }
    }
    None
}

fn read_vcpu_run_mmio_data(vcpu: &VcpuRef) -> Option<u64> {
    let pages = get_run_pages().read();
    for entry in pages.iter() {
        if Arc::ptr_eq(&entry.vcpu, vcpu) {
            let kvm_run = unsafe { &*(entry.page.vaddr as *const KvmRun) };
            if kvm_run.exit_reason == KVM_EXIT_MMIO {
                let mmio = unsafe { &kvm_run.exit_data.mmio };
                if mmio.is_write == 0 {
                    let len = mmio.len as usize;
                    return Some(match len {
                        1 => mmio.data[0] as u64,
                        2 => u16::from_le_bytes([mmio.data[0], mmio.data[1]]) as u64,
                        4 => u32::from_le_bytes([
                            mmio.data[0],
                            mmio.data[1],
                            mmio.data[2],
                            mmio.data[3],
                        ]) as u64,
                        8 => u64::from_le_bytes([
                            mmio.data[0],
                            mmio.data[1],
                            mmio.data[2],
                            mmio.data[3],
                            mmio.data[4],
                            mmio.data[5],
                            mmio.data[6],
                            mmio.data[7],
                        ]),
                        _ => return None,
                    });
                }
            }
            return None;
        }
    }
    None
}

/// Write VmExit information into a vCPU's shared kvm_run page.
/// Does nothing if the vCPU has no registered run page (backward compat
/// with the old pointer-based KVM_RUN path).
pub fn write_vcpu_run_exit(vcpu: &VcpuRef, exit: &crate::hypervisor::VmExit) -> bool {
    let pages = get_run_pages().read();
    for entry in pages.iter() {
        if Arc::ptr_eq(&entry.vcpu, vcpu) {
            // SAFETY: vaddr points to a page-aligned, zero-initialized allocation
            // that is exclusively owned by this kvm_run page management code.
            let kvm_run = unsafe { &mut *(entry.page.vaddr as *mut KvmRun) };
            kvm_run.exit_data = KvmRunExitData {
                _padding: [0u8; 256],
            };
            write_vm_exit(kvm_run, exit, vcpu);
            return true;
        }
    }
    false
}

/// Free a vCPU's shared kvm_run page. Called when the vCPU kernel object
/// is dropped.
pub fn free_vcpu_run_page(vcpu: &VcpuRef) {
    let mut pages = get_run_pages().write();
    let idx = pages.iter().position(|e| Arc::ptr_eq(&e.vcpu, vcpu));
    if let Some(i) = idx {
        use crate::mem::page::free_raw_pages;
        let entry = pages.remove(i);
        free_raw_pages(entry.page.vaddr as *mut crate::mem::page::Page, 1);
    }
}

// ---------------------------------------------------------------------------
// System-level (/dev/kvm) ioctl dispatcher
// ---------------------------------------------------------------------------

pub fn handle_system_ioctl(
    request: u32,
    _arg: usize,
    abi: &mut LinuxAbi,
) -> Result<Option<usize>, ()> {
    match request {
        KVM_GET_API_VERSION => Ok(Some(KVM_API_VERSION)),

        KVM_CREATE_VM => {
            let task = mytask().ok_or(())?;
            let owner_mm = task.vm_manager.clone();
            let vm = crate::hypervisor::vm::GLOBAL_VM_MANAGER
                .create_vm(owner_mm)
                .map_err(|_| ())?;
            let kernel_obj = KernelObject::HypervisorVm(vm);
            let handle = task.handle_table.insert(kernel_obj).map_err(|_| ())?;
            let fd = abi.allocate_fd(handle).map_err(|_| ())?;
            Ok(Some(fd))
        }

        KVM_CHECK_EXTENSION => {
            const KVM_CAP_IRQCHIP: usize = 0;
            const KVM_CAP_NR_VCPUS: usize = 9;
            const KVM_CAP_COALESCED_MMIO: usize = 15;
            const KVM_CAP_ONE_REG: usize = 70;
            const KVM_CAP_MAX_VCPUS: usize = 66;
            match _arg {
                KVM_CAP_IRQCHIP => Ok(Some(1)),
                KVM_CAP_ONE_REG => Ok(Some(1)),
                KVM_CAP_NR_VCPUS => Ok(Some(1)),
                KVM_CAP_MAX_VCPUS => Ok(Some(1)),
                _ => match arch::check_extension(_arg) {
                    Some(val) => Ok(Some(val)),
                    None => Ok(Some(0)),
                },
            }
        }

        KVM_GET_VCPU_MMAP_SIZE => Ok(Some(core::mem::size_of::<KvmRun>())),

        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// VM-level ioctl dispatcher
// ---------------------------------------------------------------------------

pub fn handle_vm_ioctl(
    request: u32,
    arg: usize,
    vm: &VmRef,
    abi: &mut LinuxAbi,
) -> Result<Option<usize>, ()> {
    match request {
        KVM_CREATE_VCPU => {
            let vcpu_id = arg as u32;
            let task = mytask().ok_or(())?;
            let vcpu = vm.create_vcpu(vcpu_id).map_err(|_| ())?;
            register_vcpu_run_page(&vcpu).map_err(|_| ())?;
            let kernel_obj = KernelObject::HypervisorVcpu(vcpu);
            let handle = task.handle_table.insert(kernel_obj).map_err(|_| ())?;
            let fd = abi.allocate_fd(handle).map_err(|_| ())?;
            Ok(Some(fd))
        }

        KVM_SET_USER_MEMORY_REGION => {
            let task = match mytask() {
                Some(t) => t,
                None => {
                    crate::println!("[KVM] SET_MEM: no task");
                    return Err(());
                }
            };
            let kva = match task.vm_manager.translate_to_kva(arg) {
                Some(k) => k,
                None => {
                    crate::println!("[KVM] SET_MEM: translate_to_kva({:#x}) failed", arg);
                    return Err(());
                }
            };
            // SAFETY: caller guarantees arg points to a valid KvmUserspaceMemoryRegion
            let region = unsafe { &*(kva as *const KvmUserspaceMemoryRegion) };
            crate::println!(
                "[KVM] SET_MEM: slot={} gpa={:#x} size={:#x} ua={:#x} flags={}",
                region.slot,
                region.guest_phys_addr,
                region.memory_size,
                region.userspace_addr,
                region.flags
            );

            let flags = MemorySlotFlags {
                readonly: (region.flags & KVM_MEM_READONLY) != 0,
            };

            match vm.set_memory_region(
                region.slot,
                region.guest_phys_addr,
                region.memory_size,
                region.userspace_addr,
                flags,
            ) {
                Ok(()) => Ok(Some(0)),
                Err(e) => {
                    crate::println!("[KVM] SET_MEM: set_memory_region err: {}", e);
                    Err(())
                }
            }
        }

        KVM_IRQ_LINE => {
            if arg == 0 {
                return Err(());
            }

            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmIrqLevel
            let irq_level = unsafe { &*(kva as *const KvmIrqLevel) };

            if let Some(vcpu) = vm.get_vcpu(0) {
                if irq_level.level != 0 {
                    vcpu.inject_interrupt(InterruptType::External);
                } else {
                    vcpu.clear_interrupt(InterruptType::External);
                }
            }

            Ok(Some(0))
        }

        KVM_CREATE_IRQCHIP => Ok(Some(0)),

        _ => arch::handle_vm_ioctl(request, arg, vm, abi),
    }
}

// ---------------------------------------------------------------------------
// VCPU-level ioctl dispatcher
// ---------------------------------------------------------------------------

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

static MMIO_PENDING_READ_REG: AtomicU8 = AtomicU8::new(0xFF);
static MMIO_PENDING_VALID: AtomicBool = AtomicBool::new(false);

pub fn handle_vcpu_ioctl(request: u32, arg: usize, vcpu: &VcpuRef) -> Result<Option<usize>, ()> {
    match request {
        KVM_RUN => {
            if let Some(task) = mytask() {
                task.default_time_slice.store(10, Ordering::SeqCst);
            }

            if MMIO_PENDING_VALID.load(Ordering::Acquire) {
                let reg = MMIO_PENDING_READ_REG.load(Ordering::Acquire);
                if reg < 32 {
                    let val = if arg != 0 {
                        let task = mytask().ok_or(())?;
                        let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
                        // SAFETY: caller guarantees arg points to a valid KvmRun
                        let kvm_run = unsafe { &*(kva as *const KvmRun) };
                        if kvm_run.exit_reason == KVM_EXIT_MMIO {
                            let mmio = unsafe { &kvm_run.exit_data.mmio };
                            if mmio.is_write == 0 {
                                let len = mmio.len as usize;
                                Some(match len {
                                    1 => mmio.data[0] as u64,
                                    2 => u16::from_le_bytes([mmio.data[0], mmio.data[1]]) as u64,
                                    4 => u32::from_le_bytes([
                                        mmio.data[0],
                                        mmio.data[1],
                                        mmio.data[2],
                                        mmio.data[3],
                                    ]) as u64,
                                    8 => u64::from_le_bytes([
                                        mmio.data[0],
                                        mmio.data[1],
                                        mmio.data[2],
                                        mmio.data[3],
                                        mmio.data[4],
                                        mmio.data[5],
                                        mmio.data[6],
                                        mmio.data[7],
                                    ]),
                                    _ => 0,
                                })
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        read_vcpu_run_mmio_data(vcpu)
                    };
                    if let Some(val) = val {
                        use crate::arch::hv::reg_index::reg;
                        let _ = vcpu.set_reg(reg as u32, val);
                    }
                }
                MMIO_PENDING_VALID.store(false, Ordering::Release);
            }

            let mut sbi_count = 0u32;
            loop {
                let exit = vcpu.run().map_err(|_| ())?;

                if let crate::hypervisor::VmExit::FirmwareCall { .. } = &exit {
                    match arch::handle_firmware_call_in_kernel(vcpu) {
                        arch::FirmwareCallResult::Handled => {
                            sbi_count += 1;
                            continue;
                        }
                        arch::FirmwareCallResult::SystemOff
                        | arch::FirmwareCallResult::SystemReset => break,
                        arch::FirmwareCallResult::ForwardToUserspace => break,
                    }
                }

                if let crate::hypervisor::VmExit::MmioRead { reg, .. } = &exit {
                    MMIO_PENDING_READ_REG.store(*reg, Ordering::Release);
                    MMIO_PENDING_VALID.store(true, Ordering::Release);
                }

                if !write_vcpu_run_exit(vcpu, &exit) && arg != 0 {
                    let task = mytask().ok_or(())?;
                    let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
                    // SAFETY: caller guarantees arg points to a valid KvmRun
                    let kvm_run = unsafe { &mut *(kva as *mut KvmRun) };
                    write_vm_exit(kvm_run, &exit, vcpu);
                }

                break;
            }

            Ok(Some(0))
        }

        KVM_INTERRUPT => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a u32 (kvm_interrupt.irq)
            let irq = unsafe { *(kva as *const u32) };
            if irq == KVM_INTERRUPT_SET {
                vcpu.inject_interrupt(InterruptType::External);
            } else {
                vcpu.clear_interrupt(InterruptType::External);
            }
            Ok(Some(0))
        }

        KVM_GET_REGS => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmRegs
            let kvm_regs = unsafe { &mut *(kva as *mut KvmRegs) };
            *kvm_regs = arch::read_regs_to_kvm(vcpu);
            Ok(Some(0))
        }

        KVM_SET_REGS => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmRegs
            let kvm_regs = unsafe { &*(kva as *const KvmRegs) };
            arch::write_kvm_to_regs(vcpu, kvm_regs);
            Ok(Some(0))
        }

        KVM_GET_ONE_REG => {
            if arg == 0 {
                return Err(());
            }

            let task = mytask().ok_or(())?;
            let one_reg_kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmOneReg
            let one_reg = unsafe { *(one_reg_kva as *const arch::KvmOneReg) };
            let value = arch::get_one_reg(vcpu, one_reg.id)?;
            let value_kva = task
                .vm_manager
                .translate_to_kva(one_reg.addr as usize)
                .ok_or(())?;
            // SAFETY: caller guarantees one_reg.addr points to a valid u64
            unsafe { *(value_kva as *mut u64) = value };
            Ok(Some(0))
        }

        KVM_SET_ONE_REG => {
            if arg == 0 {
                return Err(());
            }

            let task = mytask().ok_or(())?;
            let one_reg_kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmOneReg
            let one_reg = unsafe { *(one_reg_kva as *const arch::KvmOneReg) };
            let value_kva = task
                .vm_manager
                .translate_to_kva(one_reg.addr as usize)
                .ok_or(())?;
            // SAFETY: caller guarantees one_reg.addr points to a valid u64
            let value = unsafe { *(value_kva as *const u64) };
            arch::set_one_reg(vcpu, one_reg.id, value)?;
            Ok(Some(0))
        }

        KVM_GET_MP_STATE => {
            if arg == 0 {
                return Err(());
            }

            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmMpState
            let mp_state = unsafe { &mut *(kva as *mut KvmMpState) };
            *mp_state = KvmMpState {
                mp_state: KVM_MP_STATE_RUNNABLE,
            };
            Ok(Some(0))
        }

        KVM_SET_MP_STATE => {
            if arg == 0 {
                return Err(());
            }

            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmMpState
            let _mp_state = unsafe { &*(kva as *const KvmMpState) };
            Ok(Some(0))
        }

        _ => arch::handle_vcpu_ioctl(request, arg, vcpu),
    }
}

// ---------------------------------------------------------------------------
// VmExit → kvm_run conversion
// ---------------------------------------------------------------------------

fn write_vm_exit(kvm_run: &mut KvmRun, exit: &crate::hypervisor::VmExit, vcpu: &VcpuRef) {
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
            arch::write_firmware_exit(kvm_run, exit, vcpu);
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
        _min_wait_ticks: u64,
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
    let _id = dm.register_device_with_name(String::from(KVM_DEVICE_NAME), dev);
}

crate::driver_initcall!(register_kvm_device);
