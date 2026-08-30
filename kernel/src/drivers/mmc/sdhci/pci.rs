//! PCI binding for the standard SDHCI controller exposed by QEMU.

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::device::Device;
use crate::device::manager::{DeviceManager, DriverPriority};
use crate::device::pci::config::{self, PciConfig, command};
use crate::device::pci::device::PciDeviceInfo;
use crate::device::pci::driver::{PciDeviceDriver, PciDeviceId};
use crate::driver_initcall;
use crate::drivers::mmc::core::EmmcBlockDevice;
use crate::environment::PAGE_SIZE;
use crate::{early_println, println, vm};

use super::SdhciHost;

const PCI_CLASS_SD_HOST_CONTROLLER: u32 = 0x080500;
const PCI_CLASS_BASE_SUBCLASS_MASK: u32 = 0xffff00;

static NEXT_MMC_DEVICE_INDEX: AtomicUsize = AtomicUsize::new(0);

fn enable_controller(device: &PciDeviceInfo) {
    let config = PciConfig::new(device.ecam_vaddr());
    let address = device.address();
    let current = config.read_u16(&address, config::offset::COMMAND);
    config.write_u16(
        &address,
        config::offset::COMMAND,
        current | command::MEMORY_SPACE | command::BUS_MASTER | command::INTERRUPT_DISABLE,
    );
}

fn probe_sdhci_pci(device: &PciDeviceInfo) -> Result<(), &'static str> {
    let bar = device
        .mmio_bar(0)
        .ok_or("SDHCI PCI function has no assigned BAR 0")?;
    let physical_base = usize::try_from(bar.base).map_err(|_| "SDHCI BAR address is too large")?;
    let aperture_size = usize::try_from(bar.size.max(PAGE_SIZE as u64))
        .map_err(|_| "SDHCI BAR aperture is too large")?;

    enable_controller(device);
    let mmio_base = vm::ioremap(physical_base, aperture_size)?;
    let host = SdhciHost::new(mmio_base, true);
    let index = NEXT_MMC_DEVICE_INDEX.fetch_add(1, Ordering::Relaxed);
    let owned_name = format!("mmcblk{}", index);
    let name: &'static str = Box::leak(owned_name.clone().into_boxed_str());

    let block_device = match EmmcBlockDevice::probe(name, Box::new(host)) {
        Ok(block_device) => block_device,
        Err(error) => {
            vm::iounmap(mmio_base);
            early_println!(
                "[mmc] Failed to identify eMMC at {:02x}:{:02x}.{}: {}",
                device.address().bus,
                device.address().device,
                device.address().function,
                error.as_str()
            );
            return Err(error.as_str());
        }
    };

    let card = block_device.card_info();
    println!(
        "[mmc] {}: {} sectors ({} MiB), EXT_CSD rev {}, device type {:#04x}, media generation {}",
        name,
        card.sector_count(),
        card.sector_count().saturating_mul(512) / (1024 * 1024),
        card.ext_csd_revision(),
        card.device_type(),
        block_device.media_generation()
    );

    let registered: Arc<dyn Device> = Arc::new(block_device);
    DeviceManager::get_manager().register_device_with_name(owned_name, registered);
    Ok(())
}

fn remove_sdhci_pci(_device: &PciDeviceInfo) -> Result<(), &'static str> {
    // PCI runtime removal is not wired into the bus enumerator yet. The MMC
    // core still detects card loss before each operation, so stale media is
    // rejected rather than accessed if a removable host is added later.
    Ok(())
}

fn register_driver() {
    let id_table = vec![PciDeviceId::from_class(
        PCI_CLASS_SD_HOST_CONTROLLER,
        PCI_CLASS_BASE_SUBCLASS_MASK,
    )];
    let driver = PciDeviceDriver::new("sdhci-pci", id_table, probe_sdhci_pci, remove_sdhci_pci);
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard);
}

driver_initcall!(register_driver);
