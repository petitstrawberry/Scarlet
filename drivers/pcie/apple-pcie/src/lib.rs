#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        manager::{DeviceManager, DriverPriority, is_probe_defer, probe_defer},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
};
use scarlet_driver_apple_dart::get_dart_by_phandle;
use scarlet_driver_apple_msi::{MsiPortConfig, register_apple_msi_controller};

const CORE_RC_PHYIF_CTL: usize = 0x0024;
const CORE_RC_PHYIF_STAT: usize = 0x0028;
const CORE_RC_CTL: usize = 0x0050;
const CORE_RC_STAT: usize = 0x0058;
const CORE_FABRIC_STAT: usize = 0x4000;

const CORE_RC_CTL_RUN: u32 = 1 << 0;
const CORE_RC_PHYIF_CTL_RUN: u32 = 1 << 0;
const CORE_RC_PHYIF_STAT_REFCLK: u32 = 1 << 4;
const CORE_RC_STAT_READY: u32 = 1 << 0;

const PORT_LTSSMCTL: usize = 0x0080;
const PORT_INTSTAT: usize = 0x0100;
const PORT_INTMSK: usize = 0x0104;
const PORT_LINKSTS: usize = 0x0208;
const PORT_APPCLK: usize = 0x0800;
const PORT_STATUS: usize = 0x0804;
const PORT_REFCLK: usize = 0x0810;
const PORT_PERST: usize = 0x0814;
const PORT_RID2SID: usize = 0x0828;

const PHY_LANE_CFG: usize = 0x0000;
const PHY_LANE_CTL: usize = 0x0004;

const PHY_LANE_CFG_REFCLK0_REQ: u32 = 1 << 0;
const PHY_LANE_CFG_REFCLK0_ACK: u32 = 1 << 2;
const PHY_LANE_CFG_REFCLK1_REQ: u32 = 1 << 1;
const PHY_LANE_CFG_REFCLK1_ACK: u32 = 1 << 3;
const PHY_LANE_CFG_REFCLK_ENABLE: u32 = 1 << 9;
const PHY_LANE_CFG_REFCLK1_ENABLE: u32 = 1 << 10;

const PHY_LANE_CTL_CFGACC: u32 = 1 << 15;

const PORT_LTSSMCTL_START: u32 = 1 << 0;
const PORT_LINKSTS_UP: u32 = 1 << 0;
const PORT_APPCLK_EN: u32 = 1 << 0;
const PORT_APPCLK_CLKGATE_DIS: u32 = 1 << 8;
const PORT_REFCLK_EN: u32 = 1 << 0;
const PORT_REFCLK_CLKGATE_DIS: u32 = 1 << 8;
const PORT_PERST_OFF: u32 = 1 << 0;
const PORT_STATUS_READY: u32 = 1 << 0;

const PORT_INT_LINK_UP: u32 = 1 << 12;
const PORT_INT_LINK_DOWN: u32 = 1 << 14;

const PORT_RID2SID_VALID: u32 = 1 << 31;

const PCI_CONFIG_VENDOR_ID: u16 = 0x0000;
const PCI_CONFIG_COMMAND: u16 = 0x0004;
const PCI_CONFIG_HEADER_TYPE: u16 = 0x000C;
const PCI_CONFIG_CLASS_REV: u16 = 0x0008;

const PCI_COMMAND_IO_SPACE: u16 = 1 << 0;
const PCI_COMMAND_MEMORY_SPACE: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;
const PCI_COMMAND_SERR: u16 = 1 << 8;

pub struct ApplePciePort {
    port_idx: u32,
    port_base: usize,
    phy_base: usize,
    ecam_base: usize,
    initialized: bool,
    msi_config: Option<Arc<MsiPortConfig>>,
    msi_phandle: u32,
    msi_base_vector: u32,
    msi_num_vectors: u32,
}

impl ApplePciePort {
    fn read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.port_base + offset) }
    }

    fn write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.port_base + offset, val) }
    }

    fn phy_read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.phy_base + offset) }
    }

    fn phy_write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.phy_base + offset, val) }
    }

    fn ecam_read16(&self, bus: u32, dev: u32, func: u32, offset: u16) -> u16 {
        let addr = self.ecam_base
            + (bus as usize) * 0x100000
            + (dev as usize) * 0x8000
            + (func as usize) * 0x1000
            + (offset as usize);
        unsafe { mmio::read16(addr) }
    }

    fn ecam_write16(&self, bus: u32, dev: u32, func: u32, offset: u16, val: u16) {
        let addr = self.ecam_base
            + (bus as usize) * 0x100000
            + (dev as usize) * 0x8000
            + (func as usize) * 0x1000
            + (offset as usize);
        unsafe { mmio::write16(addr, val) }
    }

    fn setup_refclk(&self) -> Result<(), &'static str> {
        self.phy_write32(PHY_LANE_CTL, PHY_LANE_CTL_CFGACC);

        let mut cfg = self.phy_read32(PHY_LANE_CFG);
        cfg |= PHY_LANE_CFG_REFCLK0_REQ;
        self.phy_write32(PHY_LANE_CFG, cfg);

        let mut timeout = 10000;
        while timeout > 0 {
            cfg = self.phy_read32(PHY_LANE_CFG);
            if cfg & PHY_LANE_CFG_REFCLK0_ACK != 0 {
                break;
            }
            timeout -= 1;
            core::hint::spin_loop();
        }
        if timeout == 0 {
            return Err("pcie: REFCLK0 ACK timeout");
        }

        cfg = self.phy_read32(PHY_LANE_CFG);
        cfg |= PHY_LANE_CFG_REFCLK1_REQ;
        self.phy_write32(PHY_LANE_CFG, cfg);

        timeout = 10000;
        while timeout > 0 {
            cfg = self.phy_read32(PHY_LANE_CFG);
            if cfg & PHY_LANE_CFG_REFCLK1_ACK != 0 {
                break;
            }
            timeout -= 1;
            core::hint::spin_loop();
        }
        if timeout == 0 {
            return Err("pcie: REFCLK1 ACK timeout");
        }

        cfg = self.phy_read32(PHY_LANE_CFG);
        cfg |= PHY_LANE_CFG_REFCLK_ENABLE | PHY_LANE_CFG_REFCLK1_ENABLE;
        self.phy_write32(PHY_LANE_CFG, cfg);

        self.phy_write32(PHY_LANE_CTL, 0);

        Ok(())
    }

    pub fn init(&mut self) -> Result<(), &'static str> {
        early_println!("[apple-pcie] port {}: initializing...", self.port_idx);

        self.write32(PORT_APPCLK, PORT_APPCLK_EN | PORT_APPCLK_CLKGATE_DIS);
        self.setup_refclk()?;
        self.write32(PORT_REFCLK, PORT_REFCLK_EN | PORT_REFCLK_CLKGATE_DIS);

        for _ in 0..1000 {
            core::hint::spin_loop();
        }

        self.write32(PORT_PERST, PORT_PERST_OFF);

        let mut timeout = 250000;
        while timeout > 0 {
            let status = self.read32(PORT_STATUS);
            if status & PORT_STATUS_READY != 0 {
                break;
            }
            timeout -= 1;
            core::hint::spin_loop();
        }
        if timeout == 0 {
            early_println!(
                "[apple-pcie] port {}: timeout waiting for ready",
                self.port_idx
            );
            return Err("pcie: port ready timeout");
        }

        self.write32(PORT_LTSSMCTL, PORT_LTSSMCTL_START);
        self.write32(PORT_INTMSK, PORT_INT_LINK_UP | PORT_INT_LINK_DOWN);

        let rid2sid = PORT_RID2SID_VALID | (self.port_idx << 16);
        self.write32(PORT_RID2SID, rid2sid);

        let msi = Arc::new(MsiPortConfig::new(
            self.port_base,
            self.msi_base_vector,
            self.msi_num_vectors,
        ));
        msi.enable_msi();
        register_apple_msi_controller(Arc::clone(&msi), self.msi_phandle);
        self.msi_config = Some(msi);

        self.initialized = true;
        early_println!(
            "[apple-pcie] port {}: initialized successfully",
            self.port_idx
        );
        Ok(())
    }

    pub fn is_link_up(&self) -> bool {
        self.read32(PORT_LINKSTS) & PORT_LINKSTS_UP != 0
    }

    pub fn port_idx(&self) -> u32 {
        self.port_idx
    }

    pub fn ecam_base(&self) -> usize {
        self.ecam_base
    }

    pub fn read_root_vendor_id(&self) -> u16 {
        self.ecam_read16(0, 0, 0, PCI_CONFIG_VENDOR_ID)
    }

    pub fn enable_root_port(&self) {
        let cmd = self.ecam_read16(0, 0, 0, PCI_CONFIG_COMMAND);
        let new_cmd = cmd | PCI_COMMAND_MEMORY_SPACE | PCI_COMMAND_BUS_MASTER | PCI_COMMAND_SERR;
        self.ecam_write16(0, 0, 0, PCI_CONFIG_COMMAND, new_cmd);
    }
}

static PCIE_REGISTRY: Mutex<alloc::vec::Vec<Arc<Mutex<ApplePciePort>>>> =
    Mutex::new(alloc::vec::Vec::new());

pub fn register_port(port: ApplePciePort) -> u32 {
    let mut guard = PCIE_REGISTRY.lock();
    let id = guard.len() as u32;
    guard.push(Arc::new(Mutex::new(port)));
    id
}

pub fn get_port(id: u32) -> Option<Arc<Mutex<ApplePciePort>>> {
    let guard = PCIE_REGISTRY.lock();
    guard.get(id as usize).cloned()
}

/// Probe an Apple PCIe controller and initialize its ports.
///
/// MSI setup intentionally uses both paths: direct port MMIO programming via
/// [`MsiPortConfig::enable_msi`] for hardware initialization, and DeviceManager
/// MSI controller registration so PCI endpoint drivers can resolve vectors via
/// the common MSI framework.
fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let resources = device.get_resources();
    let mem_resources: Vec<_> = resources
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    if mem_resources.len() < 4 {
        return Err("apple-pcie: expected at least 4 memory regions (config, rc, port0, port1)");
    }

    let config_paddr = mem_resources[0].start;
    let _rc_paddr = mem_resources[1].start;
    let config_size = mem_resources[0].end - mem_resources[0].start + 1;

    early_println!(
        "[apple-pcie] probing {} config={:#x} ({} regions)",
        device.name(),
        config_paddr,
        mem_resources.len()
    );

    let config_vaddr = scarlet::vm::ioremap(config_paddr, config_size)
        .map_err(|_| "pcie: config ioremap failed")?;

    if let Some(iommu_prop) = device.property("iommu-map") {
        let bytes = iommu_prop.value();
        let entry_size = 16;
        let mut offset = 0usize;
        while offset + entry_size <= bytes.len() {
            let rid_base =
                u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap_or([0; 4]));
            let dart_phandle =
                u32::from_be_bytes(bytes[offset + 4..offset + 8].try_into().unwrap_or([0; 4]));
            let sid_base =
                u32::from_be_bytes(bytes[offset + 8..offset + 12].try_into().unwrap_or([0; 4]));
            let rid_length =
                u32::from_be_bytes(bytes[offset + 12..offset + 16].try_into().unwrap_or([0; 4]));
            offset += entry_size;

            if let Some(dart) = get_dart_by_phandle(dart_phandle) {
                early_println!(
                    "[apple-pcie] iommu-map: RID {:#x}-{:#x} -> DART phandle={:#x} SID {:#x}",
                    rid_base,
                    rid_base + rid_length - 1,
                    dart_phandle,
                    sid_base
                );
                for sid in sid_base..sid_base + rid_length {
                    dart.enable_bypass(sid as usize);
                }
            } else {
                early_println!(
                    "[apple-pcie] iommu-map: DART phandle={:#x} not ready, deferring",
                    dart_phandle
                );
                let defer = probe_defer();
                if let Err(e) = defer {
                    debug_assert!(is_probe_defer(e));
                    return Err(e);
                }
                return defer;
            }
        }
    }

    let msi_base_vector: u32;
    let msi_num_vectors: u32;

    let msi_phandle = device
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

    if let Some(msi_prop) = device.property("msi-ranges") {
        let bytes = msi_prop.value();
        if bytes.len() >= 20 {
            msi_base_vector = u32::from_be_bytes(bytes[8..12].try_into().unwrap_or([0; 4]));
            msi_num_vectors = u32::from_be_bytes(bytes[16..20].try_into().unwrap_or([0; 4]));
            early_println!(
                "[apple-pcie] msi-ranges: base={:#x} nvecs={}",
                msi_base_vector,
                msi_num_vectors
            );
        } else {
            msi_base_vector = 0;
            msi_num_vectors = 32;
        }
    } else {
        msi_base_vector = 0;
        msi_num_vectors = 32;
    }

    let port_regions = &mem_resources[2..];
    let mut initialized_port_ids: alloc::vec::Vec<u32> = alloc::vec::Vec::new();

    for (port_idx, port_mem) in port_regions.iter().enumerate() {
        let port_paddr = port_mem.start;
        let port_size = port_mem.end - port_mem.start + 1;

        let port_vaddr =
            scarlet::vm::ioremap(port_paddr, port_size).map_err(|_| "pcie: port ioremap failed")?;

        let phy_base = config_vaddr + 0x84000 + port_idx * 0x4000;
        let port_msi_base = msi_base_vector + (port_idx as u32) * msi_num_vectors;

        let mut port = ApplePciePort {
            port_idx: port_idx as u32,
            port_base: port_vaddr,
            phy_base,
            ecam_base: config_vaddr,
            initialized: false,
            msi_config: None,
            msi_phandle,
            msi_base_vector: port_msi_base,
            msi_num_vectors,
        };

        if let Err(e) = port.init() {
            early_println!("[apple-pcie] port {} init failed: {}", port_idx, e);
            continue;
        }

        let id = register_port(port);
        initialized_port_ids.push(id);

        early_println!(
            "[apple-pcie] port {} at {:#x} initialized and registered (id={})",
            port_idx,
            port_paddr,
            id
        );
    }

    for port_id in &initialized_port_ids {
        if let Some(port) = get_port(*port_id) {
            let port = port.lock();
            if port.is_link_up() {
                early_println!("[apple-pcie] port {}: link is UP", port.port_idx());
                port.enable_root_port();
            } else {
                early_println!("[apple-pcie] port {}: no link detected", port.port_idx());
            }
        }
    }

    let pci_bus = scarlet::device::pci::PciBus::new(config_paddr, config_size);
    match pci_bus.scan_and_register() {
        Ok(()) => early_println!("[apple-pcie] PCI devices registered with DeviceManager"),
        Err(e) => early_println!("[apple-pcie] PCI scan failed: {}", e),
    }

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_pcie_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-pcie",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-pcie",
            "apple,pcie",
            "apple,t6000-pcie",
            "apple,t6020-pcie",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_pcie_driver);

#[used]
static SCARLET_DRIVER_APPLE_PCIE_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
