//! Virtual Machine management

extern crate alloc;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::hv::ArchVm;
use crate::hypervisor::memory::{MemorySlot, MemorySlotFlags, MemorySlotManager};
use crate::hypervisor::vcpu::{VcpuId, VcpuObject};
use crate::object::capability::ControlOps;
use crate::task::mytask;

/// Scarlet Native VM control commands (via HandleControl)
pub mod vm_ctl {
    pub const SCTL_VM_SET_MEMORY_REGION: u32 = 0x01;
    pub const SCTL_VM_GET_VCPU_COUNT: u32 = 0x02;
}

/// Userspace-facing memory region descriptor (C ABI)
#[repr(C)]
pub struct ScarletVmMemoryRegion {
    pub slot_id: u32,
    pub flags: u32,
    pub guest_phys_addr: u64,
    pub memory_size: u64,
    pub host_phys_addr: u64,
}

pub type VmId = u32;

const MAX_VCPUS: usize = 256;
const MAX_MEMORY_SLOTS: u32 = 512;

static NEXT_VM_ID: Mutex<VmId> = Mutex::new(0);

struct VmState {
    arch: ArchVm,
    memory_slots: MemorySlotManager,
    vcpus: Vec<Arc<VcpuObject>>,
}

/// Virtual machine with internal mutability.
///
/// All mutable state is behind a `Mutex<VmState>`, allowing `&self` methods.
/// Follows the same pattern as `SharedMemory`.
pub struct VmObject {
    id: VmId,
    state: Mutex<VmState>,
    max_vcpus: usize,
}

impl VmObject {
    pub fn new() -> Result<Arc<Self>, &'static str> {
        let arch = ArchVm::new()?;
        let id = {
            let mut next = NEXT_VM_ID.lock();
            let id = *next;
            *next = next.wrapping_add(1);
            id
        };

        let vm = Arc::new(Self {
            id,
            state: Mutex::new(VmState {
                arch,
                memory_slots: MemorySlotManager::new(MAX_MEMORY_SLOTS),
                vcpus: Vec::new(),
            }),
            max_vcpus: MAX_VCPUS,
        });

        Ok(vm)
    }

    pub fn id(&self) -> VmId {
        self.id
    }

    pub fn create_vcpu(self: &Arc<Self>, vcpu_id: VcpuId) -> Result<Arc<VcpuObject>, &'static str> {
        let mut state = self.state.lock();
        if state.vcpus.len() >= self.max_vcpus {
            return Err("Maximum number of vCPUs reached");
        }

        for existing in &state.vcpus {
            if existing.id() == vcpu_id {
                return Err("vCPU ID already exists");
            }
        }

        let vcpu = VcpuObject::new(vcpu_id, Arc::downgrade(self), &state.arch)?;
        let vcpu_ref = Arc::new(vcpu);
        state.vcpus.push(Arc::clone(&vcpu_ref));
        Ok(vcpu_ref)
    }

    pub fn set_memory_region(
        &self,
        slot_id: u32,
        guest_phys_addr: u64,
        memory_size: u64,
        host_phys_addr: u64,
        flags: MemorySlotFlags,
    ) -> Result<(), &'static str> {
        let slot = MemorySlot {
            slot_id,
            guest_phys_addr,
            memory_size,
            host_phys_addr,
            flags,
        };

        let mut state = self.state.lock();
        state.memory_slots.set_slot(slot)?;

        if memory_size > 0 {
            state
                .arch
                .map_memory(guest_phys_addr, host_phys_addr, memory_size, flags)?;
        } else {
            state.arch.unmap_memory(guest_phys_addr, memory_size)?;
        }

        Ok(())
    }

    pub fn vcpu_count(&self) -> usize {
        self.state.lock().vcpus.len()
    }
}

impl ControlOps for VmObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        use vm_ctl::*;

        match command {
            SCTL_VM_SET_MEMORY_REGION => {
                if arg == 0 {
                    return Err("Invalid argument pointer");
                }

                let target_ptr = if let Some(current_task) = mytask() {
                    current_task
                        .vm_manager
                        .translate_vaddr(arg)
                        .ok_or("Invalid user pointer")?
                } else {
                    arg
                };

                // SAFETY: pointer was translated from a valid user mapping
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
            SCTL_VM_GET_VCPU_COUNT => Ok(self.vcpu_count() as i32),
            _ => Err("Unsupported VM control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        use vm_ctl::*;
        alloc::vec![
            (SCTL_VM_SET_MEMORY_REGION, "Set memory region"),
            (SCTL_VM_GET_VCPU_COUNT, "Get vCPU count"),
        ]
    }
}
