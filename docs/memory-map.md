# Scarlet Memory Map & User/Kernel Separation

This document reflects the **current implementation** for RISC-V, AArch64, and x86_64.
It describes the **actual** address-space strategy in code, not general OS theory.

---

## 1. Core Design (Implementation Reality)

Scarlet **separates user space and kernel space** by using **different page tables**.

- **Kernel page table**: kernel image + device MMIO + trampoline + kernel stack windows
- **User page table**: user image + user stack + guard page

The **trampoline** is the only controlled bridge during user↔kernel transitions.

---

## 2. Global Layout Concepts

```
Kernel page table (shared)          User page table (per-task)
=========================          ==========================

Kernel image (VA = PA)              User image (VA from user LD)
Device MMIO                         User heap/text/data
Trampoline (high VA)                User stack (near USER_STACK_END)
Per-task kernel stack windows       Guard page
```

---

## 3. RISC-V (Sv48)

### Kernel Page Table

Kernel is linked at **physical addresses** and mapped **VA=PA** in the kernel page table.
The trampoline is mapped to the **top of the virtual address space**.

```
RISC-V Kernel VA (shared PT)
============================

0xffff_ffff_ffff_ffff  ┌────────────────────────────────┐  TRAMPOLINE_VA_END
                       │ Trampoline (shared)            │
0xffff_ffff_ffff_f000  ├────────────────────────────────┤
                       │ (gap - user stack in user PT)  │
0xffff_ffff_ffff_0000  ├────────────────────────────────┤  KERNEL_VM_STACK_START
                       │ Kernel VM stack                │
0xffff_ffff_fffe_ffff  ├────────────────────────────────┤  KERNEL_KSTACK_REGION_END
                       │ Kernel kstack windows          │
0xffff_ffff_ffe0_0000  ├────────────────────────────────┤  KERNEL_KSTACK_REGION_START (approx)
                       │ (gap)                          │
                       ├────────────────────────────────┤
                       │ DRAM (VA=PA, device-dependent) │  DRAM_END
                       │                                │
                       │ Kernel image (VA=PA)           │  __KERNEL_SPACE_END
                       │   .init .text .rodata .data    │
                       │   .bss .trampoline (phys)      │
0x????_????_????_????  ├────────────────────────────────┤  __KERNEL_SPACE_START
                       │ (gap - bootloader/FDT)         │
0x????_????_????_????  └────────────────────────────────┘  DRAM_START
```

### User Page Table

User programs are linked from **0x0** (see `user/lds/user.ld`).
User stack is placed near **USER_STACK_END** (top of user VA range).
**Trampoline is shared** between user and kernel page tables.

```
RISC-V User VA (per-task PT)
============================

0xffff_ffff_ffff_ffff  ┌────────────────────────────────┐  TRAMPOLINE_VA_END
                       │ Trampoline (shared)            │
0xffff_ffff_ffff_f000  ├────────────────────────────────┤  USER_STACK_END
                       │ User stack                     │
                       │ Guard page                     │
                       ├────────────────────────────────┤
                       │ (gap)                          │
                       ├────────────────────────────────┤
                       │ User heap / text / data        │
0x0000_0000_0000_0000  └────────────────────────────────┘
```

**Switching:** RISC-V trap entry swaps `satp` inside the trampoline.

---

## 4. AArch64 (48-bit, TTBR0/TTBR1 split)

### Kernel Page Table (TTBR1)

Kernel is linked at **physical addresses** and mapped **VA=PA** in the kernel page table.
Trampoline and kernel stack windows live in the **high VA** region managed by TTBR1.

```
AArch64 Kernel VA (TTBR1)
=========================

0xffff_ffff_ffff_ffff  ┌────────────────────────────────┐  TRAMPOLINE_VA_END
                       │ Trampoline                     │
0xffff_ffff_fffe_ffff  ├────────────────────────────────┤
                       │ Kernel kstack windows          │
0xffff_ffff_ffe0_ffff  ├────────────────────────────────┤  KERNEL_KSTACK_REGION_START (approx)
                       │ (gap)                          │
                       ├────────────────────────────────┤
                       │ DRAM (VA=PA, device-dependent) │  DRAM_END
                       │                                │
                       │ Kernel image (VA=PA)           │  __KERNEL_SPACE_END
                       │   .head .init .text .rodata    │
                       │   .data .bss .trampoline (phys)│
0x????_????_????_????  ├────────────────────────────────┤  __KERNEL_SPACE_START
                       │ (gap - bootloader/DTB)         │
0x????_????_????_????  └────────────────────────────────┘  DRAM_START
```

### User Page Table (TTBR0)

User programs are linked from **0x0000_0000_0001_0000**
(`user/lds/user_aarch64.ld`). User stack is near `USER_STACK_END`
within the **lower canonical** range.

**Note:** On AArch64, trampoline is NOT mapped in user TTBR0.
Traps from EL0 automatically enter EL1 and use TTBR1 (kernel) space.

```
AArch64 User VA (TTBR0, per-task)
=================================

0x0000_7fff_ffff_ffff  ┌────────────────────────────────┐  VMMAX (lower canonical end)
                       │ (gap)                          │
0x0000_7fff_ffff_0000  ├────────────────────────────────┤  USER_STACK_END
                       │ User stack                     │
                       │ Guard page                     │
                       ├────────────────────────────────┤
                       │ (gap)                          │
                       ├────────────────────────────────┤
                       │ User heap / text / data        │
0x0000_0000_0001_0000  └────────────────────────────────┘
```

**Switching:** traps enter EL1 and use TTBR1; TTBR0 is per-task user space.

---

## 5. x86_64 (Limine)

### Kernel Page Table (Kernel PML4)

Kernel is linked at **higher half** (`0xffffffff80000000`).
HHDM (Higher Half Direct Map) provides direct access to all physical memory.

```
x86_64 Kernel VA (Kernel PML4)
==============================

0xffff_ffff_ffff_ffff  ┌────────────────────────────────┐  TRAMPOLINE_VA_END
                       │ Trampoline (shared)            │
0xffff_ffff_ffff_f000  ├────────────────────────────────┤
                       │ Kernel kstack windows          │
0xffff_ffff_fffe_ffff  ├────────────────────────────────┤  KERNEL_KSTACK_REGION_END (approx)
                       │ (gap)                          │
                       ├────────────────────────────────┤
                       │ Kernel image (Higher Half)     │
                       │   .init .text .rodata .data    │
                       │   .bss .trampoline             │
0xffff_ffff_8000_0000  ├────────────────────────────────┤  KERNEL_BASE (-2GB start)
                       │                                │
                       │ (HUGE GAP - approx 128TB)      │
                       │                                │
                       ├────────────────────────────────┤  HHDM_END (HHDM_START + RAM size)
                       │ HHDM (All Physical Memory)     │
0xffff_8000_0000_0000  └────────────────────────────────┘  HHDM_START (from Limine)
```

### User Page Table (Per-task PML4)

User programs are linked from **0x400000** (typical base).
User stack is placed in **lower canonical** range (same as AArch64 pattern).
**Trampoline is shared** between user and kernel page tables.

```
x86_64 User VA (Per-task PML4)
==============================

0xffff_ffff_ffff_ffff  ┌────────────────────────────────┐  TRAMPOLINE_VA_END
                       │ Trampoline (shared)            │
0xffff_ffff_ffff_f000  ├────────────────────────────────┤  USER_STACK_END
                       │ (gap - unmapped kernel space)  │
0x0000_7fff_ffff_ffff  ├────────────────────────────────┤  VMMAX (lower canonical end)
                       │ User stack                     │
                       │ Guard page                     │
                       ├────────────────────────────────┤
                       │ (gap)                          │
                       ├────────────────────────────────┤
                       │ User heap / text / data        │
0x0000_0000_0040_0000  └────────────────────────────────┘  User Image Base (Typical)
```

**Switching:** x86_64 uses IDT for traps; `cr3` switch in trampoline; `iretq` returns to user mode.

---

## 6. Trampoline & Transition Flow

```
User trap/irq
    ↓
_user_trap_entry  (in .trampoline.text)
    ↓  switch page table (satp/ttbr/cr3)
    ↓  switch to kernel stack
Kernel handler
    ↓
_user_trap_exit   (in .trampoline.text)
    ↓  restore user context
    ↓  switch back to user page table
User resumes
```

The per-CPU structs stored in `.trampoline.data` provide:

- Kernel stack pointer
- Kernel trap handler
- Current user page table (satp/ttbr/cr3)

---

## 7. Invariants

- User and kernel **never share the same page table**
- Kernel image is mapped **only** in kernel page table
- User memory is mapped **only** in user page table
- Trampoline is the **only** cross-domain execution entry
- **RISC-V / x86_64:** Trampoline is mapped in **both** user and kernel page tables
- **AArch64:** Trampoline is mapped **only** in kernel (TTBR1); traps automatically switch to EL1
