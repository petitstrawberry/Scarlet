# Kernel virtual memory layout

This page describes Scarlet's implemented layout, reviewed on 2026-09-06.
It is not a table of every address an ISA could support. The source of truth
is [environment](../../kernel/src/environment.rs), the
[address helpers](../../kernel/src/vm/addr.rs), and the selected BSP linker
script. Both current ports use 4 KiB pages.

## Common kernel regions

The reference BSPs link the kernel in the higher half. Code, heap, direct-map
aliases, MMIO mappings, and stack infrastructure are distinct mappings.

| Region | Current virtual base / reservation | Meaning |
| --- | --- | --- |
| Scarlet HHDM | `0xffff_8000_0000_0000` | Fixed offset for the selected physical direct-map regions; **not** a populated 64 TiB mapping |
| IOREMAP | `0xffff_c000_0000_0000..=0xffff_c000_3fff_ffff` | 1 GiB virtual window for dynamic physical mappings |
| Kernel heap | `0xffff_d000_0000_0000`, initial size 512 MiB | Dedicated fixed-VA heap mapping, separate from its HHDM alias |
| Kernel image | `0xffff_ffff_8000_0000` in the reference linker scripts | Linker-defined text, read-only data, data, and BSS; not the entire region up to the last VA |
| Trampoline / kernel stacks | Upper-VA windows derived from arch constants | Trampoline reserve, kernel VM stack, and per-task kernel stack slots |

These shared constants live in
[environment/common.rs](../../kernel/src/environment/common.rs).
Image extents are defined by the
[RISC-V](../../projects/riscv64-limine-full/bsp/lds/riscv64_limine.ld) and
[AArch64](../../projects/aarch64-limine-full/bsp/lds/aarch64_limine.ld)
BSP linker scripts. A reserved VA range does not imply that every page in it
is currently backed.

## Sparse direct mapping

[DirectMapRegions](../../kernel/src/vm/direct_map.rs) records the physical
regions belonging to the runtime HHDM. The boot path derives them from the
memory inventory and required kernel mappings. A bounding interval around
those regions can contain holes; it is not evidence that those holes are RAM,
mapped, or safe to dereference.

For a physical address that belongs to the runtime direct map,
`VA = SCARLET_HHDM_BASE + PA`. Use `phys_to_virt` to check membership and
perform the conversion. Adding the constant manually bypasses that check.
In particular:

- MMIO is not ordinary directly mapped RAM; use `ioremap`.
- A boot module outside the runtime direct map needs an explicit mapping.
  `BootInfo::get_initramfs_vaddr` handles this with a Normal-memory mapping
  without enlarging the HHDM's region set.
- A virtual pointer, physical address, and device DMA address are different
  values. Address conversion alone does not establish contiguity, ownership,
  IOMMU translation, cache coherence, or completion.

## Boot addressing phases

The layout state in `vm::addr` progresses through
`Uninitialized → Bootloader → BootKernel → Runtime`.

During the Limine handoff, boot pointers use the loader's HHDM offset and
recorded bounds. The boot page-table setup installs Scarlet's fixed HHDM and
heap mapping, records sparse direct-map membership, and transitions the layout.
Runtime kernel VM initialization completes that setup. A pointer borrowed from
the loader must not be assumed to keep the same virtual address across the
switch. See [boot](../boot/limine.md) and
[start_kernel](../../kernel/src/lib.rs).

| Helper | Intended use and limits |
| --- | --- |
| `boot_phys_to_virt`, `boot_virt_to_phys` | Explicit bootloader-layout conversions for early handoff data; not runtime aliases |
| `phys_to_virt` / `phys_to_kernel_virt` | Current-layout direct-map conversion; panics when the PA is outside the recorded mapping |
| `virt_to_phys` / `kernel_virt_to_phys` | Current-layout conversion for recognized kernel image, heap, or direct-map addresses; panics for unrecognized addresses |
| `phys_to_kernel_image_virt` | Kernel-image alias only, not a general physical-memory mapping |

These helpers use recorded layout information; `virt_to_phys` is not a
general page-table walk for arbitrary process or IOREMAP pointers. For other
mappings use the owning VM's translation API and handle translation failure.

## Architecture-specific stacks and user addresses

Do not describe the current implementation as “all user mappings are in the
lower half” on both architectures. Its existing stack policy differs:

| Constant | RISC-V | AArch64 |
| --- | --- | --- |
| `VMMAX` | `0xffff_ffff_ffff_ffff` | `0x0000_7fff_ffff_ffff` |
| `TRAMPOLINE_VA_RESERVE` | One page (4 KiB) | 64 KiB |
| `USER_STACK_END` (exclusive) | `0xffff_ffff_ffff_f000` | `0x0000_7fff_ffff_0000` |

RISC-V retains a high-VA user-stack convention below the trampoline. AArch64
places the user stack below its lower-range `VMMAX`, while kernel stack and
trampoline infrastructure remain in the upper range. Page-table selection and
permissions, not the phrase “higher half” alone, determine accessibility.

[environment/riscv64.rs](../../kernel/src/environment/riscv64.rs) and
[environment/aarch64.rs](../../kernel/src/environment/aarch64.rs) define these
values. [environment.rs](../../kernel/src/environment.rs) derives the shared
kernel-stack windows. This document records the policy; it does not change it
or promise a fixed application address layout.

## Dynamic physical mappings

[vm::ioremap](../../kernel/src/vm/ioremap.rs) provides:

```rust
pub fn ioremap(paddr: usize, size: usize) -> Result<usize, &'static str>;
pub fn memremap_normal(paddr: usize, size: usize) -> Result<usize, &'static str>;
pub fn iounmap(vaddr: usize);
```

Use `ioremap` for device MMIO and `memremap_normal` for an explicit
Normal-memory mapping outside the direct map. Both allocate in the IOREMAP
window, register the mapping with the kernel VM, and install page-table entries
with the appropriate attributes. `iounmap` releases the mapping, not the
underlying physical resource or a device's outstanding work.

Initialization follows `kernel_vm_init`. The allocator uses a bump position
and reuses released ranges; mappings participate in the normal VM/TLB path.
Drivers must still retain mappings and physical resources until all users,
including asynchronous device operations, have finished.
