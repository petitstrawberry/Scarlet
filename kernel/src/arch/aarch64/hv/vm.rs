extern crate alloc;

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering, fence};
use spin::Mutex;

use super::guest_vcpu::GuestVcpu;
use super::mmu::{
    alloc_vmid, create_stage2_page_mapping, free_stage2, get_stage2_root, init_stage2,
    set_guest_root_stage2, verify_hgatp_stage2,
};
use super::switch::{HCR_EL2_GUEST, HOST_HV_CTX, arch_run_guest_loop, el2_guest_exit_vector};
use super::sysreg::GuestSystemRegs;
use super::trap::arch_guest_trap_handler;
use super::vgic::VgicState;
use crate::arch::{Mode, Trapframe};
use crate::hypervisor::memory::{MemorySlot, MemorySlotFlags, MemorySlotManager};
use crate::hypervisor::types::{InterruptType, VmExit};
use crate::hypervisor::vcpu::{VcpuId, VcpuObject};
use crate::hypervisor::vm::{ScarletVmMemoryRegion, VmId, VmObject, vm_ctl};
use crate::object::capability::ControlOps;
use crate::task::mytask;
use crate::vm::manager::VirtualMemoryManager;

const IRQ_BIT_SOFTWARE: u64 = 1 << 0;
const IRQ_BIT_TIMER: u64 = 1 << 1;
const IRQ_BIT_EXTERNAL: u64 = 1 << 2;

struct VcpuInternalState {
    guest: GuestVcpu,
    vgic: VgicState,
}

pub struct Aarch64VcpuObject {
    id: VcpuId,
    vm: Weak<Aarch64VmObject>,
    state: Mutex<VcpuInternalState>,
    irqs_pending: AtomicU64,
    irqs_pending_mask: AtomicU64,
    last_irq_state: AtomicU64,
    vgic_num_lrs: usize,
}

impl Aarch64VcpuObject {
    pub fn new(id: VcpuId, vm: &Arc<Aarch64VmObject>) -> Arc<Self> {
        let num_lrs = super::vgic::probe_vgic();
        Arc::new(Self {
            id,
            vm: Arc::downgrade(vm),
            state: Mutex::new(VcpuInternalState {
                guest: GuestVcpu::new(vm.id(), id),
                vgic: VgicState::new(num_lrs),
            }),
            irqs_pending: AtomicU64::new(0),
            irqs_pending_mask: AtomicU64::new(0),
            last_irq_state: AtomicU64::new(0),
            vgic_num_lrs: num_lrs,
        })
    }

    pub fn vm_id(&self) -> VmId {
        self.vm.upgrade().map(|v| v.id()).unwrap_or(0)
    }

    fn setup_for_guest(
        &self,
        task: &crate::task::Task,
        vcpu: &mut GuestVcpu,
        vgic: &mut VgicState,
        vm: &Aarch64VmObject,
    ) {
        // Step 1: Save host EL2 state
        let hcr: u64;
        let vbar: u64;
        let ich_hcr: u64;
        let ich_vmcr: u64;
        // SAFETY: the host kernel runs at EL2 in VHE mode, so EL2 register accesses are valid here.
        unsafe {
            asm!(
                "mrs {hcr}, hcr_el2",
                "mrs {vbar}, vbar_el2",
                "mrs {ich_hcr}, ich_hcr_el2",
                "mrs {ich_vmcr}, ich_vmcr_el2",
                hcr = out(reg) hcr,
                vbar = out(reg) vbar,
                ich_hcr = out(reg) ich_hcr,
                ich_vmcr = out(reg) ich_vmcr,
                options(nostack),
            );
            HOST_HV_CTX.hcr_el2 = hcr;
            HOST_HV_CTX.vbar_el2 = vbar;
            HOST_HV_CTX.ich_hcr_el2 = ich_hcr;
            HOST_HV_CTX.ich_vmcr_el2 = ich_vmcr;
        }

        // Step 2: Set VBAR_EL2 to guest exit vector
        // SAFETY: VBAR_EL2 is retargeted to the guest exit vector before guest entry.
        unsafe {
            asm!(
                "msr vbar_el2, {vector}",
                "isb",
                vector = in(reg) el2_guest_exit_vector as usize,
                options(nostack),
            );
        }

        // Step 3: Set VTTBR_EL2 to guest stage-2 page table
        vm.set_guest_root_pagetable();

        // Ensure all the above setup is visible before guest entry.
        fence(Ordering::SeqCst);

        // Step 4: Switch HCR_EL2 to guest config (clears TGE, enables VM)
        // SAFETY: switching HCR_EL2 to the guest configuration is required for guest execution.
        unsafe {
            asm!("msr hcr_el2, {0}", "isb", in(reg) HCR_EL2_GUEST, options(nostack));
        }

        // Step 5: Restore guest _EL12 system registers (now safe: TGE=0)
        vcpu.sysregs.restore();

        // Step 6: Restore guest VGIC state
        if vgic.hcr == 0 && vgic.vmcr == 0 {
            super::vgic::vgic_guest_entry_init(self.vgic_num_lrs);
            vgic.hcr = super::vgic::read_hcr();
            vgic.vmcr = super::vgic::read_vmcr();
            super::vgic::save_lrs(self.vgic_num_lrs, &mut vgic.lr_shadow);
        } else {
            super::vgic::restore_guest_state(vgic);
        }

        task.vcpu.lock().set_mode(vcpu.get_mode());
    }

    fn save_guest_vgic_state(&self, vgic: &mut VgicState) {
        super::vgic::save_guest_state(vgic);
    }

    fn restore_host_vgic_state(&self) {
        let (ich_hcr, ich_vmcr) = unsafe { (HOST_HV_CTX.ich_hcr_el2, HOST_HV_CTX.ich_vmcr_el2) };
        super::vgic::restore_host_vgic(self.vgic_num_lrs, ich_hcr, ich_vmcr);
    }

    fn sync_interrupts(&self, _guest: &GuestVcpu) {
        let last_irq_state = self.last_irq_state.load(Ordering::Acquire);
        let timer_shadowed = last_irq_state & IRQ_BIT_TIMER;
        if timer_shadowed == 0 {
            return;
        }

        let timer_active = super::vgic::is_virq_pending(self.vgic_num_lrs, 27);
        if !timer_active {
            self.irqs_pending
                .fetch_and(!IRQ_BIT_TIMER, Ordering::AcqRel);
            self.irqs_pending_mask
                .fetch_or(IRQ_BIT_TIMER, Ordering::Release);
            self.last_irq_state
                .fetch_and(!IRQ_BIT_TIMER, Ordering::AcqRel);
        }
    }

    fn inject_pending_interrupts(&self, _guest: &mut GuestVcpu) {
        let mask = self.irqs_pending_mask.swap(0, Ordering::AcqRel);
        if mask == 0 {
            return;
        }

        let pending = self.irqs_pending.load(Ordering::Acquire);
        let val = pending & mask;

        if (val & IRQ_BIT_TIMER) != 0 {
            let _ = super::vgic::inject_virq(self.vgic_num_lrs, 27, 0x80, true);
        } else {
            let _ = super::vgic::clear_virq(self.vgic_num_lrs, 27);
        }

        if (val & IRQ_BIT_EXTERNAL) != 0 {
            let _ = super::vgic::inject_virq(self.vgic_num_lrs, 32, 0x80, true);
        } else {
            let _ = super::vgic::clear_virq(self.vgic_num_lrs, 32);
        }

        if (val & IRQ_BIT_SOFTWARE) != 0 {
            let _ = super::vgic::inject_virq(self.vgic_num_lrs, 3, 0x80, false);
        } else {
            let _ = super::vgic::clear_virq(self.vgic_num_lrs, 3);
        }

        self.last_irq_state.store(val, Ordering::Release);
    }

    fn prepare_normal_task_and_save_guest(
        &self,
        task: &crate::task::Task,
        vcpu: &mut GuestVcpu,
        guest_tf: &mut Trapframe,
    ) {
        vcpu.sysregs = GuestSystemRegs::save();
        vcpu.save(guest_tf);

        let mut task_vcpu = task.vcpu.lock();
        task_vcpu.set_mode(Mode::User);
    }
}

impl VcpuObject for Aarch64VcpuObject {
    fn id(&self) -> VcpuId {
        self.id
    }

    fn inject_interrupt(&self, irq_type: InterruptType) {
        let bit = match irq_type {
            InterruptType::Software => IRQ_BIT_SOFTWARE,
            InterruptType::Timer => IRQ_BIT_TIMER,
            InterruptType::External => IRQ_BIT_EXTERNAL,
        };

        self.irqs_pending.fetch_or(bit, Ordering::Release);
        self.irqs_pending_mask.fetch_or(bit, Ordering::Release);
    }

    fn clear_interrupt(&self, irq_type: InterruptType) {
        let bit = match irq_type {
            InterruptType::Software => IRQ_BIT_SOFTWARE,
            InterruptType::Timer => IRQ_BIT_TIMER,
            InterruptType::External => IRQ_BIT_EXTERNAL,
        };

        self.irqs_pending.fetch_and(!bit, Ordering::AcqRel);
        self.irqs_pending_mask.fetch_or(bit, Ordering::Release);
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

        self.sync_interrupts(&vcpu.guest);
        self.inject_pending_interrupts(&mut vcpu.guest);

        {
            let vcpu_state = &mut *vcpu;
            self.setup_for_guest(task, &mut vcpu_state.guest, &mut vcpu_state.vgic, &vm);
        }
        // SAFETY: the guest world switch restores host EL2 state before returning.
        unsafe { arch_run_guest_loop(&mut guest_tf, &vcpu.guest, arch) };

        loop {
            self.save_guest_vgic_state(&mut vcpu.vgic);
            self.restore_host_vgic_state();
            vcpu.guest.save(&guest_tf);

            self.sync_interrupts(&vcpu.guest);
            self.inject_pending_interrupts(&mut vcpu.guest);

            match arch_guest_trap_handler(&mut guest_tf, &vm) {
                Some(exit) => {
                    self.prepare_normal_task_and_save_guest(task, &mut vcpu.guest, &mut guest_tf);
                    return Ok(exit);
                }
                None => {
                    vcpu.guest.save(&guest_tf);
                    // Save the actual guest EL12 system register state before re-entry.
                    // Without this, setup_for_guest() would restore stale sysregs from
                    // before the guest ran, clobbering any EL1 sysreg changes the guest
                    // made during its last execution phase.
                    vcpu.guest.sysregs = GuestSystemRegs::save();
                    {
                        let vcpu_state = &mut *vcpu;
                        self.setup_for_guest(
                            task,
                            &mut vcpu_state.guest,
                            &mut vcpu_state.vgic,
                            &vm,
                        );
                    }
                    // SAFETY: same contract as the initial guest entry above.
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
}

pub type Vm = Aarch64VmObject;

pub struct Aarch64VmObject {
    id: VmId,
    owner_mm: VirtualMemoryManager,
    state: Mutex<VmInternalState>,
}

impl Drop for Aarch64VmObject {
    fn drop(&mut self) {
        let vmid = self.state.lock().vmid;
        free_stage2(vmid);
    }
}

impl Aarch64VmObject {
    pub fn new(id: VmId, owner_mm: VirtualMemoryManager) -> Result<Self, &'static str> {
        let vmid = alloc_vmid();
        init_stage2(vmid)?;

        Ok(Self {
            id,
            owner_mm,
            state: Mutex::new(VmInternalState {
                vcpus: Vec::new(),
                memory_slots: MemorySlotManager::new(),
                vmid,
                fast_path_flags: 0,
            }),
        })
    }

    pub fn owner_mm(&self) -> &VirtualMemoryManager {
        &self.owner_mm
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
        // SAFETY: get_stage2_root returns a valid raw pointer to a stage-2 page
        // table root owned by the hypervisor MMU subsystem for this vmid.
        let root = unsafe { &mut *root };
        create_stage2_page_mapping(root, gpa, hpa, writable, state.vmid)
    }

    pub fn set_guest_root_pagetable(&self) {
        let state = self.state.lock();
        if let Some(root) = get_stage2_root(state.vmid) {
            // SAFETY: get_stage2_root returns a valid raw pointer to this VM's
            // stage-2 root page table while the VM exists.
            let root = unsafe { &*root };
            set_guest_root_stage2(root, state.vmid);
        }
    }

    pub fn verify_guest_root_pagetable(&self) {
        let state = self.state.lock();
        if let Some(root) = get_stage2_root(state.vmid) {
            // SAFETY: get_stage2_root returns a valid raw pointer to this VM's
            // stage-2 root page table while the VM exists.
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
        self.state.lock().memory_slots.set_slot(MemorySlot {
            slot_id,
            guest_phys_addr,
            memory_size,
            userspace_addr: host_vaddr,
            flags,
        })
    }
}

impl VmObject for Aarch64VmObject {
    fn id(&self) -> VmId {
        self.id
    }

    fn owner_mm(&self) -> &VirtualMemoryManager {
        &self.owner_mm
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
        let vcpu = Aarch64VcpuObject::new(vcpu_id, self);
        self.state.lock().vcpus.push(vcpu.clone());
        Ok(vcpu)
    }

    fn set_memory_region(
        &self,
        slot_id: u32,
        guest_phys_addr: u64,
        memory_size: u64,
        host_userspace_addr: u64,
        flags: MemorySlotFlags,
    ) -> Result<(), &'static str> {
        self.set_memory_region_impl(
            slot_id,
            guest_phys_addr,
            memory_size,
            host_userspace_addr,
            flags,
        )
    }
}

impl ControlOps for Aarch64VmObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            vm_ctl::SET_MEMORY_REGION => {
                let target_ptr = self
                    .owner_mm
                    .translate_to_kva(arg)
                    .ok_or("Invalid user pointer")?;
                // SAFETY: caller guarantees arg points to a valid ScarletVmMemoryRegion
                // in the VM's owner address space.
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
        .translate_to_kva(arg)
        .ok_or("Invalid user pointer")
}

use crate::hypervisor::vcpu::{VcpuOneReg, vcpu_ctl};

impl ControlOps for Aarch64VcpuObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            vcpu_ctl::RUN => Err("Use sys_shv_vcpu_run"),
            vcpu_ctl::GET_ONE_REG => {
                let value = self.get_reg(arg as u32)?;
                Ok(value as i32)
            }
            vcpu_ctl::SET_ONE_REG => {
                let target_ptr = translate_user_ptr(arg)?;
                // SAFETY: translate_user_ptr validated the address; caller
                // guarantees it points to a valid VcpuOneReg.
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
            vcpu_ctl::CLEAR_INTERRUPT => {
                let irq_type = match arg {
                    0 => InterruptType::Software,
                    1 => InterruptType::Timer,
                    2 => InterruptType::External,
                    _ => return Err("Invalid interrupt type"),
                };
                self.clear_interrupt(irq_type);
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
            (vcpu_ctl::CLEAR_INTERRUPT, "Clear interrupt"),
        ]
    }
}
