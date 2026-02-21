extern crate alloc;

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use spin::Mutex;

use super::csr::{
    read_hcounteren, read_htimedelta, write_hcounteren, write_hedeleg, write_hideleg,
    write_htimedelta,
};
use super::guest_vcpu::GuestVcpu;
use super::mmu::{
    alloc_vmid, free_stage2, get_stage2_root, init_stage2, map_stage2_page_new,
    set_guest_root_stage2, verify_hgatp_stage2,
};
use super::switch::arch_run_guest_loop;
use super::trap::arch_guest_trap_handler;
use crate::arch::{Arch, Trapframe, set_next_mode, set_trapvector};
use crate::hypervisor::memory::{MemorySlot, MemorySlotFlags, MemorySlotManager};
use crate::hypervisor::types::{InterruptType, VmExit};
use crate::hypervisor::vcpu::{VcpuId, VcpuObject};
use crate::hypervisor::vm::{ScarletVmMemoryRegion, VmId, VmObject, vm_ctl};
use crate::object::capability::ControlOps;
use crate::task::mytask;
use crate::vm::{get_guest_trapvector_trampoline, get_trampoline_trap_vector};

pub struct RiscvVmState {
    hcounteren: u64,
    htimedelta: u64,
    hedeleg: u64,
    hideleg: u64,
}

impl RiscvVmState {
    pub fn new() -> Self {
        Self {
            hcounteren: 0x02,
            htimedelta: 0,
            hedeleg: !0,
            hideleg: !0,
        }
    }

    pub fn save(&mut self) {
        self.hcounteren = read_hcounteren();
        self.htimedelta = read_htimedelta();
    }

    pub fn apply(&self) {
        write_hcounteren(self.hcounteren);
        write_htimedelta(self.htimedelta);
        write_hedeleg(self.hedeleg);
        write_hideleg(self.hideleg);
    }
}

impl Default for RiscvVmState {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Riscv64VcpuObject {
    id: VcpuId,
    vm: Weak<Riscv64VmObject>,
    state: Mutex<VcpuInternalState>,
}

struct VcpuInternalState {
    guest: GuestVcpu,
    pending_software_irq: bool,
    pending_timer_irq: bool,
    pending_external_irq: bool,
}

impl Riscv64VcpuObject {
    pub fn new(id: VcpuId, vm: &Arc<Riscv64VmObject>) -> Arc<Self> {
        Arc::new(Self {
            id,
            vm: Arc::downgrade(vm),
            state: Mutex::new(VcpuInternalState {
                guest: GuestVcpu::new(0, id),
                pending_software_irq: false,
                pending_timer_irq: false,
                pending_external_irq: false,
            }),
        })
    }

    pub fn vm_id(&self) -> VmId {
        self.vm.upgrade().map(|v| v.id()).unwrap_or(0)
    }

    fn setup_for_guest(
        &self,
        task: &crate::task::Task,
        vcpu: &mut VcpuInternalState,
        vm: &Riscv64VmObject,
    ) {
        let mode = vcpu.guest.get_mode();
        set_next_mode(mode);

        let guest_tv = get_guest_trapvector_trampoline();
        set_trapvector(guest_tv);
        task.vcpu.lock().set_mode(mode);

        vm.set_guest_root_pagetable();

        self.inject_pending_interrupts(vcpu);
    }

    fn inject_pending_interrupts(&self, vcpu: &mut VcpuInternalState) {
        use super::csr::{read_hvip, write_hvip};

        let mut hvip = read_hvip();

        const VSSIP: u64 = 1 << 2;
        const VSTIP: u64 = 1 << 6;
        const VSEIP: u64 = 1 << 10;

        if vcpu.pending_software_irq {
            hvip |= VSSIP;
            vcpu.pending_software_irq = false;
        }
        if vcpu.pending_timer_irq {
            hvip |= VSTIP;
            vcpu.pending_timer_irq = false;
        }
        if vcpu.pending_external_irq {
            hvip |= VSEIP;
            vcpu.pending_external_irq = false;
        }

        write_hvip(hvip);
    }

    /// Prepares the current task to run the guest and saves the guest state back to the vCPU object.
    fn prepare_normal_task_and_save_guest(
        &self,
        task: &crate::task::Task,
        vcpu: &mut VcpuInternalState,
        guest_tf: &Trapframe,
    ) {
        let mut task_vcpu = task.vcpu.lock();
        vcpu.guest.save(guest_tf);
        task_vcpu.set_mode(crate::arch::Mode::User);
        set_next_mode(task_vcpu.get_mode());
        set_trapvector(get_trampoline_trap_vector());
    }
}

impl VcpuObject for Riscv64VcpuObject {
    fn id(&self) -> VcpuId {
        self.id
    }

    fn inject_interrupt(&self, irq_type: InterruptType) {
        let mut state = self.state.lock();
        match irq_type {
            InterruptType::Software => state.pending_software_irq = true,
            InterruptType::Timer => state.pending_timer_irq = true,
            InterruptType::External => state.pending_external_irq = true,
        }
    }

    fn get_reg(&self, index: u32) -> Result<u64, &'static str> {
        self.state.lock().guest.get_reg(index)
    }

    fn set_reg(&self, index: u32, value: u64) -> Result<(), &'static str> {
        self.state.lock().guest.set_reg(index, value)
    }

    fn run(&self) -> Result<VmExit, &'static str> {
        let vm = self.vm.upgrade().ok_or("VM no longer exists")?;
        let mut vcpu = self.state.lock();

        let arch = crate::arch::get_cpu();
        let task = mytask().ok_or("No current task")?;

        let mut guest_tf = Trapframe::new();

        self.setup_for_guest(task, &mut vcpu, &vm);
        unsafe { arch_run_guest_loop(&mut guest_tf, &vcpu.guest, arch) };

        loop {
            match arch_guest_trap_handler(&mut guest_tf, &vm) {
                Some(exit) => {
                    self.prepare_normal_task_and_save_guest(task, &mut vcpu, &mut guest_tf);

                    if let VmExit::MmioWrite {
                        epc,
                        addr,
                        size,
                        reg,
                        data: _,
                    } = exit
                    {
                        let data = vcpu.guest.get_mmio_data(reg, size);
                        return Ok(VmExit::MmioWrite {
                            epc,
                            addr,
                            size,
                            reg,
                            data,
                        });
                    }

                    return Ok(exit);
                }
                None => {
                    vcpu.guest.save(&guest_tf);
                    self.setup_for_guest(task, &mut vcpu, &vm);
                    unsafe { arch_run_guest_loop(&mut guest_tf, &vcpu.guest, arch) };
                }
            }
        }
    }
}

struct VmInternalState {
    vcpus: Vec<Arc<dyn VcpuObject>>,
    memory_slots: MemorySlotManager,
    vmid: u16,
    fast_path_flags: u32,
    riscv_state: RiscvVmState,
}

pub type Vm = Riscv64VmObject;

pub struct Riscv64VmObject {
    id: VmId,
    state: Mutex<VmInternalState>,
}

impl Drop for Riscv64VmObject {
    fn drop(&mut self) {
        let vmid = self.state.lock().vmid;
        free_stage2(vmid);
    }
}

impl Riscv64VmObject {
    pub fn new(id: VmId) -> Result<Self, &'static str> {
        let vmid = alloc_vmid();
        init_stage2(vmid)?;

        Ok(Self {
            id,
            state: Mutex::new(VmInternalState {
                vcpus: Vec::new(),
                memory_slots: MemorySlotManager::new(),
                vmid,
                fast_path_flags: 0,
                riscv_state: RiscvVmState::new(),
            }),
        })
    }

    pub fn id(&self) -> VmId {
        self.id
    }

    pub fn vmid(&self) -> u16 {
        self.state.lock().vmid
    }

    pub fn find_memory_slot(&self, gpa: u64) -> Option<MemorySlot> {
        self.state.lock().memory_slots.find_slot(gpa).cloned()
    }

    pub fn map_stage2_page(&self, gpa: u64, hpa: u64, writable: bool) -> Result<(), &'static str> {
        let state = self.state.lock();
        let root = get_stage2_root(state.vmid).ok_or("No Stage2 root")?;
        let root = unsafe { &mut *root };
        map_stage2_page_new(root, gpa, hpa, writable, state.vmid)
    }

    pub fn set_guest_root_pagetable(&self) {
        let state = self.state.lock();
        if let Some(root) = get_stage2_root(state.vmid) {
            let root = unsafe { &*root };
            set_guest_root_stage2(root, state.vmid);
        }
        state.riscv_state.apply();
    }

    pub fn verify_guest_root_pagetable(&self) {
        let state = self.state.lock();
        if let Some(root) = get_stage2_root(state.vmid) {
            let root = unsafe { &*root };
            verify_hgatp_stage2(root, state.vmid);
        }
    }

    pub fn vcpu_count(&self) -> usize {
        self.state.lock().vcpus.len()
    }

    pub fn get_vcpu(&self, vcpu_id: VcpuId) -> Option<Arc<dyn VcpuObject>> {
        self.state
            .lock()
            .vcpus
            .iter()
            .find(|v| v.id() == vcpu_id)
            .cloned()
    }

    fn set_memory_region_impl(
        &self,
        slot_id: u32,
        guest_phys_addr: u64,
        memory_size: u64,
        host_vaddr: u64,
        flags: MemorySlotFlags,
    ) -> Result<(), &'static str> {
        let task = mytask().ok_or("No current task")?;
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
}

impl VmObject for Riscv64VmObject {
    fn id(&self) -> VmId {
        self.id
    }

    fn create_vcpu(self: &Arc<Self>, vcpu_id: VcpuId) -> Result<Arc<dyn VcpuObject>, &'static str> {
        {
            let state = self.state.lock();
            for existing in &state.vcpus {
                if existing.id() == vcpu_id {
                    return Err("vCPU ID already exists");
                }
            }
        }
        let vcpu = Riscv64VcpuObject::new(vcpu_id, self);
        self.state.lock().vcpus.push(vcpu.clone());
        Ok(vcpu)
    }

    fn set_memory_region(
        &self,
        slot_id: u32,
        guest_phys_addr: u64,
        memory_size: u64,
        host_phys_addr: u64,
        flags: MemorySlotFlags,
    ) -> Result<(), &'static str> {
        self.set_memory_region_impl(slot_id, guest_phys_addr, memory_size, host_phys_addr, flags)
    }
}

impl ControlOps for Riscv64VmObject {
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
                self.set_memory_region_impl(
                    region.slot_id,
                    region.guest_phys_addr,
                    region.memory_size,
                    region.host_phys_addr,
                    flags,
                )?;
                Ok(0)
            }
            vm_ctl::GET_VCPU_COUNT => Ok(self.vcpu_count() as i32),
            vm_ctl::SET_FAST_PATH => {
                self.state.lock().fast_path_flags = arg as u32;
                Ok(0)
            }
            _ => Err("Unsupported VM control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![
            (vm_ctl::SET_MEMORY_REGION, "Set memory region"),
            (vm_ctl::GET_VCPU_COUNT, "Get vCPU count"),
            (vm_ctl::SET_FAST_PATH, "Set fast path flags"),
        ]
    }
}

fn translate_user_ptr(arg: usize) -> Result<usize, &'static str> {
    if arg == 0 {
        return Err("Invalid argument pointer");
    }
    mytask()
        .ok_or("No current task")?
        .vm_manager
        .translate_vaddr(arg)
        .ok_or("Invalid user pointer")
}

use crate::hypervisor::vcpu::{VcpuOneReg, vcpu_ctl};

impl ControlOps for Riscv64VcpuObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            vcpu_ctl::RUN => Err("Use sys_shv_vcpu_run"),
            vcpu_ctl::GET_ONE_REG => {
                let value = self.get_reg(arg as u32)?;
                Ok(value as i32)
            }
            vcpu_ctl::SET_ONE_REG => {
                let target_ptr = translate_user_ptr(arg)?;
                let one_reg = unsafe { core::ptr::read(target_ptr as *const VcpuOneReg) };
                self.set_reg(one_reg.index, one_reg.value)?;
                Ok(0)
            }
            vcpu_ctl::INJECT_INTERRUPT => {
                let irq_type = match arg {
                    0 => InterruptType::Software,
                    1 => InterruptType::Timer,
                    2 => InterruptType::External,
                    _ => return Err("Invalid interrupt type"),
                };
                self.inject_interrupt(irq_type);
                Ok(0)
            }
            _ => Err("Unsupported vCPU control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![
            (vcpu_ctl::RUN, "Run vCPU"),
            (vcpu_ctl::GET_ONE_REG, "Get one register"),
            (vcpu_ctl::SET_ONE_REG, "Set one register"),
            (vcpu_ctl::INJECT_INTERRUPT, "Inject interrupt"),
        ]
    }
}
