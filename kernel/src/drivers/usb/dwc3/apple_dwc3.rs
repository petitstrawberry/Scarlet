#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::Mutex;

use super::dwc3_core::{
    DWC3_GCTL, DWC3_GUSB2PHYACC, DWC3_GUSB2PHYCFG, DWC3_GUSB3PIPECTL, Dwc3Core,
};
use crate::{
    device::{
        DeviceInfo,
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    driver_initcall, early_println,
    interrupt::InterruptId,
};

const DWC3_APPLE_CTRL0: usize = 0xc800;
const DWC3_APPLE_CTRL1: usize = 0xc804;
const DWC3_APPLE_CIO_LFPS: usize = 0xcd38;
const DWC3_APPLE_CIO_BW_NGT: usize = 0xcd3c;
const DWC3_APPLE_CIO_LINK_TIMER: usize = 0xcd40;

const APPLE_CTRL0_PIPE_RESET_DISABLE: u32 = 1 << 1;
const APPLE_CTRL0_U2_EXIT_LFPS: u32 = 1 << 2;
const APPLE_CTRL0_FORCE_PLL: u32 = 1 << 4;
const APPLE_CTRL1_UTMI_REDUCE: u32 = 1 << 1;

const GCTL_PRTCAPDIR_HOST: u32 = 1 << 12;
const GUSB2PHYCFG_SUSPHY: u32 = 1 << 6;
const GUSB3PIPECTL_SUSPHY: u32 = 1 << 17;

pub struct AppleDwc3 {
    core: Dwc3Core,
    dr_mode: alloc::string::String,
}

impl AppleDwc3 {
    pub fn new(base_addr: usize, dr_mode: &str) -> Self {
        Self {
            core: Dwc3Core::new(base_addr),
            dr_mode: alloc::string::String::from(dr_mode),
        }
    }

    pub fn core(&self) -> &Dwc3Core {
        &self.core
    }

    pub fn init(&mut self) -> Result<(), &'static str> {
        early_println!("[apple-dwc3] initializing...");

        let (major, minor) = self.core.read_revision();
        early_println!("[apple-dwc3] SNPSID revision: {}.{}", major, minor);

        let is_usb3 = self.core.is_usb3();
        early_println!("[apple-dwc3] USB3 capable: {}", is_usb3);

        // Apple CTRL0: disable pipe reset, enable U2 exit LFPS, force PLL
        let ctrl0 = self.core.read32(DWC3_APPLE_CTRL0)
            | APPLE_CTRL0_PIPE_RESET_DISABLE
            | APPLE_CTRL0_U2_EXIT_LFPS
            | APPLE_CTRL0_FORCE_PLL;
        self.core.write32(DWC3_APPLE_CTRL0, ctrl0);

        // Apple CTRL1: UTMI reduce
        let ctrl1 = self.core.read32(DWC3_APPLE_CTRL1) | APPLE_CTRL1_UTMI_REDUCE;
        self.core.write32(DWC3_APPLE_CTRL1, ctrl1);

        // Apple CIO setup (asahi-linux dwc3_apple_setup_cio)
        self.core.write32(DWC3_APPLE_CIO_LFPS, 0x0f800f80);
        self.core.write32(DWC3_APPLE_CIO_BW_NGT, 0x0fc00fc0);
        self.core.write32(DWC3_APPLE_CIO_LINK_TIMER, 0x140a10);

        // USB2 PHY ACC: set UTMI_PHYDATREQ bit
        let usb2phyacc = self.core.read32(DWC3_GUSB2PHYACC) | (0xff << 8);
        self.core.write32(DWC3_GUSB2PHYACC, usb2phyacc);

        // USB3 PIPE: read-modify-write (clear any pending bits)
        let usb3pipectl = self.core.read32(DWC3_GUSB3PIPECTL);
        self.core.write32(DWC3_GUSB3PIPECTL, usb3pipectl);

        // GCTL: set port capability to HOST (bits 13:12 = 01)
        let gctl = self.core.read32(DWC3_GCTL) & !(0x3 << 12);
        self.core.write32(DWC3_GCTL, gctl | GCTL_PRTCAPDIR_HOST);

        // Enable suspend PHY on both USB2 and USB3
        let usb2cfg = self.core.read32(DWC3_GUSB2PHYCFG) | GUSB2PHYCFG_SUSPHY;
        self.core.write32(DWC3_GUSB2PHYCFG, usb2cfg);

        let usb3cfg = self.core.read32(DWC3_GUSB3PIPECTL) | GUSB3PIPECTL_SUSPHY;
        self.core.write32(DWC3_GUSB3PIPECTL, usb3cfg);

        early_println!("[apple-dwc3] initialized (dr_mode={})", self.dr_mode);
        Ok(())
    }
}

static DWC3_REGISTRY: Mutex<alloc::vec::Vec<Arc<Mutex<AppleDwc3>>>> =
    Mutex::new(alloc::vec::Vec::new());

pub fn register_dwc3(dwc3: AppleDwc3) -> u32 {
    let mut guard = DWC3_REGISTRY.lock();
    let id = guard.len() as u32;
    guard.push(Arc::new(Mutex::new(dwc3)));
    id
}

pub fn get_dwc3(id: u32) -> Option<Arc<Mutex<AppleDwc3>>> {
    let guard = DWC3_REGISTRY.lock();
    guard.get(id as usize).cloned()
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resource = device
        .get_resources()
        .iter()
        .find(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-dwc3: no memory resource found")?;

    let paddr = mem_resource.start;
    let size = mem_resource.end - mem_resource.start + 1;

    early_println!(
        "[apple-dwc3] probing {} at paddr={:#x}, size={:#x}",
        device.name(),
        paddr,
        size
    );

    let base_addr = crate::vm::ioremap(paddr, size).map_err(|_| "dwc3: ioremap failed")?;

    let dr_mode = device
        .property("dr_mode")
        .and_then(|p| p.as_str())
        .unwrap_or("otg");

    if let Some(phys_prop) = device.property("phys") {
        let bytes = phys_prop.value();
        let entry_size = 8; // phandle(4) + index(4), #phy-cells = 1
        let mut offset = 0usize;
        while offset + entry_size <= bytes.len() {
            let phy_phandle =
                u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap_or([0; 4]));
            if let Some(_phy) =
                crate::drivers::phy::apple_atcphy::get_atcphy_by_phandle(phy_phandle)
            {
                early_println!("[apple-dwc3] ATC PHY ready (phandle={:#x})", phy_phandle);
            } else {
                early_println!(
                    "[apple-dwc3] ATC PHY not found (phandle={:#x})",
                    phy_phandle
                );
            }
            offset += entry_size;
        }
    }

    let mut dwc3 = AppleDwc3::new(base_addr, dr_mode);
    dwc3.init()?;

    let _id = register_dwc3(dwc3);

    early_println!("[apple-dwc3] registered (id={})", _id);

    if dr_mode == "host" {
        let irq_resource = device
            .get_resources()
            .iter()
            .find(|r| matches!(r.res_type, PlatformDeviceResourceType::IRQ));

        let interrupt_id = irq_resource.map(|r| r.start as InterruptId);

        match crate::drivers::usb::xhci::bind_xhci_mmio(base_addr, interrupt_id) {
            Ok(()) => early_println!("[apple-dwc3] xHCI bound successfully"),
            Err(e) => early_println!("[apple-dwc3] xHCI bind failed: {}", e),
        }
    }

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_dwc3_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-dwc3",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-dwc3",
            "apple,dwc3",
            "snps,dwc3",
            "apple,t6000-dwc3",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

driver_initcall!(register_dwc3_driver);
