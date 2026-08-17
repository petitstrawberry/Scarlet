//! ARM Generic Interrupt Controller v3 (GICv3) implementation (AArch64)
//!
//! This driver is intended for environments where trapping MMIO in the IRQ path is
//! undesirable (e.g. QEMU+HVF). It uses the GICv3 system register interface
//! (ICC_*_EL1) to acknowledge and complete interrupts.
//!
//! The distributor / redistributor are still configured via MMIO during init.

use crate::device::platform::resource::PlatformDeviceResource;
use crate::{
    arch::mmio,
    device::{
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_initcall,
    interrupt::{
        CpuId, InterruptError, InterruptId, InterruptResult, Priority,
        controllers::{ExternalInterruptController, IrqFlow, IrqMapping, PendingIrq},
    },
};

use alloc::{boxed::Box, vec};
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum number of interrupts supported by this implementation.
const MAX_INTERRUPTS: InterruptId = 1020;

/// Maximum number of CPUs supported by this implementation.
const MAX_CPUS: CpuId = 8;

/// SGI used by the scheduler to request a reschedule on another CPU.
const RESCHEDULE_SGI: u32 = 0;

/// Sentinel used until a logical CPU publishes its architectural affinity.
const INVALID_MPIDR_AFFINITY: u64 = u64::MAX;

/// MPIDR affinity fields (Aff3 and Aff2:Aff0), excluding U/MT and RES0 bits.
const MPIDR_AFFINITY_MASK: u64 = 0xFF00_FFFF_FF;

// Distributor register offsets (GICD)
const GICD_CTLR: usize = 0x0000;
const GICD_TYPER: usize = 0x0004;
const GICD_IGROUPR: usize = 0x0080;
const GICD_ISENABLER: usize = 0x0100;
const GICD_ICENABLER: usize = 0x0180;
const GICD_ISPENDR: usize = 0x0200;
const GICD_IPRIORITYR: usize = 0x0400;

// Redistributor register offsets (GICR)
// GICv3 redistributor has RD frame at base, and SGI/PPI frame at base + 0x10000.
const GICR_WAKER: usize = 0x0014;
const GICR_SGI_BASE: usize = 0x10000;
const GICR_IGROUPR0: usize = 0x0080;
const GICR_ISENABLER0: usize = 0x0100;
const GICR_ICENABLER0: usize = 0x0180;
const GICR_ISPENDR0: usize = 0x0200;
const GICR_IPRIORITYR: usize = 0x0400;

#[inline]
fn read_icc_iar1_el1() -> u32 {
    let v: u64;
    unsafe {
        asm!("mrs {0}, ICC_IAR1_EL1", out(reg) v, options(nostack));
    }
    v as u32
}

#[inline]
fn write_icc_eoir1_el1(v: u32) {
    unsafe {
        asm!(
            "msr ICC_EOIR1_EL1, {0}",
            "isb",
            in(reg) (v as u64),
            options(nostack)
        );
    }
}

#[inline]
fn current_el() -> u64 {
    let el: u64;
    unsafe {
        asm!("mrs {}, CurrentEL", out(reg) el, options(nostack));
    }
    (el >> 2) & 0x3
}

#[inline]
fn write_icc_sre_el1(v: u64) {
    unsafe {
        asm!("msr ICC_SRE_EL1, {0}", "isb", in(reg) v, options(nostack));
    }
}

#[inline]
fn write_icc_sre_el2(v: u64) {
    unsafe {
        asm!("msr ICC_SRE_EL2, {0}", "isb", in(reg) v, options(nostack));
    }
}

#[inline]
fn write_icc_pmr_el1(v: u64) {
    unsafe {
        asm!("msr ICC_PMR_EL1, {0}", "isb", in(reg) v, options(nostack));
    }
}

#[inline]
fn write_icc_bpr1_el1(v: u64) {
    unsafe {
        asm!("msr ICC_BPR1_EL1, {0}", "isb", in(reg) v, options(nostack));
    }
}

#[inline]
fn write_icc_ctlr_el1(v: u64) {
    unsafe {
        asm!("msr ICC_CTLR_EL1, {0}", "isb", in(reg) v, options(nostack));
    }
}

#[inline]
fn write_icc_igrpen1_el1(v: u64) {
    unsafe {
        asm!("msr ICC_IGRPEN1_EL1, {0}", "isb", in(reg) v, options(nostack));
    }
}

#[inline]
fn write_icc_sgi1r_el1(v: u64) {
    unsafe {
        asm!(
            "dsb ishst",
            "msr ICC_SGI1R_EL1, {0}",
            "isb",
            in(reg) v,
            options(nostack)
        );
    }
}

#[inline]
fn current_mpidr_affinity() -> u64 {
    let mpidr: u64;
    unsafe {
        asm!("mrs {0}, MPIDR_EL1", out(reg) mpidr, options(nostack));
    }
    mpidr & MPIDR_AFFINITY_MASK
}

#[inline]
fn sgi1r_for_affinity(intid: u64, affinity: u64) -> InterruptResult<u64> {
    let aff0 = affinity & 0xff;
    let aff1 = (affinity >> 8) & 0xff;
    let aff2 = (affinity >> 16) & 0xff;
    let aff3 = (affinity >> 32) & 0xff;

    // ICC_SGI1R_EL1.TargetList addresses Aff0[3:0]. Range Selector support
    // for Aff0 >= 16 is optional and Scarlet does not negotiate it yet.
    if aff0 >= 16 {
        return Err(InterruptError::InvalidCpuId);
    }

    Ok((aff3 << 48)
        | (aff2 << 32)
        | (intid << 24)
        | (aff1 << 16)
        | (1u64 << aff0))
}

#[inline]
fn gicd_max_interrupt_id(dist_base_addr: usize) -> InterruptId {
    // GICD_TYPER.ITLinesNumber[4:0] gives (#interrupts / 32) - 1.
    // Convert this into a 0-based maximum interrupt ID.
    let typer = unsafe { mmio::read32(dist_base_addr + GICD_TYPER) };
    let it_lines = (typer & 0x1f) as InterruptId;
    let total = (it_lines + 1) * 32;
    total.saturating_sub(1)
}

/// ARM GICv3 implementation.
pub struct GicV3 {
    dist_base_addr: usize,
    redist_base_addr: usize,
    max_interrupts: InterruptId,
    max_cpus: CpuId,
    cpu_mpidr_affinity: [AtomicU64; MAX_CPUS as usize],
}

impl GicV3 {
    pub fn new(
        dist_base_addr: usize,
        redist_base_addr: usize,
        max_interrupts: InterruptId,
        max_cpus: CpuId,
    ) -> Self {
        Self {
            dist_base_addr,
            redist_base_addr,
            max_interrupts: max_interrupts.min(MAX_INTERRUPTS),
            max_cpus: max_cpus.min(MAX_CPUS),
            cpu_mpidr_affinity: [const { AtomicU64::new(INVALID_MPIDR_AFFINITY) };
                MAX_CPUS as usize],
        }
    }

    #[inline]
    fn dist_reg_addr(&self, offset: usize) -> usize {
        self.dist_base_addr + offset
    }

    #[inline]
    fn redist_reg_addr(&self, cpu_id: CpuId, offset: usize) -> usize {
        // GICv3 redistributor frames are typically laid out per-CPU with a 128KB stride
        // (64KB RD frame + 64KB SGI/PPI frame).
        const GICR_STRIDE: usize = 0x20000;
        self.redist_base_addr + (cpu_id as usize * GICR_STRIDE) + offset
    }

    #[inline]
    fn redist_sgi_reg_addr(&self, cpu_id: CpuId, offset: usize) -> usize {
        self.redist_reg_addr(cpu_id, GICR_SGI_BASE + offset)
    }

    fn validate_interrupt_id(&self, interrupt_id: InterruptId) -> InterruptResult<()> {
        if interrupt_id > self.max_interrupts {
            Err(InterruptError::InvalidInterruptId)
        } else {
            Ok(())
        }
    }

    fn validate_cpu_id(&self, cpu_id: CpuId) -> InterruptResult<()> {
        if cpu_id >= self.max_cpus {
            Err(InterruptError::InvalidCpuId)
        } else {
            Ok(())
        }
    }

    fn init_distributor(&self) {
        // Put all interrupts into Group 1 (non-secure).
        let words = (self.max_interrupts as usize + 32) / 32;

        crate::early_println!(
            "[interrupt] GICv3 dist: CTLR@{:#x} <= 0",
            self.dist_reg_addr(GICD_CTLR)
        );
        unsafe {
            // Disable distributor while programming.
            mmio::write32(self.dist_reg_addr(GICD_CTLR), 0x0);
        }

        crate::early_println!(
            "[interrupt] GICv3 dist: IGROUPR words={} base={:#x}",
            words,
            self.dist_reg_addr(GICD_IGROUPR)
        );
        for i in 0..words {
            // Keep per-iteration logging off; HVF aborts are synchronous so the last marker is enough.
            unsafe {
                mmio::write32(self.dist_reg_addr(GICD_IGROUPR + i * 4), 0xFFFF_FFFF);
            }
        }

        crate::early_println!(
            "[interrupt] GICv3 dist: CTLR@{:#x} <= 3",
            self.dist_reg_addr(GICD_CTLR)
        );
        unsafe {
            // Enable Group 0 + Group 1.
            mmio::write32(self.dist_reg_addr(GICD_CTLR), 0x3);
        }
    }

    fn init_redistributor(&self, cpu_id: CpuId) {
        // Wake up redistributor (best-effort).
        let waker = self.redist_reg_addr(cpu_id, GICR_WAKER);
        unsafe {
            let mut v = mmio::read32(waker);
            // Clear ProcessorSleep (bit 1).
            v &= !(1 << 1);
            mmio::write32(waker, v);

            // Wait for ChildrenAsleep (bit 2) to clear.
            for _ in 0..1_000_000 {
                let cur = mmio::read32(waker);
                if (cur & (1 << 2)) == 0 {
                    break;
                }
            }

            // Group 1 for SGI/PPI.
            mmio::write32(self.redist_sgi_reg_addr(cpu_id, GICR_IGROUPR0), 0xFFFF_FFFF);

            // Enable SGI 0 used by the scheduler as the reschedule IPI.
            mmio::write8(
                self.redist_sgi_reg_addr(cpu_id, GICR_IPRIORITYR) + RESCHEDULE_SGI as usize,
                0x80,
            );
            mmio::write32(
                self.redist_sgi_reg_addr(cpu_id, GICR_ISENABLER0),
                1 << RESCHEDULE_SGI,
            );

            // Set virtual timer PPI priority to 0x80.
            let timer_ppi = crate::drivers::pic::arm_generic_timer::timer_ppi_irq();
            mmio::write8(
                self.redist_sgi_reg_addr(cpu_id, GICR_IPRIORITYR) + timer_ppi as usize,
                0x80,
            );
        }
    }

    fn init_cpu_interface_sysregs(&self) {
        // Enable system register interface and unmask Group 1 interrupts.
        // ICC_SRE_EL1.SRE (bit 0) must be 1.
        // ICC_SRE_EL2.SRE must also be 1 to allow ICH_*_EL2 register access
        // from the hypervisor (VGIC save/restore). Without this, any MRS/MSR
        // on ICH_HCR_EL2, ICH_VMCR_EL2, etc. traps with EC=0x18.
        // Only write ICC_SRE_EL2 when running at EL2; accessing EL2 registers
        // from EL1 causes an Undefined Instruction exception.
        if current_el() >= 2 {
            write_icc_sre_el2(1);
        }
        write_icc_sre_el1(1);
        write_icc_pmr_el1(0xFF);
        write_icc_bpr1_el1(0);
        write_icc_ctlr_el1(0);
        write_icc_igrpen1_el1(1);
    }

    fn dist_enable_addr(&self, interrupt_id: InterruptId) -> usize {
        let word_offset = interrupt_id / 32;
        self.dist_reg_addr(GICD_ISENABLER + (word_offset as usize * 4))
    }

    fn dist_disable_addr(&self, interrupt_id: InterruptId) -> usize {
        let word_offset = interrupt_id / 32;
        self.dist_reg_addr(GICD_ICENABLER + (word_offset as usize * 4))
    }

    fn dist_priority_addr(&self, interrupt_id: InterruptId) -> usize {
        self.dist_reg_addr(GICD_IPRIORITYR + interrupt_id as usize)
    }
}

impl ExternalInterruptController for GicV3 {
    fn init(&mut self) -> InterruptResult<()> {
        crate::early_println!(
            "[interrupt] GICv3 init: dist={:#x} redist={:#x}",
            self.dist_base_addr,
            self.redist_base_addr
        );
        self.init_distributor();
        Ok(())
    }

    fn enable_interrupt(&self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        self.validate_cpu_id(cpu_id)?;

        let bit = 1u32 << (interrupt_id % 32);

        unsafe {
            if interrupt_id < 32 {
                // SGI/PPI live in redistributor.
                mmio::write32(self.redist_sgi_reg_addr(cpu_id, GICR_ISENABLER0), bit);
            } else {
                mmio::write32(self.dist_enable_addr(interrupt_id), bit);
            }
        }

        Ok(())
    }

    fn disable_interrupt(&self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        self.validate_cpu_id(cpu_id)?;

        let bit = 1u32 << (interrupt_id % 32);

        unsafe {
            if interrupt_id < 32 {
                mmio::write32(self.redist_sgi_reg_addr(cpu_id, GICR_ICENABLER0), bit);
            } else {
                mmio::write32(self.dist_disable_addr(interrupt_id), bit);
            }
        }

        Ok(())
    }

    fn mask_irq(&self, irq: &PendingIrq) -> InterruptResult<()> {
        self.disable_interrupt(irq.mapping.hwirq, irq.cpu_id)
    }

    fn unmask_irq(&self, irq: &PendingIrq) -> InterruptResult<()> {
        self.enable_interrupt(irq.mapping.hwirq, irq.cpu_id)
    }

    fn set_priority(
        &mut self,
        interrupt_id: InterruptId,
        priority: Priority,
    ) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;

        unsafe {
            if interrupt_id < 32 {
                mmio::write8(
                    self.redist_sgi_reg_addr(0, GICR_IPRIORITYR) + interrupt_id as usize,
                    priority as u8,
                );
            } else {
                mmio::write8(self.dist_priority_addr(interrupt_id), priority as u8);
            }
        }

        Ok(())
    }

    fn get_priority(&self, interrupt_id: InterruptId) -> InterruptResult<Priority> {
        self.validate_interrupt_id(interrupt_id)?;

        let v = unsafe {
            if interrupt_id < 32 {
                mmio::read8(self.redist_sgi_reg_addr(0, GICR_IPRIORITYR) + interrupt_id as usize)
            } else {
                mmio::read8(self.dist_priority_addr(interrupt_id))
            }
        };

        Ok(v as Priority)
    }

    fn set_threshold(&mut self, _cpu_id: CpuId, threshold: Priority) -> InterruptResult<()> {
        // Priority mask via sysreg.
        write_icc_pmr_el1(threshold as u64);
        Ok(())
    }

    fn get_threshold(&self, _cpu_id: CpuId) -> InterruptResult<Priority> {
        // Not currently needed.
        Ok(0)
    }

    fn claim_interrupt(&self, cpu_id: CpuId) -> InterruptResult<Option<InterruptId>> {
        self.validate_cpu_id(cpu_id)?;

        let iar = read_icc_iar1_el1();

        // ICC_IAR1_EL1 returns INTID in bits [23:0] in system register mode.
        let intid = iar & 0x00FF_FFFF;
        if intid >= 1020 {
            Ok(None)
        } else {
            Ok(Some(intid))
        }
    }

    fn claim_pending_irq(&self, cpu_id: CpuId) -> InterruptResult<Option<PendingIrq>> {
        Ok(self
            .claim_interrupt(cpu_id)?
            .map(|interrupt_id| PendingIrq {
                mapping: IrqMapping::legacy(interrupt_id, IrqFlow::FastEoi),
                cpu_id,
            }))
    }

    fn complete_interrupt(&self, cpu_id: CpuId, interrupt_id: InterruptId) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        self.validate_cpu_id(cpu_id)?;

        // Stateless completion: write INTID back.
        write_icc_eoir1_el1(interrupt_id);
        Ok(())
    }

    fn eoi_irq(&self, irq: &PendingIrq) -> InterruptResult<()> {
        self.complete_interrupt(irq.cpu_id, irq.mapping.hwirq)
    }

    fn is_pending(&self, interrupt_id: InterruptId) -> bool {
        if self.validate_interrupt_id(interrupt_id).is_err() {
            return false;
        }

        unsafe {
            if interrupt_id < 32 {
                let pending = mmio::read32(self.redist_sgi_reg_addr(0, GICR_ISPENDR0));
                (pending & (1 << (interrupt_id % 32))) != 0
            } else {
                let word_offset = interrupt_id / 32;
                let bit_offset = interrupt_id % 32;
                let pending_addr = self.dist_reg_addr(GICD_ISPENDR + (word_offset as usize * 4));
                let pending = mmio::read32(pending_addr);
                (pending & (1 << bit_offset)) != 0
            }
        }
    }

    fn max_interrupts(&self) -> InterruptId {
        self.max_interrupts
    }

    fn max_cpus(&self) -> CpuId {
        self.max_cpus
    }

    fn translate_irq_resource(
        &self,
        resource: &PlatformDeviceResource,
    ) -> InterruptResult<InterruptId> {
        if resource.res_type != PlatformDeviceResourceType::IRQ {
            return Err(InterruptError::InvalidInterruptId);
        }

        Ok(resource
            .irq_metadata
            .map_or(resource.start as InterruptId, |metadata| {
                match metadata.irq_type {
                    0 => 32 + metadata.irq_number,
                    1 => 16 + metadata.irq_number,
                    _ => metadata.irq_number,
                }
            }))
    }

    fn map_irq_resource(&self, resource: &PlatformDeviceResource) -> InterruptResult<IrqMapping> {
        let hwirq = self.translate_irq_resource(resource)?;
        Ok(IrqMapping::legacy(hwirq, IrqFlow::FastEoi))
    }

    fn init_for_cpu(&mut self, cpu_id: CpuId) -> InterruptResult<()> {
        self.validate_cpu_id(cpu_id)?;
        self.cpu_mpidr_affinity[cpu_id as usize]
            .store(current_mpidr_affinity(), Ordering::Release);
        self.init_redistributor(cpu_id);
        self.init_cpu_interface_sysregs();
        Ok(())
    }

    fn send_ipi(
        &self,
        target_cpu_id: CpuId,
        ipi_type: crate::interrupt::controllers::LocalInterruptType,
    ) -> InterruptResult<()> {
        self.validate_cpu_id(target_cpu_id)?;

        let intid = match ipi_type {
            crate::interrupt::controllers::LocalInterruptType::Software => RESCHEDULE_SGI as u64,
            crate::interrupt::controllers::LocalInterruptType::External => 1u64,
            crate::interrupt::controllers::LocalInterruptType::Timer => {
                crate::drivers::pic::arm_generic_timer::timer_ppi_irq() as u64
            }
        };

        if intid >= 16 {
            return Err(InterruptError::InvalidInterruptId);
        }

        let affinity = self.cpu_mpidr_affinity[target_cpu_id as usize].load(Ordering::Acquire);
        if affinity == INVALID_MPIDR_AFFINITY {
            return Err(InterruptError::InvalidCpuId);
        }

        // ICC_SGI1R_EL1 targets architectural affinity, not Scarlet's
        // scheduler CPU number. In particular, SC7180 identifies its CPUs as
        // Aff1=0..7, Aff0=0, whereas QEMU's flat topology uses Aff1=0,
        // Aff0=0..N. Record each CPU's MPIDR during per-CPU initialization and
        // encode the target from that affinity.
        let sgi1r = sgi1r_for_affinity(intid, affinity)?;
        write_icc_sgi1r_el1(sgi1r);

        Ok(())
    }
}

unsafe impl Send for GicV3 {}
unsafe impl Sync for GicV3 {}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    // Extract distributor and redistributor base addresses from device tree.
    let mem_resources: alloc::vec::Vec<_> = device
        .get_resources()
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    let dist_paddr = mem_resources
        .get(0)
        .map(|r| r.start)
        .ok_or("No memory resource found for GICv3 distributor")?;
    let dist_size = mem_resources
        .get(0)
        .map(|r| r.end - r.start + 1)
        .unwrap_or(0x10000);

    let redist_paddr = mem_resources
        .get(1)
        .map(|r| r.start)
        .ok_or("No memory resource found for GICv3 redistributor")?;
    let redist_size = mem_resources
        .get(1)
        .map(|r| r.end - r.start + 1)
        .unwrap_or(0x20000);

    // Map distributor and redistributor MMIO regions into the kernel virtual address space.
    let dist_base_addr = crate::vm::ioremap(dist_paddr, dist_size).map_err(|e| {
        crate::early_println!(
            "[interrupt] GICv3 dist ioremap({:#x}, {:#x}) failed: {}",
            dist_paddr,
            dist_size,
            e
        );
        e
    })?;
    let redist_base_addr = crate::vm::ioremap(redist_paddr, redist_size).map_err(|e| {
        crate::early_println!(
            "[interrupt] GICv3 redist ioremap({:#x}, {:#x}) failed: {}",
            redist_paddr,
            redist_size,
            e
        );
        crate::vm::iounmap(dist_base_addr);
        e
    })?;

    let max_interrupts = gicd_max_interrupt_id(dist_base_addr);
    let max_cpus = crate::environment::MAX_NUM_CPUS as u32;

    crate::early_println!(
        "[interrupt] GICv3 selected: dist={:#x} redist={:#x} max_intid={} max_cpus={}",
        dist_base_addr,
        redist_base_addr,
        max_interrupts,
        max_cpus
    );

    let gic = Box::new(GicV3::new(
        dist_base_addr,
        redist_base_addr,
        max_interrupts,
        max_cpus,
    ));

    crate::interrupt::InterruptManager::global()
        .register_external_controller(gic)
        .map_err(|_| "Failed to register GICv3")?;

    crate::arch::interrupt::configure_timer_interrupt_route(
        crate::arch::interrupt::TimerInterruptRoute::ExternalControllerIrq,
        Some(crate::drivers::pic::arm_generic_timer::timer_ppi_irq()),
    );

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "arm,gic-v3",
        probe_fn,
        remove_fn,
        vec!["arm,gic-v3", "arm,gic-v3.1", "arm,gic-v3.2"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical)
}

early_initcall!(register_driver);
