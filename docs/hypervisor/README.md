# Scarlet Hypervisor (SHV)

A Type-2 hypervisor built into Scarlet OS.

## Overview

SHV (Scarlet Hypervisor) is a Type-2 (hosted) hypervisor for running guest
operating systems. It provides Scarlet-native hypervisor objects and a Linux
`/dev/kvm` compatibility layer used by KVM-oriented VMMs.

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    User Space                            │
│  ┌─────────────────────────────────────────────────────┐│
│  │                    U-SHV                            ││
│  │  (Userspace VMM)                                    ││
│  │  - Device emulation (UART/PL011, PLIC, etc.)        ││
│  │  - Guest management                                 ││
│  │  - MMIO handling                                    ││
│  │  - SBI/PSCI firmware handling                       ││
│  │  - KVM-oriented VMMs through Linux /dev/kvm          ││
│  └─────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────┘
                          │ syscalls
                          ▼
┌─────────────────────────────────────────────────────────┐
│                    Kernel Space                          │
│  ┌─────────────────────────────────────────────────────┐│
│  │                    SHV                              ││
│  │  (Kernel Hypervisor)                                ││
│  │  - VM-entry / VM-exit                               ││
│  │  - Stage-2 MMU management                           ││
│  │  - vCPU state management                            ││
│  │  - Timer handling                                   ││
│  └─────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────┘
```

## Supported Architectures

| Architecture | Status | Notes |
|--------------|--------|-------|
| RISC-V 64-bit (H-extension) | Implemented | SHV and Linux `/dev/kvm` paths, primary development target |
| AArch64 | Implemented | EL2/VHE path with Stage-2 translation, guest entry/exit, trap handling, timer support, VGIC/PSCI work, and Linux `/dev/kvm` compatibility |

## Quick Start

### Running a Guest with U-SHV

```bash
# Run a simple guest binary
ushv /path/to/guest.bin

# Specify memory size
ushv -m 512 /path/to/guest.bin

# Specify initramfs
ushv -i /path/to/initramfs.cpio /path/to/guest.bin
```

Linux ABI guests can also use Scarlet's `/dev/kvm` compatibility layer. That
path maps KVM ioctls onto SHV and is intended for KVM-oriented VMMs such as
kvmtool and Firecracker-class microVM workloads.

### Programmatic Usage

```rust
use scarlet_std::hypervisor::{Vm, Vcpu, VcpuExitReason};

// Create a VM
let vm = Vm::create()?;

// Add memory region
vm.add_memory_region(0, 0x80000000, 128 * 1024 * 1024, host_addr)?;

// Create a vCPU
let vcpu = vm.create_vcpu(0)?;

// Load guest code (omitted)

// Run the vCPU
loop {
    let exit = vcpu.run()?;
    
    match exit.reason {
        VcpuExitReason::MmioRead | VcpuExitReason::MmioWrite => {
            // Handle MMIO
        }
        VcpuExitReason::Shutdown | VcpuExitReason::Hlt => break,
        _ => {}
    }
}
```

## Documentation

- [Type-2 Hypervisor Design](type2-design.md) - Architecture design
- [Implementation Status](status.md) - Current implementation status

## Code Locations

| Component | Path | Description |
|-----------|------|-------------|
| Kernel SHV | `kernel/src/hypervisor/` | Kernel-side hypervisor |
| RISC-V H-ext | `kernel/src/arch/riscv64/hv/` | RISC-V specific implementation |
| AArch64 virt | `kernel/src/arch/aarch64/hv/` | AArch64 specific implementation |
| U-SHV | `user/bin/src/ushv/` | Userspace VMM |
| User API | `user/lib/std/src/hypervisor/` | Userspace API |
| Guest Tests | `guest_tests/` | Test guest programs |

## Feature Flag

Enable the `hypervisor` feature to include the hypervisor:

```toml
[features]
hypervisor = []
```

## Limitations

Current implementation has the following limitations:

- No SMP support (single vCPU only)
- No device passthrough
- No IOMMU support
- No nested virtualization
- No live migration

See [Implementation Status](status.md) for details.
