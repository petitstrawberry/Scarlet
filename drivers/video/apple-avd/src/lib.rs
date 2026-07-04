#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        iommu::{DmaContext, IommuDomainConfig, IommuDomainType},
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
    sync::Mutex,
    vm,
};

const AVD_DEFAULT_IOVA_BASE: u64 = 0x4000_0000;
const AVD_DEFAULT_IOVA_SIZE: u64 = 0x4000_0000;

const REG_STATUS: usize = 0x0000;
const REG_CONTROL: usize = 0x0004;
const REG_IRQ_STATUS: usize = 0x0010;
const REG_IRQ_MASK: usize = 0x0014;
const REG_FW_BASE_LO: usize = 0x0100;
const REG_FW_BASE_HI: usize = 0x0104;
const REG_FW_SIZE: usize = 0x0108;
const REG_MAILBOX_AP_TO_CM3: usize = 0x0200;
const REG_MAILBOX_CM3_TO_AP: usize = 0x0204;

const CONTROL_CM3_RESET: u32 = 1 << 0;
const CONTROL_CM3_RUN: u32 = 1 << 1;
const CONTROL_IRQ_ENABLE: u32 = 1 << 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppleAvdSoc {
    T8103,
    T6000,
    T6020,
    T8112,
    Unknown,
}

impl AppleAvdSoc {
    fn from_device(device: &PlatformDeviceInfo) -> Self {
        for compatible in device.compatible() {
            match compatible {
                "apple,t8103-avd" => return Self::T8103,
                "apple,t6000-avd" => return Self::T6000,
                "apple,t6020-avd" => return Self::T6020,
                "apple,t8112-avd" => return Self::T8112,
                _ => {}
            }
        }
        Self::Unknown
    }

    fn name(self) -> &'static str {
        match self {
            Self::T8103 => "t8103",
            Self::T6000 => "t6000",
            Self::T6020 => "t6020",
            Self::T8112 => "t8112",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AvdFirmwareState {
    Missing,
    Staged,
    Running,
    Faulted,
}

#[derive(Clone, Copy)]
struct AvdRegisters {
    base: usize,
}

impl AvdRegisters {
    fn new(base: usize) -> Self {
        Self { base }
    }

    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        // SAFETY: `base` is an ioremap'd Apple AVD MMIO base and offsets are
        // register offsets defined by this driver.
        unsafe { mmio::read32(self.base + offset) }
    }

    #[inline]
    fn write32(&self, offset: usize, value: u32) {
        // SAFETY: `base` is an ioremap'd Apple AVD MMIO base and offsets are
        // register offsets defined by this driver.
        unsafe { mmio::write32(self.base + offset, value) }
    }

    fn status(&self) -> u32 {
        self.read32(REG_STATUS)
    }

    fn irq_status(&self) -> u32 {
        self.read32(REG_IRQ_STATUS)
    }

    fn mask_irqs(&self) {
        self.write32(REG_IRQ_MASK, 0);
    }

    fn enable_irqs(&self) {
        self.write32(REG_IRQ_MASK, u32::MAX);
    }

    fn hold_cm3_in_reset(&self) {
        self.write32(REG_CONTROL, CONTROL_CM3_RESET);
    }

    fn run_cm3(&self) {
        self.write32(REG_CONTROL, CONTROL_CM3_RUN | CONTROL_IRQ_ENABLE);
    }

    fn stage_firmware_window(&self, dma_addr: u64, size: usize) {
        self.write32(REG_FW_BASE_LO, dma_addr as u32);
        self.write32(REG_FW_BASE_HI, (dma_addr >> 32) as u32);
        self.write32(REG_FW_SIZE, size as u32);
    }

    fn send_mailbox(&self, value: u32) {
        self.write32(REG_MAILBOX_AP_TO_CM3, value);
    }

    fn recv_mailbox(&self) -> u32 {
        self.read32(REG_MAILBOX_CM3_TO_AP)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AvdStatusSnapshot {
    status: u32,
    irq_status: u32,
    mailbox: u32,
}

/// Apple AVD hardware instance discovered from ADT/FDT.
pub struct AppleAvd {
    name: &'static str,
    soc: AppleAvdSoc,
    paddr: usize,
    size: usize,
    irq: Option<u32>,
    registers: AvdRegisters,
    dma: DmaContext,
    firmware_state: AvdFirmwareState,
}

impl AppleAvd {
    fn new(
        name: &'static str,
        soc: AppleAvdSoc,
        paddr: usize,
        size: usize,
        irq: Option<u32>,
        registers: AvdRegisters,
        dma: DmaContext,
    ) -> Self {
        Self {
            name,
            soc,
            paddr,
            size,
            irq,
            registers,
            dma,
            firmware_state: AvdFirmwareState::Missing,
        }
    }

    /// Return the firmware node name used for this AVD instance.
    ///
    /// # Returns
    ///
    /// Static platform device name from discovery.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Return the physical MMIO base address.
    ///
    /// # Returns
    ///
    /// Physical address of the AVD register aperture.
    pub fn paddr(&self) -> usize {
        self.paddr
    }

    /// Return the physical MMIO aperture size.
    ///
    /// # Returns
    ///
    /// Byte length of the AVD register aperture.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Return the interrupt line discovered for the device.
    ///
    /// # Returns
    ///
    /// Interrupt number when firmware supplied one.
    pub fn irq(&self) -> Option<u32> {
        self.irq
    }

    /// Return the device DMA context.
    ///
    /// # Returns
    ///
    /// DMA context resolved through DART when firmware declares an IOMMU.
    pub fn dma_context(&self) -> &DmaContext {
        &self.dma
    }

    fn snapshot(&self) -> AvdStatusSnapshot {
        AvdStatusSnapshot {
            status: self.registers.status(),
            irq_status: self.registers.irq_status(),
            mailbox: self.registers.recv_mailbox(),
        }
    }

    fn prepare_for_firmware(&mut self, firmware_dma_addr: u64, firmware_size: usize) {
        self.registers.mask_irqs();
        self.registers.hold_cm3_in_reset();
        self.registers
            .stage_firmware_window(firmware_dma_addr, firmware_size);
        self.firmware_state = AvdFirmwareState::Staged;
    }

    fn start_firmware(&mut self) {
        self.registers.enable_irqs();
        self.registers.run_cm3();
        self.firmware_state = AvdFirmwareState::Running;
    }

    fn mark_firmware_faulted(&mut self) {
        self.firmware_state = AvdFirmwareState::Faulted;
    }
}

static AVD_REGISTRY: Mutex<Vec<Arc<Mutex<AppleAvd>>>> = Mutex::new(Vec::new());

fn register_avd(avd: AppleAvd) -> u32 {
    let mut registry = AVD_REGISTRY.lock();
    let id = registry.len() as u32;
    registry.push(Arc::new(Mutex::new(avd)));
    id
}

/// Return a registered Apple AVD instance by index.
///
/// # Arguments
///
/// * `id` - Zero-based registration index.
///
/// # Returns
///
/// Reference-counted AVD instance when present.
pub fn get_apple_avd(id: u32) -> Option<Arc<Mutex<AppleAvd>>> {
    let registry = AVD_REGISTRY.lock();
    registry.get(id as usize).cloned()
}

fn first_mem_resource(device: &PlatformDeviceInfo) -> Option<(usize, usize)> {
    device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .map(|resource| {
            let size = resource
                .end
                .checked_sub(resource.start)
                .and_then(|span| span.checked_add(1))
                .unwrap_or(0);
            (resource.start, size)
        })
}

fn first_irq(device: &PlatformDeviceInfo) -> Option<u32> {
    device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::IRQ))
        .map(|resource| resource.start as u32)
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let (paddr, size) = first_mem_resource(device).ok_or("apple-avd: missing MMIO resource")?;
    if size == 0 {
        return Err("apple-avd: empty MMIO resource");
    }

    let vaddr = vm::ioremap(paddr, size).map_err(|_| "apple-avd: MMIO ioremap failed")?;
    let dma = DeviceManager::get_manager().resolve_platform_dma_context(
        device,
        IommuDomainConfig {
            domain_type: IommuDomainType::Dma,
            iova_base: AVD_DEFAULT_IOVA_BASE,
            iova_size: AVD_DEFAULT_IOVA_SIZE,
        },
    )?;

    let soc = AppleAvdSoc::from_device(device);
    let irq = first_irq(device);
    let avd = AppleAvd::new(
        device.name(),
        soc,
        paddr,
        size,
        irq,
        AvdRegisters::new(vaddr),
        dma,
    );
    let snapshot = avd.snapshot();
    let id = register_avd(avd);

    early_println!(
        "[apple-avd] registered {} id={} soc={} mmio={:#x}+{:#x} irq={:?} status={:#x} irq_status={:#x}",
        device.name(),
        id,
        soc.name(),
        paddr,
        size,
        irq,
        snapshot.status,
        snapshot.irq_status
    );

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_avd_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-avd",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-avd",
            "apple,t6000-avd",
            "apple,t6020-avd",
            "apple,t8112-avd",
            "apple,avd",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_apple_avd_driver);

#[used]
static SCARLET_DRIVER_APPLE_AVD_ANCHOR: fn() = force_link;

/// Keep the Apple AVD driver crate linked into Scarlet module bundles.
pub fn force_link() {}
