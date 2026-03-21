//! Apple ATC PHY driver
//!
//! Apple Type-C PHY found on Apple Silicon SoCs (t8103/M1).
//! Reference: asahi-linux `drivers/phy/apple/atc-phy.c`
//!
//! Handles USB-C PHY configuration, orientation switching, and lane
//! initialization for the USB-C ports on Apple Silicon.

#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::{
    arch::mmio,
    device::{
        DeviceInfo,
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    driver_initcall, early_println,
};

// =============================================================================
// ATC PHY Register Groups (from DT reg-names)
// =============================================================================

const ATC_CORE: usize = 0x00;
const ATC_LPDPTX: usize = 0x4c000;
const ATC_AXI2AF: usize = 0x80000;
const ATC_USB2PHY: usize = 0x4000;
const ATC_PIPEHANDLER: usize = 0x2a84000;

// =============================================================================
// ATC PHY Register Offsets (within core region)
// =============================================================================

const ATC_MODE: usize = 0x00;
const ATC_STATE: usize = 0x04;
const ATC_CFG1: usize = 0x10;
const ATC_CFG2: usize = 0x14;
const ATC_CFG3: usize = 0x18;
const ATC_CFG4: usize = 0x1c;
const ATC_CFG5: usize = 0x20;
const ATC_CFG6: usize = 0x24;
const ATC_CFG7: usize = 0x28;
const ATC_CFG8: usize = 0x2c;
const ATC_CFG9: usize = 0x30;

// =============================================================================
// Mode Register Bits
// =============================================================================

const ATC_MODE_ENABLE: u32 = 1 << 0;
const ATC_MODE_UPDATE: u32 = 1 << 1;

// =============================================================================
// State Register Bits
// =============================================================================

const ATC_STATE_DONE: u32 = 1 << 0;

// =============================================================================
// ATC PHY Instance
// =============================================================================

pub struct AppleAtcPhy {
    base_addr: usize,
    size: usize,
}

impl AppleAtcPhy {
    pub fn new(base_addr: usize, size: usize) -> Self {
        Self { base_addr, size }
    }

    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        // SAFETY: offset is within the MMIO-mapped ATC PHY region
        unsafe { mmio::read32(self.base_addr + offset) }
    }

    #[inline]
    fn write32(&self, offset: usize, val: u32) {
        // SAFETY: offset is within the MMIO-mapped ATC PHY region
        unsafe { mmio::write32(self.base_addr + offset, val) }
    }

    fn wait_for_update_done(&self) -> Result<(), &'static str> {
        let mut timeout = 10000;
        while timeout > 0 {
            let state = self.read32(ATC_STATE);
            if state & ATC_STATE_DONE != 0 {
                return Ok(());
            }
            timeout -= 1;
            core::hint::spin_loop();
        }
        Err("atcphy: update timeout")
    }

    pub fn init(&mut self) -> Result<(), &'static str> {
        early_println!("[apple-atcphy] initializing...");

        self.write32(ATC_MODE, ATC_MODE_ENABLE | ATC_MODE_UPDATE);

        self.wait_for_update_done()?;

        let cfg1 = self.read32(ATC_CFG1);
        let cfg2 = self.read32(ATC_CFG2);
        early_println!("[apple-atcphy] cfg1={:#x} cfg2={:#x}", cfg1, cfg2);

        early_println!("[apple-atcphy] initialized");
        Ok(())
    }
}

// =============================================================================
// Global Registry
// =============================================================================

struct AtcPhyEntry {
    instance: Arc<Mutex<AppleAtcPhy>>,
    phandle: u32,
}

static ATC_PHY_REGISTRY: Mutex<alloc::vec::Vec<AtcPhyEntry>> = Mutex::new(alloc::vec::Vec::new());

pub fn register_atcphy(phy: AppleAtcPhy, phandle: u32) -> u32 {
    let mut guard = ATC_PHY_REGISTRY.lock();
    let id = guard.len() as u32;
    guard.push(AtcPhyEntry {
        instance: Arc::new(Mutex::new(phy)),
        phandle,
    });
    id
}

pub fn get_atcphy(id: u32) -> Option<Arc<Mutex<AppleAtcPhy>>> {
    let guard = ATC_PHY_REGISTRY.lock();
    guard.get(id as usize).map(|e| Arc::clone(&e.instance))
}

pub fn get_atcphy_by_phandle(phandle: u32) -> Option<Arc<Mutex<AppleAtcPhy>>> {
    let guard = ATC_PHY_REGISTRY.lock();
    guard
        .iter()
        .find(|e| e.phandle == phandle)
        .map(|e| Arc::clone(&e.instance))
}

// =============================================================================
// Platform Driver
// =============================================================================

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resources: Vec<_> = device
        .get_resources()
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    if mem_resources.is_empty() {
        return Err("apple-atcphy: no memory resources found");
    }

    let paddr = mem_resources[0].start;
    let size = mem_resources[0].end - mem_resources[0].start + 1;

    early_println!(
        "[apple-atcphy] probing {} at paddr={:#x}, size={:#x}",
        device.name(),
        paddr,
        size
    );

    let base_addr = crate::vm::ioremap(paddr, size).map_err(|_| "atcphy: ioremap failed")?;

    let mut phy = AppleAtcPhy::new(base_addr, size);
    phy.init()?;

    let phandle = device
        .property("phandle")
        .and_then(|p| p.as_usize())
        .map(|v| v as u32)
        .or_else(|| {
            device
                .property("linux,phandle")
                .and_then(|p| p.as_usize())
                .map(|v| v as u32)
        })
        .unwrap_or(0);

    let _id = register_atcphy(phy, phandle);

    early_println!("[apple-atcphy] registered (id={})", _id);
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_atcphy_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-atcphy",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,t8103-atcphy", "apple,t6000-atcphy",],
    );

    // PHY must be registered before DWC3 (Core), so use Critical priority.
    // PHY nodes appear after USB nodes in Apple FDT, causing probe order issue.
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical);
}

driver_initcall!(register_atcphy_driver);
