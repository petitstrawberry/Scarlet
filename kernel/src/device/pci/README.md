# PCI Bus Support

This directory contains the PCI (Peripheral Component Interconnect) bus implementation for the Scarlet kernel.

## Overview

The PCI subsystem provides infrastructure for:
- PCI device discovery through bus enumeration
- Configuration space access via ECAM (Enhanced Configuration Access Mechanism)
- Device-to-driver matching using vendor/device IDs and class codes
- Integration with the kernel's DeviceManager

## Architecture

### Module Structure

```
pci/
├── mod.rs       - Core PCI types (PciAddress, PciBus)
├── config.rs    - Configuration space access (ECAM-based)
├── device.rs    - PCI device information (PciDeviceInfo)
├── driver.rs    - PCI driver matching (PciDeviceDriver, PciDeviceId)
└── scan.rs      - Bus scanning and device enumeration
```

### Key Components

#### PciAddress
Represents a PCI device location:
- Segment (domain)
- Bus number (0-255)
- Device number (0-31)
- Function number (0-7)

#### PciBus
The main bus manager that:
- Stores ECAM base address and size
- Maintains a list of discovered devices
- Provides scanning functionality

#### PciConfig
Handles configuration space access through ECAM:
- Read/write operations for 8/16/32-bit values
- Helper methods for standard registers (vendor ID, device ID, class code)

#### PciDeviceInfo
Contains device information:
- Vendor and device IDs
- Class code (base class, subclass, interface)
- Subsystem IDs
- Interrupt configuration
- Implements the `DeviceInfo` trait for DeviceManager integration

#### PciDeviceDriver
Driver matching and probing:
- Device ID table with wildcards
- Class-based matching with masks
- Probe and remove functions
- Implements the `DeviceDriver` trait

## Usage

### Initializing PCI

```rust
use crate::device::pci::PciBus;

// Create PCI bus with ECAM base from device tree/ACPI
let ecam_base = 0x3000_0000;
let ecam_size = 0x1000_0000;
let pci_bus = PciBus::new(ecam_base, ecam_size);

// Scan for devices and register with DeviceManager
pci_bus.scan_and_register();
```

### Registering a PCI Driver

```rust
use crate::device::pci::driver::{PciDeviceDriver, PciDeviceId};
use crate::device::manager::{DeviceManager, DriverPriority};

// Define device IDs this driver supports
let id_table = vec![
    PciDeviceId::new(0x8086, 0x1234), // Intel device 0x1234
    PciDeviceId::new(0x8086, 0x5678), // Intel device 0x5678
];

// Create driver with probe/remove functions
let driver = PciDeviceDriver::new(
    "my_pci_driver",
    id_table,
    |device| {
        // Probe function
        println!("Probing device {:04x}:{:04x}", 
                 device.vendor_id(), device.device_id());
        Ok(())
    },
    |device| {
        // Remove function
        Ok(())
    },
);

// Register with DeviceManager
DeviceManager::get_mut_manager()
    .register_driver(Box::new(driver), DriverPriority::Standard);
```

### Class-Based Matching

```rust
// Match all network controllers
let id = PciDeviceId::from_class(0x020000, 0xFF0000);

// Match all storage controllers
let id = PciDeviceId::from_class(0x010000, 0xFF0000);
```

## ECAM (Enhanced Configuration Access Mechanism)

PCI Express uses ECAM for configuration space access. Each function gets 4KB of configuration space:

```
Physical Address = ECAM_BASE + (bus << 20) | (device << 15) | (function << 12) + offset
```

### Memory Layout

- Each segment can have up to 256 buses
- Each bus can have up to 32 devices
- Each device can have up to 8 functions
- Each function has 4KB of configuration space

Total ECAM space per segment: 256MB (256 buses * 32 devices * 8 functions * 4KB)

## Configuration Space Layout

Standard PCI configuration header (first 64 bytes):

```
Offset | Size | Description
-------|------|------------
0x00   | 16   | Vendor ID
0x02   | 16   | Device ID
0x04   | 16   | Command
0x06   | 16   | Status
0x08   | 8    | Revision ID
0x09   | 24   | Class Code (base, sub, interface)
0x0C   | 8    | Cache Line Size
0x0D   | 8    | Latency Timer
0x0E   | 8    | Header Type
0x0F   | 8    | BIST
0x10   | 32   | BAR0
...    | ...  | Additional BARs
0x2C   | 16   | Subsystem Vendor ID
0x2E   | 16   | Subsystem ID
0x3C   | 8    | Interrupt Line
0x3D   | 8    | Interrupt Pin
```

## Device Classes

Common PCI device classes:
- `0x01` - Mass Storage Controller
- `0x02` - Network Controller
- `0x03` - Display Controller
- `0x06` - Bridge Device
- `0x0C` - Serial Bus Controller (USB, etc.)

## Integration with DeviceManager

The PCI subsystem integrates with the existing DeviceManager:

1. **Device Discovery**: `PciBus::scan()` enumerates devices
2. **Device Registration**: Devices are wrapped and registered with DeviceManager
3. **Driver Matching**: PCI drivers can be registered at different priority levels
4. **Probe/Remove**: DeviceManager calls driver probe/remove functions

## Testing

The implementation includes comprehensive tests:

```bash
cargo make test
```

Test coverage includes:
- PCI address ECAM offset calculation
- Bus validation
- Device info creation and matching
- Driver probe functionality
- Scanner device name generation

## Future Enhancements

Potential improvements:
- MSI/MSI-X interrupt support
- PCIe capabilities parsing
- Power management (D-states)
- Hot-plug support
- Better device naming with string pool
- BAR (Base Address Register) management
- DMA support for PCI devices

## References

- [PCI Express Base Specification](https://pcisig.com/)
- [Linux PCI Documentation](https://www.kernel.org/doc/html/latest/PCI/index.html)
- [OSDev PCI Wiki](https://wiki.osdev.org/PCI)
