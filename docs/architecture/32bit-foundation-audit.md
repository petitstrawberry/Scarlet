# 32-bit portability foundation: audit and design boundaries

Status: initial inventory and proposed refactoring sequence, 2026-09-08.
Source baseline: `7f591dd5898e09d137ba97b0a591d6f09f97dec8`.

The objective is to make Scarlet's common implementation suitable for future
32-bit targets. ARMv5TE and RISC-V 32 are design constraints that expose different
assumptions. This work does not select a board, add a CPU port, promise boot on
either architecture, or implement 32-bit processes on a 64-bit kernel.

This first step inventories the assumptions before changing contracts. The
existing RV64 and AArch64 behavior and published native ABI form the regression
baseline. The interfaces below are design proposals, not implemented APIs.

## Coverage and evidence

The scan covers all 729 tracked Rust files, plus C, assembly, linker scripts,
target specifications, build scripts, and configuration: 898 source/configuration
files in total. It includes inactive architecture code, tests, and vendored
`user/lib/sunset`. Documentation, lock files, and dependency manifests were also
consulted for integration scope. Binary rootfs/guest artifacts were not decoded.
External Rust/SDK/SGFX/UI/library repositories were not audited internally.

The [review scanner](../../scripts/audit-32bit-foundation.py) produces every
matching file and line number. Its 659 matching files are **review candidates**,
not 659 defects. Counts include comments and overlapping categories. It does not
infer Rust types, expand macros, resolve conditional compilation, or prove the
absence of other problems. Findings below combine those searches with manual
inspection of the boundary implementations; this is not a complete runtime
verification of the kernel or of every match.

| Search category | Files | Matching lines | Interpretation |
| --- | ---: | ---: | --- |
| `usize` / `isize` | 483 | 10,091 | Many are correct local pointers or slice lengths. |
| `u64` / `i64` / `u128` / `i128` | 338 | 5,532 | Many are required data widths. |
| Casts to `usize` / `isize` | 290 | 2,629 | Inspect the source type and valid range. |
| Atomic types, all widths | 122 | 1,099 | Pointer CAS availability matters independently of word size. |
| `AtomicU64` / `AtomicI64` | 32 | 320 | Includes optional, architecture-specific, and test code. |
| `Arc` / `Weak` mentions | 203 | 3,094 | Includes imports, calls, comments, and types. |
| Layout declarations/assertions | 145 | 829 | Size, alignment, padding, and field widths all matter. |
| Typed memory/volatile/raw-slice sites | 195 | 1,310 | Includes valid operations; review alignment and access granularity. |

Reproduce from the repository root with Python 3 and Git:

```sh
python3 scripts/audit-32bit-foundation.py
python3 scripts/audit-32bit-foundation.py --format json > /tmp/scarlet-width-inventory.json
python3 scripts/audit-32bit-foundation.py --category atomic64 --format json
```

The scanner reads tracked working-tree content and records its base commit. It
excludes itself, Markdown, lock files, and non-source binary artifacts. Its JSON
also lists the scanned files, skipped files, patterns, and matching line numbers.

The [native syscall checklist](32bit-foundation-syscalls.md) enumerates all 143
entries in the kernel table, including the invalid entry, compatibility-only
event calls, and optional/debug calls. It assigns each entry a boundary review
family; it is not a claim that each handler has been dynamically tested.

### Checks actually run

Using the repository's `nightly-2025-12-31` toolchain
(`rustc 1.94.0-nightly (0e8999942 2025-12-30)`), with offline dependencies and build
output outside the worktree:

| Check | Observed result |
| --- | --- |
| `scarlet-abi` tests on `aarch64-apple-darwin` | All 7 existing native contract tests pass. This is a host layout test, not an AArch64 kernel test. |
| `scarlet-abi` check on `riscv32imac-unknown-none-elf`, building `core,compiler_builtins` | Fails at `src/lib.rs:118`: `RawTaskDebugInfoV1` is 56 bytes, but the assertion requires 64. |
| Same check on `armv5te-none-eabi` | The same 56-versus-64 compile failure. |
| `rustc --print cfg` on RV32IMAC | Pointer width 32; atomics available for 8, 16, 32 and pointer widths; no 64-bit atomics. |
| `rustc --print cfg` on ARMv5TE bare metal | Pointer width 32; no `target_has_atomic` widths advertised. |

Commands, assuming the pinned Cargo/rustc are on `PATH`:

```sh
cargo test --offline --manifest-path user/lib/scarlet-abi/Cargo.toml \
  --target aarch64-apple-darwin --target-dir /tmp/scarlet-foundation-host
cargo check --offline --manifest-path user/lib/scarlet-abi/Cargo.toml \
  --target riscv32imac-unknown-none-elf -Z build-std=core,compiler_builtins \
  --target-dir /tmp/scarlet-foundation-rv32
cargo check --offline --manifest-path user/lib/scarlet-abi/Cargo.toml \
  --target armv5te-none-eabi -Z build-std=core,compiler_builtins \
  --target-dir /tmp/scarlet-foundation-armv5
rustc --print cfg --target riscv32imac-unknown-none-elf
rustc --print cfg --target armv5te-none-eabi
```

These are compiler probes of the common ABI crate, not new Scarlet targets.
No full kernel build, QEMU boot, Linux conformance test, or new hardware test was
run for this documentation/audit change. Fixing the first compile error would
not establish portability of the remaining layers.

## Separate the dimensions currently conflated

| Dimension | Common-code contract to establish |
| --- | --- |
| Kernel pointers and allocation lengths | `usize` remains appropriate for actual kernel virtual pointers, slice lengths, and indices. Arithmetic must be checked before allocating or forming a range. |
| Machine register words | Represent transport words at the executing CPU's width. A syscall register is not necessarily a complete logical argument or result. |
| User ABI words and pointers | Decode according to an explicit ABI data model, alignment, and byte order. Validate before converting to a kernel-accessible address. |
| Persistent quantities | File sizes/positions, disk capacity, timestamps, deadlines, sequence IDs and protocol-defined 64-bit fields retain their required width. |
| Physical addresses | A physical-address representation must not inherit the virtual pointer width. Represent wide input and reject unsupported ranges explicitly. |
| DMA and IOVA addresses | Device address width, mapping constraints, and CPU physical width are separate. A DMA address is not a dereferenceable kernel pointer. |
| Synchronization capability | Model native CAS, available atomic widths, execution context, and UP/SMP separately from pointer width. |
| ABI memory layout | Fixed field widths alone do not fix alignment, implicit padding, endianness, or signedness. |
| Optional architecture services | MMU geometry, cache maintenance, instruction/context state, and virtualization must be supplied through defined boundaries. |

ARMv5TE makes the no-pointer-atomic case essential. Rust exposes atomics through
`target_has_atomic`, and `alloc::sync::Arc` requires pointer atomics. A Linux
target's atomic support may depend on OS assistance and cannot establish a
bare-metal kernel capability. See the [Rust conditional compilation reference](https://doc.rust-lang.org/reference/conditional-compilation.html#target_has_atomic),
[atomic portability documentation](https://doc.rust-lang.org/stable/core/sync/atomic/),
and [`Arc` availability](https://doc.rust-lang.org/alloc/sync/struct.Arc.html).

ARMv5TE's ARM/Thumb interworking also prevents a common entry-address abstraction
from assuming one instruction encoding or stripping state bits indiscriminately.
The [Rust ARMv5TE target documentation](https://doc.rust-lang.org/rustc/platform-support/armv5te-none-eabi.html)
describes this constraint. Exact execution-state handling belongs to a future
architecture adapter.

## Inventory

Priorities describe refactoring dependencies: **P0** is a prerequisite for the
common foundation; **P1** closes width-sensitive semantic or boundary gaps;
**P2** isolates optional integrations and architecture implementations. A P2
component must be cleanly excludable; it is not implicitly portable.

### Syscall transport, native ABI, and user memory

| ID | Priority | Evidence | Problem and required foundation |
| --- | --- | --- | --- |
| ABI-01 | P0 | [`AbiModule::handle_syscall`](../../kernel/src/abi/mod.rs#L104), [native dispatch macro](../../kernel/src/syscall/macros.rs#L33), [`scarlet-sys`](../../user/lib/scarlet-sys/src/lib.rs#L51) | Arguments and successful results flow as `usize`; there is no logical wide-result contract. Separate register extraction, ABI decoding, kernel operations, and result encoding. |
| ABI-02 | P0 | [RV64 trap accessors](../../kernel/src/arch/riscv64/mod.rs#L559), [AArch64 accessors](../../kernel/src/arch/aarch64/mod.rs#L450), [native handlers](../../kernel/src/task/syscall.rs#L1621) | Handlers receive concrete trapframes and advance PC individually. Define continuation/result ownership for ordinary returns, blocking, exec, exit, and event return; a wide return must not be overwritten by the existing single-word trap exit. |
| ABI-03 | P0 | [`RawEnvironmentExec`](../../user/lib/scarlet-abi/src/lib.rs#L31), [`string_array` / `exec_options`](../../kernel/src/executor/syscall.rs#L366) | Five native-width fields in userland, but kernel reads 48 bytes, 8-byte pointer slots, and `usize::from_ne_bytes([u8; 8])`. The latter cannot compile on 32-bit; other fixed slice conversions can panic. Give the record an explicit layout adapter and share pointer-array decoding. Handle-transfer entries remain two `u32`s. |
| ABI-04 | P0 | [debug ABI structs](../../user/lib/scarlet-abi/src/lib.rs#L91), [kernel snapshots](../../kernel/src/task/mod.rs#L259), [contract tests](../../user/lib/scarlet-abi/tests/native_contract.rs#L65) | Native `usize` IDs coexist with asserted fixed record sizes. `RawTaskDebugInfoV1` demonstrably becomes 56 bytes. `RawCpuDebugInfoV1` can retain its total size through padding while changing field width: size-only tests are insufficient. Define ID widths and explicit padding/layout variants, preserving the 64-bit contract. |
| ABI-05 | P1 | [clock and sleep handlers](../../kernel/src/task/syscall.rs#L1598), [clock wrappers](../../user/lib/scarlet-os/src/time.rs#L14), [thread sleep](../../user/lib/std/src/thread.rs#L239) | Nanoseconds cross one word. A 32-bit monotonic result wraps every 4.294967296 seconds; sleep durations truncate. `SystemTime` widens a 32-bit `usize::MAX` and compares against `u64::MAX`, losing the unavailable sentinel. Specify wide value transport and separate status from data. |
| ABI-06 | P1 | [file handlers](../../kernel/src/object/capability/file/syscall.rs#L22), [file wrappers](../../user/lib/scarlet-os/src/handle/capability/file.rs#L160), [path truncate](../../kernel/src/fs/vfs_v2/syscall.rs#L171) | 64-bit offsets/lengths are cast into one word. On 32-bit, a passed `-1` read as `usize as i64` becomes positive `4294967295`; large seek results truncate. Decode signed values at their specified width and transport full file positions/lengths. |
| ABI-07 | P1 | [`MemoryMap`](../../kernel/src/object/capability/memory_mapping/syscall.rs#L115), [mapping capability](../../kernel/src/object/capability/memory_mapping/mod.rs) | File/object offsets share `usize` with mapping lengths; all six argument words are already used. Define a wide offset path, likely a versioned request record, and keep individual mapping lengths bounded by the address space. |
| ABI-08 | P1 | [scheduler decoding](../../kernel/src/task/syscall.rs#L140), [scheduler wire records](../../user/lib/scarlet-abi/src/lib.rs#L351) | The fixed 128/160-byte scheduler records are a useful starting point, but the encoded `u64` CPU-mask pointer is cast to `usize`. Reject nonrepresentable high bits before access; keep byte-array CPU masks independent of word width. |
| ABI-09 | P1 | [native futex](../../kernel/src/sync/futex.rs#L98), [poll](../../kernel/src/object/capability/selectable/syscall.rs#L88), [poll wrapper](../../user/lib/scarlet-os/src/poll.rs) | Futex timeout is one word with an all-ones sentinel, and its user word is accessed as `AtomicU32`. Poll already carries `i64/u64` time fields in memory, but reads a byte buffer as an aligned struct and multiplies `nfds` without checking. Preserve futex word semantics, define wide timeout transport, and fix decoding/range validation. |
| ABI-10 | P1 | [usercopy](../../kernel/src/library/std/usercopy.rs#L9), [string decoding](../../kernel/src/library/std/string.rs#L67), [Linux pagewise copying](../../kernel/src/abi/linux/generic/fs.rs#L207) | Multiple copy paths use unchecked `address + offset`; pointer arrays also multiply/add native word sizes. Centralize validated user ranges, checked advancement, access permissions, and pagewise copies. Checked narrowing must precede address validation. |
| ABI-11 | P1 | [task structures](../../kernel/src/task/mod.rs#L179), [legacy task wrappers](../../user/lib/std/src/task.rs#L1130), [`top`](../../user/std-bin/src/top.rs#L63), [task manager](../../user/std-bin/src/task_manager.rs#L134) | Snapshot structures are copied/duplicated across kernel and clients with native-width IDs/counts. Migrate all producers and consumers together; shared definitions and complete field-offset tests are needed, not only `scarlet-abi` changes. |
| ABI-12 | P1 | [native syscall table](../../kernel/src/syscall/mod.rs#L208), [published syscall enum](../../user/lib/scarlet-abi/src/lib.rs#L705), [handle control](../../kernel/src/object/handle/syscall.rs) | Numeric dispatch, public definitions, and operation-specific errors are separate. Inventory compatibility-only entries and signed/native sentinel/status encodings. Keep each published convention in its adapter; do not impose one new errno conversion on all calls. |
| ABI-13 | P1 | [native RV64 ABI](../../kernel/src/abi/scarlet/riscv64.rs#L463), [native AArch64 ABI](../../kernel/src/abi/scarlet/aarch64.rs), [IPC syscalls](../../kernel/src/ipc/syscall.rs) | Event delivery combines common policy with register/frame/restorer details in two large implementations. Extract common event state/policy while leaving user frame construction and restoration to the ABI/context adapter. Preserve payload widths and callback execution state. |

### Linux ABI and executable loading

| ID | Priority | Evidence | Problem and required foundation |
| --- | --- | --- | --- |
| LIN-01 | P0 | [generic Linux table](../../kernel/src/abi/linux/generic/mod.rs#L433), [Linux macro](../../kernel/src/abi/linux/generic/macros.rs), [RV64 adapter](../../kernel/src/abi/linux/riscv64/mod.rs#L53) | The common table couples Linux syscall numbers to trapframe-based handlers. Split shared Linux operations from per-ABI syscall numbering and argument decoding. A table shared by the current 64-bit implementations is not a universal 32-bit table. |
| LIN-02 | P1 | [`pread64`, `pwrite64`, `lseek`](../../kernel/src/abi/linux/generic/fs.rs#L1449), [`fallocate`](../../kernel/src/abi/linux/generic/fs.rs#L3006), [`ftruncate`](../../kernel/src/abi/linux/generic/fs.rs#L3303), [`mmap`](../../kernel/src/abi/linux/generic/mm.rs) | A logical 64-bit argument is read from one register; mmap assumes its present offset convention. Provide operation signatures with full offsets and separate register-pair/reordered-argument/offset-unit adapters. Legacy `mmap2` or `_llseek` support, when added, must use its exact Linux ABI. |
| LIN-03 | P1 | [`LinuxStat` / `LinuxStatFs`](../../kernel/src/abi/linux/generic/fs.rs#L289), [`LinuxSysinfo`](../../kernel/src/abi/linux/generic/proc.rs#L879), [time records](../../kernel/src/abi/linux/generic/time.rs), [futex timeout decoding](../../kernel/src/abi/linux/generic/futex.rs#L257) | `stat`/`statfs` explicitly implement the 64-bit asm-generic layout; `sysinfo` has native words with a zero-length historical tail; timespec paths use 64-bit fields. Separate kernel data from ABI-specific stat/stat64/statx, sysinfo, time32/time64, timeval, itimerspec and sigevent encodings. Do not make every 32-bit time structure time32. |
| LIN-04 | P1 | [`LinuxMsghdr` / `LinuxCmsghdr`](../../kernel/src/abi/linux/generic/socket.rs#L250), [`IoVec`](../../kernel/src/abi/linux/generic/fs.rs#L2648), [epoll serialization](../../kernel/src/abi/linux/generic/fs.rs#L132) | `msghdr` embeds 64-bit addresses/lengths while cmsg/iovec use native words; control-message and epoll layout/alignment require ABI-specific handling. Keep socket/file operations common after validating and decoding nested arrays and buffers. |
| LIN-05 | P1 | [`Sigaction` / `SigAltStack`](../../kernel/src/abi/linux/generic/signal.rs#L302), [RV64 signal frame](../../kernel/src/abi/linux/riscv64/signal.rs), [AArch64 signal implementation](../../kernel/src/abi/linux/aarch64/signal.rs) | Generic signal structures encode current assumptions; RV64 uses a custom fixed frame, and AArch64 has explicit signal-frame TODOs/stubs. Isolate layout/state restoration and retain these as existing compatibility gaps; they are not a sound universal frame specification. |
| LIN-06 | P1 | [Linux clone](../../kernel/src/abi/linux/generic/proc.rs#L1124), [robust-list registration](../../kernel/src/abi/linux/generic/proc.rs#L103), [thread state](../../kernel/src/abi/linux/generic/mod.rs#L62), [affinity](../../kernel/src/abi/linux/generic/proc.rs#L131) | TLS/child-TID pointers, clone argument order, robust-list metadata, and CPU-mask words are tied to current callers. Shared process operations should accept normalized requests. Robust-list registration alone is not proof of complete robust-futex semantics. |
| ELF-01 | P0 | [`ElfHeader::parse`](../../kernel/src/task/elf_loader/mod.rs#L283), [`ProgramHeader::parse`](../../kernel/src/task/elf_loader/mod.rs#L343) | The common loader rejects non-ELF64 and parses fixed 64/56-byte headers. Decode ELF32 and ELF64 into a class-independent model; validate class-dependent offsets/sizes and checked file/memory ranges before mapping. Widened decoded `u64` values can remain `u64`. |
| ELF-02 | P1 | [loader header fields](../../kernel/src/task/elf_loader/mod.rs#L217), [Linux recognition](../../kernel/src/abi/linux/riscv64/mod.rs#L141), [native recognition](../../kernel/src/abi/scarlet/riscv64.rs#L692), [ABI registry](../../kernel/src/abi/mod.rs#L514) | Executable `e_machine` and `e_flags` are parsed without a corresponding execution-compatibility check in the loader; recognition scores magic/OSABI/path. Add a binary identity/data-model check before ABI selection commits to execution. Endian-aware parsing is not permission to execute an incompatible binary. |
| ELF-03 | P1 | [`AuxVec`](../../kernel/src/task/elf_loader/mod.rs#L167), [Linux initial stack](../../kernel/src/abi/linux/riscv64/mod.rs#L285), [native initial stack](../../kernel/src/abi/scarlet/riscv64.rs#L1042), [runtime entry](../../user/lib/scarlet-rt/src/arch/riscv64.rs#L41) | Linux writes 8-byte argc/pointers and 16-byte auxv entries. Native code uses host-sized words. Introduce an ABI-aware stack writer for argc/argv/envp/auxv and alignment, with architecture hooks for entry registers/TLS; share safe pagewise writes. |
| ELF-04 | P2 | [LSM parser](../../kernel/src/lsm/elf.rs#L225), [LSM loader](../../kernel/src/lsm/loader.rs#L49), [symbol entries](../../kernel/src/lsm/symbol.rs#L175), [module builds](../../modules/loadable) | LSM uses ELF64 symbols/RELA, architecture relocation modules, a 256 MiB VA reservation, and native Rust symbol addresses. Separate object-format decoding, relocation semantics and VA allocation. Keep module ABI/toolchain compatibility distinct from user syscall ABI. |

Linux adapter contracts must be checked against architecture UAPI, not inferred
from Rust/C function calling conventions. The [ARM syscall header](https://github.com/torvalds/linux/blob/master/arch/arm/include/uapi/asm/unistd.h)
selects EABI/OABI tables and private ARM calls; the [asm-generic table](https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/unistd.h)
contains width/compatibility-dependent selection. The [AAPCS32](https://github.com/ARM-software/abi-aa/blob/main/aapcs32/aapcs32.rst)
and [RISC-V psABI](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/)
are references for procedure/data layout; they do not by themselves specify
Scarlet native syscall argument allocation. New Linux ports remain separate work.

### Address spaces, memory quantities, and storage

| ID | Priority | Evidence | Problem and required foundation |
| --- | --- | --- | --- |
| MEM-01 | P0 | [shared environment constants](../../kernel/src/environment/common.rs#L10), [VMM defaults](../../kernel/src/vm/manager.rs#L70) | Shared code contains 64-bit all-ones limits, upper-half addresses, mmap base at 4 GiB, and a 48-bit canonical limit. Move layout into a validated address-space policy supplied by the existing architecture implementations; use semantic unlimited limits. A future 32-bit policy should not require changing common code. |
| MEM-02 | P0 | [`MemoryArea`](../../kernel/src/vm/vmem.rs#L133), [address translation](../../kernel/src/vm/addr.rs), [`BootInfo`](../../kernel/src/lib.rs#L454), [PMM](../../kernel/src/mem/pmm.rs) | Physical and virtual addresses share inclusive `usize` ranges. Introduce distinct physical/virtual ranges and checked conversions; decide representable range length/endpoints explicitly, including a range reaching the top of a 32-bit address space. Do not globally convert pointers to `u64`. |
| MEM-03 | P0 | [common boot mapping](../../kernel/src/vm/boot.rs#L1), [common VMM](../../kernel/src/vm/manager.rs#L51), [direct-map metadata](../../kernel/src/vm/direct_map.rs), [memory initialization](../../kernel/src/vm/mod.rs) | Common code imports concrete PTE/page-table representations and assumes boot table allocation from one `PAGE_SIZE` frame. Direct mapping and heap rebasing are tightly coupled to the existing layout. Define page-table allocation/geometry, address validation, mapping attributes, TLB and cache synchronization as architecture services. A future table need not have the current size/levels. |
| MEM-04 | P1 | [heap/stack/ioremap capacities](../../kernel/src/environment/common.rs#L3), [module VA size](../../kernel/src/lsm/loader.rs#L53), [anonymous mapping](../../kernel/src/object/capability/memory_mapping/syscall.rs#L140) | 512 MiB heap, 1 GiB ioremap window, per-task stacks, and module reservations share the address budget. Move capacities to platform/layout policy and validate non-overlap. Check align-up, page-count multiplication, and allocation limits before effects. |
| MEM-05 | P1 | [`FileMetadata.size`](../../kernel/src/fs/mod.rs#L342), [ext2 node](../../kernel/src/fs/vfs_v2/drivers/ext2/node.rs#L126), [mapping capability](../../kernel/src/object/capability/memory_mapping/mod.rs), [page cache](../../kernel/src/mem/page_cache.rs) | A 64-bit syscall fix would still lose large file sizes in shared VFS `usize` metadata and downstream casts. Keep file size, object position and page-cache object offsets wide; use checked `usize` only for a bounded resident window, buffer or allocation. |
| MEM-06 | P1 | [block interface](../../kernel/src/device/block/mod.rs), [block requests](../../kernel/src/device/block/request.rs), [VirtIO block capacity](../../kernel/src/drivers/block/virtio_blk.rs#L835), [partition handling](../../kernel/src/device/block/partition.rs#L111) | Disk capacity in bytes and sector addressing flow through native-sized APIs; capacity multiplication is cast to `usize`. Define wide disk quantities and bounded transfer lengths. Some partition conversions already reject overflow and should be preserved as explicit limits. |
| MEM-07 | P1 | [FDT](../../kernel/src/device/fdt.rs), [platform resources](../../kernel/src/device/platform/resource.rs#L13), [PCI BAR decoding](../../kernel/src/device/pci/config.rs#L611), [VirtIO PCI map](../../kernel/src/drivers/virtio/pci.rs#L185) | Firmware/PCI resource widths are not CPU pointer widths. Keep full resource addresses until a checked mapping decision. VirtIO PCI already uses `usize::try_from` for a map address; extend the explicit policy rather than replacing it with truncating casts. |

### Synchronization and execution capabilities

| ID | Priority | Evidence | Problem and required foundation |
| --- | --- | --- | --- |
| SYN-01 | P0 | [spinlock](../../kernel/src/sync/spinlock.rs#L17), [RW spinlock](../../kernel/src/sync/rw_spinlock.rs#L13), [Once](../../kernel/src/sync/once.rs), [preemption](../../kernel/src/sync/preempt.rs) | Basic synchronization directly needs atomic types/CAS, not just 64-bit counters. Define a lowest-level backend contract before implementing a fallback. An atomic fallback that acquires these same locks would recurse. |
| SYN-02 | P0 | [task ownership](../../kernel/src/task/mod.rs), [VMM ownership](../../kernel/src/vm/manager.rs#L90), [object graph](../../kernel/src/object/mod.rs), [wakers](../../kernel/src/sync/waker.rs) | `alloc::sync::{Arc, Weak}` is pervasive and unavailable on the tested no-pointer-atomic target. Introduce a shared-ownership provider boundary with the required semantics. Inventory includes `new_cyclic`, `downcast`, `downgrade`, `ptr_eq`, strong counts, unsizing to trait objects, and ordinary clone/drop. `Rc` is not a general `Send + Sync` replacement. |
| SYN-03 | P0 | [scheduler](../../kernel/src/sched/scheduler.rs#L82), [task time accounting](../../kernel/src/task/mod.rs#L867), [timer](../../kernel/src/timer.rs#L145), [wall clock](../../kernel/src/time.rs#L50) | Wide clocks, deadlines, tokens and counters use `AtomicU64`. Categorize publication/snapshot, unique-ID, accounting, and RMW needs. Preserve 64-bit values with appropriate synchronization; shrinking to `AtomicUsize` changes wraparound and timing semantics. |
| SYN-04 | P1 | [breadcrumbs](../../kernel/src/breadcrumb.rs#L161), [preempt diagnostics](../../kernel/src/sync/preempt.rs#L148), [timer diagnostics](../../kernel/src/timer.rs#L208), [scheduler diagnostics](../../kernel/src/sched/scheduler.rs#L333) | Interrupt and lock-stall observation expects progress without taking the observed lock. Define bounded snapshot/retry or explicit unavailable results; a blocking fallback or a reader spinning forever on an interrupted writer is unsuitable here. |
| SYN-05 | P1 | [VFS identity](../../kernel/src/fs/vfs_v2/core.rs#L43), [network TCP](../../kernel/src/network/tcp.rs#L393), [sensor sequences](../../kernel/src/device/sensor.rs#L271), [random state](../../kernel/src/random.rs#L48), [MMC](../../kernel/src/drivers/mmc/core.rs#L111), [xHCI](../../kernel/src/drivers/usb/xhci/mod.rs#L760) | Atomic64 dependencies extend beyond scheduling. Audit allocation, teardown, interrupt usage and sequence wrap separately for these users. See the full scanner output for all 32 files, including user applications and tests. |
| SYN-06 | P1 | [IRQ guard](../../kernel/src/sync/irq_guard.rs), [CPU-local storage](../../kernel/src/sync/cpu_local.rs), [CPU masks](../../kernel/src/sched/scheduler.rs#L1019), [native futex user access](../../kernel/src/sync/futex.rs#L53) | UP/SMP and kernel/user synchronization need separate guarantees. IRQ masking can provide a kernel UP critical section only with the relevant interrupt classes/preemption controlled; it does not serialize other CPUs or create a userland atomic implementation. Keep CPU-set representation and runtime capabilities explicit. |

The reference-counting provider is a design dependency, not a request to write a
new general-purpose `Arc`. An existing implementation must be evaluated against
these operations, allocator integration, trait-object coercions, reference-count
overflow handling, memory ordering, and supported execution contexts before
adoption. Likewise, adding a generic atomic crate is insufficient without its
required platform backend and a separate userland strategy.

### Device/wire contracts, optional code, and integration

| ID | Priority | Evidence | Problem and required foundation |
| --- | --- | --- | --- |
| DEV-01 | P0 | [`device::dma::DmaAddr`](../../kernel/src/device/dma/mod.rs#L14), [`device::iommu` address types](../../kernel/src/device/iommu/mod.rs#L15) | `DmaAddr` means `usize` in one subsystem and `u64` in another. Establish separate kernel VA, CPU PA and device/IOVA types plus checked mapping/transfer constraints. |
| DEV-02 | P1 | [RV64 MMIO helpers](../../kernel/src/arch/riscv64/mmio.rs#L64), [VirtIO PCI register writes](../../kernel/src/drivers/virtio/device/mod.rs#L593), [xHCI registers](../../kernel/src/drivers/usb/xhci/registers.rs) | A `u64` volatile access does not specify a valid pair of 32-bit device transactions. Define device-required access ordering/latching or native-width capability. Keep hardware register widths fixed; do not implement every 64-bit register by one generic split helper. |
| DEV-03 | P1 | [VirtIO queues](../../kernel/src/drivers/virtio/queue.rs), [IOMMU DMA mappings](../../kernel/src/device/iommu/mod.rs#L299), [GPU submission](../../kernel/src/device/gpu/submission.rs), [audio DMA state](../../kernel/src/device/audio/mod.rs) | DMA buffers currently rely on allocation/address/cache behavior of the existing backends. Expose alignment, address mask, coherent/noncoherent synchronization and mapping lifetime requirements. This is a common contract; future cache instructions are port work. |
| DEV-04 | P1 | [display UAPI](../../kernel/src/device/graphics/display_device.rs#L75), [framebuffer client](../../user/lib/framebuffer/src/lib.rs#L123), [framebuffer info](../../kernel/src/device/graphics/framebuffer_device.rs#L164), [network requests](../../kernel/src/network/syscall.rs#L117) | Duplicated public records contain `usize` pointers, IDs or offsets. Add explicit layout definitions/codecs and jointly migrate users. Small TTY/PTY packed scalar controls need separate range checks, not an automatic 64-bit redesign. |
| DEV-05 | P1 | [GPU UAPI](../../kernel/src/device/gpu/abi.rs), [async GPU UAPI](../../kernel/src/device/gpu/async_abi.rs), [video UAPI](../../kernel/src/device/video.rs#L2316), [GPU usercopy](../../kernel/src/device/gpu/connection.rs#L505) | Explicit-width GPU/video records are useful existing contracts, but pointer/length conversion and raw-copy helpers still require overflow, padding, alignment and valid-value review. Cover synchronous, asynchronous and imported-buffer paths together. |
| DEV-06 | P1 | [sensor encoding](../../kernel/src/device/sensor.rs#L408), [input events](../../kernel/src/device/input/mod.rs), [SAS protocol](../../user/lib/sas-protocol/src/lib.rs#L127), [SWS wire tests](../../user/lib/sws-protocol/tests/wire_contract.rs), [SBus messages](../../user/lib/sbus/src/message.rs) | Service protocols and shared-memory rings must retain field widths, byte order and publication semantics. Sensor's explicit zeroed serialization is a useful pattern. Add cross-layout fixtures and audit counter access, not only record size. |
| OPT-01 | P2 | [arch exports](../../kernel/src/arch/mod.rs#L49), [environment selection](../../kernel/src/environment.rs), [RV64 context](../../kernel/src/arch/riscv64/context.rs), [AArch64 context](../../kernel/src/arch/aarch64/context.rs) | Existing arch directories legitimately contain fixed register sizes, assembly offsets, page tables and FPU/vector state. Define the common interface using the two current implementations; adding ARMv5TE/RV32 assembly, tables, boot code and context switches is later port work. |
| OPT-02 | P2 | [kernel features](../../kernel/Cargo.toml), [SHV aliases](../../kernel/src/hypervisor/mod.rs#L72), [KVM adapters](../../kernel/src/abi/linux/device/kvm/mod.rs), [userspace SHV](../../user/lib/scarlet-os/src/hypervisor/arch/mod.rs) | Hardware virtualization is enabled by default and types depend on existing arch backends. Make unsupported services cleanly excludable with defined errors, while preserving current 64-bit guest/register wire formats. A portable foundation does not imply hardware virtualization on every CPU. |
| OPT-03 | P2 | [runtime selection](../../user/lib/scarlet-rt/src/arch/mod.rs), [`scarlet-sys` selection](../../user/lib/scarlet-sys/src/lib.rs#L51), [legacy thread magic](../../user/lib/std/src/thread.rs#L56) | Only two current architectures are selected; a 64-bit magic constant is typed `usize`. Separate common runtime logic from entry/TLS/restorer/syscall assembly and use explicitly sized metadata where needed. Do not copy an existing assembly file behind a broader cfg. |
| OPT-04 | P2 | [target specs](../../kernel/targets), [user target specs](../../user/targets), [BSPs](../../projects), [root tasks](../../Makefile.toml), [CI](../../.github/workflows/ci.yml#L121) | Build/run/CI are oriented to existing 64-bit projects. Add compile/layout gates for extracted common code without pretending there is a new bootable target. Actual BSP/image/firmware/runner work remains a distinct milestone. |
| OPT-05 | P2 | [flake dependencies](../../flake.nix#L9), [workspace patches](../../.cargo/Cargo.toml), [Rust development helper](../../scripts/scarlet-rust-dev.sh#L75), [bundles](../../bundles) | Scarlet Rust `std`, SDK, SGFX/UI, patched networking/crypto libraries, native C dependencies, and packaged Linux binaries have their own support matrices. Record and validate these dependency edges separately. No conclusion about their internal 32-bit support is made here. |

Inactive examples are retained in the candidate inventory: for example,
[`drivers/pic/clint.rs`](../../kernel/src/drivers/pic/clint.rs#L117) reads 64-bit
timer MMIO, but its module is currently commented out in
[`drivers/pic/mod.rs`](../../kernel/src/drivers/pic/mod.rs#L6). This is future reuse
work, not an active generic-kernel blocker.

## Refactoring design

### ABI decoding around typed common operations

Establish four boundaries without changing the current public numbers or
64-bit binary layouts:

1. The architecture adapter captures register words and owns applying a final
   continuation/result to the trapframe.
2. The selected ABI adapter decodes those words and user-memory records into
   typed requests: handles/IDs, validated user pointers, signed offsets, lengths,
   timeouts, flags and wide values. Its description includes word width, byte
   order, alignment, register-pair allocation and return conventions.
3. Common operations accept these requests without knowing syscall numbers or
   manipulating a trapframe. Blocking/exec/event-return operations explicitly
   represent their continuation needs.
4. The ABI adapter encodes results, error conventions and output records. A
   successful 64-bit value and an error sentinel must never become ambiguous
   because of truncation.

The initial extraction should use the existing 64-bit adapters. It must allow
testing a 32-bit data model without adding a CPU backend. Kernel pointer width
must not be used as a substitute for a user record's specified layout. This
separation leaves room for later compatibility modes but does not implement one.

For native records already defined in native words, specify explicit 32/64
layouts and adapters rather than silently changing published fields. For new
records, prefer explicit-width fields, reserved/padding bytes, and size/version
validation. A fixed-width `u64` pointer slot is acceptable only if upper bits and
the caller's address range are validated before narrowing. Rust `repr(C)` alone
does not define the complete wire contract.

The first wide-value design exercise must cover clocks, signed seek/truncate,
futex timeout, and the six-word mmap call together. Compare register-pair versus
request/output-record transport against these cases before assigning any new
native syscall numbers. Linux adapters must follow Linux, independently of that
native design decision.

### Address and memory policy

Start with a small set of semantic boundaries, not a global generic rewrite:

- A kernel virtual address wraps `usize`; physical/device addresses preserve
  their full supported representation, with checked conversion at mapping edges.
- File sizes and file/object offsets remain 64-bit. A resident buffer's length
  is `usize` with range and `isize::MAX` allocation/slice restrictions where needed.
- Separate physical and virtual ranges. Establish checked endpoint/length
  operations before changing existing inclusive range users.
- A layout policy supplies user address bounds, stack/mmap placement, kernel
  direct-map/heap/ioremap/module ranges and resource budgets. Validate it once
  and use it across boot, VMM and ELF/stack placement.
- An MMU interface owns table allocation requirements and geometry, entry
  encoding, attributes, translation, invalidation and cache effects. Keep the
  existing RV64/AArch64 implementations intact behind it initially.

Wide physical representation does not require mapping every physical address
into a 32-bit direct map. Unsupported/unmapped ranges must remain distinguishable
from truncated addresses; a future high-memory strategy is a separate decision.

### Synchronization foundation

Define a primitive backend below preemption-aware locks, Once, diagnostics and
shared ownership. Capability selection must express at least pointer atomics,
64-bit atomics, and the synchronization environment. Do not falsify a target's
atomic capabilities merely to make `Arc` compile.

Preserve the current native atomic implementation on supported targets. For a
no-CAS design, first specify critical-section entry/exit, nesting, boot-time
availability, IRQ/FIQ or equivalent interrupt coverage, ordering, and UP/SMP
restrictions. Supply and test a common provider seam; actual privileged
instructions belong to a future architecture implementation.

Refactor wide state by semantics: a unique-ID allocator, a guarded clock value,
an accounting counter and a diagnostic snapshot need not use the same fallback.
Diagnostic readers must not wait indefinitely for a writer they interrupted.
Locks, the atomic fallback, preemption bookkeeping and diagnostics must have an
acyclic dependency graph. Userland cannot inherit a privileged IRQ-masking
backend; its shared-memory/futex/reference-counting strategy is an explicit
dependency of the eventual runtime work.

### Preserve legitimate 64-bit data

These scan matches are not candidates for automatic shrinking:

- VirtIO's queue addresses/features and PCI's 64-bit BARs. The queue-address
  getters already return `Option<u64>`, so their `>> 32` operations are valid.
- Disk formats, file offsets, filesystem IDs and cache IDs constructed in `u64`.
- Nanosecond clocks, deadlines, wide accounting, and intermediate `u128` timer
  calculations.
- GPU/video/sensor/SWS/SAS/SBus protocol fields whose contracts specify a width.
- Existing 64-bit CPU registers, PTEs, guest state and architecture assembly.

Likewise, local slice indices, Rust pointers and checked allocation sizes should
not all become `u64`. The requirement is to separate meanings and check the
conversions between them.

## Implementation sequence and acceptance criteria

Each step should be reviewable on the existing supported systems. Later work
must not relabel an excluded subsystem or a passing host test as a completed port.

| Step | Work | Prerequisites | Evidence required to complete the step |
| --- | --- | --- | --- |
| 1 | Freeze current native syscall/layout fixtures; introduce testable ABI data-model and checked scalar/user-range utilities. | This inventory. | Existing 64-bit fixtures unchanged; independent 32/64 byte fixtures cover sign extension, high-bit rejection, overflow and alignment. Common utilities compile for ARMv5TE and RV32 without arch stubs. |
| 2 | Specify and introduce primitive synchronization/shared-ownership provider boundaries. | Execution-context contract, including the no-pointer-atomic case. | Current atomics/Arc behavior retained; required ownership APIs supported; lock/fallback dependency graph checked; a no-CAS compile harness does not depend on `alloc::sync::Arc`. No untested SMP fallback is advertised. |
| 3 | Split kernel VA, PA/device address and file quantity boundaries; move current memory layouts behind policy. | Checked range/conversion utilities. | Current RV64/AArch64 layouts match; a synthetic 32-bit layout has valid nonoverlapping ranges and explicit limits; >4 GiB file/resource cases reject or preserve values as specified. |
| 4 | Extract native syscall decode/encode and common operations; define wide transport; reconcile duplicate UAPI records. | ABI fixtures; address and synchronization boundaries. | Clock/seek/futex/mmap probes retain wide values; signed failures and successful results remain distinct; all entries in the native checklist are accounted for; current 64-bit user programs retain behavior. |
| 5 | Split ELF class parsing/stack writing and Linux common semantics from ABI codecs. | Steps 1, 3 and syscall request/result boundaries. | ELF32/ELF64 fixtures, malformed/truncated/overflowing input, incompatible machine/flags, pointer-array boundaries, time32/time64 and nested-message records tested without requiring a new port. |
| 6 | Consolidate device/wire contracts and isolate optional runtime/LSM/SHV/build dependencies. | Address policy, codecs and capability boundaries. | Device bytes stay unchanged where specified; pointer/length conversions checked; optional features compile out cleanly on the common-code harness; dependency limitations recorded. |
| 7 | Add maintained foundation CI gates alongside existing 64-bit kernel tests. | Extracted modules from earlier steps. | Cross-check actual common production modules on RV32 and ARMv5TE; do not test only a duplicate model or cfg away everything. Existing `test-riscv64` and `test-aarch64` pass for runtime changes. |

Boundary test cases should include `0`, `0x7fffffff`, `0x80000000`, `0xffffffff`,
`0x1_0000_0000`, negative offsets/errno, all-ones sentinels, >4.3-second durations,
>4 GiB file positions and disk capacities, last-page buffers, cross-page pointer
arrays, unaligned records, and checked length/alignment overflow. Test clock and
snapshot consistency under interruption in the synchronization backend harness.
Use literal byte/offset expectations, not expected values computed from the
implementation's own `size_of`.

The foundation milestone is reached when common code has explicit contracts
for these dimensions, the common components can be checked against both 32-bit
constraint sets, and existing 64-bit behavior remains validated. Choosing boards,
writing ARMv5TE/RV32 boot/trap/MMU backends, producing images, and booting new
systems are subsequent port projects.
