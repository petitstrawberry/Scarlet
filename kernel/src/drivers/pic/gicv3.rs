//! ARM Generic Interrupt Controller v3 (GICv3) implementation (AArch64)
//!
//! This driver is intended for environments where trapping MMIO in the IRQ path is
//! undesirable (e.g. QEMU+HVF). It uses the GICv3 system register interface
//! (ICC_*_EL1) to acknowledge and complete interrupts.
//!
//! The distributor / redistributor are still configured via MMIO during init.

use crate::{
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
use core::{
    arch::asm,
    ptr::{read_volatile, write_volatile},
};

/// Maximum number of interrupts supported by this implementation.
const MAX_INTERRUPTS: InterruptId = 1020;

/// Maximum number of CPUs supported by this implementation.
const MAX_CPUS: CpuId = 8;

// Distributor register offsets (GICD)
const GICD_CTLR: usize = 0x0000;
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

/// ARM GICv3 implementation.
pub struct GicV3 {
    dist_base_addr: usize,
    redist_base_addr: usize,
    max_interrupts: InterruptId,
    max_cpus: CpuId,
    last_iar: [u32; MAX_CPUS as usize],
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
            last_iar: [0; MAX_CPUS as usize],
        }
    }

    #[inline]
    fn dist_reg_addr(&self, offset: usize) -> usize {
        self.dist_base_addr + offset
    }

    #[inline]
    fn redist_reg_addr(&self, cpu_id: CpuId, offset: usize) -> usize {
        // Minimal implementation: assume a single redistributor region and CPU0.
        // This matches the current single-core bring-up.
        let _ = cpu_id;
        self.redist_base_addr + offset
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
        unsafe {
            // Disable distributor while programming.
            write_volatile(self.dist_reg_addr(GICD_CTLR) as *mut u32, 0x0);

            // Put all interrupts into Group 1 (non-secure).
            let words = (self.max_interrupts as usize + 32) / 32;
            for i in 0..words {
                write_volatile(
                    self.dist_reg_addr(GICD_IGROUPR + i * 4) as *mut u32,
                    0xFFFF_FFFF,
                );
            }

            // Enable Group 0 + Group 1.
            write_volatile(self.dist_reg_addr(GICD_CTLR) as *mut u32, 0x3);
        }
    }

    fn init_redistributor(&self, cpu_id: CpuId) {
        // Wake up redistributor (best-effort).
        let waker = self.redist_reg_addr(cpu_id, GICR_WAKER);
        unsafe {
            let mut v = read_volatile(waker as *const u32);
            // Clear ProcessorSleep (bit 1).
            v &= !(1 << 1);
            write_volatile(waker as *mut u32, v);

            // Wait for ChildrenAsleep (bit 2) to clear.
            for _ in 0..1_000_000 {
                let cur = read_volatile(waker as *const u32);
                if (cur & (1 << 2)) == 0 {
                    break;
                }
            }

            // Group 1 for SGI/PPI.
            write_volatile(
                self.redist_sgi_reg_addr(cpu_id, GICR_IGROUPR0) as *mut u32,
                0xFFFF_FFFF,
            );

            // Set virtual timer PPI priority to 0x80.
            let timer_ppi = crate::drivers::pic::arm_generic_timer::CNTV_PPI_IRQ;
            write_volatile(
                (self.redist_sgi_reg_addr(cpu_id, GICR_IPRIORITYR) + timer_ppi as usize) as *mut u8,
                0x80,
            );
        }
    }

    fn init_cpu_interface_sysregs(&self) {
        // Enable system register interface and unmask Group 1 interrupts.
        // ICC_SRE_EL1.SRE (bit 0) must be 1.
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
        // Configure distributor + redistributor for CPU0.
        self.init_distributor();
        self.init_redistributor(0);
        self.init_cpu_interface_sysregs();
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
                write_volatile(
                    self.redist_sgi_reg_addr(cpu_id, GICR_ISENABLER0) as *mut u32,
                    bit,
                );
            } else {
                write_volatile(self.dist_enable_addr(interrupt_id) as *mut u32, bit);
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
                write_volatile(
                    self.redist_sgi_reg_addr(cpu_id, GICR_ICENABLER0) as *mut u32,
                    bit,
                );
            } else {
                write_volatile(self.dist_disable_addr(interrupt_id) as *mut u32, bit);
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
                write_volatile(
                    (self.redist_sgi_reg_addr(0, GICR_IPRIORITYR) + interrupt_id as usize)
                        as *mut u8,
                    priority as u8,
                );
            } else {
                write_volatile(
                    self.dist_priority_addr(interrupt_id) as *mut u8,
                    priority as u8,
                );
            }
        }

        Ok(())
    }

    fn get_priority(&self, interrupt_id: InterruptId) -> InterruptResult<Priority> {
        self.validate_interrupt_id(interrupt_id)?;

        let v = unsafe {
            if interrupt_id < 32 {
                read_volatile(
                    (self.redist_sgi_reg_addr(0, GICR_IPRIORITYR) + interrupt_id as usize)
                        as *const u8,
                )
            } else {
                read_volatile(self.dist_priority_addr(interrupt_id) as *const u8)
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
        self.last_iar[cpu_id as usize] = iar;

        let intid = iar & 0x3FF;
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

        let iar = self.last_iar[cpu_id as usize];
        let eoi_value = if iar != 0 { iar } else { interrupt_id };
        write_icc_eoir1_el1(eoi_value);
        self.last_iar[cpu_id as usize] = 0;
        Ok(())
    }

    fn is_pending(&self, interrupt_id: InterruptId) -> bool {
        if self.validate_interrupt_id(interrupt_id).is_err() {
            return false;
        }

        unsafe {
            if interrupt_id < 32 {
                let pending =
                    read_volatile(self.redist_sgi_reg_addr(0, GICR_ISPENDR0) as *const u32);
                (pending & (1 << (interrupt_id % 32))) != 0
            } else {
                let word_offset = interrupt_id / 32;
                let bit_offset = interrupt_id % 32;
                let pending_addr = self.dist_reg_addr(GICD_ISPENDR + (word_offset as usize * 4));
                let pending = read_volatile(pending_addr as *const u32);
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

    let dist_base_addr = mem_resources
        .get(0)
        .map(|r| r.start as usize)
        .ok_or("No memory resource found for GICv3 distributor")?;

    let redist_base_addr = mem_resources
        .get(1)
        .map(|r| r.start as usize)
        .ok_or("No memory resource found for GICv3 redistributor")?;

    // TODO: Parse actual interrupt/CPU counts from DT.
    let max_interrupts = 256;
    let max_cpus = 4;

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

    DeviceManager::get_mut_manager().register_driver(Box::new(driver), DriverPriority::Critical)
}

early_initcall!(register_driver);
