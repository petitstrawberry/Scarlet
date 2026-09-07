#![allow(dead_code)]

use crate::arch::mmio;

pub const DWC3_GSBUSCFG0: usize = 0xc100;
pub const DWC3_GUSB2PHYCFG: usize = 0xc200;
pub const DWC3_GCTL: usize = 0xc110;
pub const DWC3_GUCTL: usize = 0xc12c;
pub const DWC3_GRXTHRCFG: usize = 0xc18c;
pub const DWC3_GTXTHRCFG: usize = 0xc190;
pub const DWC3_GUSB2PHYACC: usize = 0xc280;
pub const DWC3_GUSB3PIPECTL: usize = 0xc2c0;
pub const DWC3_GEVNTADRLO: usize = 0xc400;
pub const DWC3_GEVNTADRHI: usize = 0xc404;
pub const DWC3_GEVNTSIZ: usize = 0xc408;
pub const DWC3_GEVNTCOUNT: usize = 0xc40c;
pub const DWC3_GHWPARAMS1: usize = 0xc144;
pub const DWC3_GHWPARAMS3: usize = 0xc14c;
pub const DWC3_GSNPSID: usize = 0xc120;
pub const DWC3_GUCTL1: usize = 0xc11c;
pub const DWC3_GUSB3PIPEFMT: usize = 0xc660;

pub const GCTL_CORESOFTRESET: u32 = 1 << 11;
pub const GCTL_SCALEDOWN_MASK: u32 = 0x3 << 4;
pub const GCTL_PRTCAP_MASK: u32 = 0x3 << 12;
pub const GCTL_PRTCAP_HOST: u32 = 1 << 12;
pub const GCTL_PRTCAPDIR_HOST: u32 = 1 << 12;
pub const GCTL_DSBLCLKGTNG: u32 = 1 << 0;
pub const GCTL_SOFITPSYNC: u32 = 1 << 10;

pub const GSBUSCFG0_INCRX: u32 = 1 << 0;
pub const GSBUSCFG0_INCR4B: u32 = 1 << 1;
pub const GSBUSCFG0_INCR8B: u32 = 1 << 2;
pub const GSBUSCFG0_INCR16B: u32 = 1 << 3;
pub const GSBUSCFG0_INCR32B: u32 = 1 << 4;
pub const GSBUSCFG0_INCR64B: u32 = 1 << 5;
pub const GSBUSCFG0_INCR128B: u32 = 1 << 6;
pub const GSBUSCFG0_INCR256B: u32 = 1 << 7;

pub const GHWPARAMS3_SSPHY_IFC_MASK: u32 = 0x3;

pub const GSNPSID_MASK: u32 = 0xfffff000;

pub struct Dwc3Core {
    base_addr: usize,
}

impl Dwc3Core {
    pub fn new(base_addr: usize) -> Self {
        Self { base_addr }
    }

    pub fn base_addr(&self) -> usize {
        self.base_addr
    }

    #[inline]
    pub fn read32(&self, offset: usize) -> u32 {
        // SAFETY: offset is within the MMIO-mapped DWC3 region
        unsafe { mmio::read32(self.base_addr + offset) }
    }

    #[inline]
    pub fn write32(&self, offset: usize, val: u32) {
        // SAFETY: offset is within the MMIO-mapped DWC3 region
        unsafe { mmio::write32(self.base_addr + offset, val) }
    }

    pub fn read_revision(&self) -> (u32, u32) {
        let snpsid = self.read32(DWC3_GSNPSID) & GSNPSID_MASK;
        let major = snpsid >> 12 & 0xf;
        let minor = snpsid >> 4 & 0xff;
        (major, minor)
    }

    pub fn is_usb3(&self) -> bool {
        (self.read32(DWC3_GHWPARAMS3) & GHWPARAMS3_SSPHY_IFC_MASK) != 0
    }

    pub fn global_soft_reset(&self) {
        let gctl = self.read32(DWC3_GCTL);
        self.write32(DWC3_GCTL, gctl | GCTL_CORESOFTRESET);
    }

    pub fn wait_for_reset(&self) -> Result<(), &'static str> {
        let deadline = crate::time::current_time() + 1_000_000;
        while crate::time::current_time() < deadline {
            if self.read32(DWC3_GCTL) & GCTL_CORESOFTRESET == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err("dwc3: global soft reset timeout")
    }
}
