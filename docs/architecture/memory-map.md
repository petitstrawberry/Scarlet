# Virtual Memory Map

This document describes the virtual memory address space layout for each supported architecture in Scarlet OS. The design follows a **Higher Half Kernel** architecture with **HHDM (Higher Half Direct Mapping)** for efficient physical memory access.

## Design Principles

### Higher Half Kernel
- The kernel resides in the upper half of the virtual address space
- User processes occupy the lower half
- Clear separation improves security and simplifies memory management

### HHDM (Higher Half Direct Mapping)
- All physical memory is directly mapped into the kernel's virtual address space
- Enables fast physical-to-virtual address translation via simple offset arithmetic
- Simplifies kernel code that needs to access arbitrary physical memory

## Address Space Layout

### RISC-V 64-bit (Sv48)

Sv48 provides a 48-bit virtual address space with 4-level page tables.

```
                            Sv48 Virtual Address Space
┌─────────────────────────────────────────────────────────────────────────┐
│ Address                                                                 │
│                                                                         │
│ 0xFFFF_FFFF_FFFF_FFFF ─┬───────────────────────────────────────────────┤
│                        │                                                │
│                        │              Kernel Image                      │
│                        │         (Code, RO-data, RW-data, BSS)          │
│                        │                                                │
│ 0xFFFFFFFF_80000000 ───┼─────────────┬──────────────────────────────────┤ ← KERNEL_BASE (linker)
│                        │  (reserved) │                                  │
│                        │             │                                  │
│ 0xFFFF_C000_00000000 ──┼─────────────┼──────────────────────────────────┤
│                        │  (gap)      │                                  │
│                        │             │                                  │
│ 0xFFFF_BFFF_FFFF_FFFF ─┼─────────────┼─┬────────────────────────────────┤ ← HHDM_END
│                        │             │ │                                │
│                        │   Kernel    │ │       HHDM Region (64 TB)      │
│                        │   Space     │ │   Direct Physical Memory Map   │
│                        │             │ │   VA = PA + HHDM_OFFSET        │
│                        │             │ │                                │
│ 0xFFFF_8000_0000_0000 ─┼─────────────┼─┴────────────────────────────────┤ ← HHDM_START (= HHDM_OFFSET)
│                        │             │                                  │
│                        │  (gap)      │     (Upper Half)                 │
│                        │             │                                  │
│ 0x0000_8000_0000_0000 ─┼─────────────┼──────────────────────────────────┤
│                        │             │                                  │
│                        │  (hole)     │     Non-canonical                │
│                        │             │     (Invalid addresses)          │
│                        │             │                                  │
│ 0x0000_7FFF_FFFF_FFFF ─┼─────────────┼──────────────────────────────────┤ ← USER_SPACE_END
│                        │             │                                  │
│                        │   User      │       User Space (128 TB)        │
│                        │   Space     │       (Lower Half)               │
│                        │             │                                  │
│ 0x0000_0000_0000_0000 ─┴─────────────┴──────────────────────────────────┤
└─────────────────────────────────────────────────────────────────────────┘
```

#### Memory Regions

| Region | Start Address | End Address | Size | Description |
|--------|---------------|-------------|------|-------------|
| User Space | `0x0000_0000_0000_0000` | `0x0000_7FFF_FFFF_FFFF` | 128 TB | User process virtual memory |
| Non-canonical | `0x0000_8000_0000_0000` | `0xFFFF_7FFF_FFFF_FFFF` | - | Invalid (hole) |
| Kernel Space | `0xFFFF_8000_0000_0000` | `0xFFFF_FFFF_FFFF_FFFF` | 128 TB | Entire upper half (HHDM + Kernel Image) |
| ├─ HHDM | `0xFFFF_8000_0000_0000` | `0xFFFF_BFFF_FFFF_FFFF` | 64 TB | Direct physical memory mapping |
| └─ Kernel Image | `0xFFFFFFFF_80000000` | `0xFFFF_FFFF_FFFF_FFFF` | ~2 GB | Kernel code, data, heap, stacks |

#### Constants

```rust
// RISC-V Sv48 — source: kernel/src/environment/common.rs
pub const SCARLET_HHDM_BASE: usize = 0xFFFF_8000_0000_0000;  // HHDM direct map
pub const IOREMAP_START: usize   = 0xFFFF_C000_0000_0000;    // Dynamic MMIO mapping
pub const IOREMAP_END: usize     = 0xFFFF_C000_3FFF_FFFF;    // 1 GiB
pub const KERNEL_HEAP_BASE: usize = 0xFFFF_D000_0000_0000;   // Kernel heap (512 MiB)
// KERNEL_BASE (linker): 0xFFFF_FFFF_8000_0000 (from projects/riscv64-limine-full/lds/)
```

### AArch64

AArch64 uses a 48-bit (or 52-bit with LVA) virtual address space with up to 4-level page tables (with 4KB granule).

```
                          AArch64 Virtual Address Space
┌─────────────────────────────────────────────────────────────────────────┐
│ Address                                                                 │
│                                                                         │
│ 0xFFFF_FFFF_FFFF_FFFF ─┬───────────────────────────────────────────────┤
│                        │                                                │
│                        │              Kernel Image                      │
│                        │         (Code, RO-data, RW-data, BSS)          │
│                        │                                                │
│ 0xFFFFFFFF_80000000 ──┼─────────────┬──────────────────────────────────┤ ← KERNEL_BASE (linker)
│                        │  (reserved) │                                  │
│                        │             │                                  │
│ 0xFFFF_4000_00000000 ──┼─────────────┼──────────────────────────────────┤
│                        │  (gap)      │                                  │
│                        │             │                                  │
│ 0xFFFF_BFFF_FFFF_FFFF ─┼─────────────┼─┬────────────────────────────────┤ ← HHDM_END
│                        │             │ │                                │
│                        │   Kernel    │ │       HHDM Region (64 TB)      │
│                        │   Space     │ │   Direct Physical Memory Map   │
│                        │             │ │   VA = PA + HHDM_OFFSET        │
│                        │             │ │                                │
│ 0xFFFF_8000_0000_0000 ─┼─────────────┼─┴────────────────────────────────┤ ← HHDM_START (= HHDM_OFFSET)
│                        │             │                                  │
│                        │  (gap)      │     (Upper Half)                 │
│                        │             │                                  │
│ 0x0001_0000_0000_0000 ─┼─────────────┼──────────────────────────────────┤
│                        │             │                                  │
│                        │  (hole)     │     Non-canonical                │
│                        │             │     (Invalid addresses)          │
│                        │             │                                  │
│ 0x0000_FFFF_FFFF_FFFF ─┼─────────────┼──────────────────────────────────┤ ← USER_SPACE_END
│                        │             │                                  │
│                        │   User      │       User Space (256 TB)        │
│                        │   Space     │       (Lower Half)               │
│                        │             │                                  │
│ 0x0000_0000_0000_0000 ─┴─────────────┴──────────────────────────────────┤
└─────────────────────────────────────────────────────────────────────────┘
```

#### Memory Regions

| Region | Start Address | End Address | Size | Description |
|--------|---------------|-------------|------|-------------|
| User Space | `0x0000_0000_0000_0000` | `0x0000_FFFF_FFFF_FFFF` | 256 TB | User process virtual memory |
| Non-canonical | `0x0001_0000_0000_0000` | `0xFFFF_7FFF_FFFF_FFFF` | - | Invalid (hole) |
| Kernel Space | `0xFFFF_8000_0000_0000` | `0xFFFF_FFFF_FFFF_FFFF` | 128 TB | Entire upper half (HHDM + Kernel Image) |
| ├─ HHDM | `0xFFFF_8000_0000_0000` | `0xFFFF_BFFF_FFFF_FFFF` | 64 TB | Direct physical memory mapping |
| └─ Kernel Image | `0xFFFFFFFF_80000000` | `0xFFFF_FFFF_FFFF_FFFF` | ~2 GB | Kernel code, data, heap, stacks |

#### Constants

```rust
// AArch64 — source: kernel/src/environment/common.rs
pub const SCARLET_HHDM_BASE: usize = 0xFFFF_8000_0000_0000;  // HHDM direct map
pub const IOREMAP_START: usize   = 0xFFFF_C000_0000_0000;    // Dynamic MMIO mapping
pub const IOREMAP_END: usize     = 0xFFFF_C000_3FFF_FFFF;    // 1 GiB
pub const KERNEL_HEAP_BASE: usize = 0xFFFF_D000_0000_0000;   // Kernel heap (512 MiB)
// KERNEL_BASE (linker): 0xFFFF_FFFF_8000_0000 (from projects/aarch64-limine-full/lds/)
```

## Address Translation Functions

The kernel provides address translation through a layout-aware system defined in
`kernel/src/vm/addr.rs`. Rather than using a single fixed offset, the kernel
tracks both the bootloader-provided HHDM offset and the runtime HHDM offset
(`SCARLET_HHDM_BASE`), switching between them during boot page-table
initialization.

```rust
/// Convert virtual address to physical address (runtime layout).
/// Panics if the address is not in the direct-mapped region.
pub fn virt_to_phys(vaddr: usize) -> usize;

/// Convert physical address to virtual address (runtime layout).
/// Uses the current HHDM offset (SCARLET_HHDM_BASE after boot).
pub fn phys_to_virt(paddr: usize) -> usize;

/// Convert a boot-time virtual address to physical address.
/// Uses the bootloader-provided layout (early boot only).
pub fn boot_virt_to_phys(vaddr: usize) -> usize;
```

After the kernel transitions to its own page tables, `phys_to_virt(paddr)`
returns `paddr + SCARLET_HHDM_BASE`. The layout system handles the transition
transparently, so driver code does not need to know which phase it is in.

### Usage Guidelines

| Scenario | Function | Example |
|----------|----------|---------|
| Store heap pointer in `pmarea` | `virt_to_phys()` | `pmarea.start = virt_to_phys(pages as usize)` |
| Access physical memory via pointer | `phys_to_virt()` | `ptr = phys_to_virt(paddr) as *mut u8` |
| Device DMA address | Already PA | `desc.addr = translate_vaddr(vaddr)` (returns PA) |
| Free raw pages | `phys_to_virt()` | `free_raw_pages(phys_to_virt(paddr), n)` |

## IOREMAP Region

The IOREMAP region provides virtual address space for dynamically mapping physical device MMIO regions at runtime.  Unlike the legacy approach of statically identity-mapping device memory (VA == PA) at boot, drivers now call `ioremap(paddr, size)` to obtain a kernel virtual address for their MMIO registers.  `iounmap(vaddr)` releases the mapping when the device is removed.

```
          IOREMAP Region (both RISC-V and AArch64)

  0xFFFF_C000_3FFF_FFFF ──── IOREMAP_END
                              │
                              │  IOREMAP Region (1 GiB)
                              │  Dynamically-allocated device MMIO mappings
                              │  (Linux-style ioremap)
                              │
  0xFFFF_C000_0000_0000 ──── IOREMAP_START
```

### Constants

```rust
pub const IOREMAP_START: usize = 0xFFFF_C000_0000_0000;
pub const IOREMAP_END:   usize = 0xFFFF_C000_3FFF_FFFF; // 1 GiB
```

These constants are shared between RISC-V and AArch64 since the region falls in the same canonical gap between `HHDM_END` (`0xFFFF_BFFF_FFFF_FFFF`) and the kernel image (`0xFFFF_FFFF_8000_0000`).

### API

```rust
/// Map a physical MMIO region into the kernel virtual address space.
pub fn ioremap(paddr: usize, size: usize) -> Result<usize, &'static str>;

/// Unmap a previously ioremap'd region.
pub fn iounmap(vaddr: usize);
```

### Design

- The allocator uses a bump pointer with a first-fit free list for reuse.
- `ioremap_init()` is called once at the end of `kernel_vm_init()`.
- Each mapping is registered in the kernel `VirtualMemoryManager` so that
  `translate_to_phys()` and page-fault handlers see it correctly.
- Page table entries are installed (and TLB flushed) via the normal `map_memory_area` path.

## References

- RISC-V Privileged Architecture (Sv48): https://riscv.org/technical/specifications/
- ARM Architecture Reference Manual (AArch64)
- Linux kernel memory layout documentation
