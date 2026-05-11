//! ARM Generic Interrupt Controller v3 (GICv3) implementation (AArch64)
//!
//! This driver is intended for environments where trapping MMIO in the IRQ path is
//! undesirable (e.g. QEMU+HVF). It uses the GICv3 system register interface
//! (ICC_*_EL1) to acknowledge and complete interrupts.
//!
//! The distributor / redistributor are still configured via MMIO during init.

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
        CpuId, InterruptError, InterruptId, InterruptManager, InterruptResult, Priority,
        controllers::ExternalInterruptController,
    },
};

use alloc::{boxed::Box, vec};
use core::arch::asm;

/// Maximum number of interrupts supported by this implementation.
const MAX_INTERRUPTS: InterruptId = 1020;

/// Maximum number of CPUs supported by this implementation.
const MAX_CPUS: CpuId = 8;

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

            // Set virtual timer PPI priority to 0x80.
            let timer_ppi = crate::drivers::pic::arm_generic_timer::CNTV_PPI_IRQ;
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
        write_icc_sre_el2(1);
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
        // Configure distributor + redistributor for CPU0.

        crate::early_println!("[interrupt] GICv3 init: distributor...");
        self.init_distributor();

        crate::early_println!("[interrupt] GICv3 init: redistributor...");
        self.init_redistributor(0);

        crate::early_println!("[interrupt] GICv3 init: sysregs...");
        self.init_cpu_interface_sysregs();

        crate::early_println!("[interrupt] GICv3 init: done");
        Ok(())
    }

    fn enable_interrupt(
        &mut self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
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

    fn disable_interrupt(
        &mut self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
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

    fn claim_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<Option<InterruptId>> {
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

    fn complete_interrupt(
        &mut self,
        cpu_id: CpuId,
        interrupt_id: InterruptId,
    ) -> InterruptResult<()> {
        self.validate_interrupt_id(interrupt_id)?;
        self.validate_cpu_id(cpu_id)?;

        // Stateless completion: write INTID back.
        write_icc_eoir1_el1(interrupt_id);
        Ok(())
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
    // PlatformDeviceInfo doesn't expose CPU topology here; current bring-up is single-core.
    let max_cpus = 1;

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

    InterruptManager::with_manager(|manager| {
        manager
            .register_external_controller(gic)
            .map_err(|_| "Failed to register GICv3")?;
        Ok(())
    })?;

    crate::arch::interrupt::configure_timer_interrupt_route(
        crate::arch::interrupt::TimerInterruptRoute::ExternalControllerIrq,
        Some(crate::drivers::pic::arm_generic_timer::CNTV_PPI_IRQ),
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
