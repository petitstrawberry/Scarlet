extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

use crate::arch::vm::{alloc_virtual_address_space_for_stage2, get_root_pagetable};
use crate::object::capability::ControlOps;
use crate::task::mytask;

use super::memory::{MemorySlot, MemorySlotFlags, MemorySlotManager};
use super::vcpu::VcpuObject;

pub type VmId = u32;

pub mod vm_ctl {
    pub const SET_MEMORY_REGION: u32 = 0x01;
    pub const GET_VCPU_COUNT: u32 = 0x02;
}

#[repr(C)]
pub struct ScarletVmMemoryRegion {
    pub slot_id: u32,
    pub flags: u32,
    pub guest_phys_addr: u64,
    pub memory_size: u64,
    pub host_phys_addr: u64,
}

struct VmState {
    vcpus: Vec<Arc<VcpuObject>>,
    memory_slots: MemorySlotManager,
    vmid: u16,
}

pub struct VmObject {
    id: VmId,
    state: Mutex<VmState>,
}

impl VmObject {
    pub fn new(id: VmId) -> Result<Self, &'static str> {
        let vmid = alloc_virtual_address_space_for_stage2();
        Ok(Self {
            id,
            state: Mutex::new(VmState {
                vcpus: Vec::new(),
                memory_slots: MemorySlotManager::new(),
                vmid,
            }),
        })
    }

    pub fn id(&self) -> VmId {
        self.id
    }

    pub fn create_vcpu(
        self: &Arc<Self>,
        vcpu_id: super::vcpu::VcpuId,
    ) -> Result<Arc<VcpuObject>, &'static str> {
        for existing in &self.state.lock().vcpus {
            if existing.id() == vcpu_id {
                return Err("vCPU ID already exists");
            }
        }
        let vcpu = VcpuObject::new(vcpu_id, Arc::downgrade(self))?;
        self.state.lock().vcpus.push(Arc::clone(&vcpu));
        Ok(vcpu)
    }

    pub fn vcpu_count(&self) -> usize {
        self.state.lock().vcpus.len()
    }

    pub fn get_vcpu(&self, vcpu_id: super::vcpu::VcpuId) -> Option<Arc<VcpuObject>> {
        self.state
            .lock()
            .vcpus
            .iter()
            .find(|v| v.id() == vcpu_id)
            .cloned()
    }

    pub fn set_memory_region(
        &self,
        slot_id: u32,
        guest_phys_addr: u64,
        memory_size: u64,
        host_vaddr: u64,
        flags: MemorySlotFlags,
    ) -> Result<(), &'static str> {
        let task = crate::task::mytask().ok_or("No current task")?;
        let host_paddr = task
            .vm_manager
            .translate_vaddr(host_vaddr as usize)
            .ok_or("Failed to translate host_vaddr")? as u64;
        self.state.lock().memory_slots.set_slot(MemorySlot {
            slot_id,
            guest_phys_addr,
            memory_size,
            host_phys_addr: host_paddr,
            flags,
        })
    }

    pub fn find_memory_slot(&self, gpa: u64) -> Option<MemorySlot> {
        self.state.lock().memory_slots.find_slot(gpa).cloned()
    }

    pub fn map_stage2_page(
        &self,
        gpa: u64,
        hpa: u64,
        writable: bool,
        accessed: bool,
        dirty: bool,
    ) -> Result<(), &'static str> {
        let state = self.state.lock();
        let pagetable = get_root_pagetable(state.vmid).ok_or("No page table")?;
        let vmid = state.vmid;
        #[cfg(target_arch = "riscv64")]
        {
            crate::arch::hv::mmu::map_stage2_page(
                pagetable, gpa, hpa, writable, accessed, dirty, vmid,
            )
        }

        #[cfg(not(target_arch = "riscv64"))]
        {
            let _ = pagetable;
            let _ = gpa;
            let _ = hpa;
            let _ = writable;
            let _ = vmid;
            Err("Stage-2 mapping is only supported on riscv64")
        }
    }

    pub fn set_guest_root_pagetable(&self) {
        let state = self.state.lock();
        if let Some(pagetable) = get_root_pagetable(state.vmid) {
            #[cfg(target_arch = "riscv64")]
            {
                crate::arch::hv::mmu::set_guest_root_pagetable(pagetable, state.vmid);
            }
            #[cfg(not(target_arch = "riscv64"))]
            {
                let _ = pagetable;
                let _ = state.vmid;
            }
        }
    }

    #[cfg(target_arch = "riscv64")]
    pub fn verify_guest_root_pagetable(&self) {
        let state = self.state.lock();
        if let Some(pagetable) = get_root_pagetable(state.vmid) {
            crate::arch::hv::mmu::verify_hgatp(pagetable, state.vmid);
        }
    }
}

impl ControlOps for VmObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            vm_ctl::SET_MEMORY_REGION => {
                let task = mytask().ok_or("No current task")?;
                let target_ptr = task
                    .vm_manager
                    .translate_vaddr(arg)
                    .ok_or("Invalid user pointer")?;
                let region = unsafe { core::ptr::read(target_ptr as *const ScarletVmMemoryRegion) };
                let flags = MemorySlotFlags {
                    readonly: (region.flags & 1) != 0,
                };
                self.set_memory_region(
                    region.slot_id,
                    region.guest_phys_addr,
                    region.memory_size,
                    region.host_phys_addr,
                    flags,
                )?;
                Ok(0)
            }
            vm_ctl::GET_VCPU_COUNT => Ok(self.vcpu_count() as i32),
            _ => Err("Unsupported VM control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![
            (vm_ctl::SET_MEMORY_REGION, "Set memory region"),
            (vm_ctl::GET_VCPU_COUNT, "Get vCPU count")
        ]
    }
}

pub struct VirtualMachineManager {
    vms: Mutex<Vec<Arc<VmObject>>>,
    next_id: AtomicU32,
}

impl VirtualMachineManager {
    pub const fn new() -> Self {
        Self {
            vms: Mutex::new(Vec::new()),
            next_id: AtomicU32::new(1),
        }
    }
    pub fn create_vm(&self) -> Result<Arc<VmObject>, &'static str> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let vm = Arc::new(VmObject::new(id)?);
        self.vms.lock().push(Arc::clone(&vm));
        Ok(vm)
    }
    pub fn get_vm_by_id(&self, id: VmId) -> Option<Arc<VmObject>> {
        self.vms.lock().iter().find(|vm| vm.id() == id).cloned()
    }
}

pub static GLOBAL_VM_MANAGER: VirtualMachineManager = VirtualMachineManager::new();
