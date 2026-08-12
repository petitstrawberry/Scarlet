# AArch64 lock-stall and `Sbrk` investigation

## Scope

This document records the investigation of the MacBook AArch64 incident in
which `/bin/sas`, `/bin/files`, and `/bin/video_player` simultaneously consumed
100% CPU while sampled inside kernel syscall 13 (`Sbrk`). It also defines the
minimum evidence required for the next reproduction.

The incident is consistent with multiple tasks spinning on a shared kernel
lock. It is not established that `Sbrk` itself owns the stalled lock: the
sampled syscall identifies the blocked call path, while the old watchdog did
not identify the real lock holder.

## Observed evidence

The affected tasks remained active in kernel mode with a stable sampled PC for
more than one second:

```text
pc=0xffffffff801f3300
mode=kernel
same_pc=true
syscall_active=true
syscall=13
```

On QEMU, the old watchdog reported active diagnostic slots near scheduler lock
acquisition:

```text
[sync-watchdog] spin contention on cpu=7 preempt_count=2
[sync-watchdog] owner_cpu=7 kind=IrqGuard phase=acquiring lock=0x0 ...
[sync-watchdog] owner_cpu=7 kind=IrqSpinLock phase=acquiring lock=0x... ...
```

Those `owner_cpu` labels were not reliable. The implementation enumerated every
active per-CPU preemption-debug slot and labeled it as an owner. In addition,
most spin-lock success paths never changed their slot from `acquiring` to
`held`. A PC resolved near `note_spin_contention()` therefore identifies the
waiting loop, not the code that retained the lock.

## Confirmed watchdog deadlock hazard

The kernel allocator is:

```text
Talck<RawIrqSpinLock, DynamicHeapHandler>
```

Talc invokes `DynamicHeapHandler::handle_oom()` while its allocator lock is
held. The handler asks the PMM for contiguous pages and may wait on a PMM lock
under contention or fragmentation.

The previous contention reporter was unsafe in that state:

```text
allocator RawIrqSpinLock held
  -> DynamicHeapHandler::handle_oom
    -> PMM allocation / spin wait
      -> note_spin_contention
        -> debug_active_snapshots creates Vec
        -> format_location creates String
        -> println! enters the normal print path
```

The `Vec` or `String` allocation can recursively enter the already-held Talc
lock. The normal print path can also add a print lock to the existing lock
cycle. Thus a recoverable allocator/PMM contention episode could become a
permanent self-deadlock when the watchdog fired.

The watchdog must never allocate, wait on the normal print lock, or acquire a
lock whose availability is part of the failure being diagnosed.

## Current diagnostic contract

With `sync-debug`, every tracked guard has a fixed preallocated slot containing:

- source kind;
- phase (`acquiring` or `held`);
- lock address;
- task id captured at registration;
- acquisition timestamp;
- acquisition instruction address and link/return address;
- source location from `track_caller`;
- the sampled waiter spin count.

All spin-lock and RW-spin-lock success paths publish `held` only after atomic
acquisition succeeds. Standalone `PreemptGuard` and `IrqGuard` instances also
publish `held` immediately.

The watchdog report path uses only atomics, stack values, direct timer reads,
and `emergency_println!`. It does not create `Vec` or `String` values and does
not enter the normal print lock.

At the threshold, the reporting CPU's `acquiring` slot identifies the waiter.
Only `held` slots with the exact same non-zero lock address are reported as
owners. Each RW reader owns a separate slot, so all sampled readers are listed.
Matching `acquiring` slots on other CPUs are reported as peer waiters.

Example:

```text
[sync-watchdog] waiter_cpu=7 kind=IrqSpinLock lock=0xffffffff... task=42 spins=4194304 at kernel/src/...
[sync-watchdog]   owner_cpu=3 kind=IrqSpinLock lock=0xffffffff... task=17 held_ns=8123456 acquire_pc=0xffffffff... acquire_lr=0xffffffff... at kernel/src/...
[sync-watchdog]   peer_waiter_cpu=5 kind=IrqSpinLock lock=0xffffffff... task=31 spins=... at kernel/src/...
```

`acquire_pc` and `acquire_lr` are snapshots from successful acquisition. They
are not live register values from the stalled owner. They are intended to
identify the acquisition call path in the exact booted ELF.

If no matching held slot is observed, the watchdog explicitly reports that the
owner is unavailable. Possible reasons include an untracked lock, a release or
handoff racing the snapshot, a raw spin loop without a debug slot, or slot
capacity exhaustion. An unrelated active slot must not be interpreted as the
owner.

## Required next MacBook reproduction

### 1. Preserve the exact build identity

Before booting, save all of the following together:

- the exact unstripped kernel ELF;
- the boot image copied to the machine;
- `git rev-parse HEAD`;
- enabled kernel features;
- the ELF SHA-256 digest;
- the build log and linker map, when available.

Do not resolve addresses against a later rebuild. LLVM code folding,
inlining, link ordering, and feature changes can move or merge the relevant
instructions.

The `aarch64-limine-full` project already enables `sync-debug` in its kernel
feature set. Confirm the actual MacBook project does the same.

### 2. Reproduce the shared-pressure workload

Run the workload that previously started SAS, Files, and video_player together.
Add controlled memory pressure that repeatedly grows and releases userspace
heaps so the PMM and kernel heap encounter fragmentation. Keep serial or other
emergency-console output available.

Record:

- the first watchdog report, not only repeated reports after the system is
  saturated;
- every waiter, owner, reader, and peer-waiter line for the same lock address;
- the task CPU-hog sample including syscall number and sampled PC;
- PMM free-page and largest-order information, when available;
- whether timer interrupts and other CPUs continue making progress.

### 3. Resolve every address against the saved ELF

```sh
llvm-addr2line -afiC -e <exact-kernel-elf> \
  <sampled-pc> <acquire-pc> <acquire-lr> ...
```

Resolve the waiter source location and owner acquisition source location as
well. The owner acquisition path is the primary evidence for the lock-order or
missing-unlock investigation.

### 4. Classify the result

- **One matching held owner:** inspect that critical section for an unbounded
  loop, nested lock order, interrupt reentrancy, or missing release.
- **Several matching held readers:** a writer is blocked by readers; inspect
  each reader's acquisition site and held duration.
- **Only peer waiters:** the owner is untracked, in raw handoff, or was lost
  before the snapshot; instrument that exact lock implementation next.
- **No tracked waiter:** the spin came from a raw loop or a nested diagnostic
  path. The active-slot dump is context only and must not be called owner data.

## `Sbrk` path review

`sys_sbrk` calls:

```text
Task::adjust_brk
  -> Task::set_brk_locked
    -> Task::allocate_brk_pages
      -> Task::allocate_data_pages
        -> PMM contiguous allocation
        -> VM map insertion
        -> task page-allocation ownership tracking
```

The transaction lock serializes concurrent `brk` changes in a shared address
space. PMM allocation is completed before VM-map insertion, and the reviewed
buddy-allocation loops are bounded. No independent infinite loop was found in
that path.

There is a separate VM correctness defect in `set_brk_locked()`: when growing,
it tests only whether `search_memory_map(prev_addr)` returns one map. If a map
covers the first growth page but not the whole requested range, allocation is
skipped for the entire range and `brk` is still published. The eventual fix
must validate coverage of the full page-aligned interval and allocate every
unmapped run transactionally. This defect is not proven to be the lock-stall
root cause and should not be used to explain the MacBook incident without a
reproduction.

## Waker test result

The failing
`test_prepare_wait_repairs_locally_owned_ready_task` did not demonstrate a
runtime `Ready -> NotInitialized` transition. Its fixture registered a fresh
`Task::new()` without calling `Task::init()`, then asserted that the task was
`Ready`. The fixture now initializes the task before scheduler registration.
No waker state-transition behavior was changed.

The scheduler already rejects an ordinary context switch while
`preempt_count != 0`; therefore a normal scheduler switch cannot strand a task
while it retains one of these spin-lock guards. A future reproduction may
still expose interrupt reentrancy, raw-lock code, or scheduler ownership bugs,
but the old waker test failure is not evidence of them.

## Completion criteria

The incident can be considered identified only when a reproduction provides:

1. the exact booted ELF and commit;
2. a waiter line with a concrete non-zero lock address;
3. a matching held owner or a bounded explanation for why the owner is
   untracked;
4. resolved waiter and owner acquisition addresses;
5. a code path explaining why the owner could not release the lock;
6. a stress run demonstrating that the fix removes the stall without hiding
   watchdog output or introducing a new lock cycle.
