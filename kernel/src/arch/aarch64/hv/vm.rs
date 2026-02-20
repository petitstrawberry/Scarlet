extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::hypervisor::memory::MemorySlotFlags;
use crate::hypervisor::vcpu::{VcpuId, VcpuObject};
use crate::hypervisor::vm::{VmId, VmObject, vm_ctl};
use crate::object::capability::ControlOps;

struct VmInternalState {
    vcpus: Vec<Arc<dyn VcpuObject>>,
}

pub type Vm = Aarch64VmObject;

pub struct Aarch64VmObject {
    id: VmId,
    state: Mutex<VmInternalState>,
}

impl Aarch64VmObject {
    pub fn new(id: VmId) -> Self {
        Self {
            id,
            state: Mutex::new(VmInternalState { vcpus: Vec::new() }),
        }
    }

    pub fn vcpu_count(&self) -> usize {
        self.state.lock().vcpus.len()
    }
}

impl VmObject for Aarch64VmObject {
    fn id(&self) -> VmId {
        self.id
    }

    fn create_vcpu(&self, _vcpu_id: VcpuId) -> Result<Arc<dyn VcpuObject>, &'static str> {
        todo!("create_vcpu not implemented for aarch64")
    }

    fn set_memory_region(
        &self,
        _slot_id: u32,
        _guest_phys_addr: u64,
        _memory_size: u64,
        _host_phys_addr: u64,
        _flags: MemorySlotFlags,
    ) -> Result<(), &'static str> {
        todo!("set_memory_region not implemented for aarch64")
    }
}

impl ControlOps for Aarch64VmObject {
    fn control(&self, command: u32, _arg: usize) -> Result<i32, &'static str> {
        match command {
            vm_ctl::GET_VCPU_COUNT => Ok(self.vcpu_count() as i32),
            _ => Err("Unsupported VM control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![(vm_ctl::GET_VCPU_COUNT, "Get vCPU count")]
    }
}
