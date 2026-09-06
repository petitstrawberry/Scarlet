# ABI model and compatibility

Scarlet ABI support is implemented as kernel ABI modules over shared kernel
objects:

- Binary format detection selects the ABI implementation.
- Each ABI translates its syscall surface into Scarlet kernel primitives.
- ABIs share VFS nodes, sockets, task objects, devices, and events rather than
  communicating through a VM boundary.

ABI compatibility translates operating-system interfaces; it does not emulate
CPU instructions between architectures. See the
[kernel development map](../kernel/README.md) for the ABI and syscall source paths.

## ABI layers

| ABI | State |
| --- | --- |
| Scarlet native | Main in-tree userland and services. |
| xv6 RISC-V 64 | Supported for shell and common xv6 commands. |
| Linux RISC-V/AArch64 | Partial but actively used syscall layer for selected Buildroot/BusyBox, GUI, and service workloads. |

The Linux ABI also exposes a `/dev/kvm` compatibility layer backed by SHV, so
KVM-oriented VMMs can target Scarlet's hypervisor path instead of a separate
kernel API. See the [SHV overview](../hypervisor/README.md).

## Guides and references

- [Native application development](../userspace/README.md)
- [Linux ABI status](linux/status.md)
- [Linux userspace artifacts](linux/userspace-artifacts.md)
- [Linux ABI demo](linux/demo.md)
- [Linux rootfs deployment](linux/deployment.md)
- [Linux thread support](linux/thread-support.md)
- [Runtime delegation](runtime-delegation.md)
