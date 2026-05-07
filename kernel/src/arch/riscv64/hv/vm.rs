extern crate alloc;

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use super::csr::{
    write_hcounteren, write_hedeleg, write_hgeie, write_hideleg, write_htimedelta, write_hvip,
};
use super::guest_vcpu::GuestVcpu;
use super::mmu::{
    STAGE2_MAX_PAGE_LEVEL, alloc_vmid, free_stage2, get_stage2_root, init_stage2,
    map_stage2_page_at_level, set_guest_root_stage2, verify_hgatp_stage2,
};
use super::switch::arch_run_guest_loop;
use super::trap::arch_guest_trap_handler;
use crate::arch::hv::csr::{self, HypervisorCsrState};
use crate::arch::{Arch, Trapframe, set_next_mode, set_trapvector};
use crate::hypervisor::memory::{MemorySlot, MemorySlotFlags, MemorySlotManager};
use crate::hypervisor::types::{InterruptType, VmExit};
use crate::hypervisor::vcpu::{VcpuId, VcpuObject};
use crate::hypervisor::vm::{ScarletVmMemoryRegion, VmId, VmObject, vm_ctl};
use crate::library::std::print;
use crate::object::capability::ControlOps;
use crate::task::mytask;
use crate::vm::manager::VirtualMemoryManager;
use crate::vm::{get_guest_trapvector_trampoline, get_trampoline_trap_vector};
use crate::{hypervisor, print};

pub type RiscvVmState = HypervisorCsrState;

/// Returns the RISC-V Stage-2 page size represented by a page-table level.
///
/// Level 0 is 4 KiB, level 1 is 2 MiB, level 2 is 1 GiB, and level 3 is
/// 512 GiB.
fn stage2_page_size(level: usize) -> u64 {
    1u64 << (12 + 9 * level)
}

/// Chooses the largest Stage-2 page level usable for a GPA-to-HPA mapping.
///
/// GPA and HPA must have matching offsets within the selected page size, and
/// the resulting page must be fully contained in the memory slot.
fn best_stage2_page_level(slot: &MemorySlot, gpa: u64, hpa: u64) -> usize {
    for level in (1..=STAGE2_MAX_PAGE_LEVEL).rev() {
        let page_size = stage2_page_size(level);
        debug_assert!(page_size.is_power_of_two());
        let page_mask = page_size - 1;
        let gpa_base = gpa & !(page_size - 1);
        let hpa_base = hpa & !(page_size - 1);
        // The faulting address may be inside a huge page. The aligned mapping
        // bases are valid only when GPA and HPA have the same in-page offset.
        if (gpa & page_mask) != (hpa & page_mask) {
            continue;
        }
        if gpa_base < slot.guest_phys_addr {
            continue;
        }
        if hpa_base < slot.host_phys_addr {
            continue;
        }
        let Some(page_end) = gpa_base.checked_add(page_size) else {
            continue;
        };
        let Some(slot_end) = slot.guest_phys_addr.checked_add(slot.memory_size) else {
            continue;
        };
        let Some(host_page_end) = hpa_base.checked_add(page_size) else {
            continue;
        };
        let Some(host_slot_end) = slot.host_phys_addr.checked_add(slot.memory_size) else {
            continue;
        };
        if page_end <= slot_end && host_page_end <= host_slot_end {
            return level;
        }
    }
    0
}

pub struct Riscv64VcpuObject {
    id: VcpuId,
    vm: Weak<Riscv64VmObject>,
    state: Mutex<VcpuInternalState>,
    irqs_pending: AtomicU64,
    irqs_pending_mask: AtomicU64,
    last_hvip: AtomicU64,
    first_run: AtomicBool,
}

struct VcpuInternalState {
    guest: GuestVcpu,
}

impl Riscv64VcpuObject {
    pub fn new(id: VcpuId, vm: &Arc<Riscv64VmObject>) -> Arc<Self> {
        Arc::new(Self {
            id,
            vm: Arc::downgrade(vm),
            state: Mutex::new(VcpuInternalState {
                guest: GuestVcpu::new(vm.id(), id),
            }),
            irqs_pending: AtomicU64::new(0),
            irqs_pending_mask: AtomicU64::new(0),
            last_hvip: AtomicU64::new(0),
            first_run: AtomicBool::new(true),
        })
    }

    fn init_on_first_hart(&self, vcpu: &mut GuestVcpu, vm: &Riscv64VmObject) {
        if self.first_run.swap(false, Ordering::AcqRel) {
            vcpu.init_csrs();
            let state = vm.state.lock();
            // Initialize the H-extension CSRs for this vCPU based on the VM's initial state
            state.riscv_state.restore();
        }
    }

    pub fn vm_id(&self) -> VmId {
        self.vm.upgrade().map(|v| v.id()).unwrap_or(0)
    }

    fn setup_for_guest(
        &self,
        task: &crate::task::Task,
        vcpu: &mut GuestVcpu,
        vm: &Riscv64VmObject,
    ) {
        self.init_on_first_hart(vcpu, vm);
        let mode = vcpu.get_mode();
        set_next_mode(mode);

        let guest_tv = get_guest_trapvector_trampoline();
        set_trapvector(guest_tv);
        task.vcpu.lock().set_mode(mode);

        vm.set_guest_root_pagetable();
    }

    fn inject_pending_interrupts(&self) {
        use super::csr::{read_hvip, write_hvip};

        let mask = self.irqs_pending_mask.swap(0, Ordering::AcqRel);
        if mask == 0 {
            return;
        }

        let pending = self.irqs_pending.load(Ordering::Acquire);
        let val = pending & mask;

        let hvip = read_hvip();
        let new_hvip = (hvip & !mask) | val;

        // crate::println!(
        //     "[inject_pending] mask={:#x} pending={:#x} val={:#x} hvip={:#x}->new_hvip={:#x}",
        //     mask,
        //     pending,
        //     val,
        //     hvip,
        //     new_hvip
        // );

        self.last_hvip.store(new_hvip, Ordering::Release);
        write_hvip(new_hvip);
    }

    fn sync_interrupts(&self) {
        use super::csr::read_hvip;

        let hvip = read_hvip();
        let last_hvip = self.last_hvip.load(Ordering::Acquire);
        let pending = self.irqs_pending.load(Ordering::Acquire);

        let vs_bits = (1u64 << 2) | (1u64 << 6) | (1u64 << 10);
        let changed = last_hvip ^ hvip;
        let guest_cleared = last_hvip & !hvip & changed & vs_bits;

        if guest_cleared != 0 {
            self.irqs_pending
                .fetch_and(!guest_cleared, Ordering::Release);
            self.irqs_pending_mask
                .fetch_or(guest_cleared, Ordering::Release);
        }
    }

    /// Prepares the current task to run the guest and saves the guest state back to the vCPU object.
    fn prepare_normal_task_and_save_guest(
        &self,
        task: &crate::task::Task,
        vcpu: &mut GuestVcpu,
        guest_tf: &Trapframe,
    ) {
        let mut task_vcpu = task.vcpu.lock();
        vcpu.save(guest_tf);
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
        let bit = match irq_type {
            InterruptType::Software => 1 << 2,
            InterruptType::Timer => 1 << 6,
            InterruptType::External => 1 << 10,
        };

        self.irqs_pending.fetch_or(bit, Ordering::Release);
        self.irqs_pending_mask.fetch_or(bit, Ordering::Release);
    }

    fn clear_interrupt(&self, irq_type: InterruptType) {
        let bit = match irq_type {
            InterruptType::Software => 1 << 2,
            InterruptType::Timer => 1 << 6,
            InterruptType::External => 1 << 10,
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

        self.sync_interrupts();
        self.inject_pending_interrupts();

        self.setup_for_guest(task, &mut vcpu.guest, &vm);
        // SAFETY: arch_run_guest_loop switches to guest mode and back;
        // it restores host state before returning. guest_tf is a stack-
        // allocated Trapframe that survives the guest entry/exit cycle.
        unsafe { arch_run_guest_loop(&mut guest_tf, &vcpu.guest, arch) };

        loop {
            vcpu.guest.save(&guest_tf);

            self.sync_interrupts();

            self.inject_pending_interrupts();

            match arch_guest_trap_handler(&mut guest_tf, &vm) {
                Some(exit) => {
                    self.prepare_normal_task_and_save_guest(task, &mut vcpu.guest, &mut guest_tf);
                    return Ok(exit);
                }
                None => {
                    vcpu.guest.save(&guest_tf);
                    self.setup_for_guest(task, &mut vcpu.guest, &vm);
                    // SAFETY: same as the initial arch_run_guest_loop call above.
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
    /// Owner address space captured from the creating task. Immutable after
    /// construction — stored outside the mutex so `owner_mm()` can return
    /// a reference without locking.
    owner_mm: VirtualMemoryManager,
    state: Mutex<VmInternalState>,
}

impl Drop for Riscv64VmObject {
    fn drop(&mut self) {
        let vmid = self.state.lock().vmid;
        free_stage2(vmid);
    }
}

impl Riscv64VmObject {
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
                riscv_state: RiscvVmState::new(),
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
        // SAFETY: get_stage2_root returns a valid raw pointer to a page table
        // root owned by the hypervisor MMU subsystem for this vmid.
        let root = unsafe { &mut *root };
        let level = state
            .memory_slots
            .find_slot(gpa)
            .map(|slot| best_stage2_page_level(slot, gpa, hpa))
            .unwrap_or(0);
        let page_size = stage2_page_size(level);
        let page_mask = page_size - 1;
        map_stage2_page_at_level(
            root,
            gpa & !page_mask,
            hpa & !page_mask,
            writable,
            state.vmid,
            level,
        )
    }

    pub fn set_guest_root_pagetable(&self) {
        let state = self.state.lock();
        if let Some(root) = get_stage2_root(state.vmid) {
            // SAFETY: same as map_stage2_page — root is a valid stage2 page table pointer.
            let root = unsafe { &*root };
            set_guest_root_stage2(root, state.vmid);
        }
    }

    pub fn verify_guest_root_pagetable(&self) {
        let state = self.state.lock();
        if let Some(root) = get_stage2_root(state.vmid) {
            // SAFETY: same as map_stage2_page — root is a valid stage2 page table pointer.
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

impl VmObject for Riscv64VmObject {
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
        let vcpu = Riscv64VcpuObject::new(vcpu_id, self);
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

impl ControlOps for Riscv64VmObject {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_best_stage2_page_level_uses_largest_aligned_slot_range() {
        let slot = MemorySlot {
            slot_id: 0,
            guest_phys_addr: 0,
            memory_size: stage2_page_size(2),
            host_phys_addr: stage2_page_size(2),
            flags: MemorySlotFlags::default(),
        };

        assert_eq!(best_stage2_page_level(&slot, 0, stage2_page_size(2)), 2);
        assert_eq!(
            best_stage2_page_level(&slot, 0x1000, stage2_page_size(2) + 0x1000),
            2
        );
        let partial_slot = MemorySlot {
            slot_id: 1,
            guest_phys_addr: stage2_page_size(1),
            memory_size: stage2_page_size(1),
            host_phys_addr: stage2_page_size(2) + stage2_page_size(1),
            flags: MemorySlotFlags::default(),
        };
        assert_eq!(
            best_stage2_page_level(
                &partial_slot,
                stage2_page_size(1),
                stage2_page_size(2) + stage2_page_size(1)
            ),
            1
        );
        assert_eq!(
            best_stage2_page_level(&slot, 0x1000, stage2_page_size(2) + 0x2000),
            0
        );
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
