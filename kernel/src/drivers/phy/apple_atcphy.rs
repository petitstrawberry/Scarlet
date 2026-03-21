//! Apple ATC PHY driver
//!
//! Apple Type-C PHY found on Apple Silicon SoCs (t8103/M1).

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
// =============================================================================

const ATCPHY_POWER_CTRL: usize = 0x20000;
const ATCPHY_POWER_STAT: usize = 0x20004;
const ATCPHY_MISC: usize = 0x20008;

const ATCPHY_POWER_SLEEP_SMALL: u32 = 1 << 0;
const ATCPHY_POWER_SLEEP_BIG: u32 = 1 << 1;
const ATCPHY_POWER_CLAMP_EN: u32 = 1 << 2;
const ATCPHY_POWER_APB_RESET_N: u32 = 1 << 3;
const ATCPHY_POWER_PHY_RESET_N: u32 = 1 << 4;

const ATCPHY_MISC_RESET_N: u32 = 1 << 0;
const ATCPHY_MISC_LANE_SWAP: u32 = 1 << 2;

const ACIOPHY_LANE_MODE: usize = 0x48;
const ACIOPHY_CROSSBAR: usize = 0x4c;
const ACIOPHY_CROSSBAR_PROTOCOL_MASK: u32 = 0x1f;
const ACIOPHY_CROSSBAR_PROTOCOL_USB3_DP: u32 = 0x10;

const ACIOPHY_LANE_MODE_USB3: u32 = 0x3;
const ACIOPHY_LANE_MODE_DP: u32 = 0x5;

// =============================================================================
// =============================================================================

const USB2PHY_USBCTL: usize = 0x00;
const USB2PHY_CTL: usize = 0x04;
const USB2PHY_SIG: usize = 0x08;
const USB2PHY_MISCTUNE: usize = 0x1c;

const USB2PHY_USBCTL_RUN: u32 = 1 << 1;

const USB2PHY_CTL_RESET: u32 = 1 << 0;
const USB2PHY_CTL_PORT_RESET: u32 = 1 << 1;
const USB2PHY_CTL_APB_RESET_N: u32 = 1 << 2;
const USB2PHY_CTL_SIDDQ: u32 = 1 << 3;

const USB2PHY_SIG_VBUSDET_FORCE_VAL: u32 = 1 << 0;
const USB2PHY_SIG_VBUSDET_FORCE_EN: u32 = 1 << 1;
const USB2PHY_SIG_VBUSVLDEXT_FORCE_VAL: u32 = 1 << 2;
const USB2PHY_SIG_VBUSVLDEXT_FORCE_EN: u32 = 1 << 3;

const USB2PHY_MISCTUNE_APBCLK_GATE_OFF: u32 = 1 << 29;
const USB2PHY_MISCTUNE_REFCLK_GATE_OFF: u32 = 1 << 30;

// =============================================================================
// =============================================================================

const PIPEHANDLER_OVERRIDE: usize = 0x00;
const PIPEHANDLER_OVERRIDE_VALUES: usize = 0x04;
const PIPEHANDLER_MUX_CTRL: usize = 0x0c;
const PIPEHANDLER_LOCK_REQ: usize = 0x10;
const PIPEHANDLER_LOCK_ACK: usize = 0x14;
const PIPEHANDLER_NONSELECTED_OVERRIDE: usize = 0x20;

const PIPEHANDLER_OVERRIDE_RXVALID: u32 = 1 << 0;
const PIPEHANDLER_OVERRIDE_RXDETECT: u32 = 1 << 2;

const PIPEHANDLER_OVERRIDE_VAL_RXDETECT0: u32 = 1 << 1;
const PIPEHANDLER_OVERRIDE_VAL_RXDETECT1: u32 = 1 << 2;

const PIPEHANDLER_MUX_CTRL_DATA_MASK: u32 = 0x7;
const PIPEHANDLER_MUX_CTRL_CLK_MASK: u32 = 0x7 << 3;
const PIPEHANDLER_MUX_CTRL_CLK_OFF: u32 = 0;
const PIPEHANDLER_MUX_CTRL_CLK_USB3: u32 = 1;
const PIPEHANDLER_MUX_CTRL_DATA_USB3: u32 = 0;

const PIPEHANDLER_LOCK_EN: u32 = 1 << 0;

const PIPEHANDLER_NATIVE_RESET: u32 = 1 << 12;
const PIPEHANDLER_DUMMY_PHY_EN: u32 = 1 << 15;
const PIPEHANDLER_NATIVE_POWER_DOWN_MASK: u32 = 0xf;

// =============================================================================
// ATC PHY Instance
// =============================================================================

pub struct AppleAtcPhy {
    core_base: usize,
    usb2phy_base: usize,
    pipehandler_base: usize,
}

impl AppleAtcPhy {
    pub fn new(core_base: usize, usb2phy_base: usize, pipehandler_base: usize) -> Self {
        Self {
            core_base,
            usb2phy_base,
            pipehandler_base,
        }
    }

    fn small_delay(&self) {
        for _ in 0..1000 {
            core::hint::spin_loop();
        }
    }

    fn core_read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.core_base + offset) }
    }

    fn core_write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.core_base + offset, val) }
    }

    fn core_set32(&self, offset: usize, bits: u32) {
        self.core_write32(offset, self.core_read32(offset) | bits);
    }

    fn core_clear32(&self, offset: usize, bits: u32) {
        self.core_write32(offset, self.core_read32(offset) & !bits);
    }

    fn usb2phy_read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.usb2phy_base + offset) }
    }

    fn usb2phy_write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.usb2phy_base + offset, val) }
    }

    fn usb2phy_set32(&self, offset: usize, bits: u32) {
        self.usb2phy_write32(offset, self.usb2phy_read32(offset) | bits);
    }

    fn usb2phy_clear32(&self, offset: usize, bits: u32) {
        self.usb2phy_write32(offset, self.usb2phy_read32(offset) & !bits);
    }

    fn ph_read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.pipehandler_base + offset) }
    }

    fn ph_write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.pipehandler_base + offset, val) }
    }

    fn ph_set32(&self, offset: usize, bits: u32) {
        self.ph_write32(offset, self.ph_read32(offset) | bits);
    }

    fn ph_clear32(&self, offset: usize, bits: u32) {
        self.ph_write32(offset, self.ph_read32(offset) & !bits);
    }

    fn poll_core(
        &self,
        offset: usize,
        mask: u32,
        domain: &'static str,
    ) -> Result<(), &'static str> {
        let mut timeout = 10000;
        while timeout != 0 {
            if self.core_read32(offset) & mask == mask {
                return Ok(());
            }
            self.small_delay();
            timeout -= 1;
        }
        early_println!("[apple-atcphy] timeout waiting for {} power domain", domain);
        Err("apple-atcphy: core power domain timeout")
    }

    fn usb2_power_on(&self) {
        let sig = USB2PHY_SIG_VBUSDET_FORCE_VAL
            | USB2PHY_SIG_VBUSDET_FORCE_EN
            | USB2PHY_SIG_VBUSVLDEXT_FORCE_VAL
            | USB2PHY_SIG_VBUSVLDEXT_FORCE_EN;
        self.usb2phy_write32(USB2PHY_SIG, sig);
        self.small_delay();

        self.usb2phy_clear32(USB2PHY_CTL, USB2PHY_CTL_SIDDQ);
        self.small_delay();

        self.usb2phy_clear32(USB2PHY_CTL, USB2PHY_CTL_RESET);
        self.small_delay();
        self.usb2phy_clear32(USB2PHY_CTL, USB2PHY_CTL_PORT_RESET);
        self.small_delay();
        self.usb2phy_set32(USB2PHY_CTL, USB2PHY_CTL_APB_RESET_N);
        self.small_delay();

        self.usb2phy_clear32(USB2PHY_MISCTUNE, USB2PHY_MISCTUNE_APBCLK_GATE_OFF);
        self.usb2phy_clear32(USB2PHY_MISCTUNE, USB2PHY_MISCTUNE_REFCLK_GATE_OFF);

        self.usb2phy_write32(USB2PHY_USBCTL, USB2PHY_USBCTL_RUN);
    }

    fn core_power_on(&self) -> Result<(), &'static str> {
        self.core_set32(ATCPHY_MISC, ATCPHY_MISC_RESET_N);

        self.core_set32(ATCPHY_POWER_CTRL, ATCPHY_POWER_SLEEP_SMALL);
        self.poll_core(ATCPHY_POWER_STAT, ATCPHY_POWER_SLEEP_SMALL, "small")?;

        self.core_set32(ATCPHY_POWER_CTRL, ATCPHY_POWER_SLEEP_BIG);
        self.poll_core(ATCPHY_POWER_STAT, ATCPHY_POWER_SLEEP_BIG, "big")?;

        self.core_clear32(ATCPHY_POWER_CTRL, ATCPHY_POWER_CLAMP_EN);
        self.core_set32(ATCPHY_POWER_CTRL, ATCPHY_POWER_APB_RESET_N);

        Ok(())
    }

    fn configure_crossbar(&self) {
        let crossbar = self.core_read32(ACIOPHY_CROSSBAR);
        self.core_write32(
            ACIOPHY_CROSSBAR,
            (crossbar & !ACIOPHY_CROSSBAR_PROTOCOL_MASK) | ACIOPHY_CROSSBAR_PROTOCOL_USB3_DP,
        );

        let lane_mode = (ACIOPHY_LANE_MODE_USB3 << 0)
            | (ACIOPHY_LANE_MODE_USB3 << 3)
            | (ACIOPHY_LANE_MODE_DP << 6)
            | (ACIOPHY_LANE_MODE_DP << 9);
        self.core_write32(ACIOPHY_LANE_MODE, lane_mode);
    }

    fn configure_pipehandler_usb3(&self) {
        self.ph_clear32(
            PIPEHANDLER_OVERRIDE_VALUES,
            PIPEHANDLER_OVERRIDE_VAL_RXDETECT0 | PIPEHANDLER_OVERRIDE_VAL_RXDETECT1,
        );
        self.ph_set32(PIPEHANDLER_OVERRIDE, PIPEHANDLER_OVERRIDE_RXVALID);
        self.ph_set32(PIPEHANDLER_OVERRIDE, PIPEHANDLER_OVERRIDE_RXDETECT);

        self.ph_set32(PIPEHANDLER_LOCK_REQ, PIPEHANDLER_LOCK_EN);

        let nonselected = self.ph_read32(PIPEHANDLER_NONSELECTED_OVERRIDE);
        self.ph_write32(
            PIPEHANDLER_NONSELECTED_OVERRIDE,
            (nonselected & !PIPEHANDLER_NATIVE_POWER_DOWN_MASK) | 3,
        );
        self.ph_clear32(PIPEHANDLER_NONSELECTED_OVERRIDE, PIPEHANDLER_NATIVE_RESET);

        let mut mux = self.ph_read32(PIPEHANDLER_MUX_CTRL);
        mux = (mux & !PIPEHANDLER_MUX_CTRL_CLK_MASK) | (PIPEHANDLER_MUX_CTRL_CLK_OFF << 3);
        self.ph_write32(PIPEHANDLER_MUX_CTRL, mux);
        self.small_delay();

        mux = (mux & !PIPEHANDLER_MUX_CTRL_DATA_MASK) | PIPEHANDLER_MUX_CTRL_DATA_USB3;
        self.ph_write32(PIPEHANDLER_MUX_CTRL, mux);
        self.small_delay();

        mux = (mux & !PIPEHANDLER_MUX_CTRL_CLK_MASK) | (PIPEHANDLER_MUX_CTRL_CLK_USB3 << 3);
        self.ph_write32(PIPEHANDLER_MUX_CTRL, mux);
        self.small_delay();

        self.ph_clear32(PIPEHANDLER_OVERRIDE, PIPEHANDLER_OVERRIDE_RXVALID);
        self.ph_clear32(PIPEHANDLER_OVERRIDE, PIPEHANDLER_OVERRIDE_RXDETECT);

        self.ph_clear32(PIPEHANDLER_LOCK_REQ, PIPEHANDLER_LOCK_EN);
    }

    pub fn init(&mut self) -> Result<(), &'static str> {
        early_println!("[apple-atcphy] initializing...");

        self.usb2_power_on();
        self.core_power_on()?;
        self.configure_crossbar();
        self.configure_pipehandler_usb3();

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

    if mem_resources.len() < 5 {
        return Err("apple-atcphy: expected at least 5 memory resources");
    }

    let core_paddr = mem_resources[0].start;
    let core_size = mem_resources[0].end - mem_resources[0].start + 1;

    let usb2phy_paddr = mem_resources[3].start;
    let usb2phy_size = mem_resources[3].end - mem_resources[3].start + 1;

    let pipehandler_paddr = mem_resources[4].start;
    let pipehandler_size = mem_resources[4].end - mem_resources[4].start + 1;

    early_println!(
        "[apple-atcphy] probing {} core={:#x} usb2phy={:#x} pipehandler={:#x}",
        device.name(),
        core_paddr,
        usb2phy_paddr,
        pipehandler_paddr
    );

    let core_base = crate::vm::ioremap(core_paddr, core_size)
        .map_err(|_| "apple-atcphy: ioremap core failed")?;
    let usb2phy_base = crate::vm::ioremap(usb2phy_paddr, usb2phy_size)
        .map_err(|_| "apple-atcphy: ioremap usb2phy failed")?;
    let pipehandler_base = crate::vm::ioremap(pipehandler_paddr, pipehandler_size)
        .map_err(|_| "apple-atcphy: ioremap pipehandler failed")?;

    let mut phy = AppleAtcPhy::new(core_base, usb2phy_base, pipehandler_base);
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
        alloc::vec!["apple,t8103-atcphy", "apple,t6000-atcphy"],
    );

    // PHY must be registered before DWC3 (Core), so use Critical priority.
    // PHY nodes appear after USB nodes in Apple FDT, causing probe order issue.
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical);
}

driver_initcall!(register_atcphy_driver);
