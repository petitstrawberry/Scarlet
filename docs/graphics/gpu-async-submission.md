# Asynchronous GPU submission: implementation boundary

Status (2026-09-06): generic kernel admission, read-only completion capabilities,
`gpu-raw` wrappers, and **real VirtIO/VirGL asynchronous execution** are implemented.
VirtIO queues advertise 16 retained submissions, with a shared device-wide bound
of 16; existing synchronous operations remain available. **A618 still reports
zero async capacity.** Native SGFX/facade receipts and the SWS/ScarletUI VirGL
consumer paths are implemented. On 2026-09-06 the user confirmed normal rendering
after the local vertex-storage repair described below.
This is not a performance claim or completion of the coordinated 1.0 release gate.

The user approved portable completion tracking and actual asynchronous Scarlet
execution for 1.0; the portable semantics are recorded in the
[SGFX completion contract](https://github.com/petitstrawberry/sgfx/blob/main/docs/completion-contract.md).
Scarlet currently patches SGFX and ScarletUI to the sibling local checkouts
while repairing the consumer rendering regression. The experimental diagnostic
also builds the sibling SGFX checkout so it exercises the same repair.
SGFX's own lockfile includes the asynchronous Scarlet GPU transport at
`f7adec91`. The native adapter stages each bounded logical stream before a
single admission and restores CPU initialization/revision state on rejection.

## Additive ABI

`GPU_ABI_VERSION` remains 1. Existing synchronous `GPU_QUEUE_SUBMIT`, writable
timelines, record layouts, and result meanings are unchanged. New operations:

| Control | Code | Fixed-width record | Meaning |
| --- | --- | --- | --- |
| `GPU_COMPLETION_QUERY` | `0x4769` | `GpuCompletionInfo`, 24 bytes | Read pending, complete, or failed state; no userspace signal operation |
| `GPU_QUEUE_QUERY_ASYNC` | `0x476a` | `GpuQueueAsyncInfo`, 24 bytes | Query implemented per-queue async limits; zero capacity means unsupported |
| `GPU_QUEUE_SUBMIT_ASYNC` | `0x476b` | `GpuQueueSubmitAsync`, 40 bytes | Copy and enqueue owned commands; return after acceptance, without waiting for GPU completion |

The async command limit is independent of the legacy synchronous staging limit.
VirtIO accepts up to the existing generic **2 MiB** bound in one owned async
request, while synchronous submits retain their **64 KiB** limit. This lets
SGFX lower multiple native packets into one admission operation: contention
rejects the entire stream before acceptance, instead of stranding an uploaded
prefix between separate admissions. The DMA allocation and the read-only
completion still belong to the kernel until retirement.

Async submission separates `accepted` from `result`:

- `accepted == 0`: no work from this call was accepted. Busy (`result == 6`)
  is retryable capacity pressure, not a request to wait inside the syscall.
- `accepted == 1`: the returned completion handle covers all possibly accepted
  work and preceding work ordered on that backend queue, even when `result` is
  nonzero. Handle **zero is valid**; use acceptance, not a handle sentinel.
- An empty command stream is a queue checkpoint. It is not an immediate success
  receipt or a CPU wait, and still consumes admission capacity until retired.
- A control/transport failure is not a side-effect-free rejection. If the
  response cannot be published, the kernel closes its undelivered handle while
  the driver continues to own accepted work. `gpu-raw::GpuSubmitError::Failed`
  can have `completion: None` when observation could not be delivered/adopted.
  The SGFX adapter propagates that uncertainty as a failed observation,
  not certify the unknown work using only an older chunk's successful receipt.

Completion is terminal and read-only. Read/exception readiness means a terminal
observation exists, not that it succeeded; callers must query the state.
Write readiness is never reported. Complete certifies GPU-access retirement,
not pixel correctness, cache visibility, readback, presentation, or SWS release.
Failure reasons distinguish device loss, producer abandonment, and other
execution failure. None certifies that hardware has stopped accessing backing.
Dropping the unique kernel producer cannot report successful completion.

The selectable wait rechecks readiness after registration. This also covers
multiple observers that race a broadcast after another observer has consumed
the Waker's single coalesced notification. Timeout zero is a readiness query;
finite waits use a deadline, not an unbounded GPU wait.

## SWS and ScarletUI handoff

On VirGL, the paint encoder and SWS quad compositor use a frame-scoped tracked
executor. It retains up to 16 receipts, waits for the oldest only at capacity
pressure, and retries only a proven `Busy` rejection of the current stream.
The consumer retry policy is bounded; no accepted frame or failed prefix is
replayed. Large texture uploads are split into bounded row strips before native
lowering. SGFX itself never waits to make submission capacity available.

ScarletUI observes the whole frame before committing its shared image to SWS.
SWS observes its composition before display presentation and only then promotes
the presented frame and acknowledges eligible old commit tokens. SWS release
is still a separate requirement before a producer reuses its slot. There is no
new cross-process GPU fence transfer protocol: this is a frame-handoff wait,
not a claim that the entire render loop is nonblocking.

Admission/observation failure prevents image handoff and future reuse of the
uncertain frame's cache. SWS invalidates shared-image epochs instead of sending
a successful release; accepted resources remain independently retained by the
kernel. Adreno retains its explicit synchronous consumer path while it reports
zero async capacity; no already-complete receipt or silent fallback is used.

## Kernel ownership and driver obligations

Generic admission is bounded to the lesser of the backend's advertised limit
and 32 retained submissions per queue. This does not replace a driver's shared
device/transport pool bound. Closing observer handles never frees an in-flight
slot. Admission uses nonblocking attachment-lock acquisition and a cached
command-size limit rather than waiting behind legacy GPU operations.

Before enqueue, the kernel reserves the response handle and snapshots attached
images/buffers while both attachment locks are held. `GpuSubmission` owns copied
command bytes, backend/context authority, generic backing references (including
import pins), and its admission permit. The driver receives it before the
syscall returns. Closing queue/context/resource/process handles or a response
copy failure cannot remove that ownership.

`GpuBackendQueue::enqueue` must:

1. Validate the opaque dialect and attachment authority while generic attachment
   locks are held; publish no unauthorized commands. Return the entire request
   in Busy/Rejected only if nothing was accepted, so its resources can be freed.
2. Retain accepted requests in an independently driven in-flight queue. Returning
   Failed after a possible prefix still requires retained work and an observable
   completion. Do not require the submitting process or receipt to remain alive.
3. Preserve GPU mappings/authority across later detach. Generic backing references
   alone do not preserve a driver's IOMMU mappings or hardware context bindings.
   Driver command/response DMA, fence, and staging allocations must also survive.
4. Call `complete` only after covered accesses and preceding queue work retire.
   `fail` reports an error but keeps backing and capacity; `retire_failed` releases
   them only after hardware quiescence/reset. Run retirement outside locks that
   resource/context destruction can re-enter, in a context permitting that work.
5. Bound shared transport storage and return Busy without waiting for a free slot.
   Preserve ordering with existing synchronous submission and upload/readback
   operations, including when both interfaces share a device.

Dropping an unretired `GpuSubmission` is a fail-safe: its generic command storage,
resource references, and slot are permanently quarantined rather than freed.
It is not a normal retirement strategy and does not automatically quarantine a
driver's separate DMA/staging allocations. Backends must still retain those.

## Verification and remaining work

### VirtIO/VirGL transport and lifetime

The control queue retains command/response DMA independently of callers and
matches used entries to descriptor heads, including out-of-order responses.
Publication and consumption use architecture I/O barriers. Invalid used IDs,
invalid response lengths, timeouts, and failed checkpoint fences permanently
stop new control work and quarantine unretired DMA, backing and admission slots.
Installed queue memory is also retained when no reset proof exists. Recovery
and actual fault/reset injection remain open; quarantine is not a reset path.

A nonempty async submission publishes two chains in one available-ring update:
the payload and an independent empty fenced `SUBMIT_3D` checkpoint. An empty
submission publishes only the checkpoint. Both storage and ring capacity are
reserved before publication, so Busy/rejection cannot accept a prefix. VirGL's
legacy fence callback is 32-bit; fence IDs are rejected at exhaustion instead
of wrapping into earlier work.

A command response alone need not establish GPU retirement; VirtIO requires a
fence for that observation. Moreover, QEMU's VirGL error path can respond before
creating an execution fence. The independent checkpoint lets a failed payload
retire its possibly accepted prefix without poisoning the queue; a malformed or
failed checkpoint instead requires quarantine. See the
[VirtIO GPU fencing requirements, sections 5.7.6.5 and 5.7.6.7](https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html)
and [QEMU's VirGL command/error/fence path](https://github.com/qemu/qemu/blob/v10.1.0/hw/display/virtio-gpu-virgl.c#L884).

The completion worker is registered before async support is advertised. PCI
INTx and platform MMIO IRQ handlers acknowledge the device and wake it without
taking the synchronous core lock. While work is pending, a 1 ms timed wake also
drives progress and timeout handling without userspace polling. The worker
retires owners in publication order, outside locks that destructors can re-enter.
Dropping every user receipt, queue/context handle or process does not cancel
accepted work or remove the worker's ownership.

Legacy control calls remain synchronous: before issuing their commands they
drain preceding control requests and validate the async retirement checkpoints.
In particular, detach does not remove a hardware context binding before GPU
accesses retire. Upload/readback/presentation and synchronous submit therefore
remain ordered with async work. Async enqueue uses `try_lock` and returns Busy
when a legacy operation owns the core; it does not wait behind that GPU operation.

### Evidence

With the pinned `scarlet-rust-toolchain` (`scarlet-rust-nix` `2b4ddd55`):

- Kernel suites pass 1,189 RISC-V and 1,160 AArch64 tests. New deterministic cases
  cover read-only authority, producer loss, readiness races, bounded admission,
  partial acceptance, detached backing, failed response publication, full handle
  tables, and unretired-request quarantine. The VirtIO additions exercise owned
  DMA, out-of-order used entries, atomic paired admission, duplicate publication,
  failed payload retirement, malformed checkpoints, autonomous-owner teardown,
  and a 192 KiB owned stream admitted atomically beyond the legacy staging limit.
- ScarletUI's renderer passes 35 host tests, including frame-wide observation,
  bounded receipts, Busy-only retry, permanent observation-failure invalidation,
  and padded texture strips. Its SWS platform checks on both normal Scarlet std
  targets and AArch64 legacy std. Native SGFX builds on both architectures;
  the native VirGL harness compiles but has not run for this revision.
- The user ran the six-scenario `sgfx-native-completion-smoke` successfully
  15 times, including oversized rejection and initialization rollback. Gears,
  mesh swarm, and ordinary UI nevertheless showed rendering corruption.
  Native inline vertex uploads were reusing storage while earlier draws could
  still read it. The local SGFX repair uses completion-retained upload arenas
  and ordered GPU copies; it does not restore per-submit waits. A seventh
  diagnostic checks all intermediate colored strips across scratch-buffer
  and persistent-buffer reuse, not only the last draw's color. The new binary
  builds, and the complete AArch64 release image builds successfully. The user
  subsequently confirmed normal operation, closing the reported rendering
  regression. The seventh diagnostic's individual results were not separately
  reported. Runtime verification remains user-operated.
- The opt-in [`gpu-async-smoke`](../../user/std-bin/src/gpu_async_smoke.rs) passes
  check and strict Clippy on both normal Scarlet std targets. Real AArch64 QEMU
  release-image runs pass with `virtio-gpu-gl-pci` (two CPUs) and
  `virtio-gpu-gl-device` / MMIO (one CPU), using TCG and Cocoa GL. The probe checks
  all pixels of a 16x16 async red clear via explicit readback, ordered checkpoints,
  immediate detach after queued drawing, 32 dropped receipts, and completion
  after closing all queue/context/image/connection handles. Both runs report
  `[gpu-async-smoke] ALL PASS` and exit zero alongside the existing desktop.
- `gpu-raw` checks and strict Clippy pass for both normal Scarlet **std** targets.
  Its two pure response-classification/request tests and all-target Clippy pass
  in an AArch64 Linux harness with Scarlet Rust. This is not a claim that its
  Scarlet syscalls run on Linux, or that the native crate supports x86 hosts.
- Kernel strict Clippy remains blocked by the same 1,110 existing diagnostics;
  the new code does not waive them. Native-only ELF assembly prevents running
  the `gpu-raw` crate's test harness on macOS without additional platform work.

The reported gear/swarm/UI regression is closed by the user's normal-operation
confirmation. The seventh diagnostic is available for user-operated regression
checks. A618 staging/fence retirement remains separate.
Driver fault/reset and A618 hardware
evidence remain required. Preserve the user's accepted current QEMU runtime
baseline; the historical debug-build delay is not reopened here.
