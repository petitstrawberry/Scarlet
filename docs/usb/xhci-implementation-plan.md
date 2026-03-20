# xHCI Implementation Plan

This document describes the staged plan for Scarlet's in-tree xHCI driver.

## Why In-Tree

Scarlet will implement xHCI directly in-tree so the host controller logic matches the
kernel's device model, memory ownership rules, and debugging workflow. Existing Rust
projects remain useful as references for data structure layout and controller sequencing.

## Phase 0: Scaffolding

- Create `kernel/src/drivers/usb/` with separate `xhci`, `core`, and `hid` modules.
- Define stable internal boundaries for:
  - controller registers
  - TRB encoding/decoding
  - ring state
  - xHCI contexts
  - USB descriptors and device state
  - HID boot reports
- Keep runtime behavior inert until the probe path is ready.

## Phase 1: xHCI PCI Bring-Up

Implement:

- PCI class matching for serial bus / USB / xHCI (`0x0c0330`)
- BAR decoding for MMIO register mapping
- PCI bus-master enablement through the command register
- MMIO register wrappers for capability, operational, runtime, and doorbell regions

Expected outputs:

- `XhciPciDriver` probe entry point
- xHCI register block abstraction
- validated BAR parsing helpers

## Phase 2: Controller Initialization

Implement:

- host controller reset
- controller halt/run transitions
- DCBAA allocation
- command ring allocation and CRCR programming
- event ring allocation, ERST setup, and interrupter programming

Memory rules:

- All controller-visible memory must come from physically contiguous pages.
- Physical addresses are derived from Scarlet's PMM-backed page allocations.

## Phase 3: Enumeration Core

Implement:

- Enable Slot
- Address Device
- endpoint zero context setup
- standard control transfers for device and configuration descriptors
- configuration selection and endpoint discovery

Outputs:

- `UsbDevice` state machine
- descriptor parsing for device, configuration, interface, and endpoint descriptors

## Phase 4: HID Boot Devices

Implement:

- interface classification for HID boot keyboard and mouse
- interrupt IN endpoint setup
- boot report decoding
- translation into Scarlet `EventDevice` events

Initial scope:

- keyboard key press/release
- mouse relative motion and button events

## Phase 5: Interrupt Handling

Implement:

- `InterruptCapableDevice` integration for xHCI interrupts
- event ring draining on interrupt
- port-status-change and transfer-event dispatch

The first implementation should support legacy PCI interrupt lines. MSI/MSI-X can be added
later once PCI capability support is expanded.

## Validation Strategy

- unit tests for TRB encoding, BAR decoding, and descriptor sizes
- targeted kernel build on the default architecture
- QEMU validation with xHCI plus USB keyboard and mouse

## QEMU Targets to Prepare For

Examples to support during validation:

```bash
qemu-system-riscv64 \
  -machine virt \
  -device qemu-xhci \
  -device usb-kbd \
  -device usb-mouse
```

## Risks

- MSI/MSI-X is not yet part of Scarlet's PCI support.
- xHCI reset and ownership handoff bugs are difficult to debug without detailed logging.
- Full HID report parsing is intentionally deferred.

## Implementation Notes

- Prefer small, explicit wrappers over giant controller structs.
- Keep unsafe code localized to register and ring access boundaries.
- Add `// SAFETY:` comments to every unsafe block.
- Match the existing kernel style for `Result<T, &'static str>` and `#[test_case]` tests.
