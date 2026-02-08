# PCI Integration Example

This document shows how to integrate PCI device discovery into the Scarlet kernel boot sequence.

## Overview

PCI device discovery is typically initiated after the DeviceManager has been populated with platform devices from FDT/UEFI/ACPI. The PCI host bridge information (including ECAM base address) is discovered through these firmware interfaces.

## Integration Points

### 1. During DeviceManager Population

PCI devices can be discovered as part of the normal device population flow. The PCI host bridge is itself a platform device that appears in the device tree or ACPI tables.

### 2. Example: FDT-based Discovery

In a typical FDT-based system, the device tree contains a PCI host bridge node:

```dts
pcie@30000000 {
    compatible = "pci-host-ecam-generic";
    device_type = "pci";
    #address-cells = <3>;
    #size-cells = <2>;
    reg = <0x0 0x30000000 0x0 0x10000000>;  // ECAM space
    ranges = <...>;  // Memory and I/O ranges
};
```

### 3. Creating a PCI Host Bridge Driver

```rust
use crate::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use crate::device::pci::PciBus;
use crate::device::manager::{DeviceManager, DriverPriority};

fn pci_probe(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    // Get ECAM base and size from platform device resources
    let resources = device.get_resources();
    
    // Find the memory resource (ECAM space)
    let ecam_resource = resources
        .iter()
        .find(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("No ECAM resource found")?;
    
    let ecam_base = ecam_resource.start;
    let ecam_size = ecam_resource.end - ecam_resource.start + 1;
    
    early_println!("PCI: Found ECAM at {:#x}, size {:#x}", ecam_base, ecam_size);
    
    // Create PCI bus and scan
    let pci_bus = PciBus::new(ecam_base, ecam_size);
    pci_bus.scan_and_register();
    
    Ok(())
}

pub fn register_pci_host_driver() {
    let driver = Box::new(PlatformDeviceDriver::new(
        "pci-host-ecam-generic",
        pci_probe,
        |_device| Ok(()),
        vec!["pci-host-ecam-generic"],
    ));
    
    // Register at Core priority so PCI devices are discovered early
    DeviceManager::get_mut_manager()
        .register_driver(driver, DriverPriority::Core);
}
```

### 4. Initialization in main.rs

Add the PCI host driver registration before device population:

```rust
// In start_kernel(), before populate_devices_from_source()

// Register PCI host bridge driver
#[cfg(feature = "pci")]
{
    use crate::drivers::pci_host::register_pci_host_driver;
    register_pci_host_driver();
}

// This will now discover PCI host bridges and scan for devices
device_manager.populate_devices_from_source(&boot_info.device_source, None);
```

### 5. Registering PCI Device Drivers

PCI device drivers should be registered before or during device population:

```rust
// Example: VirtIO PCI driver
use crate::device::pci::driver::{PciDeviceDriver, PciDeviceId};

fn register_virtio_pci_driver() {
    let id_table = vec![
        // VirtIO devices (Red Hat vendor ID)
        PciDeviceId::from_class(0x010000, 0xFF0000), // Storage class
        // Or specific device IDs:
        PciDeviceId::new(0x1AF4, 0x1000), // VirtIO net
        PciDeviceId::new(0x1AF4, 0x1001), // VirtIO block
    ];
    
    let driver = Box::new(PciDeviceDriver::new(
        "virtio-pci",
        id_table,
        |device| {
            early_println!("VirtIO PCI: Probing {:04x}:{:04x}", 
                         device.vendor_id(), device.device_id());
            // Initialize VirtIO device
            Ok(())
        },
        |device| {
            // Cleanup
            Ok(())
        },
    ));
    
    DeviceManager::get_mut_manager()
        .register_driver(driver, DriverPriority::Standard);
}
```

## Boot Sequence with PCI

The complete boot sequence with PCI support:

1. **Early Boot**: Architecture-specific initialization
2. **BootInfo Creation**: Extract FDT/UEFI/ACPI information
3. **Heap Initialization**: Set up memory allocator
4. **Driver Registration**: Register all drivers including PCI host and PCI device drivers
5. **Device Population**: 
   - FDT/UEFI/ACPI discovers PCI host bridge
   - PCI host driver probes, creates PciBus
   - PciBus scans for devices
   - PCI devices are matched with registered drivers
6. **Continue Boot**: Graphics, interrupts, filesystems, etc.

## Example: Complete Integration

```rust
// In kernel/src/drivers/mod.rs
pub mod pci_host;

// In kernel/src/drivers/pci_host.rs
use crate::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use crate::device::platform::resource::PlatformDeviceResourceType;
use crate::device::pci::PciBus;
use crate::device::manager::{DeviceManager, DriverPriority};
use crate::early_println;

static mut PCI_BUS: Option<PciBus> = None;

fn pci_host_probe(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    early_println!("PCI Host: Probing {}", device.name());
    
    let resources = device.get_resources();
    let ecam_resource = resources
        .iter()
        .find(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("No ECAM resource found")?;
    
    let ecam_base = ecam_resource.start;
    let ecam_size = ecam_resource.end - ecam_resource.start + 1;
    
    early_println!("PCI Host: ECAM at {:#x}, size {:#x}", ecam_base, ecam_size);
    
    let pci_bus = PciBus::new(ecam_base, ecam_size);
    pci_bus.scan_and_register();
    
    unsafe {
        PCI_BUS = Some(pci_bus);
    }
    
    Ok(())
}

pub fn register_pci_host_driver() {
    let driver = Box::new(PlatformDeviceDriver::new(
        "pci-host-ecam",
        pci_host_probe,
        |_| Ok(()),
        vec!["pci-host-ecam-generic", "pci-host-cam-generic"],
    ));
    
    DeviceManager::get_mut_manager()
        .register_driver(driver, DriverPriority::Core);
}

// In kernel/src/main.rs
use crate::drivers::pci_host;

// Before device_manager.populate_devices_from_source():
pci_host::register_pci_host_driver();
```

## Testing

To test PCI support in QEMU:

```bash
qemu-system-riscv64 \
  -machine virt \
  -device virtio-blk-pci \
  -device virtio-net-pci \
  ...
```

Check kernel output for PCI discovery messages:
```
PCI Host: Probing pcie@30000000
PCI Host: ECAM at 0x30000000, size 0x10000000
Scanning PCI bus...
Found PCI device: 1af4:1001 at 00:01.0 (class: 010000)
Found PCI device: 1af4:1000 at 00:02.0 (class: 020000)
PCI scan complete: found 2 devices
```

## Future Enhancements

- MSI/MSI-X interrupt routing
- BAR allocation and management
- PCI Express capability parsing
- Hot-plug support
- Multiple PCI segments
- IOMMU integration for DMA
