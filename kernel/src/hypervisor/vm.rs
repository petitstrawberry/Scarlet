extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

use super::memory::MemorySlotFlags;
use super::vcpu::VcpuObject;
use crate::object::capability::ControlOps;

pub type VmId = u32;

pub mod vm_ctl {
    pub const SET_MEMORY_REGION: u32 = 0x01;
    pub const GET_VCPU_COUNT: u32 = 0x02;
    pub const SET_FAST_PATH: u32 = 0x03;
}

pub mod fast_path_flags {
    pub const TIMER: u32 = 0x01;
}

#[repr(C)]
pub struct ScarletVmMemoryRegion {
    pub slot_id: u32,
    pub flags: u32,
    pub guest_phys_addr: u64,
    pub memory_size: u64,
    pub host_phys_addr: u64,
}

/// Trait for architecture-specific VM operations
pub trait VmObject: ControlOps + Send + Sync {
    fn id(&self) -> VmId;
    fn create_vcpu(
        self: &Arc<Self>,
        vcpu_id: super::vcpu::VcpuId,
    ) -> Result<Arc<dyn VcpuObject>, &'static str>;
    fn set_memory_region(
        &self,
        slot_id: u32,
        guest_phys_addr: u64,
        memory_size: u64,
        host_phys_addr: u64,
        flags: MemorySlotFlags,
    ) -> Result<(), &'static str>;
}

pub struct VirtualMachineManager {
    vms: Mutex<Vec<Arc<crate::arch::hv::Vm>>>,
    next_id: AtomicU32,
}

impl VirtualMachineManager {
    pub const fn new() -> Self {
        Self {
            vms: Mutex::new(Vec::new()),
            next_id: AtomicU32::new(1),
        }
    }

    pub fn create_vm(&self) -> Result<Arc<crate::arch::hv::Vm>, &'static str> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let vm = crate::arch::hv::create_vm(id)?;
        self.vms.lock().push(Arc::clone(&vm));
        Ok(vm)
    }

    pub fn get_vm_by_id(&self, id: VmId) -> Option<Arc<crate::arch::hv::Vm>> {
        self.vms.lock().iter().find(|vm| vm.id() == id).cloned()
    }
}

pub static GLOBAL_VM_MANAGER: VirtualMachineManager = VirtualMachineManager::new();
