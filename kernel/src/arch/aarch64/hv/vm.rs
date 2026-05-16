extern crate alloc;

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};
use spin::Mutex;

use super::guest_vcpu::GuestVcpu;
use super::mmu::{
    STAGE2_MAX_PAGE_LEVEL, alloc_vmid, free_stage2, get_stage2_root, init_stage2,
    map_stage2_page_at_level_no_flush, set_guest_root_stage2, verify_hgatp_stage2,
};
use super::switch::{HOST_HV_CTX, arch_run_guest_loop};
use super::sysreg::GuestSystemRegs;
use super::trap::arch_guest_trap_handler;
use super::vgic::VgicState;
use crate::arch::{Mode, Trapframe};
use crate::hypervisor::memory::{MemorySlot, MemorySlotFlags, MemorySlotManager};
use crate::hypervisor::mmio::VirtualMmioDeviceRef;
use crate::hypervisor::types::{InterruptType, VmExit};
use crate::hypervisor::vcpu::{VcpuId, VcpuObject};
use crate::hypervisor::vm::{ScarletVmMemoryRegion, VmId, VmObject, vm_ctl};
use crate::object::capability::ControlOps;
use crate::task::mytask;
use crate::vm::manager::VirtualMemoryManager;

const IRQ_BIT_SOFTWARE: u64 = 1 << 0;
const IRQ_BIT_TIMER: u64 = 1 << 1;
const IRQ_BIT_EXTERNAL: u64 = 1 << 2;
const GUEST_TIMER_PPI: u32 = 27;
const GUEST_IRQ_PRIORITY: u8 = 0x80;
const TIMER_CTL_ENABLE: u64 = 1 << 0;
const TIMER_CTL_IMASK: u64 = 1 << 1;
const TIMER_CTL_ISTATUS: u64 = 1 << 2;
static STAGE2_MAP_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static GUEST_TIMER_PPI_ENABLED_CPUS: AtomicU64 = AtomicU64::new(0);
static GUEST_TIMER_PPI_ENABLE_WARNED: AtomicU32 = AtomicU32::new(0);

fn stage2_page_size(level: usize) -> u64 {
    1u64 << (12 + 9 * level)
}

fn best_stage2_page_level(slot: &MemorySlot, gpa: u64, hpa: u64) -> usize {
    for level in (1..=STAGE2_MAX_PAGE_LEVEL).rev() {
        let page_size = stage2_page_size(level);
        let page_mask = page_size - 1;
        let gpa_base = gpa & !page_mask;
        let hpa_base = hpa & !page_mask;
        if (gpa & page_mask) != (hpa & page_mask) {
            continue;
        }
        if gpa_base < slot.guest_phys_addr || hpa_base < slot.userspace_addr {
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
        let Some(host_slot_end) = slot.userspace_addr.checked_add(slot.memory_size) else {
            continue;
        };

        if page_end <= slot_end && host_page_end <= host_slot_end {
            return level;
        }
    }
    0
}

fn read_cntpct_el0() -> u64 {
    let value: u64;
    // SAFETY: reading the architected physical counter is side-effect free.
    unsafe {
        asm!("mrs {value}, cntpct_el0", value = out(reg) value, options(nostack));
    }
    value
}

pub(crate) fn guest_virtual_count(sysregs: &GuestSystemRegs) -> u64 {
    read_cntpct_el0().wrapping_sub(sysregs.cntvoff_el2)
}

fn enable_guest_timer_ppi_for_current_cpu() {
    let cpu_id = crate::arch::get_cpu().get_cpuid() as u32;
    let Some(cpu_mask) = 1u64.checked_shl(cpu_id) else {
        return;
    };

    if (GUEST_TIMER_PPI_ENABLED_CPUS.load(Ordering::Acquire) & cpu_mask) != 0 {
        return;
    }

    match crate::arch::interrupt::enable_external_interrupt_line(GUEST_TIMER_PPI) {
        Ok(()) => {
            GUEST_TIMER_PPI_ENABLED_CPUS.fetch_or(cpu_mask, Ordering::Release);
        }
        Err(e) => {
            if GUEST_TIMER_PPI_ENABLE_WARNED.fetch_add(1, Ordering::Relaxed) == 0 {
                crate::println!(
                    "[AARCH64-HV] failed to enable guest virtual timer PPI: {}",
                    e
                );
            }
        }
    }
}

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
        enable_guest_timer_ppi_for_current_cpu();

        let hcr: u64;
        let vbar_el2: u64;
        let vbar_el1: u64;
        let ich_hcr: u64;
        let ich_vmcr: u64;
        let tpidr_el1: u64;
        let daif: u64;
        let cnthctl_el2: u64;
        let cntv_ctl_el0: u64;
        let cntv_cval_el0: u64;
        let cntvoff_el2: u64;
        // SAFETY: the host kernel runs at EL2 in VHE mode, so EL2 register accesses are valid here.
        unsafe {
            asm!(
                "mrs {daif}, daif",
                "mrs {hcr}, hcr_el2",
                "mrs {vbar_el2}, vbar_el2",
                "mrs {vbar_el1}, vbar_el1",
                "mrs {ich_hcr}, ich_hcr_el2",
                "mrs {ich_vmcr}, ich_vmcr_el2",
                "mrs {tpidr_el1}, tpidr_el1",
                "mrs {cnthctl_el2}, cnthctl_el2",
                "mrs {cntv_ctl_el0}, cntv_ctl_el0",
                "mrs {cntv_cval_el0}, cntv_cval_el0",
                "mrs {cntvoff_el2}, cntvoff_el2",
                hcr = out(reg) hcr,
                vbar_el2 = out(reg) vbar_el2,
                vbar_el1 = out(reg) vbar_el1,
                ich_hcr = out(reg) ich_hcr,
                ich_vmcr = out(reg) ich_vmcr,
                tpidr_el1 = out(reg) tpidr_el1,
                daif = out(reg) daif,
                cnthctl_el2 = out(reg) cnthctl_el2,
                cntv_ctl_el0 = out(reg) cntv_ctl_el0,
                cntv_cval_el0 = out(reg) cntv_cval_el0,
                cntvoff_el2 = out(reg) cntvoff_el2,
                options(nostack),
            );
            HOST_HV_CTX.hcr_el2 = hcr;
            HOST_HV_CTX.vbar_el2 = vbar_el2;
            HOST_HV_CTX.vbar_el1 = vbar_el1;
            HOST_HV_CTX.ich_hcr_el2 = ich_hcr;
            HOST_HV_CTX.ich_vmcr_el2 = ich_vmcr;
            HOST_HV_CTX.tpidr_el1 = tpidr_el1;
            HOST_HV_CTX.daif = daif;
            HOST_HV_CTX.cnthctl_el2 = cnthctl_el2;
            HOST_HV_CTX.cntv_ctl_el0 = cntv_ctl_el0;
            HOST_HV_CTX.cntv_cval_el0 = cntv_cval_el0;
            HOST_HV_CTX.cntvoff_el2 = cntvoff_el2;
        }

        // Step 2: Set VTTBR_EL2 to guest stage-2 page table
        vm.set_guest_root_pagetable();

        // Step 3: Restore guest VGIC state while still using the host VHE
        // translation regime. HCR_EL2 is switched only after this point so the
        // host does not execute general Rust code with guest trapping enabled.
        if vgic.hcr == 0 && vgic.vmcr == 0 {
            super::vgic::vgic_guest_entry_init(self.vgic_num_lrs);
            vgic.hcr = super::vgic::read_hcr();
            vgic.vmcr = super::vgic::read_vmcr();
            super::vgic::save_lrs(self.vgic_num_lrs, &mut vgic.lr_shadow);
        } else {
            super::vgic::restore_guest_state(vgic);
        }

        task.vcpu.lock().set_mode(vcpu.get_mode());

        // Ensure all the above setup is visible before guest entry.
        fence(Ordering::SeqCst);

        // The final HCR_EL2 switch and EL12 sysreg restore are performed in
        // the naked entry path immediately before eret. Keeping Rust execution
        // under the host HCR avoids trapping host sysreg and memory accesses.
    }

    fn save_guest_vgic_state(&self, vgic: &mut VgicState) {
        super::vgic::save_guest_state(vgic);
    }

    fn restore_host_vgic_state(&self) {
        let (ich_hcr, ich_vmcr) = unsafe { (HOST_HV_CTX.ich_hcr_el2, HOST_HV_CTX.ich_vmcr_el2) };
        super::vgic::restore_host_vgic(self.vgic_num_lrs, ich_hcr, ich_vmcr);
    }

    fn update_virtual_timer_irq(&self, guest: &mut GuestVcpu) {
        guest.sysregs.take_pending_into();

        let enabled = (guest.sysregs.cntv_ctl_el0 & TIMER_CTL_ENABLE) != 0;
        let masked = (guest.sysregs.cntv_ctl_el0 & TIMER_CTL_IMASK) != 0;
        let count = guest_virtual_count(&guest.sysregs);
        let expired = enabled && count >= guest.sysregs.cntv_cval_el0;

        if expired {
            guest.sysregs.cntv_ctl_el0 |= TIMER_CTL_ISTATUS;
        } else {
            guest.sysregs.cntv_ctl_el0 &= !TIMER_CTL_ISTATUS;
        }

        if expired && !masked {
            self.irqs_pending.fetch_or(IRQ_BIT_TIMER, Ordering::Release);
        } else {
            self.irqs_pending
                .fetch_and(!IRQ_BIT_TIMER, Ordering::AcqRel);
        }
        self.irqs_pending_mask
            .fetch_or(IRQ_BIT_TIMER, Ordering::Release);
    }

    fn sync_interrupts(&self, vgic: &VgicState) {
        let last_irq_state = self.last_irq_state.load(Ordering::Acquire);
        let timer_shadowed = last_irq_state & IRQ_BIT_TIMER;
        if timer_shadowed == 0 {
            return;
        }

        let timer_active = super::vgic::is_shadow_virq_pending(vgic, GUEST_TIMER_PPI);
        if !timer_active {
            self.irqs_pending
                .fetch_and(!IRQ_BIT_TIMER, Ordering::AcqRel);
            self.irqs_pending_mask
                .fetch_or(IRQ_BIT_TIMER, Ordering::Release);
            self.last_irq_state
                .fetch_and(!IRQ_BIT_TIMER, Ordering::AcqRel);
        }
    }

    fn inject_pending_interrupts(&self, vgic: &mut VgicState) {
        let mask = self.irqs_pending_mask.swap(0, Ordering::AcqRel);
        if mask == 0 {
            return;
        }

        let pending = self.irqs_pending.load(Ordering::Acquire);
        let val = pending & mask;

        if (val & IRQ_BIT_TIMER) != 0 {
            let _ =
                super::vgic::inject_shadow_virq(vgic, GUEST_TIMER_PPI, GUEST_IRQ_PRIORITY, true);
        } else {
            let _ = super::vgic::clear_shadow_virq(vgic, GUEST_TIMER_PPI);
        }

        if (val & IRQ_BIT_EXTERNAL) != 0 {
            let _ = super::vgic::inject_shadow_virq(vgic, 32, GUEST_IRQ_PRIORITY, true);
        } else {
            let _ = super::vgic::clear_shadow_virq(vgic, 32);
        }

        if (val & IRQ_BIT_SOFTWARE) != 0 {
            let _ = super::vgic::inject_shadow_virq(vgic, 3, GUEST_IRQ_PRIORITY, false);
        } else {
            let _ = super::vgic::clear_shadow_virq(vgic, 3);
        }

        self.last_irq_state.store(val, Ordering::Release);
    }

    fn prepare_normal_task_and_save_guest(
        &self,
        task: &crate::task::Task,
        vcpu: &mut GuestVcpu,
        guest_tf: &mut Trapframe,
    ) {
        vcpu.sysregs.take_pending_into();
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

        {
            let vcpu_state = &mut *vcpu;
            self.update_virtual_timer_irq(&mut vcpu_state.guest);
            self.sync_interrupts(&vcpu_state.vgic);
            self.inject_pending_interrupts(&mut vcpu_state.vgic);
        }

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

            match arch_guest_trap_handler(&mut guest_tf, &vm, &mut vcpu.guest) {
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
                    vcpu.guest.sysregs.take_pending_into();
                    {
                        let vcpu_state = &mut *vcpu;
                        self.update_virtual_timer_irq(&mut vcpu_state.guest);
                        self.sync_interrupts(&vcpu_state.vgic);
                        self.inject_pending_interrupts(&mut vcpu_state.vgic);
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
    gic_dist_addr: Option<u64>,
    gic_cpu_addr: Option<u64>,
    gic_redist_addr: Option<u64>,
    mmio_devices: Vec<VirtualMmioDeviceRef>,
    vgic_nr_irqs: u32,
    vgic_initialized: bool,
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

        let pl011: VirtualMmioDeviceRef = Arc::new(super::pl011_mmio::Pl011Mmio::new(0x0900_0000));

        Ok(Self {
            id,
            owner_mm,
            state: Mutex::new(VmInternalState {
                vcpus: Vec::new(),
                memory_slots: MemorySlotManager::new(),
                vmid,
                fast_path_flags: 0,
                gic_dist_addr: None,
                gic_cpu_addr: None,
                gic_redist_addr: None,
                mmio_devices: Vec::from([pl011]),
                vgic_nr_irqs: 64,
                vgic_initialized: false,
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

    pub fn set_gic_device_addr(&self, device_id: u64, addr_type: u64, addr: u64) {
        let mut state = self.state.lock();
        const KVM_ARM_DEVICE_VGIC_V2: u64 = 0;
        const KVM_VGIC_V2_ADDR_TYPE_DIST: u64 = 0;
        const KVM_VGIC_V2_ADDR_TYPE_CPU: u64 = 1;

        if device_id == KVM_ARM_DEVICE_VGIC_V2 {
            match addr_type {
                KVM_VGIC_V2_ADDR_TYPE_DIST => state.gic_dist_addr = Some(addr),
                KVM_VGIC_V2_ADDR_TYPE_CPU => state.gic_cpu_addr = Some(addr),
                _ => {}
            }
        }
    }

    pub fn gic_mmio_range(&self) -> Option<(u64, u64, Option<u64>)> {
        let state = self.state.lock();
        let dist = state.gic_dist_addr?;
        // GICv3: redist present
        if state.gic_redist_addr.is_some() {
            return Some((dist, 0x10000, state.gic_redist_addr));
        }
        // GICv2: cpu interface present
        Some((dist, 0x10000, state.gic_cpu_addr))
    }

    pub fn set_gicv3_addrs(&self, dist_addr: u64, redist_addr: u64) {
        let mut state = self.state.lock();
        state.gic_dist_addr = Some(dist_addr);
        state.gic_redist_addr = Some(redist_addr);
    }

    pub fn register_mmio_device(&self, device: VirtualMmioDeviceRef) {
        self.state.lock().mmio_devices.push(device);
    }

    pub fn find_mmio_device(&self, ipa: u64) -> Option<VirtualMmioDeviceRef> {
        let state = self.state.lock();
        for dev in &state.mmio_devices {
            if dev.handles(ipa) {
                return Some(Arc::clone(dev));
            }
        }
        None
    }

    pub fn set_vgicv3_dist_addr(&self, addr: u64) {
        self.state.lock().gic_dist_addr = Some(addr);
    }

    pub fn set_vgicv3_redist_addr(&self, addr: u64) {
        self.state.lock().gic_redist_addr = Some(addr);
    }

    pub fn set_vgic_nr_irqs(&self, nr_irqs: u32) {
        self.state.lock().vgic_nr_irqs = nr_irqs;
    }

    pub fn vgic_init(&self) -> Result<(), ()> {
        let mut state = self.state.lock();
        if state.vgic_initialized {
            return Ok(());
        }
        let dist_addr = state.gic_dist_addr.ok_or(())?;
        let redist_addr = state.gic_redist_addr.ok_or(())?;
        let nr_irqs = if state.vgic_nr_irqs == 0 {
            64
        } else {
            state.vgic_nr_irqs
        };
        let num_lrs = super::vgic::probe_vgic();

        let dist: VirtualMmioDeviceRef =
            Arc::new(super::vgic_mmio::VgicDist::new(dist_addr, nr_irqs, num_lrs));
        let redist: VirtualMmioDeviceRef =
            Arc::new(super::vgic_mmio::VgicRedist::new(redist_addr, num_lrs));

        state.mmio_devices.push(dist);
        state.mmio_devices.push(redist);
        state.vgic_initialized = true;
        drop(state);
        crate::println!(
            "[VGIC] INIT: dist={:#x} redist={:#x} nr_irqs={}",
            dist_addr,
            redist_addr,
            nr_irqs
        );
        Ok(())
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
        let level = state
            .memory_slots
            .find_slot(gpa)
            .map(|slot| best_stage2_page_level(slot, gpa, hpa))
            .unwrap_or(0);

        let trace_count = STAGE2_MAP_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
        for level in (0..=level).rev() {
            let page_size = stage2_page_size(level);
            let page_mask = page_size - 1;
            match map_stage2_page_at_level_no_flush(
                root,
                gpa & !page_mask,
                hpa & !page_mask,
                writable,
                state.vmid,
                level,
            ) {
                Ok(()) => {
                    set_guest_root_stage2(root, state.vmid);
                    if trace_count < 32 {
                        crate::println!(
                            "[AARCH64-S2-MAP] count={} vmid={} gpa={:#x} hpa={:#x} level={} size={:#x} writable={}",
                            trace_count,
                            state.vmid,
                            gpa,
                            hpa,
                            level,
                            page_size,
                            writable,
                        );
                    }
                    return Ok(());
                }
                Err("Cannot replace existing stage2 page table with a leaf") if level > 0 => {}
                Err(e) => return Err(e),
            }
        }

        Err("Cannot replace existing stage2 page table with a leaf")
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
