# Apple M1 Bare SMP Hang Investigation

Updated: 2026-07-18 (JST)

## Purpose

This document is the canonical investigation record for the Apple M1 bare/direct
boot SMP hang. It exists to prevent previously tested hypotheses from being
reintroduced after context loss.

Evidence is ranked in this order:

1. Repeated bare-hardware A/B results.
2. Exact logs from the kernel image used for that hardware run.
3. Current source and disassembly.
4. External source such as m1n1, Asahi Linux, Linux, or Arm documentation.
5. Static hypotheses and agent analysis.

The superseded `.debug-journal.md`, `APPLE_LIMINE_DEBUG_HANDOFF.md`, and
`review.md` files were removed after their still-relevant evidence was
consolidated here.

## Non-negotiable constraints

- Keep Scarlet at EL2/VHE for direct boot. Dropping to EL1 is not a fix.
- Do not create or deploy an image unless the user explicitly requests it.
- Real-hardware runs are performed by the user.
- Kernel execution is non-preemptible and runs with all interrupt classes masked.
- Do not add Apple-specific behavior to architecture-independent common code.
- Change one discriminating variable per hardware A/B run.
- Do not call a correctness fix the root cause unless a hardware cause toggle
  correlates with the failure.

## Reproduction matrix

| Environment | EL/regime | SMP/user placement | Result | Authority |
|---|---|---|---|---|
| Apple M1 direct/Limine | EL2/VHE host | SMP > 1, user tasks may run on APs | Intermittent hang | Repeated hardware result |
| Apple M1 direct/Limine | EL2/VHE host | SMP = 1 | Stable | Repeated hardware result |
| Apple M1 direct/Limine | EL2/VHE host | SMP > 1, all user tasks pinned to BSP | Stable | Hardware A/B |
| Apple M1 direct/Limine | EL2/VHE host | Process leaders allowed on APs | Hangs | Hardware A/B |
| Apple M1 direct/Limine | EL2/VHE host | Threads allowed on APs | Hangs | Hardware A/B |
| m1n1 HV | EL1 guest | SMP > 1 | Stable | Repeated hardware result |
| QEMU TCG | EL2/VHE available | SMP > 1 | Stable | Repeated run |
| QEMU HVF | EL1 | SMP > 1 | Stable | Repeated run |

The failure therefore requires real Apple hardware, multiple physical CPUs, and
user execution outside the BSP. Neither `fork()` nor EL2/VHE alone is sufficient.

## Decisive runtime evidence

### Fork completes

Repeated traces show that process-fork address-space cloning completes, the child
is registered and enqueued, and the parent returns from `fork()`. The visible
fork location is an exposure point, not proof that cloning itself is the cause.

### CPU 0 stops at the remote-enqueue interrupt boundary

A later run captured CPU 0 with a stable final breadcrumb of `0x4944`, which is
`IPI_SEND_DONE`. This phase is published after the interrupt controller's
`send_ipi()` call returns and after the controller lock has been released. In
the fork path, the call chain is:

1. `clone_task()` registers and adopts the child.
2. `enqueue_task()` pushes the child onto a remote CPU's ready queue while an
   `IrqGuard` keeps DAIF masked.
3. `request_remote_reschedule()` sends the remote reschedule IPI.
4. `InterruptManager::send_ipi()` records `IPI_SEND_DONE`.
5. `enqueue_task()` returns, dropping its `IrqGuard` and restoring DAIF.
6. `clone_task()` prints the fork-return trace and records `FORK_RETURN`.

The breadcrumb proves that AIC IPI submission returned; it does not by itself
prove which following instruction stalled. The next unmeasured boundary is the
DAIF restore at `IrqGuard` drop. This materially strengthens an interaction
between remote fork-child enqueueing and a pending physical FIQ. The associated
breadcrumb `aux` and `aux2` values should be retained in future captures:
`aux` is the target CPU, and `aux2=1` means the IPI controller returned success.

### A fork child completes its first user trap

The strongest captured boundary is:

```text
[fork-trace] child_task_id=16 user-return cpu=3
[fork-trace] child_task_id=16 first-user-trap cpu=3 kind=0 \
    elr=0x230c08 esr=0x82000004 far=0x230c08 \
    task_asid=17 user_ttbr=0x11000b2d12a000
[fork-trace] child_task_id=16 first-user-trap-done cpu=3 \
    current=Some(16) elr=0x230c08 user_ttbr=0x11000b2d12a000
```

`ESR=0x82000004` is a lower-EL instruction translation fault. The lazy mapping
handler returned successfully, the current task remained child 16, and the
saved user TTBR still carried ASID 17. The remaining failure boundary is after
the handler and before sustained user progress.

Other runs showed multiple fork children reaching `user-return` or
`first-user-trap-done` on APs. A specific AP's first user entry is therefore not
by itself the complete cause.

### System-wide convergence is secondary

Observed terminal breadcrumbs have included CPUs at:

- `RF`: `owner.resolve_fault()` returned, before the remaining lazy-map work.
- `AD`: scheduler obtained the ASID and is about to enter address-space setup.
- `VR`: `get_asid()` could not immediately acquire the VMM read lock.
- `VW`: a VMM writer guard is held at a recorded site.
- `PD`: work stealing completed; CPUs at `PD` can still be living idle CPUs.

The original breadcrumb snapshot protocol could combine an old phase with new
auxiliary values. It was replaced with ordered publication. Old mixed snapshots
must not be used to infer a lock owner.

There is no confirmed static VMM/TaskPool/page-table lock cycle. A stranded or
corrupted lock remains possible as a consequence of another CPU stopping, but
it is not established as the initiating cause.

### PAGE_TABLES writer release does not complete

A later stable snapshot connected the fork-child first-fault path to a concrete
page-table lock convoy:

- CPUs 0, 1, and 2 stopped at `PT_LOCK_WAIT`, waiting for per-ASID page-table
  locks.
- CPUs 4 and 7 stopped at `PT_REGISTRY_READ_WAIT` while holding those per-ASID
  locks and observing one active `PAGE_TABLES` writer.
- CPU 3/task 19 stopped at `PT_REGISTRY_DONE`, after inserting a child or root
  page table while still holding the `PAGE_TABLES` write guard.
- CPUs 5 and 6 continued idle timer heartbeats for thousands of ticks.

The source immediately following `PT_REGISTRY_DONE` drops the registry write
guard and then records `PT_REGISTRY_RELEASED`; there is no intervening lock
acquisition and the release breadcrumb never appeared. Static lock-order audit
found a one-way dependency chain, not a reverse edge or software deadlock cycle.

Scarlet uses `spin 0.10`. Its write-guard drop performs a release `fetch_and()`.
The AArch64 target previously omitted LSE, so LLVM lowered that operation to an
LL/SC retry loop rather than a single LSE atomic operation. This provides a
direct explanation for CPU 3 failing to progress under contention while also
explaining why the other page-table operations converge behind it.

## Tested and rejected hypotheses

| Hypothesis or experiment | Hardware result | Status | Notes |
|---|---|---|---|
| APs are targeted before becoming scheduler-online | AP heartbeats and later user execution were observed | Refuted for the current hang | The failure occurs after boot |
| Fork child affinity to parent/BSP | Hang remains | Refuted as sufficient fix | Does not explain all AP user failures |
| Disable task migration | Hang remains | Refuted as sufficient fix | Do not promote continuation migration without new evidence |
| Disable idle work stealing | Hang remains | Refuted as sufficient fix | `PD` CPUs can remain alive and idle |
| Disable fork COW | Hang remains | Refuted as root | COW fixes may still be valid correctness fixes |
| Suppress fast reschedule IPI | Hang remains | Refuted as sole cause | Timer FIQ remains active |
| Idle/WFI changes, including extra barriers | Hang remains | Refuted as root | AP idle heartbeats continue |
| Apple `WFI_MODE` normalization | No improvement | Refuted as sufficient fix | Affects idle behavior, not proven coherency |
| Normalize Apple `ACTLR.EnMDSB` on BSP and APs | No improvement | Refuted as sufficient fix | Direct and HV ACTLR values differed before this test |
| cpufreq/performance-state changes | No root-cause correlation | Refuted as root | Direct execution may still be slower |
| EL2 timer-bank handoff cleanup | Correctness improved; original hang remains | Refuted as root | Physical/virtual bank cleanup is retained if correct |
| AP `kernel_ttbr0` is stale | AP saves the final kernel TTBR after switching to it | Refuted | First user trap also reaches the Rust handler |
| AP TCR/MAIR is never normalized | `start_ap()` normalizes before scheduling | Refuted | Current TCR is WB/WB, Inner Shareable |
| TG1 uses the wrong 4 KiB encoding | `TG1=0b10` is 4 KiB | Refuted | Earlier decode was incorrect |
| VHE `VMALLE1{IS}` targets the wrong regime | With `E2H=TGE=1`, it targets EL2&0 | Refuted | Confirmed by Arm semantics and Linux VHE use |
| Invalid translation faults require eviction from a cached negative TLB entry | Arm does not cache translation faults in the TLB | Refuted | Invalid-to-valid publication still needs correct PTE visibility |
| m1n1 stage 2 repairs stage-1 memory attributes | m1n1 uses `PTE_MEMATTR_UNCHANGED` | Refuted | Stage 2 is not an attribute-repair explanation |
| Broad HHDM maps MMIO/reserved holes as Normal memory | Runtime direct map is now sparse and rejects attribute aliases; hang remains | Refuted for current tree | Boot-time bounds are not the runtime map |
| Repeat-TLBI CPU erratum | Apple M1 is not selected by Linux's repeat-TLBI workaround | Unsupported | Require Apple-specific evidence before revisiting |
| `get_asid()` retains a read lock after returning | The `u16` is copied and the guard drops before return | Refuted | `Arc<AtomicU16>` experiment was reverted |
| Move ASID into `Arc<AtomicU16>` | No hardware improvement | Rejected | Unnecessary design and not a root fix |
| `KernelContext::get_trapframe()` offset corrupts the active stack | Active code uses `Task::get_trapframe()` with the correct high-VA offset | Refuted for runtime path | The unused method can be cleaned separately |
| Generic lazy-map or local-TLBI failure | Same path works under HV and QEMU | Refuted as a sufficient environment explanation | A bare-only hardware/state difference is required |
| Static PAGE_TABLES/per-ASID lock cycle | Audit found only per-ASID lock -> registry lock ordering; the registry writer takes no reverse lock after `PT_REGISTRY_DONE` | Refuted for the captured stall | The observed state is a convoy behind a non-completing writer release |

## Correctness fixes that are not established root causes

| Fix | Reason to retain or evaluate separately | Root-cause status |
|---|---|---|
| Serialize kernel-stack window slot allocation through shared kernel page-table updates | Prevents competing intermediate-table publication | Not isolated by hardware A/B |
| Atomically publish intermediate page-table descriptors | Prevents losing a concurrently created table | Correctness fix; not proven root |
| Reject stale concurrent COW commits | Prevents replacing a newer private mapping | Correctness fix; COW disable did not cure hang |
| Sparse runtime direct map and ioremap alias rejection | Removes invalid Normal/Device aliases | Correctness fix; hang remains |
| Ordered breadcrumb snapshot publication | Makes diagnostic context reliable | Diagnostic fix only |
| Mask a pending direct-VHE timer source before Apple FIQ claim/scheduling | Follows the conventional level-triggered timer sequence and prevents a live level from spanning scheduler entry | Hardware improved performance; hang remains |

## Current hypothesis table

| Priority | Hypothesis | Why it fits | Counter-evidence | Decisive test | State |
|---:|---|---|---|---|---|
| 1 | LL/SC livelock in `spin::RwLock` writer release | CPU 3 stops immediately before write-guard drop; no post-release breadcrumb; readers continuously contend; target omitted LSE | Exact generated retry PC was not captured | Enable LSE without changing lock code or timing diagnostics | Hardware A/B ready |
| 2 | Direct-mode coherency/exclusive-monitor setup makes LL/SC progress pathological | Explains direct-only behavior and why m1n1 HV/QEMU work; ordinary lock memory is Normal Inner Shareable | Required memory attributes appear correct; no missing mandatory Apple coherency bit is yet identified | If LSE fixes the hang, distinguish generic LL/SC contention from direct monitor/coherency state only if further root detail is needed | Open |
| 3 | A pending physical FIQ interacts with remote fork-child enqueueing when CPU 0 restores DAIF | CPU 0 previously stopped at `IPI_SEND_DONE`; direct uses physical FIQ; timer ordering improved performance | The later snapshot identifies a concrete page-table writer-release boundary; timer fix did not remove the hang | Retain the enqueue post-restore breadcrumb during the LSE A/B | Open, reduced priority |
| 4 | Direct AP boot-stack/cache handoff leaves latent corruption | m1n1 HV explicitly reinitializes secondary MMU/stack state with barriers and cache maintenance; direct chainload follows a different path | APs reach idle, scheduler, user entry, and first trap; current task stacks are separate high-VA windows | A/B explicit AP boot-stack clean/invalidate only, using identical image/runtime settings | Open, low confidence |
| 5 | Available evidence is still insufficient | Many source-level candidates also fail the environment matrix | A narrow hardware A/B may still isolate the difference | Record one-variable runs with exact image hash and complete per-CPU tail | Always possible |

## FIQ-specific questions to answer

### Current physical-FIQ sequence

The current direct VHE path is:

1. The lower-EL FIQ vector records `trap_kind=2` and saves the user context.
2. `arch_irq_handler()` samples the architectural timer and masks it immediately
   when it is already pending.
3. The Apple driver captures and handles the remaining CPU-local FIQ sources.
4. The handler samples the architectural timer again to cover a timer becoming
   pending during the claim, and masks that newly pending level immediately.
5. If the timer was pending in either sample, `tick_with_scheduler()` programs
   the next compare value and only then restores the timer route before scheduling.
6. A simultaneous fast IPI is cleared and converted into a reschedule request.
7. Scheduling happens only after the timer has been rearmed.

The current source bitmap is:

| Bit | Meaning | Expected use |
|---:|---|---|
| `0x1` | Host physical timer | Direct EL2/VHE host |
| `0x2` | Host virtual timer | Non-VHE/EL1 host path |
| `0x4` | Guest physical timer | Must remain masked when Scarlet is not running a guest |
| `0x8` | Guest virtual timer | Must remain masked when Scarlet is not running a guest |
| `0x10` | Apple fast IPI | Reschedule request |
| `0x20` | Core PMU | Disabled during CPU FIQ preparation |
| `0x40` | Uncore PMU | Disabled during CPU FIQ preparation |

Therefore a direct VHE log showing only `0x2` as the active host timer would be
evidence of using the wrong timer bank. It must not be interpreted as an IPI.

The direct path now follows the conventional level-triggered timer order:
mask, program a future compare, then unmask. Each system-register write is
followed by an `ISB`. Arm defines a future compare, `IMASK=1`, or `ENABLE=0` as
deasserting the level source. Asahi Linux and m1n1 use the corresponding write
plus `ISB` sequence and do not poll a readback register, so Scarlet does not add
a non-reference readback loop. Hardware A/B is still required to determine
whether this ordering change affects the hang. The reason FIQ remains a useful
discriminator is the delivery model difference:

- Direct mode leaves the physical level source under Scarlet's control and
  relies on reprogramming/masking the timer before returning.
- m1n1 HV clears the relevant `VM_TMR_FIQ_ENA_EL2` source bit before guest
  return, coalesces pending causes, and injects one virtual FIQ through
  `HCR_EL2.VF`. It re-evaluates the virtual FIQ state on trapped timer writes.

The HV path can therefore hide a direct physical-source reassertion or exact
user-return timing problem even when Scarlet's guest-visible handler is the same.

The physical-FIQ investigation must record the exact order of:

1. EL0 FIQ vector entry and DAIF state.
2. Apple FIQ source snapshot.
3. Distinction between physical host timer (`0x1`), virtual host timer (`0x2`),
   fast IPI (`0x10`), guest timer bits, and no decoded source.
4. CNTP timer mask/ack/rearm in direct VHE mode.
5. Reschedule request publication.
6. Direct or deferred `schedule()` entry.
7. Task switch completion.
8. Return to EL0 and immediate reassertion, if any.

Do not infer this order from console lines emitted in FIQ context. Prefer
per-CPU lock-free breadcrumbs and dump them from a surviving safe context.

## Next discriminating experiments

### Experiment A: disable only AP architectural timers

- Keep SMP enabled.
- Keep normal user placement on APs.
- Do not combine this with IPI, migration, COW, or work-stealing changes.
- Leave the BSP timer enabled so global time and basic scheduling remain usable.

Interpretation:

| Result | Meaning |
|---|---|
| Stable across repeated boots | Strong evidence for AP timer FIQ delivery/return/schedule path |
| Same hang | Physical AP timer is not required; move to non-timer FIQ/nested exception or corruption |
| Different failure due to starvation only | Experiment needs a narrower timer-routing method, not a broader scheduler change |

### Experiment B: classify the first post-user-return FIQ without printing

Record per CPU:

- interrupted task ID and ELR;
- trap kind and saved SPSR;
- raw Apple FIQ source bits;
- timer pending state before and after acknowledgement;
- whether scheduling was requested and whether a switch completed.

Dump once from a surviving CPU. Do not print per interrupt.

### Experiment C: AP boot-stack cache maintenance

Only if Experiment A does not discriminate, mirror m1n1's secondary-stack
barrier/cache-maintenance sequence in the architecture-specific AP transition.
Treat this as a correctness A/B, not an assumed root fix.

## Direct-mode debugger feasibility

### What m1n1 can and cannot do

m1n1 does not remain as a separate debugger after direct chainload. Its direct
path shuts down m1n1-managed subsystems, relocates the next image, and branches
to the payload entry. The UART proxy loop and HV exception monitor are no longer
available once Scarlet owns EL2.

m1n1's existing GDB server works only when m1n1 remains the hypervisor. It
supports register and memory access, breakpoints/watchpoints, continue, step,
and CPU selection, but that execution mode does not reproduce this direct-only
hang and cannot be treated as an equivalent debugger environment.

Relevant local m1n1 references:

- `projects/aarch64-apple-limine-full/m1n1/upstream-m1n1/src/main.c`
- `projects/aarch64-apple-limine-full/m1n1/upstream-m1n1/src/payload.c`
- `projects/aarch64-apple-limine-full/m1n1/upstream-m1n1/src/chainload.c`
- `projects/aarch64-apple-limine-full/m1n1/upstream-m1n1/src/chainload_asm.S`
- `projects/aarch64-apple-limine-full/m1n1/upstream-m1n1/src/uartproxy.c`
- `projects/aarch64-apple-limine-full/m1n1/upstream-m1n1/src/exception.c`
- `projects/aarch64-apple-limine-full/m1n1/upstream-m1n1/proxyclient/m1n1/hv/gdbserver/`

### Direct-compatible options

| Priority | Method | What it can provide | Main limitation | Direct-mode fit |
|---:|---|---|---|---|
| 1 | Scarlet UART debug/GDB stub | Current CPU registers, memory access, breakpoints, controlled stop/continue | Must be implemented in Scarlet; unavailable if the UART/debug core also stops | High |
| 2 | Apple/Arm self-hosted debug register sampler | Potential remote PC sampling and hardware break/watchpoint support without running Scarlet under HV | Public Apple documentation confirms only part of the debug register interface; no complete public workflow | Promising research path |
| 3 | FIQ stop-the-world rendezvous | Per-CPU trapframes and synchronized crash snapshots | FIQ is not a true NMI and may perturb the suspected failure path | Medium |
| 4 | Watchdog plus reserved-memory crash dump | Post-mortem per-CPU breadcrumbs/registers after reboot | Requires a reliable independent watchdog and a RAM region that survives the chosen reset | Medium |
| 5 | Public JTAG/CoreSight probe | External halt and inspection | No source-backed public Apple M1 JTAG workflow was found | Not currently viable |

### Recommended incremental debugger path

Do not begin with a full GDB implementation. Preserve direct EL2/VHE behavior
and add capabilities in this order:

1. Define a fixed, lock-free per-CPU crash record containing ELR, SPSR, SP,
   general registers available at the trap point, current task, TTBR, ESR/FAR,
   DAIF, raw FIQ sources, and the last breadcrumb generation.
2. Add a polling UART command that can read memory and those crash records from
   a surviving CPU without taking framebuffer or scheduler locks.
3. Add a deliberate `brk`/panic entry that populates the same record and enters
   the polling monitor.
4. Reuse m1n1's host-side GDB remote packet handling as a reference for `g/G`,
   `p/P`, `m/M`, and `Z/z`; keep the target-side transport minimal.
5. Investigate Apple self-hosted debug registers as a way for one surviving CPU
   to sample or halt another CPU. Do not assume unavailable JTAG access.

This approach provides useful direct-mode post-mortem state before attempting
single-step or full SMP stop/continue support.

External references:

- [m1n1 hypervisor debugging](https://github.com/AsahiLinux/docs/blob/main/docs/sw/m1n1-hypervisor.md#using-gdblldb)
- [m1n1 direct chainload](https://github.com/AsahiLinux/m1n1/blob/a35e52dba52b1fd0b54dcfec316063362816bdcf/proxyclient/tools/chainload.py)
- [Apple CPU debug registers](https://asahilinux.org/docs/hw/cpu/debug-registers/)
- [Arm self-hosted debug](https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/Learn%20the%20Architecture/Arm%20V8A%20self-hosted%20debug.pdf)
- [Apple serial debug](https://asahilinux.org/docs/hw/soc/serial-debug/)

## Experiment log

Add every hardware run here. One row must represent one changed variable.

| Date | Commit/image SHA-256 | Single changed variable | CPU/user placement | Result | Last decisive evidence | Conclusion |
|---|---|---|---|---|---|---|
| 2026-07-18 | Historical run; exact image hash retained in prior session | Baseline with first-user-trap diagnostics | Child on CPU 3 | Hang | Child 16 reached `first-user-trap-done`, ASID 17, matching TTBR | Failure is after successful first fault handling |
| 2026-07-18 | Current test image; SHA-256 not recorded yet | Mask direct-VHE timer before FIQ claim/scheduling | Normal SMP placement | User reports visibly improved performance; hang persists | CPU 0 final phase `0x4944` (`IPI_SEND_DONE`); aux fields not yet recorded | Confirms excess physical-FIQ handling overhead and narrows the hang to the remote-enqueue/DAIF-restore boundary |
| 2026-07-18 | Current test image; SHA-256 not recorded yet | Baseline without AArch64 LSE | Normal SMP placement | Hang | CPU 3 at `PT_REGISTRY_DONE`; CPUs 4/7 observe registry writer; CPUs 0/1/2 wait downstream; CPUs 5/6 remain alive | Registry writer unlock fails to complete; static lock cycle refuted |
| 2026-07-18 | Current test image; SHA-256 not recorded yet | Enable AArch64 LSE atomics in the common kernel target | Normal SMP placement | Appears stable in initial observation; repetition pending | System progresses beyond the previous writer-release convoy | Strong preliminary support for LL/SC unlock livelock; not yet confirmed by repeated runs |
| TBD | TBD | Disable AP architectural timers only | Normal SMP placement | TBD | TBD | TBD |
| TBD | TBD | AP boot-stack cache maintenance only | Normal SMP placement | TBD | TBD | TBD |

## Relevant source locations

- `kernel/src/arch/aarch64/trap/user.rs` — EL0 trap entry/return and first-user-trap trace.
- `kernel/src/arch/aarch64/trap/interrupt.rs` — timer/FIQ/IPI scheduling decisions.
- `drivers/pic/apple-aic/src/fiq.rs` — Apple FIQ source classification and masking.
- `drivers/pic/apple-aic/src/fiq/registers.rs` — timer and Apple FIQ register access.
- `kernel/src/arch/aarch64/timer.rs` — architectural timer programming.
- `kernel/src/sched/scheduler.rs` — ready queues, deferred reschedule, task switching, and breadcrumbs.
- `kernel/src/arch/aarch64/vm/mmu/armv8_4k.rs` — PTE publication and TLBI sequences.
- `kernel/src/vm/mod.rs` — trampoline and per-task kernel-stack windows.
- `kernel/src/arch/aarch64/boot/limine.rs` — direct BSP/AP EL2/VHE handoff.
- `kernel/targets/aarch64-unknown-none-elf.json` — common AArch64 ISA features, including LSE atomics.

## Current conclusion

The root cause is not yet hardware-confirmed, but the strongest boundary is now
the `PAGE_TABLES` write-guard release. Static lock-order audit found no cycle,
and the non-LSE target lowered the release atomic to an LL/SC retry loop. The
common AArch64 target now enables LSE without changing the lock algorithm. The
initial hardware observation appears stable and progresses beyond the previous
writer-release convoy. Repeated identical fork workloads are still required
before promoting LL/SC unlock livelock from the leading hypothesis to the
confirmed root cause. Timer masking remains a valid performance/correctness fix
but is not the hang fix.
