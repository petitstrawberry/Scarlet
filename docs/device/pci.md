# PCI Bus Subsystem

## Overview

Scarlet implements PCI/PCIe bus support using ECAM (Enhanced Configuration Access Mechanism), the standard configuration-space access method for PCI Express on RISC-V and ARM platforms.

## Module Structure

```
kernel/src/device/pci/
├── mod.rs       – PciAddress, PciBus (core types)
├── config.rs    – PciConfig (ECAM-based config-space access)
├── device.rs    – PciDeviceInfo (device properties, DeviceInfo impl)
├── driver.rs    – PciDeviceDriver, PciDeviceId (driver matching)
└── scan.rs      – Bus enumeration and device scanning
```

## Architecture

```text
FDT / ACPI
    │ ecam_base, ecam_size
    ▼
 PciBus ──────────────────────────────────────────
    │                                               │
    │ scan()                                        │
    ▼                                               │
 PciConfig ── ECAM MMIO read/write ──► Config Space │
    │                                               │
    ▼                                               │
 PciDeviceInfo ──► DeviceManager ──► PciDeviceDriver │
 (vendor/device ID, class, BARs)     (probe/remove)  │
 ────────────────────────────────────────────────────┘
```

### PciAddress

Represents a PCI device location: `{ segment, bus, device, function }`.

ECAM offset: `(bus << 20) | (device << 15) | (function << 12)` — each function receives 4 KB of config space.

### PciBus

Central manager initialized with the ECAM base physical address and size from the device tree:

```rust
let pci_bus = PciBus::new(ecam_base, ecam_size);
pci_bus.scan_and_register(); // enumerate → register with DeviceManager
```

ECAM is lazily mapped via `vm::ioremap` on first access.

### PciConfig

Provides 8/16/32-bit read/write to configuration-space registers through the mapped ECAM region.

### PciDeviceInfo

Holds device properties discovered during enumeration:

- Vendor ID, Device ID
- Class code (base class, subclass, interface)
- Subsystem IDs
- BAR addresses
- Interrupt pin/line

Implements the `DeviceInfo` trait so that `DeviceManager` can match devices to drivers generically.

### PciDeviceDriver

Wraps an ID table (`PciDeviceId`) and probe/remove callbacks. Supports:

- **Exact match**: vendor + device ID
- **Class match**: class code with mask (e.g., match all network controllers)

```rust
let driver = PciDeviceDriver::new(
    "my-driver",
    vec![PciDeviceId::new(0x8086, 0x1234)],
    |device| { /* probe */ Ok(()) },
    |device| { /* remove */ Ok(()) },
);
DeviceManager::get_mut_manager()
    .register_driver(Box::new(driver), DriverPriority::Standard);
```

## Boot Flow

1. Platform init reads ECAM base/size from FDT.
2. `PciBus::scan_and_register()` walks bus/dev/func.
3. Each valid device becomes a `PciDeviceInfo`, registered with `DeviceManager`.
4. Registered `PciDeviceDriver`s are matched and probed automatically.

## Supported Devices

- VirtIO over PCI (network, block, GPU, input, RNG, etc.)
- xHCI USB host controllers
- Any PCI device with a registered driver
