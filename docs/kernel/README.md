# Kernel development map

This guide describes the current source organization and reference boot flow,
reviewed on 2026-09-06. It is an entry point to the implementation, not a claim
that every ABI, driver, or hardware configuration is complete.

Scarlet's kernel is the `scarlet` **library crate** in
[kernel/Cargo.toml](../../kernel/Cargo.toml). It is `no_std`; the project BSP
provides the executable and boot entry. AArch64 and RISC-V 64 are implemented.
ABI compatibility translates operating-system interfaces over shared kernel
objects, not CPU instructions between architectures.

## Build boundaries

The tracked reference projects are
[AArch64 full](../../projects/aarch64-limine-full/scarlet.toml),
[RISC-V full](../../projects/riscv64-limine-full/scarlet.toml), and
[AArch64 microvm](../../projects/aarch64-limine-microvm/scarlet.toml).
Each owns a `bsp/` Cargo package, target configuration, linker script, image
recipe, and runner. `cargo-scarlet` from the separate `scarlet-sdk` repository
generates `.scarlet/scarlet-modules`, which connects the kernel and enabled
static modules to that BSP.

From the Scarlet root in the Nix development environment:

```sh
# Kernel/BSP only; this does not compose all userspace layers.
cargo scarlet build --project projects/aarch64-limine-full
cargo scarlet build --project projects/riscv64-limine-full

# Complete project image, including the selected userland layers.
cargo scarlet image --project projects/aarch64-limine-full
```

`kernel/targets/*.json` are bare-metal kernel targets. Normal Scarlet user
programs instead use the std-capable Scarlet targets described in the
[userspace guide](../userspace/README.md). Do not infer that all Scarlet
programs are `no_std` from the kernel's build configuration.

The feature defaults are in `kernel/Cargo.toml`; the actual project selection
is in `[bsp.kernel].features`. The reference AArch64 full project explicitly
disables `hypervisor`, while the microvm project enables it. `profiler`,
`sync-debug`, and `aarch64-linux-boot` are separate opt-ins. A compiled driver or
feature is not evidence that a particular board was validated.

## Source map

| Responsibility | Implementation |
| --- | --- |
| Common boot handoff and entry | [BootInfo, start_kernel, start_ap](../../kernel/src/lib.rs) |
| Boot protocol responses | [boot](../../kernel/src/boot/mod.rs) and architecture boot adapters |
| CPU, traps, context, MMU, timer, virtualization | [arch](../../kernel/src/arch/mod.rs) |
| Physical pages and heap | [mem](../../kernel/src/mem/mod.rs) |
| Address spaces, direct maps, mappings, MMIO | [vm](../../kernel/src/vm/mod.rs) |
| Task identity, execution state, ELF loading | [task](../../kernel/src/task/mod.rs) and [executor](../../kernel/src/executor/mod.rs) |
| Run queues, placement, accounting | [sched](../../kernel/src/sched/mod.rs) |
| Handles and resource capabilities | [object](../../kernel/src/object/mod.rs) |
| ABI selection and syscall behavior | [abi](../../kernel/src/abi/mod.rs) and [syscall](../../kernel/src/syscall/mod.rs) |
| Filesystems and mount namespaces | [VFS v2](../../kernel/src/fs/vfs_v2/mod.rs) |
| Device interfaces and discovery | [device](../../kernel/src/device/mod.rs) |
| Hardware implementations and registration | [drivers](../../kernel/src/drivers/mod.rs), [initcall](../../kernel/src/initcall/mod.rs) |
| Synchronization and interrupts | [sync](../../kernel/src/sync/mod.rs), [interrupt](../../kernel/src/interrupt/mod.rs) |
| Loadable modules | [lsm](../../kernel/src/lsm/mod.rs) |
| Optional SHV | [hypervisor](../../kernel/src/hypervisor/mod.rs) |

Common subsystems should call the unified `crate::arch` interface. Keep new
architecture-specific implementation in the appropriate arch module instead
of adding conditional branches throughout common code. See
[multi-architecture development](../architecture/multi-architecture.md).

## Boot and initial userspace

1. The BSP enters its architecture's Limine adapter. Limine responses and the
   device tree are converted into Scarlet's `BootInfo`; Limine does not supply
   that Rust structure directly.
2. `start_kernel` initializes the PMM from **all** `usable_memory_regions`.
   The single `usable_memory_paddr` remains a primary boot scratch region, not
   the complete firmware RAM inventory.
3. Scarlet installs its own boot page table, fixes pointers after the HHDM
   transition, establishes the fixed heap mapping, and initializes the heap.
4. Early/driver initcalls run, then the runtime kernel VM and LSM symbol table
   are established. Init is registered before driver workers can take PID 1.
5. Critical interrupt controllers are discovered and initialized before other
   platform/PCI devices and graphics. The network manager is prepared before
   network-device probing. Remaining initcalls, CPU interrupts, timer, and
   available wall-clock setup follow.
6. VFS/initramfs, network command-line configuration, and optional hypervisor
   setup are prepared.
   `TransparentExecutor` loads `/init` (or the `init=` override) into the reserved
   bootstrap task.
7. On successful loading, init is enqueued. The boot CPU claims its first
   runnable task before the boot hook releases secondary CPUs, then enters the
   selected task. `start_ap` performs per-CPU setup for secondary processors.

The default [init program](../../user/bin/src/init.rs) mounts the selected
root filesystem, constructs a sealed [Environment](../abi/execution-environments.md),
and execs `/bin/stemd` in its native view; the microvm project replaces init
with `microvm-init`. Root-device policy and service startup are userspace work,
not the kernel bootloader. See [Limine boot](../boot/limine.md) and
[userspace startup](../userspace/README.md#startup-and-services).

## Memory, ownership, and drivers

The HHDM maps selected physical regions, not every possible physical address.
Use the phase-aware helpers in `vm::addr`; device MMIO uses `vm::ioremap`.
Consult the [memory map](../architecture/memory-map.md) before adding DMA,
framebuffer, or boot-time pointer conversions.

Kernel synchronization is provided by `crate::sync`, including IRQ-aware
locks. Choose the primitive for the interrupt/scheduling context; a lock
protecting CPU access does not by itself establish DMA completion or device
resource lifetime. The [GPU completion notes](../graphics/gpu-async-submission.md)
separate submission, completion, and presentation ownership.

Static external modules are ordinary `no_std` library crates selected by
`[modules]`, with a `force_link()` entry and initcall registration. The existing
[prototype](../../modules/scarlet-module-prototype/src/lib.rs) is the minimal
example. LSMs instead use `cargo scarlet new --lsm` / `build --lsm` and are
loaded as `.lsm` objects; see [LSM](../modules/lsm.md). Neither path promises
binary compatibility with arbitrary kernel/toolchain revisions.

## Tests and further reading

Kernel tests use `#[test_case]` and run in QEMU through `cargo make test-riscv64`
and `cargo make test-aarch64`. They are not host unit tests. Kernel Rustdoc
currently has doctests disabled in its Cargo manifest; a rendered API page is
not evidence that its examples ran.

For specific subsystems, start with [scheduler](../architecture/scheduler.md),
[Linux ABI status](../abi/linux/status.md), [network](../network/architecture.md),
[namespaces](../container/namespace-isolation.md), and
[SHV](../hypervisor/README.md). Dated benchmarks, board investigations, and
release baselines record their own tested revisions; do not treat them as
current universal support claims.
