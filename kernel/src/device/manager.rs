//! # Device Manager Module
//!
//! This module provides functionality for managing hardware devices in the kernel.
//!
//! ## Overview
//!
//! The device manager is responsible for:
//! - Tracking available device drivers with priority-based initialization
//! - Device discovery and initialization through FDT
//! - Managing device information and lifecycle
//!
//! ## Key Components
//!
//! - `DeviceManager`: The main device management system that handles all devices and drivers
//! - `DriverPriority`: Priority levels for controlling driver initialization order
//!
//! ## Device Discovery
//!
//! Devices are discovered through the Flattened Device Tree (FDT). The manager:
//! 1. Parses the device tree
//! 2. Matches compatible devices with registered drivers based on priority
//! 3. Probes devices with appropriate drivers in priority order
//!
//! ## Usage
//!
//! The device manager is implemented as a global singleton that can be accessed via:
//! - `DeviceManager::get_manager()` - Shared access (thread-safe via internal Mutex)
//!
//! ### Example: Registering a device driver
//!
//! ```
//! use crate::device::manager::{DeviceManager, DriverPriority};
//!
//! // Create a new device driver
//! let my_driver = Box::new(MyDeviceDriver::new());
//!
//! // Register with the device manager at Core priority
//! DeviceManager::get_manager().register_driver(my_driver, DriverPriority::Core);
//! ```

extern crate alloc;

use core::sync::atomic::Ordering;
use core::sync::atomic::{AtomicU32, AtomicUsize};

use alloc::boxed::Box;
use alloc::collections::btree_map::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::mutex::Mutex;

use crate::device::platform::PlatformDeviceInfo;
use crate::device::platform::PlatformDeviceProperty;
use crate::device::platform::resource::PlatformDeviceResource;
use crate::device::platform::resource::PlatformDeviceResourceType;
use crate::early_println;
use crate::interrupt::msi::MsiController;

use super::Device;
use super::DeviceDriver;
use super::DeviceInfo;
use super::clk::{ClkError, ClkHandle, ClkProvider};
use super::dma::{DmaChannel, DmaController, DmaError, DmaSpec};
use super::gpio::GpioController;
use super::i2c::I2cBus;
use super::iommu::{
    DmaContext, IommuAttachment, IommuController, IommuDomainConfig, IommuError, IommuSpec,
};
use super::mailbox::{MailboxChannel, MailboxClient, MailboxController, MailboxError, MailboxSpec};
use super::nvmem::{NvmemCell, NvmemError, NvmemProvider};
use super::phy::{PhyError, PhyHandle, PhyProvider};
use super::remoteproc::{RemoteProcessor, RemoteprocService, RemoteprocServiceId};
use super::reset::{ResetController, ResetHandle};
use super::spi::SpiBus;
use super::usb::UsbHostController;
use super::watchdog::Watchdog;
use crate::DeviceSource;

/// Simplified shared device type
pub type SharedDevice = Arc<dyn Device>;

/// Probe result string used when a driver cannot probe until a provider appears.
pub const PROBE_DEFER: &str = "probe: deferred";

/// Return the standard probe deferral error.
///
/// # Returns
///
/// Always returns `Err(PROBE_DEFER)` for the requested result type.
pub fn probe_defer<T>() -> Result<T, &'static str> {
    Err(PROBE_DEFER)
}

/// Check whether an error string is the probe deferral sentinel.
///
/// # Arguments
///
/// * `err` - Error string returned by a probe or pre-probe dependency hook.
///
/// # Returns
///
/// `true` when `err` is exactly [`PROBE_DEFER`].
pub fn is_probe_defer(err: &str) -> bool {
    err == PROBE_DEFER
}

/// Driver priority levels for initialization order
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DriverPriority {
    /// Critical infrastructure drivers (interrupt controllers, memory controllers)
    Critical = 0,
    /// Core system drivers (timers, basic I/O)
    Core = 1,
    /// Standard device drivers (network, storage)
    Standard = 2,
    /// Late initialization drivers (filesystems, user interface)
    Late = 3,
}

#[derive(Clone)]
struct DeferredPlatformDevice {
    priority: DriverPriority,
    device: Arc<PlatformDeviceInfo>,
}

struct OwnedClkSpec {
    phandle: u32,
    cells: Vec<u32>,
}

struct OwnedDmaSpec {
    phandle: u32,
    cells: Vec<u32>,
}

struct OwnedIommuSpec {
    phandle: u32,
    cells: Vec<u32>,
}

struct OwnedMailboxSpec {
    phandle: u32,
    cells: Vec<u32>,
}

struct OwnedNvmemSpec {
    phandle: u32,
    cells: Vec<u32>,
}

struct OwnedPhySpec {
    phandle: u32,
    cells: Vec<u32>,
}

struct OwnedResetSpec {
    phandle: u32,
    cells: Vec<u32>,
}

enum ProbeOutcome {
    Probed,
    Deferred,
    Failed,
    NoMatch,
}

impl DriverPriority {
    /// Get all priority levels in order
    pub fn all() -> &'static [DriverPriority] {
        &[
            DriverPriority::Critical,
            DriverPriority::Core,
            DriverPriority::Standard,
            DriverPriority::Late,
        ]
    }

    /// Get a human-readable description of the priority level
    pub fn description(&self) -> &'static str {
        match self {
            DriverPriority::Critical => "Critical Infrastructure",
            DriverPriority::Core => "Core System",
            DriverPriority::Standard => "Standard Devices",
            DriverPriority::Late => "Late Initialization",
        }
    }
}

static MANAGER: DeviceManager = DeviceManager::new();

/// DeviceManager
///
/// This struct is the main device management system.
/// It handles all devices and drivers with priority-based initialization.
///
/// # Fields
/// - `devices`: A mutex-protected map of all registered devices by ID.
/// - `device_by_name`: A mutex-protected map of devices by name.
/// - `name_to_id`: A mutex-protected map from device name to device ID.
/// - `drivers`: A mutex-protected map of device drivers organized by priority.
/// - `next_device_id`: Atomic counter for generating unique device IDs.
pub struct DeviceManager {
    /* Devices stored by ID */
    devices: Mutex<BTreeMap<usize, SharedDevice>>,
    /* Devices stored by name */
    device_by_name: Mutex<BTreeMap<String, SharedDevice>>,
    /* Name to ID mapping */
    name_to_id: Mutex<BTreeMap<String, usize>>,
    /* Registered device drivers organized by priority */
    drivers: Mutex<BTreeMap<DriverPriority, Vec<Box<dyn DeviceDriver>>>>,
    /* Platform devices waiting for provider drivers to become available */
    deferred_platform_devices: Mutex<Vec<DeferredPlatformDevice>>,
    /* Discovered PCI devices awaiting driver probe */
    discovered_pci_devices: Mutex<Vec<Arc<dyn DeviceInfo + Send + Sync>>>,
    /* Bus controller registries (phandle → bus) */
    spi_buses: Mutex<BTreeMap<u32, Arc<dyn SpiBus>>>,
    i2c_buses: Mutex<BTreeMap<u32, Arc<dyn I2cBus>>>,
    usb_hosts: Mutex<BTreeMap<u32, Arc<dyn UsbHostController>>>,
    gpio_controllers: Mutex<BTreeMap<u32, Arc<dyn GpioController>>>,
    clk_providers: Mutex<BTreeMap<u32, Arc<dyn ClkProvider>>>,
    dma_controllers: Mutex<BTreeMap<u32, Arc<dyn DmaController>>>,
    iommu_controllers: Mutex<BTreeMap<u32, Arc<dyn IommuController>>>,
    msi_controllers: Mutex<BTreeMap<u32, Arc<dyn MsiController>>>,
    mailbox_controllers: Mutex<BTreeMap<u32, Arc<dyn MailboxController>>>,
    nvmem_providers: Mutex<BTreeMap<u32, Arc<dyn NvmemProvider>>>,
    phy_providers: Mutex<BTreeMap<u32, Arc<dyn PhyProvider>>>,
    reset_controllers: Mutex<BTreeMap<u32, Arc<dyn ResetController>>>,
    remote_processors: Mutex<BTreeMap<u32, Arc<dyn RemoteProcessor>>>,
    watchdogs: Mutex<Vec<Arc<dyn Watchdog>>>,
    /* Next available device ID */
    next_device_id: AtomicUsize,
    next_auto_phandle: AtomicU32,
    auto_phandle_cache: Mutex<BTreeMap<usize, u32>>,
}

impl DeviceManager {
    const fn new() -> Self {
        DeviceManager {
            devices: Mutex::new(BTreeMap::new()),
            device_by_name: Mutex::new(BTreeMap::new()),
            name_to_id: Mutex::new(BTreeMap::new()),
            drivers: Mutex::new(BTreeMap::new()),
            deferred_platform_devices: Mutex::new(Vec::new()),
            discovered_pci_devices: Mutex::new(Vec::new()),
            spi_buses: Mutex::new(BTreeMap::new()),
            i2c_buses: Mutex::new(BTreeMap::new()),
            usb_hosts: Mutex::new(BTreeMap::new()),
            gpio_controllers: Mutex::new(BTreeMap::new()),
            clk_providers: Mutex::new(BTreeMap::new()),
            dma_controllers: Mutex::new(BTreeMap::new()),
            iommu_controllers: Mutex::new(BTreeMap::new()),
            msi_controllers: Mutex::new(BTreeMap::new()),
            mailbox_controllers: Mutex::new(BTreeMap::new()),
            nvmem_providers: Mutex::new(BTreeMap::new()),
            phy_providers: Mutex::new(BTreeMap::new()),
            reset_controllers: Mutex::new(BTreeMap::new()),
            remote_processors: Mutex::new(BTreeMap::new()),
            watchdogs: Mutex::new(Vec::new()),
            next_device_id: AtomicUsize::new(1),
            next_auto_phandle: AtomicU32::new(0x8000),
            auto_phandle_cache: Mutex::new(BTreeMap::new()),
        }
    }

    #[cfg(test)]
    pub const fn new_for_test() -> Self {
        Self::new()
    }

    pub fn get_manager() -> &'static DeviceManager {
        &MANAGER
    }

    fn read_be_u32(bytes: &[u8]) -> Option<u32> {
        if bytes.len() < 4 {
            return None;
        }
        Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_be_u32_cells(bytes: &[u8]) -> Option<Vec<u32>> {
        if !bytes.len().is_multiple_of(4) {
            return None;
        }

        let mut cells = Vec::new();
        for chunk in bytes.chunks_exact(4) {
            cells.push(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Some(cells)
    }

    fn get_clock_cells_for_phandle(&self, phandle: u32) -> Option<usize> {
        self.get_clk_provider_by_phandle(phandle)
            .map(|provider| provider.clock_cells())
    }

    fn get_dma_cells_for_phandle(&self, phandle: u32) -> Option<usize> {
        self.get_dma_controller_by_phandle(phandle)
            .map(|controller| controller.dma_cells())
    }

    fn get_iommu_cells_for_phandle(&self, phandle: u32) -> Option<usize> {
        let fdt = crate::device::fdt::FdtManager::get_manager().get_fdt()?;
        let node = Self::find_node_by_phandle(fdt, phandle)?;
        Self::get_u32_prop(&node, "#iommu-cells").map(|cells| cells as usize)
    }

    fn get_mailbox_cells_for_phandle(&self, phandle: u32) -> Option<usize> {
        #[cfg(test)]
        let fdt = {
            // SAFETY: Unit tests may exercise parser helpers before boot initializes the
            // global FDT manager. These tests run single-threaded in the kernel harness.
            unsafe { crate::device::fdt::FdtManager::get_mut_manager() }.get_fdt()?
        };

        #[cfg(not(test))]
        let fdt = crate::device::fdt::FdtManager::get_manager().get_fdt()?;

        let node = Self::find_node_by_phandle(fdt, phandle)?;
        Self::get_u32_prop(&node, "#mbox-cells").map(|cells| cells as usize)
    }

    fn get_nvmem_cell_cells_for_phandle(&self, phandle: u32) -> Option<usize> {
        self.get_nvmem_provider_by_phandle(phandle)
            .map(|provider| provider.cell_cells())
    }

    fn get_phy_cells_for_phandle(&self, phandle: u32) -> Option<usize> {
        self.get_phy_provider_by_phandle(phandle)
            .map(|provider| provider.phy_cells())
    }

    fn get_reset_cells_for_phandle(&self, phandle: u32) -> Option<usize> {
        self.get_reset_controller_by_phandle(phandle)
            .map(|controller| controller.reset_cells())
    }

    fn parse_clock_specs(&self, bytes: &[u8]) -> Result<Vec<OwnedClkSpec>, &'static str> {
        let cells = Self::read_be_u32_cells(bytes).ok_or("clk: malformed property")?;
        let mut specs = Vec::new();
        let mut index = 0usize;

        while index < cells.len() {
            let phandle = cells[index];
            index += 1;
            let clock_cells = self
                .get_clock_cells_for_phandle(phandle)
                .ok_or("clk: provider not found")?;
            if index + clock_cells > cells.len() {
                return Err("clk: truncated clock specifier");
            }

            specs.push(OwnedClkSpec {
                phandle,
                cells: cells[index..index + clock_cells].to_vec(),
            });
            index += clock_cells;
        }

        Ok(specs)
    }

    fn parse_assigned_parent_specs(
        &self,
        bytes: &[u8],
    ) -> Result<Vec<Option<OwnedClkSpec>>, &'static str> {
        let cells = Self::read_be_u32_cells(bytes).ok_or("clk: malformed property")?;
        let mut specs = Vec::new();
        let mut index = 0usize;

        while index < cells.len() {
            let phandle = cells[index];
            index += 1;
            if phandle == 0 {
                specs.push(None);
                continue;
            }

            let clock_cells = self
                .get_clock_cells_for_phandle(phandle)
                .ok_or("clk: provider not found")?;
            if index + clock_cells > cells.len() {
                return Err("clk: truncated clock specifier");
            }

            specs.push(Some(OwnedClkSpec {
                phandle,
                cells: cells[index..index + clock_cells].to_vec(),
            }));
            index += clock_cells;
        }

        Ok(specs)
    }

    fn parse_dma_specs(&self, bytes: &[u8]) -> Result<Vec<OwnedDmaSpec>, &'static str> {
        let cells = Self::read_be_u32_cells(bytes).ok_or("dma: malformed property")?;
        let mut specs = Vec::new();
        let mut index = 0usize;

        while index < cells.len() {
            let phandle = cells[index];
            index += 1;

            let dma_cells = self.get_dma_cells_for_phandle(phandle).ok_or(PROBE_DEFER)?;
            if index + dma_cells > cells.len() {
                return Err("dma: truncated specifier");
            }

            specs.push(OwnedDmaSpec {
                phandle,
                cells: cells[index..index + dma_cells].to_vec(),
            });
            index += dma_cells;
        }

        Ok(specs)
    }

    fn parse_iommu_specs(&self, bytes: &[u8]) -> Result<Vec<OwnedIommuSpec>, &'static str> {
        let cells = Self::read_be_u32_cells(bytes).ok_or("iommu: malformed property")?;
        let mut specs = Vec::new();
        let mut index = 0usize;

        while index < cells.len() {
            let phandle = cells[index];
            index += 1;

            if self.get_iommu_controller_by_phandle(phandle).is_none() {
                return probe_defer();
            }

            let iommu_cells = self.get_iommu_cells_for_phandle(phandle).unwrap_or(1);
            if index + iommu_cells > cells.len() {
                return Err("iommu: truncated specifier");
            }

            specs.push(OwnedIommuSpec {
                phandle,
                cells: cells[index..index + iommu_cells].to_vec(),
            });
            index += iommu_cells;
        }

        Ok(specs)
    }

    fn parse_mailbox_specs(&self, bytes: &[u8]) -> Result<Vec<OwnedMailboxSpec>, &'static str> {
        let cells = Self::read_be_u32_cells(bytes).ok_or("mailbox: malformed property")?;
        let mut specs = Vec::new();
        let mut index = 0usize;

        while index < cells.len() {
            let phandle = cells[index];
            index += 1;

            if self.get_mailbox_controller_by_phandle(phandle).is_none() {
                return probe_defer();
            }

            let mailbox_cells = self.get_mailbox_cells_for_phandle(phandle).unwrap_or(1);
            if index + mailbox_cells > cells.len() {
                return Err("mailbox: truncated specifier");
            }

            specs.push(OwnedMailboxSpec {
                phandle,
                cells: cells[index..index + mailbox_cells].to_vec(),
            });
            index += mailbox_cells;
        }

        Ok(specs)
    }

    fn parse_nvmem_specs(&self, bytes: &[u8]) -> Result<Vec<OwnedNvmemSpec>, &'static str> {
        let cells = Self::read_be_u32_cells(bytes).ok_or("nvmem: malformed property")?;
        let mut specs = Vec::new();
        let mut index = 0usize;

        while index < cells.len() {
            let phandle = cells[index];
            index += 1;

            if self.get_nvmem_provider_by_phandle(phandle).is_none() {
                return probe_defer();
            }

            let cell_cells = self.get_nvmem_cell_cells_for_phandle(phandle).unwrap_or(2);
            if index + cell_cells > cells.len() {
                return Err("nvmem: truncated cell specifier");
            }

            specs.push(OwnedNvmemSpec {
                phandle,
                cells: cells[index..index + cell_cells].to_vec(),
            });
            index += cell_cells;
        }

        Ok(specs)
    }

    fn parse_phy_specs(&self, bytes: &[u8]) -> Result<Vec<OwnedPhySpec>, &'static str> {
        let cells = Self::read_be_u32_cells(bytes).ok_or("phy: malformed property")?;
        let mut specs = Vec::new();
        let mut index = 0usize;

        while index < cells.len() {
            let phandle = cells[index];
            index += 1;

            if self.get_phy_provider_by_phandle(phandle).is_none() {
                return probe_defer();
            }

            let phy_cells = self.get_phy_cells_for_phandle(phandle).unwrap_or(0);
            if index + phy_cells > cells.len() {
                return Err("phy: truncated specifier");
            }

            specs.push(OwnedPhySpec {
                phandle,
                cells: cells[index..index + phy_cells].to_vec(),
            });
            index += phy_cells;
        }

        Ok(specs)
    }

    fn parse_reset_specs(&self, bytes: &[u8]) -> Result<Vec<OwnedResetSpec>, &'static str> {
        let cells = Self::read_be_u32_cells(bytes).ok_or("reset: malformed property")?;
        let mut specs = Vec::new();
        let mut index = 0usize;

        while index < cells.len() {
            let phandle = cells[index];
            index += 1;

            if self.get_reset_controller_by_phandle(phandle).is_none() {
                return probe_defer();
            }

            let reset_cells = self.get_reset_cells_for_phandle(phandle).unwrap_or(0);
            if index + reset_cells > cells.len() {
                return Err("reset: truncated specifier");
            }

            specs.push(OwnedResetSpec {
                phandle,
                cells: cells[index..index + reset_cells].to_vec(),
            });
            index += reset_cells;
        }

        Ok(specs)
    }

    fn clock_name_index(device: &PlatformDeviceInfo, name: &str) -> Result<usize, &'static str> {
        let names = device
            .property("clock-names")
            .ok_or("clk: clock-names missing")?
            .as_string_list()
            .ok_or("clk: malformed clock-names")?;

        names
            .iter()
            .position(|entry| *entry == name)
            .ok_or("clk: clock name not found")
    }

    fn dma_name_index(device: &PlatformDeviceInfo, name: &str) -> Result<usize, &'static str> {
        let names = device
            .property("dma-names")
            .ok_or("dma: dma-names missing")?
            .as_string_list()
            .ok_or("dma: malformed dma-names")?;

        names
            .iter()
            .position(|entry| *entry == name)
            .ok_or("dma: name not found")
    }

    fn mailbox_name_index(device: &PlatformDeviceInfo, name: &str) -> Result<usize, &'static str> {
        let names = device
            .property("mbox-names")
            .ok_or("mailbox: mbox-names missing")?
            .as_string_list()
            .ok_or("mailbox: malformed mbox-names")?;

        names
            .iter()
            .position(|entry| *entry == name)
            .ok_or("mailbox: name not found")
    }

    fn nvmem_cell_name_index(
        device: &PlatformDeviceInfo,
        name: &str,
    ) -> Result<usize, &'static str> {
        let names = device
            .property("nvmem-cell-names")
            .ok_or("nvmem: nvmem-cell-names missing")?
            .as_string_list()
            .ok_or("nvmem: malformed nvmem-cell-names")?;

        names
            .iter()
            .position(|entry| *entry == name)
            .ok_or("nvmem: cell name not found")
    }

    fn phy_name_index(device: &PlatformDeviceInfo, name: &str) -> Result<usize, &'static str> {
        let names = device
            .property("phy-names")
            .ok_or("phy: phy-names missing")?
            .as_string_list()
            .ok_or("phy: malformed phy-names")?;

        names
            .iter()
            .position(|entry| *entry == name)
            .ok_or("phy: name not found")
    }

    fn reset_name_index(device: &PlatformDeviceInfo, name: &str) -> Result<usize, &'static str> {
        let names = device
            .property("reset-names")
            .ok_or("reset: reset-names missing")?
            .as_string_list()
            .ok_or("reset: malformed reset-names")?;

        names
            .iter()
            .position(|entry| *entry == name)
            .ok_or("reset: name not found")
    }

    fn clk_error_to_str(error: ClkError) -> &'static str {
        match error {
            ClkError::Unsupported => "clk: unsupported",
            ClkError::InvalidRate => "clk: invalid rate",
            ClkError::InvalidParent => "clk: invalid parent",
            ClkError::ProviderNotFound => "clk: provider not found",
            ClkError::ClockNotFound => "clk: clock not found",
            ClkError::InvalidSpecifier => "clk: invalid specifier",
            ClkError::HardwareError => "clk: hardware error",
            ClkError::Busy => "clk: busy",
            ClkError::NotFound => "clk: not found",
        }
    }

    fn dma_error_to_str(error: DmaError) -> &'static str {
        match error {
            DmaError::InvalidSpec => "dma: invalid specifier",
            DmaError::ChannelNotFound => "dma: channel not found",
            DmaError::ChannelBusy => "dma: channel busy",
            DmaError::InvalidConfig => "dma: invalid config",
            DmaError::Unsupported => "dma: unsupported",
            DmaError::HardwareError => "dma: hardware error",
            DmaError::NotPrepared => "dma: not prepared",
        }
    }

    fn iommu_error_to_str(error: IommuError) -> &'static str {
        match error {
            IommuError::InvalidSpec => "iommu: invalid specifier",
            IommuError::ControllerNotFound => "iommu: controller not found",
            IommuError::DomainAllocationFailed => "iommu: domain allocation failed",
            IommuError::AttachFailed => "iommu: attach failed",
            IommuError::MapFailed => "iommu: map failed",
            IommuError::UnmapFailed => "iommu: unmap failed",
            IommuError::OutOfIova => "iommu: out of iova",
            IommuError::NotSupported => "iommu: not supported",
            IommuError::Busy => "iommu: busy",
        }
    }

    fn mailbox_error_to_str(error: MailboxError) -> &'static str {
        match error {
            MailboxError::ControllerNotFound => "mailbox: controller not found",
            MailboxError::InvalidChannel => "mailbox: invalid channel",
            MailboxError::Busy => "mailbox: busy",
            MailboxError::Empty => "mailbox: empty",
            MailboxError::Timeout => "mailbox: timeout",
            MailboxError::HardwareError => "mailbox: hardware error",
            MailboxError::NotSupported => "mailbox: not supported",
        }
    }

    fn nvmem_error_to_str(error: NvmemError) -> &'static str {
        match error {
            NvmemError::NotFound => "nvmem: not found",
            NvmemError::OutOfRange => "nvmem: out of range",
            NvmemError::ReadFailed => "nvmem: read failed",
            NvmemError::WriteFailed => "nvmem: write failed",
            NvmemError::NotSupported => "nvmem: not supported",
            NvmemError::Busy => "nvmem: busy",
            NvmemError::HardwareError => "nvmem: hardware error",
        }
    }

    fn phy_error_to_str(error: PhyError) -> &'static str {
        match error {
            PhyError::NotFound => "phy: not found",
            PhyError::NotSupported => "phy: not supported",
            PhyError::InvalidMode => "phy: invalid mode",
            PhyError::PowerOnFailed => "phy: power on failed",
            PhyError::PowerOffFailed => "phy: power off failed",
            PhyError::ResetFailed => "phy: reset failed",
            PhyError::Busy => "phy: busy",
            PhyError::Timeout => "phy: timeout",
            PhyError::HardwareError => "phy: hardware error",
        }
    }

    fn resolve_nvmem_spec(
        &self,
        spec: &OwnedNvmemSpec,
        name: &'static str,
    ) -> Result<NvmemCell, &'static str> {
        let provider = self
            .get_nvmem_provider_by_phandle(spec.phandle)
            .ok_or(PROBE_DEFER)?;
        if spec.cells.len() < 2 {
            return Err("nvmem: invalid cell specifier");
        }

        let offset = spec.cells[0] as usize;
        let size = spec.cells[1] as usize;
        let end = offset.checked_add(size).ok_or("nvmem: out of range")?;
        if end > provider.size() {
            return Err(Self::nvmem_error_to_str(NvmemError::OutOfRange));
        }

        Ok(NvmemCell::new(provider, offset, size, name))
    }

    fn resolve_clk_spec(&self, spec: &OwnedClkSpec) -> Result<ClkHandle, &'static str> {
        let provider = self
            .get_clk_provider_by_phandle(spec.phandle)
            .ok_or("clk: provider not found")?;
        provider
            .get_clk(&spec.cells)
            .map_err(Self::clk_error_to_str)
    }

    fn resolve_dma_spec(&self, spec: &OwnedDmaSpec) -> Result<Arc<dyn DmaChannel>, &'static str> {
        let controller = self
            .get_dma_controller_by_phandle(spec.phandle)
            .ok_or(PROBE_DEFER)?;
        let dma_spec = DmaSpec {
            controller_phandle: spec.phandle,
            cells: spec.cells.clone(),
        };
        controller
            .request_channel(&dma_spec)
            .map_err(Self::dma_error_to_str)
    }

    fn resolve_phy_spec(&self, spec: &OwnedPhySpec) -> Result<PhyHandle, &'static str> {
        let provider = self
            .get_phy_provider_by_phandle(spec.phandle)
            .ok_or(PROBE_DEFER)?;
        provider
            .get_phy(&spec.cells)
            .map_err(Self::phy_error_to_str)
    }

    fn resolve_reset_spec(&self, spec: &OwnedResetSpec) -> Result<ResetHandle, &'static str> {
        let controller = self
            .get_reset_controller_by_phandle(spec.phandle)
            .ok_or(PROBE_DEFER)?;
        Ok(ResetHandle::new(controller, spec.cells.clone()))
    }

    fn get_u32_prop<'a, 'b>(node: &fdt::node::FdtNode<'a, 'b>, name: &str) -> Option<u32> {
        let prop = node.property(name)?;
        Self::read_be_u32(prop.value)
    }

    fn find_node_by_phandle<'a>(
        fdt: &'a fdt::Fdt<'a>,
        phandle: u32,
    ) -> Option<fdt::node::FdtNode<'a, 'a>> {
        let mut stack: Vec<fdt::node::FdtNode<'a, 'a>> = Vec::new();
        stack.push(fdt.find_node("/")?);

        while let Some(node) = stack.pop() {
            if let Some(p) = Self::get_u32_prop(&node, "phandle")
                && p == phandle
            {
                return Some(node);
            }
            if let Some(p) = Self::get_u32_prop(&node, "linux,phandle")
                && p == phandle
            {
                return Some(node);
            }
            for child in node.children() {
                stack.push(child);
            }
        }

        None
    }

    fn find_node_and_parent_by_phandle<'a>(
        fdt: &'a fdt::Fdt<'a>,
        phandle: u32,
    ) -> Option<(fdt::node::FdtNode<'a, 'a>, Option<u32>)> {
        let mut stack: Vec<(fdt::node::FdtNode<'a, 'a>, Option<u32>)> = Vec::new();
        stack.push((fdt.find_node("/")?, None));

        while let Some((node, parent_phandle)) = stack.pop() {
            if let Some(p) = Self::get_u32_prop(&node, "phandle")
                && p == phandle
            {
                return Some((node, parent_phandle));
            }
            if let Some(p) = Self::get_u32_prop(&node, "linux,phandle")
                && p == phandle
            {
                return Some((node, parent_phandle));
            }

            let this_phandle = Self::get_u32_prop(&node, "phandle")
                .or_else(|| Self::get_u32_prop(&node, "linux,phandle"))
                .or(parent_phandle);
            for child in node.children() {
                stack.push((child, this_phandle));
            }
        }

        None
    }

    fn push_irq_resource(
        resources: &mut Vec<PlatformDeviceResource>,
        irq_num: usize,
        metadata: Option<crate::device::platform::resource::IrqMetadata>,
    ) {
        resources.push(PlatformDeviceResource {
            res_type: PlatformDeviceResourceType::IRQ,
            start: irq_num,
            end: irq_num,
            irq_metadata: metadata,
        });
    }

    fn mem_resource_from_region(
        region: fdt::standard_nodes::MemoryRegion,
    ) -> Option<PlatformDeviceResource> {
        let start = region.starting_address as usize;
        let size = region.size?;

        if size == 0 {
            return None;
        }

        let end = start.checked_add(size - 1)?;

        Some(PlatformDeviceResource {
            res_type: PlatformDeviceResourceType::MEM,
            start,
            end,
            irq_metadata: None,
        })
    }

    /// Register a device with the manager
    ///
    /// # Arguments
    /// * `device`: The device to register.
    ///
    /// # Returns
    ///  * The id of the registered device.
    ///
    /// # Example
    ///
    /// ```rust
    /// let device = Arc::new(MyDevice::new());
    /// let id = DeviceManager::get_manager().register_device(device);
    /// ```
    ///
    pub fn register_device(&self, device: Arc<dyn Device>) -> usize {
        let mut devices = self.devices.lock();
        let id = self.next_device_id.fetch_add(1, Ordering::SeqCst);
        devices.insert(id, device);
        id
    }

    /// Register a device with the manager by name
    ///
    /// # Arguments
    /// * `name`: The name of the device.
    /// * `device`: The device to register.
    ///
    /// # Returns
    ///  * The id of the registered device.
    ///
    pub fn register_device_with_name(&self, name: String, device: Arc<dyn Device>) -> usize {
        let mut devices = self.devices.lock();
        let mut device_by_name = self.device_by_name.lock();
        let mut name_to_id = self.name_to_id.lock();

        let id = self.next_device_id.fetch_add(1, Ordering::SeqCst);
        devices.insert(id, device.clone());
        device_by_name.insert(name.clone(), device);
        name_to_id.insert(name, id);
        id
    }

    /// Get a device by ID
    ///
    /// # Arguments
    /// * `id`: The id of the device to get.
    ///
    /// # Returns
    /// * The device if found, or None if not found.
    ///
    pub fn get_device(&self, id: usize) -> Option<SharedDevice> {
        let devices = self.devices.lock();
        devices.get(&id).cloned()
    }

    /// Get a device by name
    ///
    /// # Arguments
    /// * `name`: The name of the device to get.
    ///
    /// # Returns
    /// * The device if found, or None if not found.
    ///
    pub fn get_device_by_name(&self, name: &str) -> Option<SharedDevice> {
        let device_by_name = self.device_by_name.lock();
        device_by_name.get(name).cloned()
    }

    /// Get a device ID by name
    ///
    /// # Arguments
    /// * `name`: The name of the device to find.
    ///
    /// # Returns
    /// * The device ID if found, or None if not found.
    ///
    pub fn get_device_id_by_name(&self, name: &str) -> Option<usize> {
        let name_to_id = self.name_to_id.lock();
        name_to_id.get(name).cloned()
    }

    /// Get the number of devices
    ///
    /// # Returns
    ///
    /// The number of devices.
    ///
    pub fn get_devices_count(&self) -> usize {
        let devices = self.devices.lock();
        devices.len()
    }

    /// Get the first device of a specific type
    ///
    /// # Arguments
    /// * `device_type`: The device type to find.
    ///
    /// # Returns
    /// * The first device ID of the specified type, or None if not found.
    ///
    pub fn get_first_device_by_type(&self, device_type: super::DeviceType) -> Option<usize> {
        let devices = self.devices.lock();
        for (id, device) in devices.iter() {
            if device.device_type() == device_type {
                return Some(*id);
            }
        }
        None
    }

    /// Get all devices registered by name
    ///
    /// Returns an iterator over (name, device) pairs for all devices
    /// that were registered with explicit names.
    ///
    /// # Returns
    ///
    /// Vector of (name, device) tuples
    pub fn get_named_devices(&self) -> Vec<(String, SharedDevice)> {
        let device_by_name = self.device_by_name.lock();
        device_by_name
            .iter()
            .map(|(name, device)| (name.clone(), device.clone()))
            .collect()
    }

    pub fn borrow_drivers(&self) -> &Mutex<BTreeMap<DriverPriority, Vec<Box<dyn DeviceDriver>>>> {
        &self.drivers
    }

    pub fn register_spi_bus(&self, phandle: u32, bus: Arc<dyn SpiBus>) {
        self.spi_buses.lock().insert(phandle, bus);
    }

    pub fn get_spi_bus(&self, phandle: u32) -> Option<Arc<dyn SpiBus>> {
        self.spi_buses.lock().get(&phandle).cloned()
    }

    pub fn register_i2c_bus(&self, phandle: u32, bus: Arc<dyn I2cBus>) {
        self.i2c_buses.lock().insert(phandle, bus);
    }

    pub fn get_i2c_bus(&self, phandle: u32) -> Option<Arc<dyn I2cBus>> {
        self.i2c_buses.lock().get(&phandle).cloned()
    }

    pub fn register_usb_host(&self, id: u32, host: Arc<dyn UsbHostController>) {
        self.usb_hosts.lock().insert(id, host);
    }

    pub fn get_usb_host(&self, id: u32) -> Option<Arc<dyn UsbHostController>> {
        self.usb_hosts.lock().get(&id).cloned()
    }

    pub fn register_gpio_controller(&self, phandle: u32, gpio: Arc<dyn GpioController>) {
        self.gpio_controllers.lock().insert(phandle, gpio);
    }

    pub fn get_gpio_controller(&self, phandle: u32) -> Option<Arc<dyn GpioController>> {
        self.gpio_controllers.lock().get(&phandle).cloned()
    }

    /// Register a DMA controller by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the DMA controller node.
    /// * `controller` - DMA controller implementation.
    pub fn register_dma_controller(&self, phandle: u32, controller: Arc<dyn DmaController>) {
        self.dma_controllers.lock().insert(phandle, controller);
    }

    /// Look up a DMA controller by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the DMA controller node.
    ///
    /// # Returns
    ///
    /// DMA controller registered for `phandle`, or `None` when missing.
    pub fn get_dma_controller_by_phandle(&self, phandle: u32) -> Option<Arc<dyn DmaController>> {
        self.dma_controllers.lock().get(&phandle).cloned()
    }

    /// Register a clock provider by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the provider node.
    /// * `provider` - Clock provider implementation.
    pub fn register_clk_provider(&self, phandle: u32, provider: Arc<dyn ClkProvider>) {
        self.clk_providers.lock().insert(phandle, provider);
    }

    /// Look up a clock provider by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the provider node.
    ///
    /// # Returns
    ///
    /// Clock provider registered for `phandle`, or `None` when missing.
    pub fn get_clk_provider_by_phandle(&self, phandle: u32) -> Option<Arc<dyn ClkProvider>> {
        self.clk_providers.lock().get(&phandle).cloned()
    }

    /// Register an IOMMU controller by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the IOMMU controller node.
    /// * `controller` - IOMMU controller implementation.
    pub fn register_iommu_controller(&self, phandle: u32, controller: Arc<dyn IommuController>) {
        self.iommu_controllers.lock().insert(phandle, controller);
    }

    /// Look up an IOMMU controller by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the IOMMU controller node.
    ///
    /// # Returns
    ///
    /// IOMMU controller registered for `phandle`, or `None` when missing.
    pub fn get_iommu_controller_by_phandle(
        &self,
        phandle: u32,
    ) -> Option<Arc<dyn IommuController>> {
        self.iommu_controllers.lock().get(&phandle).cloned()
    }

    /// Register an MSI controller by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the MSI controller node.
    /// * `controller` - MSI controller implementation.
    pub fn register_msi_controller(&self, phandle: u32, controller: Arc<dyn MsiController>) {
        self.msi_controllers.lock().insert(phandle, controller);
    }

    /// Look up an MSI controller by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the MSI controller node.
    ///
    /// # Returns
    ///
    /// MSI controller registered for `phandle`, or `None` when missing.
    pub fn get_msi_controller_by_phandle(&self, phandle: u32) -> Option<Arc<dyn MsiController>> {
        self.msi_controllers.lock().get(&phandle).cloned()
    }

    /// Iterate over all registered MSI controllers.
    ///
    /// # Arguments
    ///
    /// * `f` - Callback invoked for each MSI controller. Return `false` to stop iteration.
    pub fn for_each_msi_controller<F>(&self, mut f: F)
    where
        F: FnMut(&Arc<dyn MsiController>) -> bool,
    {
        let controllers = self.msi_controllers.lock();
        for controller in controllers.values() {
            if !f(controller) {
                break;
            }
        }
    }

    /// Register a mailbox controller by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the mailbox controller node.
    /// * `controller` - Mailbox controller implementation.
    pub fn register_mailbox_controller(
        &self,
        phandle: u32,
        controller: Arc<dyn MailboxController>,
    ) {
        self.mailbox_controllers.lock().insert(phandle, controller);
    }

    /// Look up a mailbox controller by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the mailbox controller node.
    ///
    /// # Returns
    ///
    /// Mailbox controller registered for `phandle`, or `None` when missing.
    pub fn get_mailbox_controller_by_phandle(
        &self,
        phandle: u32,
    ) -> Option<Arc<dyn MailboxController>> {
        self.mailbox_controllers.lock().get(&phandle).cloned()
    }

    /// Register an NVMEM provider by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the NVMEM provider node.
    /// * `provider` - NVMEM provider implementation.
    pub fn register_nvmem_provider(&self, phandle: u32, provider: Arc<dyn NvmemProvider>) {
        self.nvmem_providers.lock().insert(phandle, provider);
    }

    /// Look up an NVMEM provider by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the NVMEM provider node.
    ///
    /// # Returns
    ///
    /// NVMEM provider registered for `phandle`, or `None` when missing.
    pub fn get_nvmem_provider_by_phandle(&self, phandle: u32) -> Option<Arc<dyn NvmemProvider>> {
        self.nvmem_providers.lock().get(&phandle).cloned()
    }

    /// Register a PHY provider by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the PHY provider node.
    /// * `provider` - PHY provider implementation.
    pub fn register_phy_provider(&self, phandle: u32, provider: Arc<dyn PhyProvider>) {
        self.phy_providers.lock().insert(phandle, provider);
    }

    /// Register a PHY controller by firmware phandle.
    ///
    /// This is an alias for [`Self::register_phy_provider`] matching common device-tree
    /// terminology used by PHY controller drivers.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the PHY controller node.
    /// * `provider` - PHY provider implementation.
    pub fn register_phy_controller(&self, phandle: u32, provider: Arc<dyn PhyProvider>) {
        self.register_phy_provider(phandle, provider);
    }

    /// Look up a PHY provider by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the PHY provider node.
    ///
    /// # Returns
    ///
    /// PHY provider registered for `phandle`, or `None` when missing.
    pub fn get_phy_provider_by_phandle(&self, phandle: u32) -> Option<Arc<dyn PhyProvider>> {
        self.phy_providers.lock().get(&phandle).cloned()
    }

    /// Register a reset controller by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the reset controller node.
    /// * `controller` - Reset controller implementation.
    pub fn register_reset_controller(&self, phandle: u32, controller: Arc<dyn ResetController>) {
        self.reset_controllers.lock().insert(phandle, controller);
    }

    /// Look up a reset controller by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the reset controller node.
    ///
    /// # Returns
    ///
    /// Reset controller registered for `phandle`, or `None` when missing.
    pub fn get_reset_controller_by_phandle(
        &self,
        phandle: u32,
    ) -> Option<Arc<dyn ResetController>> {
        self.reset_controllers.lock().get(&phandle).cloned()
    }

    /// Register a remote processor by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the remote processor node.
    /// * `processor` - Remote processor implementation.
    pub fn register_remote_processor(&self, phandle: u32, processor: Arc<dyn RemoteProcessor>) {
        self.remote_processors.lock().insert(phandle, processor);
    }

    /// Look up a remote processor by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the remote processor node.
    ///
    /// # Returns
    ///
    /// Remote processor registered for `phandle`, or `None` when missing.
    pub fn get_remote_processor_by_phandle(
        &self,
        phandle: u32,
    ) -> Option<Arc<dyn RemoteProcessor>> {
        self.remote_processors.lock().get(&phandle).cloned()
    }

    /// Look up a service exposed by a registered remote processor.
    ///
    /// # Arguments
    ///
    /// * `remoteproc_phandle` - Firmware phandle identifying the remote processor node.
    /// * `service_id` - Service identifier to resolve from that processor.
    ///
    /// # Returns
    ///
    /// Remote processor service registered for `service_id`, or `None` when the
    /// processor or service is missing.
    pub fn get_remoteproc_service(
        &self,
        remoteproc_phandle: u32,
        service_id: RemoteprocServiceId,
    ) -> Option<Arc<dyn RemoteprocService>> {
        self.get_remote_processor_by_phandle(remoteproc_phandle)?
            .get_service(service_id)
    }

    /// Resolve a named clock for a platform device from FDT properties.
    ///
    /// When `clock-names` is present, the clock is looked up by name.
    /// When `clock-names` is absent, index 0 (the first clock) is used as
    /// a fallback. This matches the convention used by device trees that
    /// reference a single clock without naming it.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing raw `clocks` and optionally `clock-names` properties.
    /// * `name` - Clock name to resolve from `clock-names`.
    ///
    /// # Returns
    ///
    /// Clock handle for the named (or first) clock.
    pub fn resolve_clk(
        &self,
        device: &PlatformDeviceInfo,
        name: &str,
    ) -> Result<ClkHandle, &'static str> {
        let index = Self::clock_name_index(device, name).unwrap_or(0);
        let clocks = device.property("clocks").ok_or("clk: clocks missing")?;
        let specs = self.parse_clock_specs(clocks.value())?;
        let spec = specs.get(index).ok_or("clk: clock index out of range")?;
        self.resolve_clk_spec(spec)
    }

    /// Resolve a named DMA channel for a platform device from FDT properties.
    ///
    /// Missing DMA controllers return [`probe_defer`] so platform probing can
    /// retry once provider drivers register.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing `dmas` and `dma-names` properties.
    /// * `name` - DMA channel name to resolve from `dma-names`.
    ///
    /// # Returns
    ///
    /// DMA channel handle backed by the registered controller.
    pub fn resolve_dma_channel(
        &self,
        device: &PlatformDeviceInfo,
        name: &str,
    ) -> Result<Arc<dyn DmaChannel>, &'static str> {
        let index = Self::dma_name_index(device, name)?;
        self.resolve_dma_channel_by_index(device, index)
    }

    /// Resolve a DMA channel for a platform device by specifier index.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing the raw `dmas` property.
    /// * `index` - Zero-based DMA specifier index to resolve.
    ///
    /// # Returns
    ///
    /// DMA channel handle backed by the registered controller.
    pub fn resolve_dma_channel_by_index(
        &self,
        device: &PlatformDeviceInfo,
        index: usize,
    ) -> Result<Arc<dyn DmaChannel>, &'static str> {
        let dmas = device.property("dmas").ok_or("dma: dmas missing")?;
        let specs = self.parse_dma_specs(dmas.value())?;
        let spec = specs.get(index).ok_or("dma: index out of range")?;

        self.resolve_dma_spec(spec)
    }

    /// Apply `assigned-clock-parents` and `assigned-clock-rates` for a platform device.
    ///
    /// Missing clock providers return [`probe_defer`] so callers can retry probing after
    /// provider drivers register. Malformed properties return hard errors.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing raw assigned-clock properties.
    ///
    /// # Returns
    ///
    /// `Ok(())` when assignments are absent or successfully applied.
    pub fn apply_assigned_clocks(&self, device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        let assigned_clocks = match device.property("assigned-clocks") {
            Some(property) => property,
            None => return Ok(()),
        };

        let clock_specs = match self.parse_clock_specs(assigned_clocks.value()) {
            Ok(specs) => specs,
            Err("clk: provider not found") => return probe_defer(),
            Err(err) => return Err(err),
        };

        if let Some(parent_property) = device.property("assigned-clock-parents") {
            let parent_specs = match self.parse_assigned_parent_specs(parent_property.value()) {
                Ok(specs) => specs,
                Err("clk: provider not found") => return probe_defer(),
                Err(err) => return Err(err),
            };

            if parent_specs.len() > clock_specs.len() {
                return Err("clk: too many assigned parents");
            }

            for (index, parent_spec) in parent_specs.iter().enumerate() {
                let Some(parent_spec) = parent_spec else {
                    continue;
                };
                let parent = self.resolve_clk_spec(parent_spec)?;
                let clock_spec = &clock_specs[index];
                let provider = self
                    .get_clk_provider_by_phandle(clock_spec.phandle)
                    .ok_or(PROBE_DEFER)?;
                provider
                    .apply_assigned_parent(&clock_spec.cells, parent)
                    .map_err(Self::clk_error_to_str)?;
            }
        }

        if let Some(rate_property) = device.property("assigned-clock-rates") {
            let rates =
                Self::read_be_u32_cells(rate_property.value()).ok_or("clk: malformed rates")?;
            if rates.len() != clock_specs.len() {
                return Err("clk: malformed rates");
            }

            for (clock_spec, rate) in clock_specs.iter().zip(rates.iter()) {
                if *rate == 0 {
                    continue;
                }
                let provider = self
                    .get_clk_provider_by_phandle(clock_spec.phandle)
                    .ok_or(PROBE_DEFER)?;
                provider
                    .apply_assigned_rate(&clock_spec.cells, *rate as u64)
                    .map_err(Self::clk_error_to_str)?;
            }
        }

        Ok(())
    }

    /// Resolve and attach a platform device IOMMU from its `iommus` property.
    ///
    /// Missing controllers return [`probe_defer`] so platform probing can retry once
    /// provider drivers register their IOMMU controllers.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing the raw `iommus` property.
    /// * `config` - Domain configuration to use when an IOMMU is present.
    ///
    /// # Returns
    ///
    /// A resolved IOMMU attachment, or `None` when the device has no IOMMU property.
    pub fn resolve_platform_iommu(
        &self,
        device: &PlatformDeviceInfo,
        config: IommuDomainConfig,
    ) -> Result<Option<IommuAttachment>, &'static str> {
        let iommus = match device.property("iommus") {
            Some(property) => property,
            None => return Ok(None),
        };

        let specs = self.parse_iommu_specs(iommus.value())?;
        if specs.is_empty() {
            return Ok(None);
        }

        let first_phandle = specs[0].phandle;
        let controller = self
            .get_iommu_controller_by_phandle(first_phandle)
            .ok_or(PROBE_DEFER)?;
        let mut streams = Vec::new();

        for spec in &specs {
            if spec.phandle != first_phandle {
                return Err("iommu: multiple controllers not supported");
            }
            let iommu_spec = IommuSpec {
                controller_phandle: spec.phandle,
                cells: spec.cells.clone(),
            };
            let mut decoded_streams = controller
                .stream_ids_from_fdt(&iommu_spec)
                .map_err(Self::iommu_error_to_str)?;
            streams.append(&mut decoded_streams);
        }

        let domain = controller
            .alloc_domain(config)
            .map_err(Self::iommu_error_to_str)?;
        for stream in &streams {
            domain
                .attach_stream(*stream)
                .map_err(Self::iommu_error_to_str)?;
        }

        Ok(Some(IommuAttachment {
            controller,
            domain,
            streams,
        }))
    }

    /// Resolve a platform device DMA context from its firmware properties.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing optional IOMMU properties.
    /// * `config` - Domain configuration to use when an IOMMU is present.
    ///
    /// # Returns
    ///
    /// DMA context for the device, falling back to direct DMA when no IOMMU exists.
    pub fn resolve_platform_dma_context(
        &self,
        device: &PlatformDeviceInfo,
        config: IommuDomainConfig,
    ) -> Result<DmaContext, &'static str> {
        Ok(DmaContext {
            iommu: self.resolve_platform_iommu(device, config)?,
            direct_dma_offset: 0,
        })
    }

    /// Resolve a platform device MSI controller from its `msi-parent` property.
    ///
    /// Missing controllers return [`probe_defer`] so platform probing can retry once
    /// provider drivers register their MSI controllers.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing the optional `msi-parent` property.
    ///
    /// # Returns
    ///
    /// The resolved MSI controller, or `None` when the device has no `msi-parent`.
    pub fn resolve_msi_controller_for_platform(
        &self,
        device: &PlatformDeviceInfo,
    ) -> Result<Option<Arc<dyn MsiController>>, &'static str> {
        let msi_parent = match device.property("msi-parent") {
            Some(property) => property.as_usize().ok_or("msi: malformed msi-parent")? as u32,
            None => return Ok(None),
        };

        self.get_msi_controller_by_phandle(msi_parent)
            .map(Some)
            .ok_or(PROBE_DEFER)
    }

    /// Resolve one mailbox specifier for a platform device by index.
    ///
    /// Missing controllers return [`probe_defer`] so platform probing can retry once
    /// provider drivers register their mailbox controllers.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing the raw `mailboxes` property.
    /// * `index` - Zero-based mailbox specifier index to resolve.
    ///
    /// # Returns
    ///
    /// A mailbox specifier suitable for [`Self::request_mailbox_channel`].
    pub fn resolve_mailbox_spec(
        &self,
        device: &PlatformDeviceInfo,
        index: usize,
    ) -> Result<MailboxSpec, &'static str> {
        let mailboxes = device
            .property("mailboxes")
            .ok_or("mailbox: mailboxes missing")?;
        let specs = self.parse_mailbox_specs(mailboxes.value())?;
        let spec = specs.get(index).ok_or("mailbox: index out of range")?;
        Ok(MailboxSpec {
            controller_phandle: spec.phandle,
            cells: spec.cells.clone(),
        })
    }

    /// Resolve one named mailbox specifier for a platform device.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing `mailboxes` and `mbox-names` properties.
    /// * `name` - Mailbox name to resolve from `mbox-names`.
    ///
    /// # Returns
    ///
    /// A mailbox specifier suitable for [`Self::request_mailbox_channel`].
    pub fn resolve_mailbox_spec_by_name(
        &self,
        device: &PlatformDeviceInfo,
        name: &str,
    ) -> Result<MailboxSpec, &'static str> {
        let index = Self::mailbox_name_index(device, name)?;
        self.resolve_mailbox_spec(device, index)
    }

    /// Request a mailbox channel from the registered controller for a specifier.
    ///
    /// Missing controllers return [`probe_defer`] so callers can retry after the
    /// provider driver registers its mailbox controller.
    ///
    /// # Arguments
    ///
    /// * `spec` - Mailbox specifier returned by [`Self::resolve_mailbox_spec`].
    /// * `client` - Optional callback sink to install on the channel.
    ///
    /// # Returns
    ///
    /// A reference-counted mailbox channel on success.
    pub fn request_mailbox_channel(
        &self,
        spec: &MailboxSpec,
        client: Option<Arc<dyn MailboxClient>>,
    ) -> Result<Arc<dyn MailboxChannel>, &'static str> {
        let controller = self
            .get_mailbox_controller_by_phandle(spec.controller_phandle)
            .ok_or(PROBE_DEFER)?;
        controller
            .request_channel(spec, client)
            .map_err(Self::mailbox_error_to_str)
    }

    /// Resolve a named NVMEM cell for a platform device from FDT properties.
    ///
    /// The resolver parses `nvmem-cells` as repeated provider specifiers. Each
    /// specifier starts with a provider phandle followed by the provider's
    /// `cell_cells()` values, which default to `(offset, size)`. Because the
    /// resolved cell must own an `Arc<dyn NvmemProvider>`, this construction lives
    /// in `DeviceManager` rather than on [`NvmemProvider`].
    ///
    /// Missing providers return [`probe_defer`] so platform probing can retry once
    /// provider drivers register their NVMEM providers.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing `nvmem-cells` and `nvmem-cell-names`.
    /// * `name` - Cell name to resolve from `nvmem-cell-names`.
    ///
    /// # Returns
    ///
    /// A cell handle backed by the registered provider.
    pub fn resolve_nvmem_cell(
        &self,
        device: &PlatformDeviceInfo,
        name: &str,
    ) -> Result<NvmemCell, &'static str> {
        let index = Self::nvmem_cell_name_index(device, name)?;
        let cells = device
            .property("nvmem-cells")
            .ok_or("nvmem: nvmem-cells missing")?;
        let specs = self.parse_nvmem_specs(cells.value())?;
        let spec = specs.get(index).ok_or("nvmem: cell index out of range")?;

        self.resolve_nvmem_spec(spec, "nvmem-cell")
    }

    /// Resolve a named PHY for a platform device from FDT properties.
    ///
    /// Missing providers return [`probe_defer`] so platform probing can retry once
    /// provider drivers register their PHY providers.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing `phys` and `phy-names` properties.
    /// * `name` - PHY name to resolve from `phy-names`.
    ///
    /// # Returns
    ///
    /// PHY handle backed by the registered provider.
    pub fn resolve_phy(
        &self,
        device: &PlatformDeviceInfo,
        name: &str,
    ) -> Result<PhyHandle, &'static str> {
        let index = Self::phy_name_index(device, name)?;
        let phys = device.property("phys").ok_or("phy: phys missing")?;
        let specs = self.parse_phy_specs(phys.value())?;
        let spec = specs.get(index).ok_or("phy: index out of range")?;

        self.resolve_phy_spec(spec)
    }

    /// Resolve a PHY for a platform device by specifier index.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing the raw `phys` property.
    /// * `index` - Zero-based PHY specifier index to resolve.
    ///
    /// # Returns
    ///
    /// PHY handle backed by the registered provider.
    pub fn resolve_phy_by_index(
        &self,
        device: &PlatformDeviceInfo,
        index: usize,
    ) -> Result<PhyHandle, &'static str> {
        let phys = device.property("phys").ok_or("phy: phys missing")?;
        let specs = self.parse_phy_specs(phys.value())?;
        let spec = specs.get(index).ok_or("phy: index out of range")?;

        self.resolve_phy_spec(spec)
    }

    /// Resolve a named reset line for a platform device from FDT properties.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing `resets` and `reset-names` properties.
    /// * `name` - Reset name to resolve from `reset-names`.
    ///
    /// # Returns
    ///
    /// Reset handle backed by the registered controller.
    pub fn resolve_reset(
        &self,
        device: &PlatformDeviceInfo,
        name: &str,
    ) -> Result<ResetHandle, &'static str> {
        let index = Self::reset_name_index(device, name)?;
        self.resolve_reset_by_index(device, index)
    }

    /// Resolve a reset line for a platform device by specifier index.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing the raw `resets` property.
    /// * `index` - Zero-based reset specifier index to resolve.
    ///
    /// # Returns
    ///
    /// Reset handle backed by the registered controller.
    pub fn resolve_reset_by_index(
        &self,
        device: &PlatformDeviceInfo,
        index: usize,
    ) -> Result<ResetHandle, &'static str> {
        let resets = device.property("resets").ok_or("reset: resets missing")?;
        let specs = self.parse_reset_specs(resets.value())?;
        let spec = specs.get(index).ok_or("reset: index out of range")?;

        self.resolve_reset_spec(spec)
    }

    fn pre_probe_resolve_iommu(&self, device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        let Some(iommus) = device.property("iommus") else {
            return Ok(());
        };

        self.parse_iommu_specs(iommus.value()).map(|_| ())
    }

    fn pre_probe_resolve_dma(&self, device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        let Some(dmas) = device.property("dmas") else {
            return Ok(());
        };

        self.parse_dma_specs(dmas.value()).map(|_| ())
    }

    fn pre_probe_resolve_mailbox(&self, device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        let Some(mailboxes) = device.property("mailboxes") else {
            return Ok(());
        };

        self.parse_mailbox_specs(mailboxes.value()).map(|_| ())
    }

    fn pre_probe_resolve_nvmem(&self, device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        let Some(cells) = device.property("nvmem-cells") else {
            return Ok(());
        };

        self.parse_nvmem_specs(cells.value()).map(|_| ())
    }

    fn pre_probe_resolve_phy(&self, device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        let Some(phys) = device.property("phys") else {
            return Ok(());
        };

        self.parse_phy_specs(phys.value()).map(|_| ())
    }

    fn apply_pinctrl_default(&self, device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        let Some(pinctrl) = device.property("pinctrl-0") else {
            return Ok(());
        };

        let states = Self::read_be_u32_cells(pinctrl.value()).ok_or("pinctrl: malformed state")?;
        if states.is_empty() {
            return Ok(());
        }

        let fdt = crate::device::fdt::FdtManager::get_manager()
            .get_fdt()
            .ok_or("pinctrl: FDT unavailable")?;

        for state_phandle in states {
            let (state_node, controller_phandle) =
                Self::find_node_and_parent_by_phandle(fdt, state_phandle)
                    .ok_or("pinctrl: state node not found")?;
            let Some(pinmux) = state_node.property("pinmux") else {
                continue;
            };

            let controller_phandle = controller_phandle.ok_or("pinctrl: state has no parent")?;
            let controller = self
                .get_gpio_controller(controller_phandle)
                .ok_or(PROBE_DEFER)?;
            let muxes = Self::read_be_u32_cells(pinmux.value).ok_or("pinctrl: malformed pinmux")?;

            for mux in &muxes {
                let pin = mux & 0xffff;
                let func = ((mux >> 16) & 0xff) as u8;
                controller.set_function(pin, func);
            }

            early_println!(
                "[pinctrl] applied device={} state phandle={:#x} controller={:#x} pins={}",
                device.name(),
                state_phandle,
                controller_phandle,
                muxes.len()
            );
        }

        Ok(())
    }

    fn deassert_device_resets(&self, device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        let Some(resets) = device.property("resets") else {
            return Ok(());
        };

        let specs = self.parse_reset_specs(resets.value())?;
        for spec in specs {
            let reset = self.resolve_reset_spec(&spec)?;
            reset.deassert()?;
        }

        Ok(())
    }

    pub fn for_each_usb_host<F>(&self, mut f: F)
    where
        F: FnMut(&Arc<dyn UsbHostController>),
    {
        let hosts = self.usb_hosts.lock();
        for ctrl in hosts.values() {
            f(ctrl);
        }
    }

    /// Register a hardware watchdog timer.
    ///
    /// Watchdogs are stored as a flat list because they are typically singleton
    /// devices and are not referenced by firmware phandle from other devices.
    ///
    /// # Arguments
    ///
    /// * `watchdog` - Watchdog implementation to register.
    pub fn register_watchdog(&self, watchdog: Arc<dyn Watchdog>) {
        self.watchdogs.lock().push(watchdog);
    }

    /// Iterate over all registered watchdog timers.
    ///
    /// # Arguments
    ///
    /// * `f` - Callback invoked for each registered watchdog.
    pub fn for_each_watchdog<F>(&self, mut f: F)
    where
        F: FnMut(&Arc<dyn Watchdog>),
    {
        let watchdogs = self.watchdogs.lock();
        for watchdog in watchdogs.iter() {
            f(watchdog);
        }
    }

    /// Ping all registered watchdog timers.
    ///
    /// Failed pings are logged and do not stop the remaining watchdogs from
    /// being pinged.
    pub fn ping_all_watchdogs(&self) {
        self.for_each_watchdog(|watchdog| {
            if let Err(error) = watchdog.ping() {
                early_println!("watchdog: failed to ping {}: {:?}", watchdog.name(), error);
            }
        });
    }

    /// Populates devices from the FDT (Flattened Device Tree).
    ///
    /// This function searches for the `/soc` node in the FDT and iterates through its children.
    /// For each child node, it checks if there is a compatible driver registered.
    /// If a matching driver is found, it probes the device using the driver's `probe` method.
    /// If the probe is successful, the device is registered with the driver.
    ///
    /// # Deprecated
    /// Use `populate_devices_from_source` with `DeviceSource::Fdt` instead.
    pub fn populate_devices(&self) {
        use super::fdt::FdtManager;

        let fdt_manager = unsafe { FdtManager::get_mut_manager() };
        let fdt = fdt_manager.get_fdt();
        if fdt.is_none() {
            early_println!("FDT not initialized");
            return;
        }

        self.populate_devices_from_fdt(None);
    }

    /// Populate devices using a specific device source
    ///
    /// # Arguments
    ///
    /// * `device_source` - The source of device information (FDT, UEFI, ACPI, etc.)
    /// * `priorities` - Optional slice of priority levels to use. If None, uses all priorities in order.
    pub fn populate_devices_from_source(
        &self,
        device_source: &DeviceSource,
        priorities: Option<&[DriverPriority]>,
    ) {
        match device_source {
            DeviceSource::Fdt(_addr) => {
                early_println!("Populating devices from FDT...");
                self.populate_devices_from_fdt(priorities);
            }
            DeviceSource::Uefi => {
                early_println!("Populating devices from UEFI...");
                self.populate_devices_from_uefi(priorities);
            }
            DeviceSource::Acpi => {
                early_println!("Populating devices from ACPI...");
                self.populate_devices_from_acpi(priorities);
            }
            DeviceSource::None => {
                early_println!("No device source available - skipping device population");
            }
        }
    }

    /// Populate devices from FDT
    fn populate_devices_from_fdt(&self, priorities: Option<&[DriverPriority]>) {
        use super::fdt::FdtManager;

        let fdt_manager = unsafe { FdtManager::get_mut_manager() };
        let fdt = fdt_manager.get_fdt();
        if fdt.is_none() {
            early_println!("FDT not initialized");
            return;
        }
        let fdt = fdt.unwrap();

        let priority_list = priorities.unwrap_or(DriverPriority::all());

        // Process each priority level separately to reduce stack depth
        for &priority in priority_list {
            self.process_priority_level(fdt, priority);
            self.retry_deferred_devices(priority);
        }

        self.retry_deferred_devices(DriverPriority::Late);
    }

    /// Process devices for a single priority level - reduces stack nesting
    fn process_priority_level(&self, fdt: &fdt::Fdt, priority: DriverPriority) {
        early_println!(
            "Populating devices with {} drivers from FDT...",
            priority.description()
        );

        let mut idx = 0;

        let root_node = match fdt.find_node("/") {
            Some(node) => node,
            None => {
                early_println!("No device tree root found");
                return;
            }
        };

        for child in root_node.children() {
            let parent_ph = Self::get_u32_prop(&root_node, "phandle")
                .or_else(|| Self::get_u32_prop(&root_node, "linux,phandle"));
            self.process_device_subtree(&child, priority, &mut idx, parent_ph);
        }

        if let Some(chosen_node) = fdt.find_node("/chosen") {
            for child in chosen_node.children() {
                let parent_ph = Self::get_u32_prop(&chosen_node, "phandle")
                    .or_else(|| Self::get_u32_prop(&chosen_node, "linux,phandle"));
                self.process_device_subtree(&child, priority, &mut idx, parent_ph);
            }
        }
    }

    fn process_device_subtree(
        &self,
        node: &fdt::node::FdtNode,
        priority: DriverPriority,
        idx: &mut usize,
        parent_phandle: Option<u32>,
    ) {
        let has_explicit_phandle = Self::get_u32_prop(node, "phandle")
            .or_else(|| Self::get_u32_prop(node, "linux,phandle"))
            .is_some();

        let this_phandle = if has_explicit_phandle {
            Self::get_u32_prop(node, "phandle")
                .or_else(|| Self::get_u32_prop(node, "linux,phandle"))
                .unwrap()
        } else {
            let node_key = node.name.as_ptr() as usize;
            let mut cache = self.auto_phandle_cache.lock();
            if let Some(&cached) = cache.get(&node_key) {
                cached
            } else {
                let ph = self.next_auto_phandle.fetch_add(1, Ordering::Relaxed);
                cache.insert(node_key, ph);
                ph
            }
        };

        self.process_single_device_node(
            node,
            priority,
            idx,
            parent_phandle,
            if has_explicit_phandle {
                None
            } else {
                Some(this_phandle)
            },
        );

        for child in node.children() {
            self.process_device_subtree(&child, priority, idx, Some(this_phandle));
        }
    }

    /// Process a single device node with minimal stack usage
    fn process_single_device_node(
        &self,
        child: &fdt::node::FdtNode,
        priority: DriverPriority,
        idx: &mut usize,
        parent_phandle: Option<u32>,
        synthetic_phandle: Option<u32>,
    ) {
        if let Some(status_prop) = child.property("status")
            && let Some(status) = status_prop.as_str()
            && status == "disabled"
        {
            return;
        }

        let compatible = child.compatible();
        if compatible.is_none() {
            return;
        }

        let compatible_iter = compatible.unwrap().all();

        let has_drivers = {
            let drivers = self.drivers.lock();
            drivers.get(&priority).is_some_and(|list| !list.is_empty())
        };

        if !has_drivers {
            return;
        }

        let resources = self.build_minimal_resources(child);
        let mut properties = self.build_device_properties(child);

        if let Some(ph) = synthetic_phandle {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&ph.to_be_bytes());
            properties.insert(0, PlatformDeviceProperty::new("phandle", &bytes));
        }

        // Try to match with drivers
        let compatible_vec: alloc::vec::Vec<&str> = compatible_iter.collect();
        if let Some(device) = self.build_platform_device(
            child,
            priority,
            idx,
            compatible_vec,
            resources,
            properties,
            parent_phandle,
        ) {
            self.try_match_and_probe_device(priority, idx, device);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_platform_device(
        &self,
        child: &fdt::node::FdtNode,
        _priority: DriverPriority,
        idx: &mut usize,
        compatible: alloc::vec::Vec<&str>,
        resources: alloc::vec::Vec<PlatformDeviceResource>,
        properties: alloc::vec::Vec<PlatformDeviceProperty>,
        parent_phandle: Option<u32>,
    ) -> Option<Arc<PlatformDeviceInfo>> {
        // SAFETY: FDT data is loaded at boot and remains resident for the kernel lifetime.
        let static_name: &'static str = unsafe { core::mem::transmute(child.name) };
        let static_compatible: alloc::vec::Vec<&'static str> = compatible
            .into_iter()
            // SAFETY: Compatible strings are borrowed from the same resident FDT blob.
            .map(|s| unsafe { core::mem::transmute(s) })
            .collect();

        Some(Arc::new(PlatformDeviceInfo::new(
            static_name,
            *idx,
            static_compatible,
            resources,
            properties,
            parent_phandle,
        )))
    }

    fn build_device_properties(
        &self,
        child: &fdt::node::FdtNode,
    ) -> alloc::vec::Vec<PlatformDeviceProperty> {
        child
            .properties()
            .map(|property| PlatformDeviceProperty::new(property.name, property.value))
            .collect()
    }

    /// Build device resources with minimal stack allocation
    fn build_minimal_resources(
        &self,
        child: &fdt::node::FdtNode,
    ) -> alloc::vec::Vec<PlatformDeviceResource> {
        let mut resources = alloc::vec::Vec::new();

        // Add memory regions
        if let Some(regions) = child.reg() {
            for region in regions {
                if let Some(res) = Self::mem_resource_from_region(region) {
                    resources.push(res);
                }
            }
        }

        // Add IRQs
        let mut parsed_any_irq = false;

        if let Some(irqs) = child.interrupts() {
            // Standard path: fdt-rs successfully parsed interrupts
            for irq in irqs {
                Self::push_irq_resource(&mut resources, irq, None);
                parsed_any_irq = true;
            }
        }

        if !parsed_any_irq
            && let Some(prop) = child.property("interrupts-extended")
            && let Some(fdt) = crate::device::fdt::FdtManager::get_manager().get_fdt()
        {
            let bytes = prop.value;
            let mut offset = 0usize;

            while offset + 4 <= bytes.len() {
                let phandle = match Self::read_be_u32(&bytes[offset..offset + 4]) {
                    Some(v) => v,
                    None => break,
                };
                offset += 4;

                let intc_node = Self::find_node_by_phandle(fdt, phandle);
                let interrupt_cells = intc_node
                    .as_ref()
                    .and_then(|n| Self::get_u32_prop(n, "#interrupt-cells"))
                    .unwrap_or(1) as usize;

                if interrupt_cells == 0 {
                    break;
                }

                let needed = interrupt_cells.saturating_mul(4);
                if offset + needed > bytes.len() {
                    break;
                }

                let cell0 = Self::read_be_u32(&bytes[offset..offset + 4]).unwrap_or(0);
                let cell1 = if interrupt_cells >= 2 {
                    Self::read_be_u32(&bytes[offset + 4..offset + 8]).unwrap_or(0)
                } else {
                    0
                };
                let cell2 = if interrupt_cells >= 3 {
                    Self::read_be_u32(&bytes[offset + 8..offset + 12]).unwrap_or(0)
                } else {
                    0
                };

                let (irq_num, metadata) = match interrupt_cells {
                    3 => (
                        cell1 as usize,
                        Some(crate::device::platform::resource::IrqMetadata {
                            irq_type: cell0,
                            irq_number: cell1,
                            irq_flags: cell2,
                        }),
                    ),
                    2 => (
                        cell0 as usize,
                        Some(crate::device::platform::resource::IrqMetadata {
                            irq_type: 0,
                            irq_number: cell0,
                            irq_flags: cell1,
                        }),
                    ),
                    1 => (cell0 as usize, None),
                    _ => (cell0 as usize, None),
                };

                Self::push_irq_resource(&mut resources, irq_num, metadata);
                parsed_any_irq = true;
                offset += needed;
            }
        }

        if !parsed_any_irq && let Some(prop) = child.property("interrupts") {
            // Fallback: Parse raw interrupts property when fdt-rs fails
            // This preserves interrupt controller metadata for later translation
            let value = prop.value;

            // Detect cell format based on property length
            let cell_size = if value.len() % 12 == 0 {
                3 // 3-cell format (e.g., ARM GIC: <type, number, flags>)
            } else if value.len() % 8 == 0 {
                2 // 2-cell format
            } else if value.len() % 4 == 0 {
                1 // 1-cell format (just interrupt number)
            } else {
                return resources; // Unknown format, skip
            };

            let num_irqs = value.len() / (cell_size * 4);

            for i in 0..num_irqs {
                let offset = i * cell_size * 4;

                let (irq_num, metadata) = match cell_size {
                    3 => {
                        // 3-cell format: <type, number, flags>
                        let irq_type = u32::from_be_bytes([
                            value[offset],
                            value[offset + 1],
                            value[offset + 2],
                            value[offset + 3],
                        ]);
                        let irq_number = u32::from_be_bytes([
                            value[offset + 4],
                            value[offset + 5],
                            value[offset + 6],
                            value[offset + 7],
                        ]);
                        let irq_flags = u32::from_be_bytes([
                            value[offset + 8],
                            value[offset + 9],
                            value[offset + 10],
                            value[offset + 11],
                        ]);

                        // Store raw number, let interrupt controller translate
                        (
                            irq_number as usize,
                            Some(crate::device::platform::resource::IrqMetadata {
                                irq_type,
                                irq_number,
                                irq_flags,
                            }),
                        )
                    }
                    2 => {
                        // 2-cell format: <number, flags>
                        let irq_number = u32::from_be_bytes([
                            value[offset],
                            value[offset + 1],
                            value[offset + 2],
                            value[offset + 3],
                        ]);
                        let irq_flags = u32::from_be_bytes([
                            value[offset + 4],
                            value[offset + 5],
                            value[offset + 6],
                            value[offset + 7],
                        ]);

                        (
                            irq_number as usize,
                            Some(crate::device::platform::resource::IrqMetadata {
                                irq_type: 0, // No type in 2-cell format
                                irq_number,
                                irq_flags,
                            }),
                        )
                    }
                    1 => {
                        // 1-cell format: just interrupt number
                        let irq_number = u32::from_be_bytes([
                            value[offset],
                            value[offset + 1],
                            value[offset + 2],
                            value[offset + 3],
                        ]);

                        (irq_number as usize, None)
                    }
                    _ => unreachable!(),
                };

                Self::push_irq_resource(&mut resources, irq_num, metadata);
            }
        }

        resources
    }

    /// Try to match device with drivers and probe if successful.
    ///
    /// # Arguments
    ///
    /// * `priority` - Driver priority bucket to match against.
    /// * `idx` - Mutable device index incremented after successful probe.
    /// * `device` - Platform device information to probe.
    fn try_match_and_probe_device(
        &self,
        priority: DriverPriority,
        idx: &mut usize,
        device: Arc<PlatformDeviceInfo>,
    ) {
        match self.probe_platform_device(priority, &device) {
            ProbeOutcome::Probed => *idx += 1,
            ProbeOutcome::Deferred => self.defer_platform_device(priority, device),
            ProbeOutcome::Failed | ProbeOutcome::NoMatch => {}
        }
    }

    fn defer_platform_device(&self, priority: DriverPriority, device: Arc<PlatformDeviceInfo>) {
        early_println!(
            "[probe] deferred {} device: {}",
            priority.description(),
            device.name()
        );
        self.deferred_platform_devices
            .lock()
            .push(DeferredPlatformDevice { priority, device });
    }

    fn probe_platform_device(
        &self,
        priority: DriverPriority,
        device: &PlatformDeviceInfo,
    ) -> ProbeOutcome {
        let drivers = self.drivers.lock();
        if let Some(driver_list) = drivers.get(&priority) {
            for driver in driver_list.iter() {
                if driver
                    .match_table()
                    .iter()
                    .any(|&c| device.compatible().contains(&c))
                {
                    if let Err(e) =
                        crate::device::power::PowerManager::enable_device_domains(device)
                    {
                        if is_probe_defer(e) {
                            return ProbeOutcome::Deferred;
                        }
                        crate::early_println!(
                            "Failed to enable power domains for {} device {}: {}",
                            priority.description(),
                            device.name(),
                            e
                        );
                    }
                    if let Err(e) = self.apply_assigned_clocks(device) {
                        if is_probe_defer(e) {
                            return ProbeOutcome::Deferred;
                        }
                        early_println!("[clk] failed to apply assigned clocks: {}", e);
                        return ProbeOutcome::Failed;
                    }
                    if let Err(e) = self.deassert_device_resets(device) {
                        if is_probe_defer(e) {
                            return ProbeOutcome::Deferred;
                        }
                        early_println!("[reset] failed to deassert device resets: {}", e);
                        return ProbeOutcome::Failed;
                    }

                    if let Err(e) = self.apply_pinctrl_default(device) {
                        if is_probe_defer(e) {
                            return ProbeOutcome::Deferred;
                        }
                        early_println!("[pinctrl] failed to apply default state: {}", e);
                        return ProbeOutcome::Failed;
                    }

                    if let Err(e) = self.pre_probe_resolve_iommu(device) {
                        if is_probe_defer(e) {
                            return ProbeOutcome::Deferred;
                        }
                        early_println!("[iommu] failed to resolve IOMMU: {}", e);
                        return ProbeOutcome::Failed;
                    }

                    if let Err(e) = self.pre_probe_resolve_dma(device) {
                        if is_probe_defer(e) {
                            return ProbeOutcome::Deferred;
                        }
                        early_println!("[dma] failed to resolve DMA: {}", e);
                        return ProbeOutcome::Failed;
                    }

                    if let Err(e) = self.pre_probe_resolve_mailbox(device) {
                        if is_probe_defer(e) {
                            return ProbeOutcome::Deferred;
                        }
                        early_println!("[mailbox] failed to resolve mailbox: {}", e);
                        return ProbeOutcome::Failed;
                    }

                    if let Err(e) = self.pre_probe_resolve_nvmem(device) {
                        if is_probe_defer(e) {
                            return ProbeOutcome::Deferred;
                        }
                        early_println!("[nvmem] failed to resolve NVMEM cell: {}", e);
                        return ProbeOutcome::Failed;
                    }

                    if let Err(e) = self.pre_probe_resolve_phy(device) {
                        if is_probe_defer(e) {
                            return ProbeOutcome::Deferred;
                        }
                        early_println!("[phy] failed to resolve PHY: {}", e);
                        return ProbeOutcome::Failed;
                    }

                    match driver.probe(device) {
                        Ok(_) => {
                            early_println!(
                                "Successfully probed {} device: {}",
                                priority.description(),
                                device.name()
                            );
                            return ProbeOutcome::Probed;
                        }
                        Err(e) => {
                            if is_probe_defer(e) {
                                return ProbeOutcome::Deferred;
                            }
                            early_println!(
                                "Failed to probe {} device {}: {}",
                                priority.description(),
                                device.name(),
                                e
                            );
                            return ProbeOutcome::Failed;
                        }
                    }
                }
            }
        }

        ProbeOutcome::NoMatch
    }

    fn retry_deferred_devices(&self, max_priority: DriverPriority) {
        loop {
            let retry_batch = {
                let mut deferred = self.deferred_platform_devices.lock();
                let mut retry_batch = Vec::new();
                let mut remaining = Vec::new();

                for item in deferred.drain(..) {
                    if item.priority <= max_priority {
                        retry_batch.push(item);
                    } else {
                        remaining.push(item);
                    }
                }

                *deferred = remaining;
                retry_batch
            };

            if retry_batch.is_empty() {
                return;
            }

            let mut made_progress = false;
            let mut still_deferred = Vec::new();
            for item in retry_batch {
                early_println!(
                    "[probe] retrying deferred {} device: {}",
                    item.priority.description(),
                    item.device.name()
                );
                match self.probe_platform_device(item.priority, &item.device) {
                    ProbeOutcome::Probed => made_progress = true,
                    ProbeOutcome::Deferred => still_deferred.push(item),
                    ProbeOutcome::Failed | ProbeOutcome::NoMatch => {}
                }
            }

            if !still_deferred.is_empty() {
                self.deferred_platform_devices
                    .lock()
                    .extend(still_deferred.iter().cloned());
            }

            if !made_progress {
                return;
            }
        }
    }

    /// Populate devices from UEFI (stub implementation)
    ///
    /// # Arguments
    ///
    /// * `priorities` - Optional slice of priority levels to use. If None, uses all priorities in order.
    ///
    /// # Note
    ///
    /// This is currently a stub implementation. UEFI device discovery will be implemented
    /// when UEFI boot support is added.
    fn populate_devices_from_uefi(&self, _priorities: Option<&[DriverPriority]>) {
        early_println!("UEFI device discovery not yet implemented");
        // TODO: Implement UEFI device discovery
        // - Enumerate UEFI protocols
        // - Create PlatformDeviceInfo from UEFI device handles
        // - Probe devices with matching drivers
    }

    /// Populate devices from ACPI (stub implementation)
    ///
    /// # Arguments
    ///
    /// * `priorities` - Optional slice of priority levels to use. If None, uses all priorities in order.
    ///
    /// # Note
    ///
    /// This is currently a stub implementation. ACPI device discovery will be implemented
    /// when x86 support is added.
    fn populate_devices_from_acpi(&self, _priorities: Option<&[DriverPriority]>) {
        early_println!("ACPI device discovery not yet implemented");
        // TODO: Implement ACPI device discovery
        // - Parse ACPI tables (DSDT, etc.)
        // - Create PlatformDeviceInfo from ACPI device objects
        // - Probe devices with matching drivers
    }

    /// Populate devices using drivers of specific priority levels
    ///
    /// # Arguments
    ///
    /// * `priorities` - Optional slice of priority levels to use. If None, uses all priorities in order.
    ///
    /// # Deprecated
    /// Use `populate_devices_from_source` instead.
    pub fn populate_devices_by_priority(&self, priorities: Option<&[DriverPriority]>) {
        self.populate_devices_from_fdt(priorities);
    }

    /// Registers a device driver with the device manager.
    ///
    /// This function takes a boxed device driver and adds it to the list of registered drivers
    /// at the specified priority level.
    ///
    /// # Arguments
    ///
    /// * `driver` - A boxed device driver that implements the `DeviceDriver` trait.
    /// * `priority` - The priority level for this driver.
    ///
    /// # Example
    ///
    /// ```rust
    /// let driver = Box::new(MyDeviceDriver::new());
    /// DeviceManager::get_manager().register_driver(driver, DriverPriority::Standard);
    /// ```
    pub fn register_driver(&self, driver: Box<dyn DeviceDriver>, priority: DriverPriority) {
        let mut drivers = self.drivers.lock();
        drivers.entry(priority).or_default().push(driver);
    }

    /// Registers a device driver with default Standard priority.
    ///
    /// This is a convenience method for backward compatibility.
    ///
    /// # Arguments
    ///
    /// * `driver` - A boxed device driver that implements the `DeviceDriver` trait.
    pub fn register_driver_default(&self, driver: Box<dyn DeviceDriver>) {
        self.register_driver(driver, DriverPriority::Standard);
    }

    /// Register a discovered PCI device with the DeviceManager.
    ///
    /// PCI host controller platform drivers call this after scanning ECAM
    /// to register discovered PCI devices. The devices will be probed
    /// against registered PCI drivers when `probe_pci_devices()` is called.
    ///
    /// # Arguments
    ///
    /// * `device` - The PCI device to register.
    pub fn register_pci_device(&self, device: Arc<dyn DeviceInfo + Send + Sync>) {
        let mut discovered = self.discovered_pci_devices.lock();
        discovered.push(device);
    }

    /// Probe all discovered PCI devices against registered drivers.
    ///
    /// Must be called after all PCI devices have been registered (via
    /// `register_pci_device()`) and all drivers have been registered.
    /// Typically called from the init sequence after platform probing.
    pub fn probe_pci_devices(&self) {
        let discovered = self.discovered_pci_devices.lock();
        if discovered.is_empty() {
            return;
        }

        let count = discovered.len();
        early_println!("Probing {} discovered PCI devices...", count);

        // Hold drivers lock during probing. This is safe because PCI driver
        // probe functions do not re-enter the drivers lock.
        let drivers = self.drivers.lock();

        for priority in DriverPriority::all() {
            let driver_list = match drivers.get(priority) {
                Some(list) if !list.is_empty() => list,
                _ => continue,
            };

            let mut claimed_ids: Vec<usize> = Vec::new();

            for driver in driver_list.iter() {
                for device in discovered.iter() {
                    if claimed_ids.contains(&device.id()) {
                        continue;
                    }

                    if let Ok(()) = driver.probe(&**device) {
                        claimed_ids.push(device.id());
                        early_println!(
                            "Successfully probed PCI device {} with driver {}",
                            device.name(),
                            driver.name()
                        );
                    }
                }
            }
        }
    }

    /// Clear all devices and reset the manager state (for testing only)
    ///
    /// This method is only available in test builds and should only be used
    /// for unit testing to ensure test isolation.
    #[cfg(test)]
    pub fn clear_for_test(&self) {
        let mut devices = self.devices.lock();
        let mut device_by_name = self.device_by_name.lock();
        let mut name_to_id = self.name_to_id.lock();
        let mut discovered = self.discovered_pci_devices.lock();
        let mut deferred = self.deferred_platform_devices.lock();

        devices.clear();
        device_by_name.clear();
        name_to_id.clear();
        discovered.clear();
        deferred.clear();
        self.spi_buses.lock().clear();
        self.i2c_buses.lock().clear();
        self.usb_hosts.lock().clear();
        self.gpio_controllers.lock().clear();
        self.clk_providers.lock().clear();
        self.dma_controllers.lock().clear();
        self.iommu_controllers.lock().clear();
        self.msi_controllers.lock().clear();
        self.mailbox_controllers.lock().clear();
        self.nvmem_providers.lock().clear();
        self.phy_providers.lock().clear();
        self.reset_controllers.lock().clear();
        self.remote_processors.lock().clear();
        self.watchdogs.lock().clear();
        self.next_device_id.store(1, Ordering::SeqCst); // Start from 1, reserve 0 for invalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::clk::{ClkError, ClkFixedRate, ClkHandle, ClkProvider};
    use crate::device::dma::{
        DmaBusWidth, DmaChannel, DmaController, DmaCyclicConfig, DmaDirection, DmaError,
        DmaPeripheralConfig, DmaSpec,
    };
    use crate::device::iommu::{
        IommuDomain, IommuDomainType, IommuMapFlags, IommuStreamId, PhysAddr,
    };
    use crate::device::mailbox::{MailboxChannelId, MailboxMessage};
    use crate::device::nvmem::NvmemProvider;
    use crate::device::phy::{Phy, PhyMode};
    use crate::device::remoteproc::{
        RemoteprocCrashHandler, RemoteprocError, RemoteprocFirmware, RemoteprocMessage,
        RemoteprocState,
    };
    use crate::device::reset::ResetController;
    use crate::device::watchdog::{Watchdog, WatchdogError};
    use crate::device::{GenericDevice, platform::*};
    use crate::interrupt::msi::{
        MsiAllocation, MsiError, MsiMessage, MsiRequest, MsiRequestFlags, MsiVector,
    };
    use alloc::vec;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    static DEFER_PROBE_COUNT: AtomicUsize = AtomicUsize::new(0);
    static DEFER_PROBE_SUCCEEDED: AtomicBool = AtomicBool::new(false);
    static CLOCK_HOOK_ORDER: AtomicUsize = AtomicUsize::new(0);
    static CLOCK_HOOK_DRIVER_PROBED: AtomicBool = AtomicBool::new(false);

    fn deferred_probe_once(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        let count = DEFER_PROBE_COUNT.fetch_add(1, Ordering::SeqCst);
        if count == 0 {
            probe_defer()
        } else {
            DEFER_PROBE_SUCCEEDED.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    fn always_deferred_probe(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        DEFER_PROBE_COUNT.fetch_add(1, Ordering::SeqCst);
        probe_defer()
    }

    fn hook_order_probe(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        CLOCK_HOOK_DRIVER_PROBED.store(
            CLOCK_HOOK_ORDER.load(Ordering::SeqCst) == 1,
            Ordering::SeqCst,
        );
        CLOCK_HOOK_ORDER.store(2, Ordering::SeqCst);
        Ok(())
    }

    fn test_platform_device() -> Arc<PlatformDeviceInfo> {
        Arc::new(PlatformDeviceInfo::new(
            "test-device",
            0,
            vec!["test,defer"],
            vec![],
            vec![],
            None,
        ))
    }

    #[test_case]
    fn test_probe_defer_constant() {
        assert_eq!(PROBE_DEFER, "probe: deferred");
        assert_eq!(probe_defer::<()>().unwrap_err(), PROBE_DEFER);
    }

    #[test_case]
    fn test_is_probe_defer_recognizes_string() {
        assert!(is_probe_defer(PROBE_DEFER));
        assert!(!is_probe_defer("other error"));
    }

    #[test_case]
    fn test_deferred_device_retried_after_provider_registers() {
        DEFER_PROBE_COUNT.store(0, Ordering::SeqCst);
        DEFER_PROBE_SUCCEEDED.store(false, Ordering::SeqCst);

        let manager = DeviceManager::new();
        let driver = Box::new(PlatformDeviceDriver::new(
            "defer-driver",
            deferred_probe_once,
            |_device| Ok(()),
            vec!["test,defer"],
        ));
        manager.register_driver(driver, DriverPriority::Core);

        let mut idx = 0;
        manager.try_match_and_probe_device(DriverPriority::Core, &mut idx, test_platform_device());
        assert_eq!(DEFER_PROBE_COUNT.load(Ordering::SeqCst), 1);
        assert_eq!(idx, 0);

        manager.retry_deferred_devices(DriverPriority::Core);
        assert_eq!(DEFER_PROBE_COUNT.load(Ordering::SeqCst), 2);
        assert!(DEFER_PROBE_SUCCEEDED.load(Ordering::SeqCst));
    }

    #[test_case]
    fn test_deferred_device_stops_retrying_after_max_passes() {
        DEFER_PROBE_COUNT.store(0, Ordering::SeqCst);

        let manager = DeviceManager::new();
        let driver = Box::new(PlatformDeviceDriver::new(
            "always-defer-driver",
            always_deferred_probe,
            |_device| Ok(()),
            vec!["test,defer"],
        ));
        manager.register_driver(driver, DriverPriority::Core);

        let mut idx = 0;
        manager.try_match_and_probe_device(DriverPriority::Core, &mut idx, test_platform_device());
        manager.retry_deferred_devices(DriverPriority::Core);
        assert_eq!(DEFER_PROBE_COUNT.load(Ordering::SeqCst), 2);
        assert_eq!(manager.deferred_platform_devices.lock().len(), 1);
    }

    struct TestClkProvider {
        clk: ClkHandle,
        clock_cells: usize,
    }

    struct TestDmaChannel {
        prepared: AtomicBool,
        running: AtomicBool,
    }

    struct TestDmaController {
        channel: Arc<TestDmaChannel>,
        dma_cells: usize,
    }

    struct TestWatchdog {
        ping_count: AtomicUsize,
    }

    impl TestWatchdog {
        fn new() -> Self {
            Self {
                ping_count: AtomicUsize::new(0),
            }
        }
    }

    impl Watchdog for TestWatchdog {
        fn name(&self) -> &'static str {
            "test-watchdog"
        }

        fn start(&self, timeout_ms: u32) -> Result<(), WatchdogError> {
            let _ = timeout_ms;
            Ok(())
        }

        fn stop(&self) -> Result<(), WatchdogError> {
            Ok(())
        }

        fn ping(&self) -> Result<(), WatchdogError> {
            self.ping_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn is_running(&self) -> bool {
            true
        }

        fn set_timeout(&self, timeout_ms: u32) -> Result<u32, WatchdogError> {
            Ok(timeout_ms)
        }

        fn get_timeout(&self) -> Option<u32> {
            Some(1000)
        }
    }

    impl TestClkProvider {
        fn new(rate: u64, clock_cells: usize) -> Self {
            Self {
                clk: ClkHandle::new(Arc::new(ClkFixedRate::new("test-clock", rate))),
                clock_cells,
            }
        }
    }

    impl TestDmaChannel {
        fn new() -> Self {
            Self {
                prepared: AtomicBool::new(false),
                running: AtomicBool::new(false),
            }
        }
    }

    impl DmaChannel for TestDmaChannel {
        fn name(&self) -> &'static str {
            "test-dma-channel"
        }

        fn prepare_cyclic(&self, config: DmaCyclicConfig) -> Result<(), DmaError> {
            config.validate()?;
            self.prepared.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn start(&self) -> Result<(), DmaError> {
            if !self.prepared.load(Ordering::SeqCst) {
                return Err(DmaError::NotPrepared);
            }
            self.running.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn stop(&self) -> Result<(), DmaError> {
            self.running.store(false, Ordering::SeqCst);
            Ok(())
        }

        fn is_running(&self) -> bool {
            self.running.load(Ordering::SeqCst)
        }
    }

    impl TestDmaController {
        fn new(dma_cells: usize) -> Self {
            Self {
                channel: Arc::new(TestDmaChannel::new()),
                dma_cells,
            }
        }
    }

    impl DmaController for TestDmaController {
        fn name(&self) -> &'static str {
            "test-dma"
        }

        fn dma_cells(&self) -> usize {
            self.dma_cells
        }

        fn request_channel(&self, spec: &DmaSpec) -> Result<Arc<dyn DmaChannel>, DmaError> {
            if spec.cells.len() != self.dma_cells {
                return Err(DmaError::InvalidSpec);
            }
            Ok(self.channel.clone())
        }
    }

    impl ClkProvider for TestClkProvider {
        fn name(&self) -> &'static str {
            "test-provider"
        }

        fn clock_cells(&self) -> usize {
            self.clock_cells
        }

        fn get_clk(&self, spec: &[u32]) -> Result<ClkHandle, ClkError> {
            if spec.len() == self.clock_cells {
                Ok(self.clk.clone())
            } else {
                Err(ClkError::InvalidSpecifier)
            }
        }
    }

    #[test_case]
    fn test_register_get_clk_provider() {
        let manager = DeviceManager::new();
        manager.register_clk_provider(0x10, Arc::new(TestClkProvider::new(24_000_000, 0)));
        let provider = manager.get_clk_provider_by_phandle(0x10);
        assert!(provider.is_some());
        assert_eq!(provider.unwrap().name(), "test-provider");
    }

    #[test_case]
    fn test_register_get_dma_controller() {
        let manager = DeviceManager::new();
        manager.register_dma_controller(0x30, Arc::new(TestDmaController::new(1)));
        let controller = manager.get_dma_controller_by_phandle(0x30);
        assert!(controller.is_some());
        assert_eq!(controller.unwrap().name(), "test-dma");
    }

    #[test_case]
    fn test_register_get_iommu_controller() {
        let manager = DeviceManager::new();
        manager.register_iommu_controller(0x40, Arc::new(TestIommuController::new()));
        let controller = manager.get_iommu_controller_by_phandle(0x40);
        assert!(controller.is_some());
        assert_eq!(controller.unwrap().name(), "test-iommu");
    }

    #[test_case]
    fn test_register_get_msi_controller() {
        let manager = DeviceManager::new();
        manager.register_msi_controller(0x50, Arc::new(TestMsiController));
        let controller = manager.get_msi_controller_by_phandle(0x50);
        assert!(controller.is_some());
        assert_eq!(controller.unwrap().name(), "test-msi");
    }

    #[test_case]
    fn test_register_watchdog_and_ping_all() {
        let manager = DeviceManager::new();
        let watchdog = Arc::new(TestWatchdog::new());
        manager.register_watchdog(watchdog.clone());

        let mut count = 0;
        manager.for_each_watchdog(|registered| {
            assert_eq!(registered.name(), "test-watchdog");
            count += 1;
        });
        assert_eq!(count, 1);

        manager.ping_all_watchdogs();
        assert_eq!(watchdog.ping_count.load(Ordering::SeqCst), 1);
    }

    #[test_case]
    fn test_resolve_msi_controller_returns_none_when_no_property() {
        let manager = DeviceManager::new();
        let device = msi_test_device(vec![]);
        let controller = manager
            .resolve_msi_controller_for_platform(&device)
            .unwrap();
        assert!(controller.is_none());
    }

    #[test_case]
    fn test_resolve_msi_controller_defers_when_missing() {
        let manager = DeviceManager::new();
        let device = msi_test_device(vec![PlatformDeviceProperty::new(
            "msi-parent",
            &be_cells(&[0x50]),
        )]);

        match manager.resolve_msi_controller_for_platform(&device) {
            Ok(_) => panic!("MSI controller resolved without provider"),
            Err(err) => assert_eq!(err, PROBE_DEFER),
        }
    }

    #[test_case]
    fn test_resolve_platform_iommu_returns_none_when_no_iommus_property() {
        let manager = DeviceManager::new();
        let device = iommu_test_device(vec![]);
        let attachment = manager
            .resolve_platform_iommu(&device, test_iommu_config())
            .unwrap();
        assert!(attachment.is_none());
    }

    #[test_case]
    fn test_resolve_platform_iommu_defers_when_controller_missing() {
        let manager = DeviceManager::new();
        let device = iommu_test_device(vec![PlatformDeviceProperty::new(
            "iommus",
            &be_cells(&[0x40, 0x10]),
        )]);
        match manager.resolve_platform_iommu(&device, test_iommu_config()) {
            Ok(_) => panic!("IOMMU resolved without controller"),
            Err(err) => assert_eq!(err, PROBE_DEFER),
        }
    }

    #[test_case]
    fn test_resolve_platform_iommu_attaches_all_streams() {
        let manager = DeviceManager::new();
        let controller = Arc::new(TestIommuController::new());
        manager.register_iommu_controller(0x40, controller.clone());
        let device = iommu_test_device(vec![PlatformDeviceProperty::new(
            "iommus",
            &be_cells(&[0x40, 0x10, 0x40, 0x20]),
        )]);

        let attachment = manager
            .resolve_platform_iommu(&device, test_iommu_config())
            .unwrap()
            .expect("expected IOMMU attachment");

        assert_eq!(controller.alloc_count.load(Ordering::SeqCst), 1);
        assert_eq!(attachment.streams.len(), 2);
        assert_eq!(
            *controller.domain.attached_streams.lock(),
            vec![
                IommuStreamId {
                    id: 0x10,
                    substream_id: None,
                },
                IommuStreamId {
                    id: 0x20,
                    substream_id: None,
                },
            ]
        );
    }

    #[test_case]
    fn test_resolve_platform_dma_context_direct_when_no_iommus() {
        let manager = DeviceManager::new();
        let device = iommu_test_device(vec![]);
        let context = manager
            .resolve_platform_dma_context(&device, test_iommu_config())
            .unwrap();
        assert!(context.iommu.is_none());
        assert_eq!(context.direct_dma_offset, 0);
    }

    #[test_case]
    fn test_resolve_dma_channel_defers_when_controller_missing() {
        let manager = DeviceManager::new();
        let device = dma_test_device(vec![PlatformDeviceProperty::new(
            "dmas",
            &be_cells(&[0x30, 7]),
        )]);

        match manager.resolve_dma_channel_by_index(&device, 0) {
            Ok(_) => panic!("DMA channel resolved without controller"),
            Err(err) => assert_eq!(err, PROBE_DEFER),
        }
    }

    #[test_case]
    fn test_resolve_dma_channel_by_name() {
        let manager = DeviceManager::new();
        manager.register_dma_controller(0x30, Arc::new(TestDmaController::new(1)));
        let device = dma_test_device(vec![
            PlatformDeviceProperty::new("dmas", &be_cells(&[0x30, 7])),
            PlatformDeviceProperty::new("dma-names", b"tx0a\0"),
        ]);

        let channel = manager.resolve_dma_channel(&device, "tx0a").unwrap();
        let config = DmaCyclicConfig {
            buffer_addr: 0x1000,
            buffer_len: 0x1000,
            period_len: 0x400,
            direction: DmaDirection::MemToDev,
            peripheral: Some(DmaPeripheralConfig {
                addr: 0x2000,
                width: DmaBusWidth::Width4,
                burst_len: 4,
            }),
        };

        assert!(channel.prepare_cyclic(config).is_ok());
        assert!(channel.start().is_ok());
        assert!(channel.is_running());
        assert!(channel.stop().is_ok());
        assert!(!channel.is_running());
    }

    #[test_case]
    fn test_parse_dma_specs_rejects_truncated_spec() {
        let manager = DeviceManager::new();
        manager.register_dma_controller(0x30, Arc::new(TestDmaController::new(1)));

        match manager.parse_dma_specs(&be_cells(&[0x30])) {
            Ok(_) => panic!("truncated DMA specifier unexpectedly parsed"),
            Err(err) => assert_eq!(err, "dma: truncated specifier"),
        }
    }

    #[test_case]
    fn test_get_missing_clk_provider_returns_none() {
        let manager = DeviceManager::new();
        assert!(manager.get_clk_provider_by_phandle(0x20).is_none());
    }

    #[test_case]
    fn test_clear_for_test_clears_clk_providers() {
        let manager = DeviceManager::new();
        manager.register_clk_provider(0x10, Arc::new(TestClkProvider::new(24_000_000, 0)));
        assert!(manager.get_clk_provider_by_phandle(0x10).is_some());
        manager.clear_for_test();
        assert!(manager.get_clk_provider_by_phandle(0x10).is_none());
    }

    #[test_case]
    fn test_clear_for_test_clears_dma_controllers() {
        let manager = DeviceManager::new();
        manager.register_dma_controller(0x30, Arc::new(TestDmaController::new(1)));
        assert!(manager.get_dma_controller_by_phandle(0x30).is_some());
        manager.clear_for_test();
        assert!(manager.get_dma_controller_by_phandle(0x30).is_none());
    }

    fn be_cells(cells: &[u32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for cell in cells {
            bytes.extend_from_slice(&cell.to_be_bytes());
        }
        bytes
    }

    fn clk_test_device(properties: Vec<PlatformDeviceProperty>) -> PlatformDeviceInfo {
        PlatformDeviceInfo::new(
            "clk-device",
            0,
            vec!["test,clk-device"],
            vec![],
            properties,
            None,
        )
    }

    fn iommu_test_device(properties: Vec<PlatformDeviceProperty>) -> PlatformDeviceInfo {
        PlatformDeviceInfo::new(
            "iommu-device",
            0,
            vec!["test,iommu-device"],
            vec![],
            properties,
            None,
        )
    }

    fn msi_test_device(properties: Vec<PlatformDeviceProperty>) -> PlatformDeviceInfo {
        PlatformDeviceInfo::new(
            "msi-device",
            0,
            vec!["test,msi-device"],
            vec![],
            properties,
            None,
        )
    }

    fn dma_test_device(properties: Vec<PlatformDeviceProperty>) -> PlatformDeviceInfo {
        PlatformDeviceInfo::new(
            "dma-device",
            0,
            vec!["test,dma-device"],
            vec![],
            properties,
            None,
        )
    }

    struct TestMsiController;

    impl MsiController for TestMsiController {
        fn name(&self) -> &'static str {
            "test-msi"
        }

        fn allocate_vectors(&self, request: MsiRequest) -> Result<MsiAllocation, MsiError> {
            let _ = request.flags.contains(MsiRequestFlags::MSI_X);
            Ok(MsiAllocation {
                vectors: vec![MsiVector {
                    virq: 32,
                    hwirq: 64,
                    message: MsiMessage {
                        address: 0xfee0_0000,
                        data: 1,
                    },
                }],
            })
        }

        fn free_vectors(&self, allocation: &MsiAllocation) {
            let _ = allocation;
        }

        fn mask_vector(&self, vector: &MsiVector) -> Result<(), MsiError> {
            let _ = vector;
            Ok(())
        }

        fn unmask_vector(&self, vector: &MsiVector) -> Result<(), MsiError> {
            let _ = vector;
            Ok(())
        }
    }

    struct TestIommuDomain {
        attached_streams: Mutex<Vec<IommuStreamId>>,
    }

    impl TestIommuDomain {
        fn new() -> Self {
            Self {
                attached_streams: Mutex::new(Vec::new()),
            }
        }
    }

    impl IommuDomain for TestIommuDomain {
        fn attach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
            self.attached_streams.lock().push(stream);
            Ok(())
        }

        fn detach_stream(&self, stream: IommuStreamId) -> Result<(), IommuError> {
            let _ = stream;
            Ok(())
        }

        fn map(
            &self,
            iova: crate::device::iommu::Iova,
            paddr: PhysAddr,
            len: usize,
            flags: IommuMapFlags,
        ) -> Result<(), IommuError> {
            let _ = (iova, paddr, len, flags);
            Ok(())
        }

        fn unmap(&self, iova: crate::device::iommu::Iova, len: usize) -> Result<(), IommuError> {
            let _ = (iova, len);
            Ok(())
        }

        fn iova_to_phys(&self, iova: crate::device::iommu::Iova) -> Option<PhysAddr> {
            let _ = iova;
            None
        }

        fn flush(&self) -> Result<(), IommuError> {
            Ok(())
        }
    }

    struct TestIommuController {
        domain: Arc<TestIommuDomain>,
        alloc_count: AtomicUsize,
    }

    impl TestIommuController {
        fn new() -> Self {
            Self {
                domain: Arc::new(TestIommuDomain::new()),
                alloc_count: AtomicUsize::new(0),
            }
        }
    }

    struct TestMailboxChannel {
        id: MailboxChannelId,
        last_message: Mutex<Option<MailboxMessage>>,
        client_set: AtomicBool,
    }

    impl TestMailboxChannel {
        fn new(id: MailboxChannelId) -> Self {
            Self {
                id,
                last_message: Mutex::new(None),
                client_set: AtomicBool::new(false),
            }
        }
    }

    impl MailboxChannel for TestMailboxChannel {
        fn id(&self) -> MailboxChannelId {
            self.id
        }

        fn try_send(&self, message: &MailboxMessage) -> Result<(), MailboxError> {
            *self.last_message.lock() = Some(*message);
            Ok(())
        }

        fn try_recv(&self) -> Result<Option<MailboxMessage>, MailboxError> {
            Ok(self.last_message.lock().take())
        }

        fn send_timeout(
            &self,
            message: &MailboxMessage,
            timeout_us: u64,
        ) -> Result<(), MailboxError> {
            let _ = timeout_us;
            self.try_send(message)
        }

        fn set_client(&self, client: Option<Arc<dyn MailboxClient>>) -> Result<(), MailboxError> {
            self.client_set.store(client.is_some(), Ordering::SeqCst);
            Ok(())
        }

        fn poll(&self) -> Result<(), MailboxError> {
            Ok(())
        }
    }

    struct TestMailboxController {
        requested: AtomicUsize,
        released: AtomicUsize,
    }

    impl TestMailboxController {
        fn new() -> Self {
            Self {
                requested: AtomicUsize::new(0),
                released: AtomicUsize::new(0),
            }
        }
    }

    impl MailboxController for TestMailboxController {
        fn name(&self) -> &'static str {
            "test-mailbox"
        }

        fn request_channel(
            &self,
            spec: &MailboxSpec,
            client: Option<Arc<dyn MailboxClient>>,
        ) -> Result<Arc<dyn MailboxChannel>, MailboxError> {
            if spec.cells.is_empty() {
                return Err(MailboxError::InvalidChannel);
            }
            self.requested.fetch_add(1, Ordering::SeqCst);
            let channel = Arc::new(TestMailboxChannel::new(MailboxChannelId(spec.cells[0])));
            channel.set_client(client)?;
            Ok(channel)
        }

        fn release_channel(&self, channel: MailboxChannelId) {
            let _ = channel;
            self.released.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct TestNvmemProvider {
        data: Mutex<Vec<u8>>,
        cell_cells: usize,
    }

    struct TestPhy {
        mode: Mutex<Option<PhyMode>>,
        power_on_count: AtomicUsize,
    }

    impl TestPhy {
        fn new() -> Self {
            Self {
                mode: Mutex::new(None),
                power_on_count: AtomicUsize::new(0),
            }
        }
    }

    impl Phy for TestPhy {
        fn name(&self) -> &'static str {
            "test-phy"
        }

        fn power_on(&self) -> Result<(), PhyError> {
            self.power_on_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn power_off(&self) -> Result<(), PhyError> {
            Ok(())
        }

        fn reset(&self) -> Result<(), PhyError> {
            Ok(())
        }

        fn set_mode(&self, mode: PhyMode) -> Result<(), PhyError> {
            *self.mode.lock() = Some(mode);
            Ok(())
        }

        fn get_mode(&self) -> Option<PhyMode> {
            *self.mode.lock()
        }
    }

    struct TestPhyProvider {
        phy: Arc<TestPhy>,
        phy_cells: usize,
    }

    impl TestPhyProvider {
        fn new(phy_cells: usize) -> Self {
            Self {
                phy: Arc::new(TestPhy::new()),
                phy_cells,
            }
        }
    }

    impl PhyProvider for TestPhyProvider {
        fn name(&self) -> &'static str {
            "test-phy-provider"
        }

        fn phy_cells(&self) -> usize {
            self.phy_cells
        }

        fn get_phy(&self, spec: &[u32]) -> Result<PhyHandle, PhyError> {
            if spec.len() == self.phy_cells {
                Ok(PhyHandle::new(self.phy.clone()))
            } else {
                Err(PhyError::NotFound)
            }
        }
    }

    struct TestResetController {
        reset_cells: usize,
        assert_count: AtomicUsize,
        deassert_count: AtomicUsize,
    }

    impl TestResetController {
        fn new(reset_cells: usize) -> Self {
            Self {
                reset_cells,
                assert_count: AtomicUsize::new(0),
                deassert_count: AtomicUsize::new(0),
            }
        }
    }

    impl ResetController for TestResetController {
        fn name(&self) -> &'static str {
            "test-reset"
        }

        fn reset_cells(&self) -> usize {
            self.reset_cells
        }

        fn assert_reset(&self, spec: &[u32]) -> Result<(), &'static str> {
            if spec.len() != self.reset_cells {
                return Err("reset: invalid specifier");
            }
            self.assert_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn deassert_reset(&self, spec: &[u32]) -> Result<(), &'static str> {
            if spec.len() != self.reset_cells {
                return Err("reset: invalid specifier");
            }
            self.deassert_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    impl TestNvmemProvider {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data: Mutex::new(data),
                cell_cells: 2,
            }
        }
    }

    impl NvmemProvider for TestNvmemProvider {
        fn name(&self) -> &'static str {
            "test-nvmem"
        }

        fn size(&self) -> usize {
            self.data.lock().len()
        }

        fn read(&self, offset: usize, buf: &mut [u8]) -> Result<(), NvmemError> {
            let end = offset
                .checked_add(buf.len())
                .ok_or(NvmemError::OutOfRange)?;
            let data = self.data.lock();
            if end > data.len() {
                return Err(NvmemError::OutOfRange);
            }

            buf.copy_from_slice(&data[offset..end]);
            Ok(())
        }

        fn write(&self, offset: usize, buf: &[u8]) -> Result<(), NvmemError> {
            let end = offset
                .checked_add(buf.len())
                .ok_or(NvmemError::OutOfRange)?;
            let mut data = self.data.lock();
            if end > data.len() {
                return Err(NvmemError::OutOfRange);
            }

            data[offset..end].copy_from_slice(buf);
            Ok(())
        }

        fn cell_cells(&self) -> usize {
            self.cell_cells
        }
    }

    struct TestRemoteprocService {
        id: RemoteprocServiceId,
    }

    impl TestRemoteprocService {
        fn new(id: RemoteprocServiceId) -> Self {
            Self { id }
        }
    }

    impl RemoteprocService for TestRemoteprocService {
        fn id(&self) -> RemoteprocServiceId {
            self.id
        }

        fn name(&self) -> &'static str {
            "test-remoteproc-service"
        }

        fn send(&self, message: &RemoteprocMessage) -> Result<(), RemoteprocError> {
            let _ = message;
            Ok(())
        }

        fn try_recv(&self) -> Result<Option<RemoteprocMessage>, RemoteprocError> {
            Ok(None)
        }

        fn set_client(
            &self,
            client: Option<Arc<dyn crate::device::remoteproc::RemoteprocServiceClient>>,
        ) -> Result<(), RemoteprocError> {
            let _ = client;
            Ok(())
        }
    }

    struct TestRemoteProcessor {
        service: Option<Arc<dyn RemoteprocService>>,
    }

    impl TestRemoteProcessor {
        fn new(service: Option<Arc<dyn RemoteprocService>>) -> Self {
            Self { service }
        }
    }

    impl RemoteProcessor for TestRemoteProcessor {
        fn name(&self) -> &'static str {
            "test-remoteproc"
        }

        fn state(&self) -> RemoteprocState {
            RemoteprocState::Offline
        }

        fn load(&self, firmware: &RemoteprocFirmware) -> Result<(), RemoteprocError> {
            let _ = firmware;
            Ok(())
        }

        fn boot(&self) -> Result<(), RemoteprocError> {
            Ok(())
        }

        fn shutdown(&self) -> Result<(), RemoteprocError> {
            Ok(())
        }

        fn suspend(&self) -> Result<(), RemoteprocError> {
            Ok(())
        }

        fn resume(&self) -> Result<(), RemoteprocError> {
            Ok(())
        }

        fn register_crash_handler(
            &self,
            handler: Arc<dyn RemoteprocCrashHandler>,
        ) -> Result<(), RemoteprocError> {
            let _ = handler;
            Ok(())
        }

        fn get_service(&self, id: RemoteprocServiceId) -> Option<Arc<dyn RemoteprocService>> {
            let service = self.service.as_ref()?;
            if service.id() == id {
                Some(service.clone())
            } else {
                None
            }
        }
    }

    impl IommuController for TestIommuController {
        fn name(&self) -> &'static str {
            "test-iommu"
        }

        fn alloc_domain(
            &self,
            config: IommuDomainConfig,
        ) -> Result<Arc<dyn IommuDomain>, IommuError> {
            assert_eq!(config.domain_type, IommuDomainType::Dma);
            self.alloc_count.fetch_add(1, Ordering::SeqCst);
            Ok(self.domain.clone())
        }

        fn stream_ids_from_fdt(&self, spec: &IommuSpec) -> Result<Vec<IommuStreamId>, IommuError> {
            if spec.cells.len() != 1 {
                return Err(IommuError::InvalidSpec);
            }
            Ok(vec![IommuStreamId {
                id: spec.cells[0],
                substream_id: None,
            }])
        }
    }

    fn test_iommu_config() -> IommuDomainConfig {
        IommuDomainConfig {
            domain_type: IommuDomainType::Dma,
            iova_base: 0,
            iova_size: 0x1_0000,
        }
    }

    #[test_case]
    fn test_register_get_mailbox_controller() {
        let manager = DeviceManager::new();
        let controller = Arc::new(TestMailboxController::new());
        manager.register_mailbox_controller(0x50, controller.clone());

        let resolved = manager.get_mailbox_controller_by_phandle(0x50).unwrap();
        assert_eq!(resolved.name(), "test-mailbox");
        assert!(manager.get_mailbox_controller_by_phandle(0x51).is_none());
    }

    #[test_case]
    fn test_register_get_nvmem_provider() {
        let manager = DeviceManager::new();
        manager.register_nvmem_provider(0x70, Arc::new(TestNvmemProvider::new(vec![1, 2, 3, 4])));

        let resolved = manager.get_nvmem_provider_by_phandle(0x70).unwrap();
        assert_eq!(resolved.name(), "test-nvmem");
        assert!(manager.get_nvmem_provider_by_phandle(0x71).is_none());
    }

    #[test_case]
    fn test_register_get_phy_provider() {
        let manager = DeviceManager::new();
        manager.register_phy_provider(0x80, Arc::new(TestPhyProvider::new(1)));

        let resolved = manager.get_phy_provider_by_phandle(0x80).unwrap();
        assert_eq!(resolved.name(), "test-phy-provider");
        assert!(manager.get_phy_provider_by_phandle(0x81).is_none());
    }

    #[test_case]
    fn test_register_get_remote_processor() {
        let manager = DeviceManager::new();
        manager.register_remote_processor(0x60, Arc::new(TestRemoteProcessor::new(None)));

        let resolved = manager.get_remote_processor_by_phandle(0x60).unwrap();
        assert_eq!(resolved.name(), "test-remoteproc");
        assert!(manager.get_remote_processor_by_phandle(0x61).is_none());
    }

    #[test_case]
    fn test_get_remoteproc_service_returns_none_when_processor_missing() {
        let manager = DeviceManager::new();
        assert!(
            manager
                .get_remoteproc_service(0x60, RemoteprocServiceId(1))
                .is_none()
        );
    }

    #[test_case]
    fn test_get_remoteproc_service_returns_none_when_service_missing() {
        let manager = DeviceManager::new();
        manager.register_remote_processor(0x60, Arc::new(TestRemoteProcessor::new(None)));

        assert!(
            manager
                .get_remoteproc_service(0x60, RemoteprocServiceId(1))
                .is_none()
        );
    }

    #[test_case]
    fn test_get_remoteproc_service_returns_registered_service() {
        let manager = DeviceManager::new();
        let service = Arc::new(TestRemoteprocService::new(RemoteprocServiceId(1)));
        manager.register_remote_processor(0x60, Arc::new(TestRemoteProcessor::new(Some(service))));

        let resolved = manager
            .get_remoteproc_service(0x60, RemoteprocServiceId(1))
            .expect("registered remoteproc service missing");
        assert_eq!(resolved.name(), "test-remoteproc-service");
        assert_eq!(resolved.id(), RemoteprocServiceId(1));
    }

    #[test_case]
    fn test_resolve_mailbox_spec_defers_when_controller_missing() {
        let manager = DeviceManager::new();
        let device = clk_test_device(vec![PlatformDeviceProperty::new(
            "mailboxes",
            &be_cells(&[0x50, 7]),
        )]);

        match manager.resolve_mailbox_spec(&device, 0) {
            Ok(_) => panic!("mailbox spec resolved without controller"),
            Err(err) => assert_eq!(err, PROBE_DEFER),
        }
    }

    #[test_case]
    fn test_resolve_mailbox_spec_parses_cells() {
        let manager = DeviceManager::new();
        manager.register_mailbox_controller(0x50, Arc::new(TestMailboxController::new()));
        let device = clk_test_device(vec![PlatformDeviceProperty::new(
            "mailboxes",
            &be_cells(&[0x50, 7]),
        )]);

        let spec = manager.resolve_mailbox_spec(&device, 0).unwrap();
        assert_eq!(spec.controller_phandle, 0x50);
        assert_eq!(spec.cells, vec![7]);

        let channel = manager.request_mailbox_channel(&spec, None).unwrap();
        assert_eq!(channel.id(), MailboxChannelId(7));
    }

    #[test_case]
    fn test_resolve_nvmem_cell_defers_when_provider_missing() {
        let manager = DeviceManager::new();
        let device = clk_test_device(vec![
            PlatformDeviceProperty::new("nvmem-cells", &be_cells(&[0x70, 1, 2])),
            PlatformDeviceProperty::new("nvmem-cell-names", b"serial-number\0"),
        ]);

        match manager.resolve_nvmem_cell(&device, "serial-number") {
            Ok(_) => panic!("NVMEM cell resolved without provider"),
            Err(err) => assert_eq!(err, PROBE_DEFER),
        }
    }

    #[test_case]
    fn test_resolve_nvmem_cell_parses_offset_and_size() {
        let manager = DeviceManager::new();
        manager.register_nvmem_provider(0x70, Arc::new(TestNvmemProvider::new(vec![1, 2, 3, 4])));
        let device = clk_test_device(vec![
            PlatformDeviceProperty::new("nvmem-cells", &be_cells(&[0x70, 1, 2])),
            PlatformDeviceProperty::new("nvmem-cell-names", b"serial-number\0"),
        ]);

        let cell = manager
            .resolve_nvmem_cell(&device, "serial-number")
            .unwrap();
        let mut buf = [0u8; 2];
        cell.read(&mut buf).unwrap();

        assert_eq!(cell.name(), "nvmem-cell");
        assert_eq!(cell.size(), 2);
        assert_eq!(buf, [2, 3]);
    }

    #[test_case]
    fn test_resolve_nvmem_cell_rejects_out_of_range() {
        let manager = DeviceManager::new();
        manager.register_nvmem_provider(0x70, Arc::new(TestNvmemProvider::new(vec![1, 2, 3, 4])));
        let device = clk_test_device(vec![
            PlatformDeviceProperty::new("nvmem-cells", &be_cells(&[0x70, 3, 2])),
            PlatformDeviceProperty::new("nvmem-cell-names", b"serial-number\0"),
        ]);

        match manager.resolve_nvmem_cell(&device, "serial-number") {
            Ok(_) => panic!("out-of-range NVMEM cell resolved"),
            Err(err) => assert_eq!(err, "nvmem: out of range"),
        }
    }

    #[test_case]
    fn test_resolve_phy_defers_when_provider_missing() {
        let manager = DeviceManager::new();
        let device = clk_test_device(vec![
            PlatformDeviceProperty::new("phys", &be_cells(&[0x80, 1])),
            PlatformDeviceProperty::new("phy-names", b"usb3\0"),
        ]);

        match manager.resolve_phy(&device, "usb3") {
            Ok(_) => panic!("PHY resolved without provider"),
            Err(err) => assert_eq!(err, PROBE_DEFER),
        }
    }

    #[test_case]
    fn test_resolve_phy_by_name_and_index() {
        let manager = DeviceManager::new();
        manager.register_phy_provider(0x80, Arc::new(TestPhyProvider::new(1)));
        let device = clk_test_device(vec![
            PlatformDeviceProperty::new("phys", &be_cells(&[0x80, 1])),
            PlatformDeviceProperty::new("phy-names", b"usb3\0"),
        ]);

        let by_name = manager.resolve_phy(&device, "usb3").unwrap();
        assert!(by_name.set_mode(PhyMode::UsbHost).is_ok());
        assert_eq!(by_name.mode(), Some(PhyMode::UsbHost));

        let by_index = manager.resolve_phy_by_index(&device, 0).unwrap();
        assert!(by_index.power_on().is_ok());
        assert!(by_index.is_powered());
    }

    #[test_case]
    fn test_parse_phy_specs_rejects_truncated_spec() {
        let manager = DeviceManager::new();
        manager.register_phy_provider(0x80, Arc::new(TestPhyProvider::new(1)));

        match manager.parse_phy_specs(&be_cells(&[0x80])) {
            Ok(_) => panic!("truncated PHY specifier unexpectedly parsed"),
            Err(err) => assert_eq!(err, "phy: truncated specifier"),
        }
    }

    #[test_case]
    fn test_parse_reset_specs_zero_cells() {
        let manager = DeviceManager::new();
        manager.register_reset_controller(0x90, Arc::new(TestResetController::new(0)));
        let specs = manager.parse_reset_specs(&be_cells(&[0x90])).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].phandle, 0x90);
        assert!(specs[0].cells.is_empty());
    }

    #[test_case]
    fn test_parse_reset_specs_one_cell() {
        let manager = DeviceManager::new();
        manager.register_reset_controller(0x90, Arc::new(TestResetController::new(1)));
        let specs = manager.parse_reset_specs(&be_cells(&[0x90, 7])).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].phandle, 0x90);
        assert_eq!(specs[0].cells, vec![7]);
    }

    #[test_case]
    fn test_parse_reset_specs_defer_when_provider_missing() {
        let manager = DeviceManager::new();
        match manager.parse_reset_specs(&be_cells(&[0x90])) {
            Ok(_) => panic!("reset specifier unexpectedly parsed without provider"),
            Err(err) => assert_eq!(err, PROBE_DEFER),
        }
    }

    #[test_case]
    fn test_deassert_device_resets_uses_explicit_resets_only() {
        let manager = DeviceManager::new();
        let reset = Arc::new(TestResetController::new(0));
        manager.register_reset_controller(0x90, reset.clone());
        let device = clk_test_device(vec![PlatformDeviceProperty::new(
            "resets",
            &be_cells(&[0x90]),
        )]);

        assert!(manager.deassert_device_resets(&device).is_ok());
        assert_eq!(reset.deassert_count.load(Ordering::SeqCst), 1);

        let power_only_device = clk_test_device(vec![PlatformDeviceProperty::new(
            "power-domains",
            &be_cells(&[0x90]),
        )]);
        assert!(manager.deassert_device_resets(&power_only_device).is_ok());
        assert_eq!(reset.deassert_count.load(Ordering::SeqCst), 1);
    }

    #[test_case]
    fn test_parse_clock_specs_zero_cells() {
        let manager = DeviceManager::new();
        manager.register_clk_provider(0x10, Arc::new(TestClkProvider::new(24_000_000, 0)));
        let specs = manager.parse_clock_specs(&be_cells(&[0x10])).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].phandle, 0x10);
        assert!(specs[0].cells.is_empty());
    }

    #[test_case]
    fn test_parse_clock_specs_one_cell() {
        let manager = DeviceManager::new();
        manager.register_clk_provider(0x10, Arc::new(TestClkProvider::new(24_000_000, 1)));
        let specs = manager.parse_clock_specs(&be_cells(&[0x10, 3])).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].phandle, 0x10);
        assert_eq!(specs[0].cells, vec![3]);
    }

    #[test_case]
    fn test_parse_clock_specs_rejects_truncated_spec() {
        let manager = DeviceManager::new();
        manager.register_clk_provider(0x10, Arc::new(TestClkProvider::new(24_000_000, 1)));
        match manager.parse_clock_specs(&be_cells(&[0x10])) {
            Ok(_) => panic!("truncated clock specifier unexpectedly parsed"),
            Err(err) => assert_eq!(err, "clk: truncated clock specifier"),
        }
    }

    #[test_case]
    fn test_resolve_clk_by_name() {
        let manager = DeviceManager::new();
        manager.register_clk_provider(0x10, Arc::new(TestClkProvider::new(24_000_000, 0)));
        let device = clk_test_device(vec![
            PlatformDeviceProperty::new("clocks", &be_cells(&[0x10])),
            PlatformDeviceProperty::new("clock-names", b"bus\0"),
        ]);
        let clk = manager.resolve_clk(&device, "bus").unwrap();
        assert_eq!(clk.rate(), 24_000_000);
    }

    #[test_case]
    fn test_resolve_clk_missing_clock_names_falls_back_to_index_zero() {
        let manager = DeviceManager::new();
        manager.register_clk_provider(0x10, Arc::new(TestClkProvider::new(24_000_000, 0)));
        let device = clk_test_device(vec![PlatformDeviceProperty::new(
            "clocks",
            &be_cells(&[0x10]),
        )]);
        let clk = manager.resolve_clk(&device, "bus").unwrap();
        assert_eq!(clk.rate(), 24_000_000);
    }

    #[test_case]
    fn test_resolve_clk_missing_provider_returns_error() {
        let manager = DeviceManager::new();
        let device = clk_test_device(vec![
            PlatformDeviceProperty::new("clocks", &be_cells(&[0x10])),
            PlatformDeviceProperty::new("clock-names", b"bus\0"),
        ]);
        match manager.resolve_clk(&device, "bus") {
            Ok(_) => panic!("clock resolved without provider"),
            Err(err) => assert_eq!(err, "clk: provider not found"),
        }
    }

    struct RecordingProvider {
        clk: ClkHandle,
        events: Mutex<Vec<&'static str>>,
    }

    impl RecordingProvider {
        fn new() -> Self {
            Self {
                clk: ClkHandle::new(Arc::new(ClkFixedRate::new("recorded", 1))),
                events: Mutex::new(Vec::new()),
            }
        }
    }

    impl ClkProvider for RecordingProvider {
        fn name(&self) -> &'static str {
            "recording-provider"
        }

        fn clock_cells(&self) -> usize {
            0
        }

        fn get_clk(&self, spec: &[u32]) -> Result<ClkHandle, ClkError> {
            if spec.is_empty() {
                Ok(self.clk.clone())
            } else {
                Err(ClkError::InvalidSpecifier)
            }
        }

        fn apply_assigned_rate(&self, spec: &[u32], rate: u64) -> Result<(), ClkError> {
            let _ = (spec, rate);
            self.events.lock().push("rate");
            Ok(())
        }

        fn apply_assigned_parent(&self, spec: &[u32], parent: ClkHandle) -> Result<(), ClkError> {
            let _ = (spec, parent);
            self.events.lock().push("parent");
            Ok(())
        }
    }

    #[test_case]
    fn test_apply_assigned_clocks_parents_before_rates() {
        let manager = DeviceManager::new();
        let provider = Arc::new(RecordingProvider::new());
        manager.register_clk_provider(0x10, provider.clone());
        manager.register_clk_provider(0x20, Arc::new(TestClkProvider::new(2, 0)));
        let device = clk_test_device(vec![
            PlatformDeviceProperty::new("assigned-clocks", &be_cells(&[0x10])),
            PlatformDeviceProperty::new("assigned-clock-parents", &be_cells(&[0x20])),
            PlatformDeviceProperty::new("assigned-clock-rates", &be_cells(&[100])),
        ]);

        assert!(manager.apply_assigned_clocks(&device).is_ok());
        assert_eq!(*provider.events.lock(), vec!["parent", "rate"]);
    }

    #[test_case]
    fn test_apply_assigned_clocks_skips_zero_rate() {
        let manager = DeviceManager::new();
        let provider = Arc::new(RecordingProvider::new());
        manager.register_clk_provider(0x10, provider.clone());
        let device = clk_test_device(vec![
            PlatformDeviceProperty::new("assigned-clocks", &be_cells(&[0x10])),
            PlatformDeviceProperty::new("assigned-clock-rates", &be_cells(&[0])),
        ]);

        assert!(manager.apply_assigned_clocks(&device).is_ok());
        assert!(provider.events.lock().is_empty());
    }

    #[test_case]
    fn test_apply_assigned_clocks_skips_zero_parent_phandle() {
        let manager = DeviceManager::new();
        let provider = Arc::new(RecordingProvider::new());
        manager.register_clk_provider(0x10, provider.clone());
        let device = clk_test_device(vec![
            PlatformDeviceProperty::new("assigned-clocks", &be_cells(&[0x10])),
            PlatformDeviceProperty::new("assigned-clock-parents", &be_cells(&[0])),
        ]);

        assert!(manager.apply_assigned_clocks(&device).is_ok());
        assert!(provider.events.lock().is_empty());
    }

    #[test_case]
    fn test_apply_assigned_clocks_rejects_malformed_rates() {
        let manager = DeviceManager::new();
        manager.register_clk_provider(0x10, Arc::new(TestClkProvider::new(24_000_000, 0)));
        let device = clk_test_device(vec![
            PlatformDeviceProperty::new("assigned-clocks", &be_cells(&[0x10])),
            PlatformDeviceProperty::new("assigned-clock-rates", &[0, 1]),
        ]);

        assert_eq!(
            manager.apply_assigned_clocks(&device).unwrap_err(),
            "clk: malformed rates"
        );
    }

    struct HookOrderProvider;

    impl ClkProvider for HookOrderProvider {
        fn name(&self) -> &'static str {
            "hook-order-provider"
        }

        fn clock_cells(&self) -> usize {
            0
        }

        fn get_clk(&self, spec: &[u32]) -> Result<ClkHandle, ClkError> {
            if spec.is_empty() {
                Ok(ClkHandle::new(Arc::new(ClkFixedRate::new("hook", 1))))
            } else {
                Err(ClkError::InvalidSpecifier)
            }
        }

        fn apply_assigned_rate(&self, spec: &[u32], rate: u64) -> Result<(), ClkError> {
            let _ = (spec, rate);
            CLOCK_HOOK_ORDER.store(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn hook_test_driver(
        probe_fn: fn(&PlatformDeviceInfo) -> Result<(), &'static str>,
    ) -> Box<dyn DeviceDriver> {
        Box::new(PlatformDeviceDriver::new(
            "hook-driver",
            probe_fn,
            |_device| Ok(()),
            vec!["test,clk-device"],
        ))
    }

    #[test_case]
    fn test_probe_applies_assigned_clocks_before_driver_probe() {
        CLOCK_HOOK_ORDER.store(0, Ordering::SeqCst);
        CLOCK_HOOK_DRIVER_PROBED.store(false, Ordering::SeqCst);

        let manager = DeviceManager::new();
        manager.register_clk_provider(0x10, Arc::new(HookOrderProvider));
        manager.register_driver(hook_test_driver(hook_order_probe), DriverPriority::Core);
        let device = Arc::new(clk_test_device(vec![
            PlatformDeviceProperty::new("assigned-clocks", &be_cells(&[0x10])),
            PlatformDeviceProperty::new("assigned-clock-rates", &be_cells(&[100])),
        ]));

        let mut idx = 0;
        manager.try_match_and_probe_device(DriverPriority::Core, &mut idx, device);
        assert!(CLOCK_HOOK_DRIVER_PROBED.load(Ordering::SeqCst));
        assert_eq!(CLOCK_HOOK_ORDER.load(Ordering::SeqCst), 2);
    }

    #[test_case]
    fn test_probe_defers_when_clock_provider_not_yet_registered() {
        CLOCK_HOOK_ORDER.store(0, Ordering::SeqCst);
        let manager = DeviceManager::new();
        manager.register_driver(hook_test_driver(hook_order_probe), DriverPriority::Core);
        let device = Arc::new(clk_test_device(vec![PlatformDeviceProperty::new(
            "assigned-clocks",
            &be_cells(&[0x10]),
        )]));

        let mut idx = 0;
        manager.try_match_and_probe_device(DriverPriority::Core, &mut idx, device);
        assert_eq!(manager.deferred_platform_devices.lock().len(), 1);
        assert_eq!(CLOCK_HOOK_ORDER.load(Ordering::SeqCst), 0);
    }

    #[test_case]
    fn test_probe_defers_when_iommu_controller_not_yet_registered() {
        CLOCK_HOOK_ORDER.store(0, Ordering::SeqCst);
        let manager = DeviceManager::new();
        manager.register_driver(hook_test_driver(hook_order_probe), DriverPriority::Core);
        let device = Arc::new(clk_test_device(vec![PlatformDeviceProperty::new(
            "iommus",
            &be_cells(&[0x40, 0x10]),
        )]));

        let mut idx = 0;
        manager.try_match_and_probe_device(DriverPriority::Core, &mut idx, device);
        assert_eq!(manager.deferred_platform_devices.lock().len(), 1);
        assert_eq!(CLOCK_HOOK_ORDER.load(Ordering::SeqCst), 0);
    }

    #[test_case]
    fn test_probe_defers_when_dma_controller_not_yet_registered() {
        CLOCK_HOOK_ORDER.store(0, Ordering::SeqCst);
        let manager = DeviceManager::new();
        manager.register_driver(hook_test_driver(hook_order_probe), DriverPriority::Core);
        let device = Arc::new(clk_test_device(vec![PlatformDeviceProperty::new(
            "dmas",
            &be_cells(&[0x30, 7]),
        )]));

        let mut idx = 0;
        manager.try_match_and_probe_device(DriverPriority::Core, &mut idx, device);
        assert_eq!(manager.deferred_platform_devices.lock().len(), 1);
        assert_eq!(CLOCK_HOOK_ORDER.load(Ordering::SeqCst), 0);
    }

    #[test_case]
    fn test_probe_hard_fails_on_malformed_assigned_clocks() {
        CLOCK_HOOK_ORDER.store(0, Ordering::SeqCst);
        let manager = DeviceManager::new();
        manager.register_clk_provider(0x10, Arc::new(HookOrderProvider));
        manager.register_driver(hook_test_driver(hook_order_probe), DriverPriority::Core);
        let device = Arc::new(clk_test_device(vec![
            PlatformDeviceProperty::new("assigned-clocks", &be_cells(&[0x10])),
            PlatformDeviceProperty::new("assigned-clock-rates", &[0, 1]),
        ]));

        let mut idx = 0;
        manager.try_match_and_probe_device(DriverPriority::Core, &mut idx, device);
        assert_eq!(manager.deferred_platform_devices.lock().len(), 0);
        assert_eq!(CLOCK_HOOK_ORDER.load(Ordering::SeqCst), 0);
    }

    #[test_case]
    fn test_device_without_assigned_clocks_probes_normally() {
        CLOCK_HOOK_ORDER.store(0, Ordering::SeqCst);
        CLOCK_HOOK_DRIVER_PROBED.store(false, Ordering::SeqCst);
        let manager = DeviceManager::new();
        manager.register_driver(hook_test_driver(hook_order_probe), DriverPriority::Core);
        let device = Arc::new(clk_test_device(vec![]));

        let mut idx = 0;
        manager.try_match_and_probe_device(DriverPriority::Core, &mut idx, device);
        assert_eq!(idx, 1);
        assert_eq!(CLOCK_HOOK_ORDER.load(Ordering::SeqCst), 2);
    }

    #[cfg(target_arch = "riscv64")]
    #[test_case]
    fn test_populate_driver() {
        static mut TEST_RESULT: bool = false;
        fn probe_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
            unsafe {
                TEST_RESULT = true;
            }
            Ok(())
        }

        let driver = Box::new(PlatformDeviceDriver::new(
            "test",
            probe_fn,
            |_device| Ok(()),
            vec!["sifive,test0"],
        ));
        let manager = DeviceManager::new();
        manager.register_driver(driver, DriverPriority::Standard);

        manager.populate_devices();
        let result = unsafe { TEST_RESULT };
        assert_eq!(result, true);
    }

    #[test_case]
    fn test_get_device_from_manager() {
        let device = Arc::new(GenericDevice::new("test"));
        let manager = DeviceManager::new();
        let id = manager.register_device(device);
        let retrieved_device = manager.get_device(id);
        assert!(retrieved_device.is_some());
        let retrieved_device = retrieved_device.unwrap();
        assert_eq!(retrieved_device.name(), "test");
    }

    #[test_case]
    fn test_get_device_by_name() {
        let device = Arc::new(GenericDevice::new("test_named"));
        let manager = DeviceManager::new();
        let _id = manager.register_device_with_name("test_device".into(), device);
        let retrieved_device = manager.get_device_by_name("test_device");
        assert!(retrieved_device.is_some());
        let retrieved_device = retrieved_device.unwrap();
        assert_eq!(retrieved_device.name(), "test_named");
    }

    #[test_case]
    fn test_get_first_device_by_type() {
        let device1 = Arc::new(GenericDevice::new("test_char"));
        let device2 = Arc::new(GenericDevice::new("test_block"));
        let manager = DeviceManager::new();
        let _id1 = manager.register_device(device1);
        let _id2 = manager.register_device(device2);

        let char_device_id = manager.get_first_device_by_type(crate::device::DeviceType::Generic);
        assert!(char_device_id.is_some());
        let char_device_id = char_device_id.unwrap();
        let char_device = manager.get_device(char_device_id).unwrap();
        assert_eq!(char_device.name(), "test_char");
    }

    #[test_case]
    fn test_get_device_out_of_bounds() {
        let manager = DeviceManager::new();
        let device = manager.get_device(999);
        assert!(device.is_none());
    }

    #[test_case]
    fn test_get_device_by_name_not_found() {
        let manager = DeviceManager::new();
        let device = manager.get_device_by_name("non_existent");
        assert!(device.is_none());
    }

    #[test_case]
    fn test_mem_resource_from_region_without_size() {
        let region = fdt::standard_nodes::MemoryRegion {
            starting_address: 0x1000 as *const u8,
            size: None,
        };

        assert!(DeviceManager::mem_resource_from_region(region).is_none());
    }

    #[test_case]
    fn test_mem_resource_from_region_with_size() {
        let region = fdt::standard_nodes::MemoryRegion {
            starting_address: 0x1000 as *const u8,
            size: Some(0x100),
        };

        let resource = DeviceManager::mem_resource_from_region(region).unwrap();
        assert_eq!(resource.start, 0x1000);
        assert_eq!(resource.end, 0x10ff);
        assert_eq!(resource.res_type, PlatformDeviceResourceType::MEM);
        assert!(resource.irq_metadata.is_none());
    }
}
