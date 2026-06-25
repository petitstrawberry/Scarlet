#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;

use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        clk::ClkHandle,
        manager::{DeviceManager, DriverPriority, is_probe_defer, probe_defer},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
    interrupt::InterruptId,
    mem::pmm,
    sync::Mutex,
};
use scarlet_driver_apple_atcphy::get_atcphy_by_phandle;

const DWC3_GSBUSCFG0: usize = 0xc100;
const DWC3_GUSB2PHYCFG: usize = 0xc200;
const DWC3_GCTL: usize = 0xc110;
const DWC3_GUSB3PIPECTL: usize = 0xc2a0;
const DWC3_GUSB2PHYACC: usize = 0xc2c0;
const DWC3_GEVNTADRLO: usize = 0xc400;
const DWC3_GEVNTADRHI: usize = 0xc404;
const DWC3_GEVNTSIZ: usize = 0xc408;
const DWC3_GEVNTCOUNT: usize = 0xc40c;
const DWC3_GHWPARAMS1: usize = 0xc440;
const DWC3_GSNPSID: usize = 0xc120;

const GCTL_SCALEDOWN_MASK: u32 = 0x3 << 29;
const GCTL_PRTCAP_MASK: u32 = 0x3 << 12;
const GCTL_PRTCAP_HOST: u32 = 1 << 12;
const GCTL_DSBLCLKGTNG: u32 = 1 << 0;

const GSBUSCFG0_INCR256B: u32 = 1 << 2;
const GSBUSCFG0_INCR16B: u32 = 1 << 3;
const GSBUSCFG0_INCR8B: u32 = 1 << 4;
const GSBUSCFG0_INCR4B: u32 = 1 << 5;
const GSBUSCFG0_INCRX: u32 = 1 << 6;
const GSBUSCFG0_INCR64B: u32 = 1 << 7;

const GHWPARAMS1_MODE_BITS: u32 = 0x7 << 28;
const GHWPARAMS1_MODE_SHIFT: u32 = 28;
const GSNPSID_MASK: u32 = 0xfffff000;

const DWC3_APPLE_CTRL0: usize = 0xc800;
const DWC3_APPLE_CTRL1: usize = 0xc804;
const DWC3_APPLE_CIO_LFPS: usize = 0xcd38;
const DWC3_APPLE_CIO_BW_NGT: usize = 0xcd3c;
const DWC3_APPLE_CIO_LINK_TIMER: usize = 0xcd40;

const APPLE_CTRL0_PIPE_RESET_DISABLE: u32 = 1 << 1;
const APPLE_CTRL0_U2_EXIT_LFPS: u32 = 1 << 2;
const APPLE_CTRL0_FORCE_PLL: u32 = 1 << 4;
const APPLE_CTRL1_UTMI_REDUCE: u32 = 1 << 1;

const GUSB2PHYCFG_SUSPHY: u32 = 1 << 6;
const GUSB3PIPECTL_SUSPHY: u32 = 1 << 17;

/// Synopsys DWC3 core register access wrapper.
pub struct Dwc3Core {
    base_addr: usize,
}

impl Dwc3Core {
    /// Create a DWC3 core wrapper for an MMIO base address.
    ///
    /// # Arguments
    ///
    /// * `base_addr` - Virtual MMIO base address for the DWC3 register block.
    ///
    /// # Returns
    ///
    /// A DWC3 core register accessor.
    pub fn new(base_addr: usize) -> Self {
        Self { base_addr }
    }

    /// Read a 32-bit DWC3 register.
    ///
    /// # Arguments
    ///
    /// * `offset` - Register offset from the DWC3 MMIO base.
    ///
    /// # Returns
    ///
    /// The register value.
    #[inline]
    pub fn read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.base_addr + offset) }
    }

    /// Write a 32-bit DWC3 register.
    ///
    /// # Arguments
    ///
    /// * `offset` - Register offset from the DWC3 MMIO base.
    /// * `val` - Value to write.
    #[inline]
    pub fn write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.base_addr + offset, val) }
    }

    /// Read the DWC3 Synopsys revision.
    ///
    /// # Returns
    ///
    /// A `(major, minor)` revision tuple decoded from `GSNPSID`.
    pub fn read_revision(&self) -> (u32, u32) {
        let snpsid = self.read32(DWC3_GSNPSID) & GSNPSID_MASK;
        let major = snpsid >> 12 & 0xf;
        let minor = snpsid >> 4 & 0xff;
        (major, minor)
    }

    /// Check whether the DWC3 core advertises USB3 capability.
    ///
    /// # Returns
    ///
    /// `true` when the hardware parameters indicate USB3 support.
    pub fn is_usb3(&self) -> bool {
        let mode = (self.read32(DWC3_GHWPARAMS1) & GHWPARAMS1_MODE_BITS) >> GHWPARAMS1_MODE_SHIFT;
        mode >= 3
    }
}

/// Apple-integrated DWC3 controller state.
pub struct AppleDwc3 {
    core: Dwc3Core,
    dr_mode: alloc::string::String,
    _bus_clk: Option<ClkHandle>,
}

impl AppleDwc3 {
    /// Create an Apple DWC3 controller instance.
    ///
    /// # Arguments
    ///
    /// * `base_addr` - Virtual MMIO base address for the controller.
    /// * `dr_mode` - USB dual-role mode string from firmware.
    /// * `bus_clk` - Optional prepared bus clock handle kept alive for this controller.
    ///
    /// # Returns
    ///
    /// A controller instance ready for hardware initialization.
    pub fn new(base_addr: usize, dr_mode: &str, bus_clk: Option<ClkHandle>) -> Self {
        Self {
            core: Dwc3Core::new(base_addr),
            dr_mode: alloc::string::String::from(dr_mode),
            _bus_clk: bus_clk,
        }
    }

    /// Initialize Apple DWC3 controller registers and event resources.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the controller initialization completed, otherwise a static error string.
    pub fn init(&mut self) -> Result<(), &'static str> {
        early_println!("[apple-dwc3] initializing...");

        let (major, minor) = self.core.read_revision();
        early_println!("[apple-dwc3] SNPSID revision: {}.{}", major, minor);

        let is_usb3 = self.core.is_usb3();
        early_println!("[apple-dwc3] USB3 capable: {}", is_usb3);

        let ctrl0 = self.core.read32(DWC3_APPLE_CTRL0)
            | APPLE_CTRL0_PIPE_RESET_DISABLE
            | APPLE_CTRL0_U2_EXIT_LFPS
            | APPLE_CTRL0_FORCE_PLL;
        self.core.write32(DWC3_APPLE_CTRL0, ctrl0);

        let ctrl1 = self.core.read32(DWC3_APPLE_CTRL1) | APPLE_CTRL1_UTMI_REDUCE;
        self.core.write32(DWC3_APPLE_CTRL1, ctrl1);

        let mut gctl = self.core.read32(DWC3_GCTL);
        gctl &= !GCTL_SCALEDOWN_MASK;
        gctl &= !GCTL_PRTCAP_MASK;
        gctl |= GCTL_PRTCAP_HOST;
        gctl |= GCTL_DSBLCLKGTNG;
        self.core.write32(DWC3_GCTL, gctl);

        let buscfg = self.core.read32(DWC3_GSBUSCFG0)
            | GSBUSCFG0_INCR256B
            | GSBUSCFG0_INCR16B
            | GSBUSCFG0_INCR8B
            | GSBUSCFG0_INCR4B
            | GSBUSCFG0_INCRX
            | GSBUSCFG0_INCR64B;
        self.core.write32(DWC3_GSBUSCFG0, buscfg);

        self.core.write32(DWC3_APPLE_CIO_LFPS, 0x0f800f80);
        self.core.write32(DWC3_APPLE_CIO_BW_NGT, 0x0fc00fc0);
        self.core.write32(DWC3_APPLE_CIO_LINK_TIMER, 0x140a10);

        let usb2phyacc = self.core.read32(DWC3_GUSB2PHYACC) | (0xff << 8);
        self.core.write32(DWC3_GUSB2PHYACC, usb2phyacc);

        let usb3pipectl = self.core.read32(DWC3_GUSB3PIPECTL);
        self.core.write32(DWC3_GUSB3PIPECTL, usb3pipectl);

        let usb2cfg = self.core.read32(DWC3_GUSB2PHYCFG) | GUSB2PHYCFG_SUSPHY;
        self.core.write32(DWC3_GUSB2PHYCFG, usb2cfg);

        let usb3cfg = self.core.read32(DWC3_GUSB3PIPECTL) | GUSB3PIPECTL_SUSPHY;
        self.core.write32(DWC3_GUSB3PIPECTL, usb3cfg);

        let evt_paddr = pmm::alloc_frame().ok_or("dwc3: failed to alloc event buffer")?;
        self.core.write32(DWC3_GEVNTADRLO, evt_paddr as u32);
        self.core.write32(DWC3_GEVNTADRHI, (evt_paddr >> 32) as u32);
        self.core.write32(DWC3_GEVNTSIZ, 0x1000);
        self.core.write32(DWC3_GEVNTCOUNT, 0);

        early_println!("[apple-dwc3] initialized (dr_mode={})", self.dr_mode);
        Ok(())
    }
}

/// Probe an Apple DWC3 platform device.
///
/// The optional `bus` clock is resolved and enabled before MMIO access. Clock lookup failure is
/// non-fatal because older or incomplete firmware descriptions may omit a clock provider.
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

    let bus_clk = match DeviceManager::get_manager().resolve_clk(device, "bus") {
        Ok(handle) => {
            let _ = handle.prepare_enable();
            Some(handle)
        }
        Err(e) if is_probe_defer(e) || e == "clk: provider not found" => {
            early_println!("[apple-dwc3] bus clock provider not ready, deferring");
            return probe_defer();
        }
        Err(
            e @ ("clk: clock-names missing" | "clk: clocks missing" | "clk: clock name not found"),
        ) => {
            early_println!("[apple-dwc3] warning: bus clock unavailable: {}", e);
            None
        }
        Err(e) => {
            early_println!("[apple-dwc3] bus clock lookup failed: {}", e);
            return Err(e);
        }
    };

    let base_addr = scarlet::vm::ioremap(paddr, size).map_err(|_| "dwc3: ioremap failed")?;

    let dr_mode = device
        .property("dr_mode")
        .and_then(|p| p.as_str())
        .unwrap_or("otg");

    if let Some(phys_prop) = device.property("phys") {
        let bytes = phys_prop.value();
        let entry_size = 8;
        let mut offset = 0usize;
        while offset + entry_size <= bytes.len() {
            let phy_phandle =
                u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap_or([0; 4]));
            if let Some(_phy) = get_atcphy_by_phandle(phy_phandle) {
                early_println!("[apple-dwc3] ATC PHY ready (phandle={:#x})", phy_phandle);
            } else {
                early_println!(
                    "[apple-dwc3] ATC PHY not yet registered, deferring (phandle={:#x})",
                    phy_phandle
                );
                return probe_defer();
            }
            offset += entry_size;
        }
    }

    let mut dwc3 = AppleDwc3::new(base_addr, dr_mode, bus_clk);
    dwc3.init()?;
    *APPLE_DWC3.lock() = Some(dwc3);

    early_println!("[apple-dwc3] registered");

    let is_host = dr_mode == "host" || dr_mode == "otg";
    if is_host {
        let irq_resource = device
            .get_resources()
            .iter()
            .find(|r| matches!(r.res_type, PlatformDeviceResourceType::IRQ));

        let interrupt_id = irq_resource.map(|r| r.start as InterruptId);

        if let Err(e) = scarlet::drivers::usb::xhci::bind_xhci_mmio(base_addr, interrupt_id) {
            early_println!("[apple-dwc3] xHCI bind failed: {}", e);
            *APPLE_DWC3.lock() = None;
            return Err(e);
        }
        early_println!("[apple-dwc3] xHCI bound successfully");
    }

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    *APPLE_DWC3.lock() = None;
    Ok(())
}

static APPLE_DWC3: Mutex<Option<AppleDwc3>> = Mutex::new(None);

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

scarlet::driver_initcall!(register_dwc3_driver);

#[used]
static SCARLET_DRIVER_APPLE_DWC3_ANCHOR: fn() = force_link;

/// Force this crate to be linked into driver builds.
#[inline(never)]
pub fn force_link() {}
