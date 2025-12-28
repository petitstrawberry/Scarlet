# AArch64 Bring-up Plan (QEMU virt)

## Goals
- Boot AArch64 kernel on QEMU virt via U-Boot + initramfs.
- Enter EL0 and successfully execute the first userspace program (`/system/scarlet/bin/init`).
- Keep shared code (scheduler, core task model, ABIs) architecture-agnostic; implement AArch64 specifics under `kernel/src/arch/aarch64/`.

## Current Status (as of 2025-12-28)
- Kernel boots, mounts initramfs, loads the AArch64 init ELF into a task.
- First schedule happens and control transfers to the trampoline.
- `arch_switch_to_user_space` now shows `epc=0x10000` (expected), but no userspace output is observed yet.

## Scope & Constraints
- Prefer changes limited to `kernel/src/arch/aarch64/**` for AArch64 bring-up fixes.
- No new “nice-to-have” features; focus on minimum pieces needed to run init.
- Use `cargo make` flows (ideally inside `scarlet-dev`) for build/run/test.

## Key Missing Pieces (Likely)
This is an intentionally practical checklist ordered by “unblocks next observation”.

### 1) Userspace entry actually executes (EL0 instruction fetch works)
- Verify EL0 page permissions and execute permissions for the init text segment.
- Confirm TTBR0_EL1 points to the task page table when returning to EL0.
- Confirm the trampoline swaps TTBR0_EL1 correctly and does not corrupt it.
- Confirm `SP_EL0` points to mapped user stack memory.

### 2) Exception visibility: always log ESR/ELR/FAR on early failures
- Ensure the AArch64 trap entry routes all EL0 exceptions to a handler that:
  - prints `EC` (ESR_EL1[31:26])
  - prints `ESR_EL1`, `ELR_EL1`, `FAR_EL1`, `SPSR_EL1`
  - prints a small summary of `Trapframe` registers (x0/x1/x8/sp/epc)
- Keep logging bounded (budget counter) so repeated faults do not spam forever.

### 3) AArch64 syscall plumbing (EL0 SVC64)
- Confirm userspace uses `svc #0` and syscall number is in `x8`.
- In `kernel/src/arch/aarch64/trap/exception.rs`:
  - handle `EC=0x15 (SVC64)` and call `syscall_dispatcher(trapframe)`.
  - increment `epc` by 4 after SVC.
- Validate syscall return value convention (x0).
- Minimum syscalls needed for `init` to print something typically include:
  - `write` (or Scarlet equivalent used by userlib)
  - `exit`
  - possibly `openat`/`read`/`close` if init loads config or spawns shell.

### 4) EL0 return state correctness (SPSR_EL1)
- Confirm `_user_trap_exit` sets `SPSR_EL1` to return to `EL0t`.
- Decide interrupt mask policy (DAIF):
  - During early bring-up, keeping DAIF masked is OK.
  - Later, enable IRQ once timer + GIC are stable.

### 5) Timer + preemption (required for stable multi-task)
- Implement a working AArch64 timer source:
  - Choose CNTP (physical timer) or CNTV (virtual timer) for QEMU virt.
  - Program `CNT{P,V}_TVAL_EL0` / enable `CNT{P,V}_CTL_EL0`.
  - Hook the timer interrupt into the trap path and call shared `timer::tick(trapframe)`.
- Remove any scheduler hacks once the interrupt-driven tick works.

### 6) Interrupt controller (GIC) support
- Implement minimal GIC driver for QEMU virt:
  - Distributor + CPU interface initialization.
  - Enable IRQ routing to EL1.
  - Acknowledge/EOI path.
- Wire it to `kernel/src/arch/aarch64/interrupt/**`.

### 7) UART interrupts (optional after init prints)
- PL011 currently runs in polling mode; interrupt support can come later.
- If needed:
  - enable RX/TX interrupts
  - connect PL011 IRQ line via GIC
  - integrate with existing TTY/event system

### 8) Rootfs / initramfs multi-arch polish
- Keep producing per-arch initramfs artifacts (already started).
- Ensure user binaries in initramfs match the target arch (AArch64 vs RISC-V).
- Optional follow-up: if rootfs (not initramfs) is used, add per-arch build/deploy steps.

## Implementation Plan (Suggested Order)
1. Trap/exception diagnostics: make EL0 failures visible.
2. Confirm SVC64 syscall path works end-to-end (log first few syscalls).
3. Ensure init can at least `write("hello")` then `exit(0)`.
4. Add AArch64 timer and a minimal interrupt path.
5. Add GIC and enable IRQs.
6. Stabilize scheduling (remove AArch64-only hacks).
7. Optional: UART interrupts.

## Acceptance Criteria
- `cargo make run-aarch64` shows userspace init printing at least one line.
- SVC64 syscalls are handled and return to EL0 correctly.
- With timer+IRQ enabled, system continues scheduling without hangs.

## Notes / Diagnostics Tips
- If the system goes silent immediately after `eret`, suspect an instruction abort (no execute permission) or missing mapping.
- If syscalls never appear, suspect that EL0 never reached `_start` or that `VBAR_EL1`/trampoline is not installed as expected.
- Keep early logs minimal and bounded to avoid hiding the first exception.
