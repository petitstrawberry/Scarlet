//! VM object management for the hypervisor subsystem.
//!
//! Each VM object owns its guest memory slots and resolves guest physical
//! addresses through an **owner address space** — a `VirtualMemoryManager`
//! captured at creation time from the task that created the VM. This ensures
//! that guest memory resolution is self-contained within the VM object and
//! does not depend on whichever task happens to be running a vCPU at any
//! given moment (Option C design).

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

use super::memory::MemorySlotFlags;
use super::vcpu::VcpuObject;
use crate::object::capability::ControlOps;
use crate::vm::manager::VirtualMemoryManager;

pub type VmId = u32;

pub mod vm_ctl {
    pub const SET_MEMORY_REGION: u32 = 0x01;
    pub const GET_VCPU_COUNT: u32 = 0x02;
    pub const SET_FAST_PATH: u32 = 0x03;
}

pub mod fast_path_flags {
    pub const TIMER: u32 = 0x01;
}

/// Scarlet-native memory region descriptor passed to `vm_ctl::SET_MEMORY_REGION`.
///
/// Note: `host_phys_addr` is a misnomer inherited from the original API — it
/// actually holds a **userspace virtual address** in the owner task's address
/// space. The kernel translates it via the VM's owner `VirtualMemoryManager`.
#[repr(C)]
pub struct ScarletVmMemoryRegion {
    pub slot_id: u32,
    pub flags: u32,
    pub guest_phys_addr: u64,
    pub memory_size: u64,
    /// Userspace virtual address in the owner task's address space.
    /// Renamed from `host_phys_addr` — the field is a VA, not a PA.
    pub host_phys_addr: u64,
}

/// Trait for architecture-specific VM operations.
///
/// Implementors store an owner `VirtualMemoryManager` captured at creation
/// time and use it to resolve guest memory mappings independently of the
/// currently-running task.
pub trait VmObject: ControlOps + Send + Sync {
    /// Returns the VM's unique identifier.
    fn id(&self) -> VmId;

    /// Creates a vCPU with the given ID attached to this VM.
    fn create_vcpu(
        self: &Arc<Self>,
        vcpu_id: super::vcpu::VcpuId,
    ) -> Result<Arc<dyn VcpuObject>, &'static str>;

    /// Registers or modifies a memory slot in the guest physical address space.
    ///
    /// `host_userspace_addr` is a virtual address in the VM's owner address
    /// space that backs the guest physical region `[guest_phys_addr,
    /// guest_phys_addr + memory_size)`.
    fn set_memory_region(
        &self,
        slot_id: u32,
        guest_phys_addr: u64,
        memory_size: u64,
        host_userspace_addr: u64,
        flags: MemorySlotFlags,
    ) -> Result<(), &'static str>;

    /// Returns a reference to the owner address space captured at VM creation.
    ///
    /// This is used by the trap handler to translate guest physical addresses
    /// through the correct address space, regardless of which task is running.
    fn owner_mm(&self) -> &VirtualMemoryManager;
}

/// Global VM manager that tracks all active VMs in the system.
pub struct VirtualMachineManager {
    vms: Mutex<Vec<Arc<crate::arch::hv::Vm>>>,
    next_id: AtomicU32,
}

impl VirtualMachineManager {
    /// Creates a new, empty `VirtualMachineManager`.
    pub const fn new() -> Self {
        Self {
            vms: Mutex::new(Vec::new()),
            next_id: AtomicU32::new(1),
        }
    }

    /// Creates a new VM bound to the given owner address space.
    ///
    /// The `owner_mm` is a clone of the creating task's `VirtualMemoryManager`.
    /// Because `VirtualMemoryManager` is `Arc`-backed, the clone is cheap and
    /// shares the same address space as the creator.
    pub fn create_vm(
        &self,
        owner_mm: VirtualMemoryManager,
    ) -> Result<Arc<crate::arch::hv::Vm>, &'static str> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let vm = crate::arch::hv::create_vm(id, owner_mm)?;
        self.vms.lock().push(Arc::clone(&vm));
        Ok(vm)
    }

    /// Looks up a VM by its ID.
    pub fn get_vm_by_id(&self, id: VmId) -> Option<Arc<crate::arch::hv::Vm>> {
        self.vms.lock().iter().find(|vm| vm.id() == id).cloned()
    }
}

/// Singleton global VM manager instance.
pub static GLOBAL_VM_MANAGER: VirtualMachineManager = VirtualMachineManager::new();
