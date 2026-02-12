//! Virtual Machine management

extern crate alloc;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::hv::ArchVm;
use crate::hypervisor::memory::{MemorySlot, MemorySlotFlags, MemorySlotManager};
use crate::hypervisor::vcpu::{Vcpu, VcpuId};

/// VM identifier
pub type VmId = u32;

/// Maximum number of vCPUs per VM
const MAX_VCPUS: usize = 256;

/// Maximum number of memory slots per VM
const MAX_MEMORY_SLOTS: u32 = 512;

/// Counter for assigning VM IDs
static NEXT_VM_ID: Mutex<VmId> = Mutex::new(0);

/// Represents a virtual machine
pub struct Vm {
    /// VM identifier
    id: VmId,
    /// Architecture-specific VM state (e.g., G-stage page tables)
    arch: ArchVm,
    /// Guest physical memory slot manager
    memory_slots: MemorySlotManager,
    /// vCPUs belonging to this VM
    vcpus: Vec<Arc<Mutex<Vcpu>>>,
    /// Maximum number of vCPUs
    max_vcpus: usize,
}

impl Vm {
    /// Create a new VM
    pub fn new() -> Result<Arc<Mutex<Self>>, &'static str> {
        let arch = ArchVm::new()?;
        let id = {
            let mut next = NEXT_VM_ID.lock();
            let id = *next;
            *next = next.wrapping_add(1);
            id
        };

        let vm = Arc::new(Mutex::new(Self {
            id,
            arch,
            memory_slots: MemorySlotManager::new(MAX_MEMORY_SLOTS),
            vcpus: Vec::new(),
            max_vcpus: MAX_VCPUS,
        }));

        Ok(vm)
    }

    /// Get the VM ID
    pub fn id(&self) -> VmId {
        self.id
    }

    /// Create a new vCPU in this VM
    ///
    /// The `self_ref` parameter is a weak reference to the VM's Arc,
    /// needed so the vCPU can reference its parent.
    pub fn create_vcpu(
        &mut self,
        vcpu_id: VcpuId,
        self_ref: Weak<Mutex<Vm>>,
    ) -> Result<Arc<Mutex<Vcpu>>, &'static str> {
        if self.vcpus.len() >= self.max_vcpus {
            return Err("Maximum number of vCPUs reached");
        }

        for existing in &self.vcpus {
            if existing.lock().id() == vcpu_id {
                return Err("vCPU ID already exists");
            }
        }

        let vcpu = Vcpu::new(vcpu_id, self_ref)?;
        let vcpu_ref = Arc::new(Mutex::new(vcpu));
        self.vcpus.push(Arc::clone(&vcpu_ref));
        Ok(vcpu_ref)
    }

    /// Set a guest physical memory region
    pub fn set_memory_region(
        &mut self,
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

        self.memory_slots.set_slot(slot)?;

        if memory_size > 0 {
            self.arch
                .map_memory(guest_phys_addr, host_phys_addr, memory_size, flags)?;
        } else {
            self.arch.unmap_memory(guest_phys_addr, memory_size)?;
        }

        Ok(())
    }

    /// Get the number of vCPUs
    pub fn vcpu_count(&self) -> usize {
        self.vcpus.len()
    }

    /// Get memory slots
    pub fn memory_slots(&self) -> &MemorySlotManager {
        &self.memory_slots
    }
}
