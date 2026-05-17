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
use crate::fs::{FileMetadata, FilePermission, FileType};
use crate::hypervisor::memory::MemorySlotFlags;
use crate::hypervisor::types::InterruptType;
use crate::hypervisor::vm::VmObject;
use crate::hypervisor::{VcpuRef, VmRef};
use crate::ipc::counter::{CounterObject, CounterWriteListener};
use crate::object::KernelObject;
use crate::object::capability::file::{FileObject, SeekFrom};
use crate::object::capability::selectable::{ReadyInterest, SelectWaitOutcome, Selectable};
use crate::object::capability::stream::{StreamError, StreamOps};
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
const KVM_CREATE_DEVICE: u32 =
    io_read_write(KVMIO, 0xe0, core::mem::size_of::<KvmCreateDevice>() as u32);

// _IOW(KVMIO, 0x86, struct kvm_interrupt)
pub const KVM_INTERRUPT: u32 = io_write(KVMIO, 0x86, 4);

const KVM_INTERRUPT_SET: u32 = u32::MAX;
const KVM_INTERRUPT_UNSET: u32 = u32::MAX - 1;

// _IOW(KVMIO, nr, struct)
pub const KVM_SET_USER_MEMORY_REGION: u32 = io_write(KVMIO, 0x46, 32);
pub const KVM_IRQ_LINE: u32 = io_write(KVMIO, 0x61, 8);
const KVM_IRQFD: u32 = io_write(KVMIO, 0x76, core::mem::size_of::<KvmIrqFd>() as u32);
const KVM_IOEVENTFD: u32 = io_write(KVMIO, 0x79, core::mem::size_of::<KvmIoEventFd>() as u32);
const KVM_REGISTER_COALESCED_MMIO: u32 = io_write(
    KVMIO,
    0x67,
    core::mem::size_of::<KvmCoalescedMmioZone>() as u32,
);
const KVM_UNREGISTER_COALESCED_MMIO: u32 = io_write(
    KVMIO,
    0x68,
    core::mem::size_of::<KvmCoalescedMmioZone>() as u32,
);
pub const KVM_SET_MP_STATE: u32 = io_write(KVMIO, 0x99, 4);
pub const KVM_SET_REGS: u32 = io_write(KVMIO, 0x82, 256);
pub const KVM_SET_ONE_REG: u32 = io_write(KVMIO, 0xAC, 16);
const KVM_SET_DEVICE_ATTR: u32 =
    io_write(KVMIO, 0xe1, core::mem::size_of::<KvmDeviceAttr>() as u32);
const KVM_GET_DEVICE_ATTR: u32 =
    io_write(KVMIO, 0xe2, core::mem::size_of::<KvmDeviceAttr>() as u32);
const KVM_HAS_DEVICE_ATTR: u32 =
    io_write(KVMIO, 0xe3, core::mem::size_of::<KvmDeviceAttr>() as u32);

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
pub struct KvmCreateDevice {
    pub type_: u32,
    pub fd: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KvmCoalescedMmioZone {
    addr: u64,
    size: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KvmIoEventFd {
    datamatch: u64,
    addr: u64,
    len: u32,
    fd: i32,
    flags: u32,
    pad: [u8; 36],
}

struct KvmIoEvent {
    vm: VmRef,
    addr: u64,
    len: u32,
    datamatch: u64,
    flags: u32,
    counter: Arc<dyn CounterObject>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KvmIrqFd {
    fd: u32,
    gsi: u32,
    flags: u32,
    resamplefd: u32,
    pad: [u8; 16],
}

struct KvmIrqFdListener {
    vm: VmRef,
    vcpu_irq: u32,
}

impl CounterWriteListener for KvmIrqFdListener {
    fn on_counter_write(&self, _value: u64) {
        if let Some(vcpu) = self.vm.get_vcpu(0) {
            vcpu.trigger_irq(self.vcpu_irq);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmDeviceAttr {
    pub flags: u32,
    pub group: u32,
    pub attr: u64,
    pub addr: u64,
}

struct KvmCreatedDevice {
    vm: VmRef,
    device_type: u32,
}

impl StreamOps for KvmCreatedDevice {
    fn read(&self, _buffer: &mut [u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }
}

impl ControlOps for KvmCreatedDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match handle_device_ioctl(command, arg, &self.vm, self.device_type) {
            Ok(Some(value)) => i32::try_from(value).map_err(|_| "KVM ioctl return out of range"),
            Ok(None) => Err("Unsupported KVM device ioctl"),
            Err(_) => Err("KVM device ioctl failed"),
        }
    }
}

impl MemoryMappingOps for KvmCreatedDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("KVM device does not support mmap")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for KvmCreatedDevice {
    fn wait_until_ready(
        &self,
        _interest: ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}

impl FileObject for KvmCreatedDevice {
    fn seek(&self, _whence: SeekFrom) -> Result<u64, StreamError> {
        Err(StreamError::NotSupported)
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::Unknown,
            size: 0,
            permissions: FilePermission {
                read: false,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 0,
            link_count: 1,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
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
    vm: VmRef,
    page: KvmRunPage,
}

static KVM_RUN_PAGES: Once<RwLock<Vec<KvmRunPageEntry>>> = Once::new();
static KVM_IOEVENTS: Once<RwLock<Vec<KvmIoEvent>>> = Once::new();

fn get_run_pages() -> &'static RwLock<Vec<KvmRunPageEntry>> {
    KVM_RUN_PAGES.call_once(|| RwLock::new(Vec::new()))
}

fn get_ioevents() -> &'static RwLock<Vec<KvmIoEvent>> {
    KVM_IOEVENTS.call_once(|| RwLock::new(Vec::new()))
}

/// Allocate a shared kvm_run page for the given vCPU and register it.
pub fn register_vcpu_run_page(vcpu: &VcpuRef, vm: &VmRef) -> Result<(), ()> {
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
        vm: Arc::clone(vm),
        page: KvmRunPage { vaddr, paddr },
    });
    Ok(())
}

fn get_vcpu_vm(vcpu: &VcpuRef) -> Option<VmRef> {
    let pages = get_run_pages().read();
    for entry in pages.iter() {
        if Arc::ptr_eq(&entry.vcpu, vcpu) {
            return Some(Arc::clone(&entry.vm));
        }
    }
    None
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

fn decode_mmio_read_data(mmio: &KvmRunMmio) -> Option<(u8, u64)> {
    if mmio.is_write != 0 {
        return None;
    }

    let len = mmio.len as usize;
    let value = match len {
        1 => mmio.data[0] as u64,
        2 => u16::from_le_bytes([mmio.data[0], mmio.data[1]]) as u64,
        4 => u32::from_le_bytes([mmio.data[0], mmio.data[1], mmio.data[2], mmio.data[3]]) as u64,
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
    };
    Some((len as u8, value))
}

fn read_vcpu_run_mmio_data(vcpu: &VcpuRef) -> Option<(u8, u64)> {
    let pages = get_run_pages().read();
    for entry in pages.iter() {
        if Arc::ptr_eq(&entry.vcpu, vcpu) {
            refresh_run_page_from_poc(entry.page.vaddr);
            let kvm_run = unsafe { &*(entry.page.vaddr as *const KvmRun) };
            if kvm_run.exit_reason == KVM_EXIT_MMIO {
                let mmio = unsafe { &kvm_run.exit_data.mmio };
                return decode_mmio_read_data(mmio);
            }
            return None;
        }
    }
    None
}

fn mmio_data_mask(size: u8) -> u64 {
    match size {
        1 => 0xff,
        2 => 0xffff,
        4 => 0xffff_ffff,
        _ => u64::MAX,
    }
}

fn signal_counter(counter: &dyn CounterObject) -> Result<(), ()> {
    let value = 1u64.to_ne_bytes();
    counter.write(&value).map(|_| ()).map_err(|_| ())
}

fn handle_ioeventfd_mmio(vcpu: &VcpuRef, addr: u64, size: u8, data: u64) -> Result<bool, ()> {
    const KVM_IOEVENTFD_FLAG_DATAMATCH: u32 = 1 << 1;

    let Some(vm) = get_vcpu_vm(vcpu) else {
        return Ok(false);
    };

    let events = get_ioevents().read();
    for event in events.iter() {
        if !Arc::ptr_eq(&event.vm, &vm) || event.addr != addr {
            continue;
        }
        if event.len != 0 && event.len != size as u32 {
            continue;
        }
        if event.flags & KVM_IOEVENTFD_FLAG_DATAMATCH != 0 {
            let mask = mmio_data_mask(size);
            if (event.datamatch & mask) != (data & mask) {
                continue;
            }
        }

        signal_counter(event.counter.as_ref())?;
        return Ok(true);
    }

    Ok(false)
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
            clean_run_page_to_poc(entry.page.vaddr);
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
    arg: usize,
    abi: &mut LinuxAbi,
) -> Result<Option<usize>, ()> {
    match request {
        KVM_GET_API_VERSION => Ok(Some(KVM_API_VERSION)),

        KVM_CREATE_VM => {
            crate::println!("[KVM] CREATE_VM");
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
            const KVM_CAP_USER_MEMORY: usize = 3;
            const KVM_CAP_NR_VCPUS: usize = 9;
            const KVM_CAP_MP_STATE: usize = 14;
            const KVM_CAP_COALESCED_MMIO: usize = 15;
            const KVM_CAP_IRQFD: usize = 32;
            const KVM_CAP_IOEVENTFD: usize = 36;
            const KVM_CAP_ONE_REG: usize = 70;
            const KVM_CAP_MAX_VCPUS: usize = 66;
            const KVM_CAP_DEVICE_CTRL: usize = 89;
            let result = match arg {
                KVM_CAP_IRQCHIP => Ok(Some(1)),
                KVM_CAP_USER_MEMORY => Ok(Some(1)),
                KVM_CAP_ONE_REG => Ok(Some(1)),
                KVM_CAP_NR_VCPUS => Ok(Some(1)),
                KVM_CAP_MP_STATE => Ok(Some(1)),
                KVM_CAP_MAX_VCPUS => Ok(Some(1)),
                KVM_CAP_DEVICE_CTRL => Ok(Some(1)),
                KVM_CAP_IRQFD => Ok(Some(1)),
                KVM_CAP_IOEVENTFD => Ok(Some(1)),
                _ => match arch::check_extension(arg) {
                    Some(val) => Ok(Some(val)),
                    None => Ok(Some(0)),
                },
            };
            crate::println!("[KVM] CHECK_EXTENSION: cap={} => {:?}", arg, result);
            result
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
            crate::println!("[KVM] CREATE_VCPU(id={})", vcpu_id);
            let task = mytask().ok_or(())?;
            let vcpu = vm.create_vcpu(vcpu_id).map_err(|_| ())?;
            register_vcpu_run_page(&vcpu, vm).map_err(|_| ())?;
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
                vcpu.set_irq_line(irq_level.irq, irq_level.level != 0);
            }

            Ok(Some(0))
        }

        KVM_CREATE_IRQCHIP => {
            crate::println!("[KVM] CREATE_IRQCHIP");
            Ok(Some(0))
        }

        KVM_REGISTER_COALESCED_MMIO | KVM_UNREGISTER_COALESCED_MMIO => Ok(Some(0)),

        KVM_IRQFD => {
            const KVM_IRQFD_FLAG_DEASSIGN: u32 = 1 << 0;

            if arg == 0 {
                return Err(());
            }

            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmIrqFd.
            let irqfd = unsafe { &*(kva as *const KvmIrqFd) };
            if irqfd.flags & !KVM_IRQFD_FLAG_DEASSIGN != 0 {
                crate::println!(
                    "[KVM] IRQFD: unsupported flags={:#x} fd={} gsi={}",
                    irqfd.flags,
                    irqfd.fd,
                    irqfd.gsi
                );
                return Err(());
            }
            crate::println!(
                "[KVM] IRQFD: fd={} gsi={} flags={:#x}",
                irqfd.fd,
                irqfd.gsi,
                irqfd.flags
            );
            if irqfd.flags & KVM_IRQFD_FLAG_DEASSIGN != 0 {
                return Ok(Some(0));
            }

            let handle = abi.get_handle(irqfd.fd as usize).ok_or(())?;
            let object = task.handle_table.get(handle).ok_or(())?;
            let counter = object.as_counter().ok_or(())?;
            let irqfd_route = irqfd.gsi;
            let listener = Arc::new(KvmIrqFdListener {
                vm: Arc::clone(vm),
                vcpu_irq: arch::irqfd_route_to_vcpu_irq(irqfd_route),
            });
            counter.add_write_listener(listener);
            Ok(Some(0))
        }

        KVM_IOEVENTFD => {
            const KVM_IOEVENTFD_FLAG_PIO: u32 = 1 << 0;
            const KVM_IOEVENTFD_FLAG_DATAMATCH: u32 = 1 << 1;
            const KVM_IOEVENTFD_FLAG_DEASSIGN: u32 = 1 << 2;
            const SUPPORTED_FLAGS: u32 = KVM_IOEVENTFD_FLAG_DATAMATCH | KVM_IOEVENTFD_FLAG_DEASSIGN;

            if arg == 0 {
                return Err(());
            }

            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmIoEventFd.
            let event = unsafe { &*(kva as *const KvmIoEventFd) };
            crate::println!(
                "[KVM] IOEVENTFD: addr={:#x} len={} fd={} flags={:#x}",
                event.addr,
                event.len,
                event.fd,
                event.flags
            );
            if event.flags & KVM_IOEVENTFD_FLAG_PIO != 0 || event.flags & !SUPPORTED_FLAGS != 0 {
                return Err(());
            }
            if event.flags & KVM_IOEVENTFD_FLAG_DEASSIGN != 0 {
                let mut events = get_ioevents().write();
                events.retain(|registered| {
                    !(Arc::ptr_eq(&registered.vm, vm)
                        && registered.addr == event.addr
                        && registered.len == event.len
                        && registered.datamatch == event.datamatch
                        && registered.flags & KVM_IOEVENTFD_FLAG_DATAMATCH
                            == event.flags & KVM_IOEVENTFD_FLAG_DATAMATCH)
                });
                return Ok(Some(0));
            }

            let handle = abi.get_handle(event.fd as usize).ok_or(())?;
            let object = task.handle_table.get(handle).ok_or(())?;
            let counter = match object {
                KernelObject::Counter(counter) => counter,
                _ => return Err(()),
            };
            get_ioevents().write().push(KvmIoEvent {
                vm: Arc::clone(vm),
                addr: event.addr,
                len: event.len,
                datamatch: event.datamatch,
                flags: event.flags,
                counter,
            });
            Ok(Some(0))
        }

        KVM_CREATE_DEVICE => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmCreateDevice.
            let create = unsafe { &mut *(kva as *mut KvmCreateDevice) };
            crate::println!("[KVM] CREATE_DEVICE: type={:#x}", create.type_);

            arch::validate_device_type(create.type_)?;

            let device = Arc::new(KvmCreatedDevice {
                vm: Arc::clone(vm),
                device_type: create.type_,
            });
            let kernel_obj = KernelObject::from_file_object(device as Arc<dyn FileObject>);
            let handle = task.handle_table.insert(kernel_obj).map_err(|_| ())?;
            let fd = abi.allocate_fd(handle).map_err(|_| ())?;
            create.fd = fd as u32;
            Ok(Some(0))
        }

        KVM_SET_DEVICE_ATTR => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmDeviceAttr.
            let attr = unsafe { &*(kva as *const KvmDeviceAttr) };
            crate::println!(
                "[KVM] SET_DEVICE_ATTR: group={} attr={:#x}",
                attr.group,
                attr.attr
            );
            arch::set_device_attr(vm, arch::default_device_type(), attr)
        }

        KVM_GET_DEVICE_ATTR => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmDeviceAttr.
            let attr = unsafe { &*(kva as *const KvmDeviceAttr) };
            crate::println!(
                "[KVM] GET_DEVICE_ATTR: group={} attr={:#x}",
                attr.group,
                attr.attr
            );
            arch::get_device_attr(vm, arch::default_device_type(), attr)
        }

        KVM_HAS_DEVICE_ATTR => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmDeviceAttr.
            let attr = unsafe { &*(kva as *const KvmDeviceAttr) };
            crate::println!(
                "[KVM] HAS_DEVICE_ATTR: group={} attr={:#x}",
                attr.group,
                attr.attr
            );
            arch::has_device_attr(vm, arch::default_device_type(), attr)
        }

        _ => {
            crate::println!("[KVM] VM_IOCTL unknown: request={:#x}", request);
            arch::handle_vm_ioctl(request, arg, vm, abi)
        }
    }
}

fn handle_device_ioctl(
    request: u32,
    arg: usize,
    vm: &VmRef,
    device_type: u32,
) -> Result<Option<usize>, ()> {
    match request {
        KVM_SET_DEVICE_ATTR => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmDeviceAttr.
            let attr = unsafe { &*(kva as *const KvmDeviceAttr) };
            crate::println!(
                "[KVM] SET_DEVICE_ATTR: device={:#x} group={} attr={:#x}",
                device_type,
                attr.group,
                attr.attr
            );
            arch::set_device_attr(vm, device_type, attr)
        }
        KVM_GET_DEVICE_ATTR => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmDeviceAttr.
            let attr = unsafe { &*(kva as *const KvmDeviceAttr) };
            arch::get_device_attr(vm, device_type, attr)
        }
        KVM_HAS_DEVICE_ATTR => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmDeviceAttr.
            let attr = unsafe { &*(kva as *const KvmDeviceAttr) };
            arch::has_device_attr(vm, device_type, attr)
        }
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// VCPU-level ioctl dispatcher
// ---------------------------------------------------------------------------

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

static MMIO_PENDING_READ_REG: AtomicU8 = AtomicU8::new(0xFF);
static MMIO_PENDING_VALID: AtomicBool = AtomicBool::new(false);
static VM_EXIT_DEBUG_COUNT: AtomicU32 = AtomicU32::new(0);

#[inline(always)]
fn clean_run_page_to_poc(vaddr: usize) {
    #[cfg(target_arch = "aarch64")]
    crate::arch::aarch64::clean_dcache_to_poc_range(vaddr, core::mem::size_of::<KvmRun>());
}

#[inline(always)]
fn refresh_run_page_from_poc(vaddr: usize) {
    #[cfg(target_arch = "aarch64")]
    crate::arch::aarch64::clean_invalidate_dcache_to_poc_range(
        vaddr,
        core::mem::size_of::<KvmRun>(),
    );
}

fn log_vm_exit(exit: &crate::hypervisor::VmExit) {
    use crate::hypervisor::VmExit;

    let count = VM_EXIT_DEBUG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    match exit {
        VmExit::MmioRead {
            epc,
            addr,
            size,
            reg,
        } => crate::println!(
            "[KVM-RUN] exit#{} MMIO_READ epc={:#x} addr={:#x} size={} reg={}",
            count,
            epc,
            addr,
            size,
            reg
        ),
        VmExit::MmioWrite {
            epc,
            addr,
            size,
            reg,
            data,
        } => crate::println!(
            "[KVM-RUN] exit#{} MMIO_WRITE epc={:#x} addr={:#x} size={} reg={} data={:#x}",
            count,
            epc,
            addr,
            size,
            reg,
            data
        ),
        VmExit::FirmwareCall { epc } => {
            crate::println!("[KVM-RUN] exit#{} FIRMWARE_CALL epc={:#x}", count, epc)
        }
        VmExit::VirtualInstruction {
            epc,
            inst,
            inst_len,
        } => crate::println!(
            "[KVM-RUN] exit#{} VIRTUAL_INSTRUCTION epc={:#x} inst={:?} inst_len={:?}",
            count,
            epc,
            inst,
            inst_len
        ),
        VmExit::IllegalInstruction {
            epc,
            inst,
            inst_len,
        } => crate::println!(
            "[KVM-RUN] exit#{} ILLEGAL_INSTRUCTION epc={:#x} inst={:?} inst_len={:?}",
            count,
            epc,
            inst,
            inst_len
        ),
        VmExit::Breakpoint { epc } => {
            crate::println!("[KVM-RUN] exit#{} BREAKPOINT epc={:#x}", count, epc)
        }
        VmExit::Wfi => crate::println!("[KVM-RUN] exit#{} WFI", count),
        VmExit::Hlt => crate::println!("[KVM-RUN] exit#{} HLT", count),
        VmExit::Shutdown => crate::println!("[KVM-RUN] exit#{} SHUTDOWN", count),
        VmExit::HostInterrupt => crate::println!("[KVM-RUN] exit#{} HOST_INTERRUPT", count),
        VmExit::FailEntry {
            hardware_entry_failure_reason,
        } => crate::println!(
            "[KVM-RUN] exit#{} FAIL_ENTRY reason={:#x}",
            count,
            hardware_entry_failure_reason
        ),
        VmExit::InternalError => crate::println!("[KVM-RUN] exit#{} INTERNAL_ERROR", count),
        VmExit::Unknown(code) => {
            crate::println!("[KVM-RUN] exit#{} UNKNOWN code={:#x}", count, code)
        }
    }
}

pub fn handle_vcpu_ioctl(
    request: u32,
    arg: usize,
    vcpu: &VcpuRef,
    trapframe: &mut crate::arch::Trapframe,
) -> Result<Option<usize>, ()> {
    match request {
        KVM_RUN => {
            if let Some(task) = mytask() {
                task.default_time_slice.store(10, Ordering::SeqCst);
            }

            if MMIO_PENDING_VALID.load(Ordering::Acquire) {
                let reg = MMIO_PENDING_READ_REG.load(Ordering::Acquire);
                if reg != 0xFF {
                    let result = if arg != 0 {
                        let task = mytask().ok_or(())?;
                        let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
                        refresh_run_page_from_poc(kva);
                        // SAFETY: caller guarantees arg points to a valid KvmRun
                        let kvm_run = unsafe { &*(kva as *const KvmRun) };
                        if kvm_run.exit_reason == KVM_EXIT_MMIO {
                            let mmio = unsafe { &kvm_run.exit_data.mmio };
                            decode_mmio_read_data(mmio)
                        } else {
                            None
                        }
                    } else {
                        read_vcpu_run_mmio_data(vcpu)
                    };
                    if let Some((size, val)) = result {
                        // crate::println!(
                        //     "[KVM-RUN] complete MMIO_READ reg={} size={} value={:#x}",
                        //     reg,
                        //     size,
                        //     val
                        // );
                        arch::complete_mmio_read(vcpu, reg, size, val);
                    }
                }
                MMIO_PENDING_VALID.store(false, Ordering::Release);
            }

            let mut sbi_count = 0u32;
            loop {
                let exit = vcpu.run().map_err(|_| ())?;
                // log_vm_exit(&exit);

                if matches!(exit, crate::hypervisor::VmExit::Wfi) {
                    vcpu.wait_for_interrupt(trapframe);
                    continue;
                }

                if matches!(exit, crate::hypervisor::VmExit::HostInterrupt) {
                    crate::sched::scheduler::schedule(trapframe);
                    continue;
                }

                if let crate::hypervisor::VmExit::FirmwareCall { .. } = &exit {
                    match arch::handle_firmware_call_in_kernel(vcpu) {
                        arch::FirmwareCallResult::Handled => {
                            sbi_count += 1;
                            continue;
                        }
                        arch::FirmwareCallResult::SystemOff
                        | arch::FirmwareCallResult::SystemReset
                        | arch::FirmwareCallResult::ForwardToUserspace => {}
                    }
                }

                if let crate::hypervisor::VmExit::MmioRead { reg, .. } = &exit {
                    MMIO_PENDING_READ_REG.store(*reg, Ordering::Release);
                    MMIO_PENDING_VALID.store(true, Ordering::Release);
                }

                if let crate::hypervisor::VmExit::MmioWrite {
                    addr, size, data, ..
                } = &exit
                    && handle_ioeventfd_mmio(vcpu, *addr, *size, *data)?
                {
                    continue;
                }

                if !write_vcpu_run_exit(vcpu, &exit) && arg != 0 {
                    let task = mytask().ok_or(())?;
                    let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
                    // SAFETY: caller guarantees arg points to a valid KvmRun
                    let kvm_run = unsafe { &mut *(kva as *mut KvmRun) };
                    write_vm_exit(kvm_run, &exit, vcpu);
                    clean_run_page_to_poc(kva);
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
        VmExit::Wfi => {
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
        VmExit::HostInterrupt => {
            kvm_run.exit_reason = KVM_EXIT_UNKNOWN;
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
