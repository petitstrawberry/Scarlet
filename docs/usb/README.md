# USB Subsystem

This directory tracks the design and implementation status of Scarlet's USB host stack.

The first target is xHCI with USB HID boot-protocol devices, using existing Scarlet
subsystems for PCI discovery, MMIO mapping, interrupt delivery, and input event
publication.

## Goals

- Implement an in-tree xHCI host controller driver.
- Reuse Scarlet's PCI, VM, PMM, and interrupt infrastructure.
- Expose USB keyboards and mice through the existing `EventDevice` abstraction.
- Keep the implementation portable across the architectures already supported by Scarlet.

## Non-Goals for the Initial Milestone

- USB audio, networking, or storage class support.
- MSI/MSI-X support.
- Full HID report descriptor parsing.
- USB hub support beyond the minimum needed for direct devices.

## Architecture Overview

```text
PCI xHCI controller
        |
        v
drivers/usb/xhci
  - registers
  - TRBs
  - ring management
  - contexts
  - command/event handling
        |
        v
drivers/usb/core
  - descriptors
  - device state
  - enumeration flow
        |
        v
drivers/usb/hid
  - boot keyboard
  - boot mouse
        |
        v
device/input/event_device
```

## Integration Points in Scarlet

- PCI matching: `kernel/src/device/pci/driver.rs`
- PCI config space access: `kernel/src/device/pci/config.rs`
- MMIO mapping: `kernel/src/vm/ioremap.rs`
- DMA-safe contiguous memory: `kernel/src/mem/page.rs`
- Interrupt registration: `kernel/src/interrupt/mod.rs`
- Input events: `kernel/src/device/input/event_device.rs`

## Milestones

1. Add USB driver scaffolding and design documents.
2. Implement xHCI PCI probe and register access helpers.
3. Implement command ring, event ring, and controller bring-up.
4. Implement USB enumeration and control transfers.
5. Implement HID boot keyboard and mouse support.
6. Validate on QEMU and real hardware targets as available.

## Reference Material

These projects are references, not dependencies for the in-tree implementation:

- `rust-osdev/xhci`
- `usb-oxide`
- Redox `xhcid`
- CrabUSB

See `xhci-implementation-plan.md` for the detailed phase plan.
