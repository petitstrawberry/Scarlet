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
│ 0xFFFFFFC0_80000000 ───┼─────────────┬──────────────────────────────────┤ ← KERNEL_BASE
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
| └─ Kernel Image | `0xFFFFFFC0_80000000` | `0xFFFF_FFFF_FFFF_FFFF` | ~1 TB | Kernel code, data, heap, stacks |

#### Constants

```rust
// RISC-V Sv48
pub const HHDM_OFFSET: usize        = 0xFFFF_8000_0000_0000;
pub const HHDM_START: usize         = 0xFFFF_8000_0000_0000;
pub const HHDM_END: usize           = 0xFFFF_BFFF_FFFF_FFFF;  // 64 TB max
pub const KERNEL_BASE: usize        = 0xFFFFFFC0_80000000;    // Link address
pub const USER_SPACE_END: usize     = 0x0000_7FFF_FFFF_FFFF;
pub const KERNEL_SPACE_START: usize = 0xFFFF_8000_0000_0000;
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
│ 0xFFFF_0000_80000000 ──┼─────────────┬──────────────────────────────────┤ ← KERNEL_BASE
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
| Kernel Space | `0xFFFF_0000_0000_0000` | `0xFFFF_FFFF_FFFF_FFFF` | 128 TB | Entire upper half (HHDM + Kernel Image) |
| ├─ HHDM | `0xFFFF_8000_0000_0000` | `0xFFFF_BFFF_FFFF_FFFF` | 64 TB | Direct physical memory mapping |
| └─ Kernel Image | `0xFFFF_0000_80000000` | `0xFFFF_7FFF_FFFF_FFFF` | ~128 TB | Kernel code, data, heap, stacks |

#### Constants

```rust
// AArch64
pub const HHDM_OFFSET: usize        = 0xFFFF_8000_0000_0000;
pub const HHDM_START: usize         = 0xFFFF_8000_0000_0000;
pub const HHDM_END: usize           = 0xFFFF_BFFF_FFFF_FFFF;  // 64 TB max
pub const KERNEL_BASE: usize        = 0xFFFF_0000_80000000;    // Link address
pub const USER_SPACE_END: usize     = 0x0000_FFFF_FFFF_FFFF;
pub const KERNEL_SPACE_START: usize = 0xFFFF_0000_0000_0000;
```

## Address Translation Functions

The kernel provides two core functions for address translation:

```rust
/// Convert virtual address to physical address
/// For addresses within HHDM: PA = VA - HHDM_OFFSET
pub const fn virt_to_phys(vaddr: usize) -> usize {
    vaddr - HHDM_OFFSET
}

/// Convert physical address to virtual address
/// Uses HHDM: VA = PA + HHDM_OFFSET
pub const fn phys_to_virt(paddr: usize) -> usize {
    paddr + HHDM_OFFSET
}
```

### Usage Guidelines

| Scenario | Function | Example |
|----------|----------|---------|
| Store heap pointer in `pmarea` | `virt_to_phys()` | `pmarea.start = virt_to_phys(pages as usize)` |
| Access physical memory via pointer | `phys_to_virt()` | `ptr = phys_to_virt(paddr) as *mut u8` |
| Device DMA address | Already PA | `desc.addr = translate_vaddr(vaddr)` (returns PA) |
| Free raw pages | `phys_to_virt()` | `free_raw_pages(phys_to_virt(paddr), n)` |

## Transition Plan

### Current State
- Identity mapping: `VA == PA`
- `virt_to_phys()` and `phys_to_virt()` are identity functions
- All conversion calls are in place

### Target State
- Higher Half Kernel with HHDM
- Kernel linked at `KERNEL_BASE` (VMA) but loaded at physical address (LMA)
- Boot code establishes HHDM mapping before jumping to kernel proper
- `virt_to_phys()` and `phys_to_virt()` apply `HHDM_OFFSET`

### Migration Steps

1. **Update linker scripts** - Set kernel VMA to higher half address
2. **Update `addr.rs`** - Add `HHDM_OFFSET` constant and implement real translation
3. **Update boot code** - Create early page tables with HHDM mapping
4. **Update `kernel_vm_init()`** - Set `vmarea != pmarea` using HHDM offset
5. **Test thoroughly** - Verify all 527+ tests still pass

## References

- RISC-V Privileged Architecture (Sv48): https://riscv.org/technical/specifications/
- ARM Architecture Reference Manual (AArch64)
- Linux kernel memory layout documentation
