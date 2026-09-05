# Asynchronous GPU submission: implementation boundary

Status (2026-09-06): generic kernel admission, read-only completion capabilities,
and `gpu-raw` wrappers are implemented. **VirtIO/VirGL and A618 still use their
existing synchronous implementations and report zero async capacity.** Native
SGFX/facade and consumer integration remain open. This is not a performance
claim or completion of the coordinated 1.0 release gate.

The user approved portable completion tracking and actual asynchronous Scarlet
execution for 1.0; the portable semantics are recorded in the
[SGFX completion contract](https://github.com/petitstrawberry/sgfx/blob/90eb0641cf3dbdb415db6c48d81f7bf224d99984/docs/completion-contract.md).
SGFX core/WGPU/host facade are implemented at that revision. It is newer than
Scarlet's consumer lockfile; this change does not update those dependencies.

## Additive ABI

`GPU_ABI_VERSION` remains 1. Existing synchronous `GPU_QUEUE_SUBMIT`, writable
timelines, record layouts, and result meanings are unchanged. New operations:

| Control | Code | Fixed-width record | Meaning |
| --- | --- | --- | --- |
| `GPU_COMPLETION_QUERY` | `0x4769` | `GpuCompletionInfo`, 24 bytes | Read pending, complete, or failed state; no userspace signal operation |
| `GPU_QUEUE_QUERY_ASYNC` | `0x476a` | `GpuQueueAsyncInfo`, 24 bytes | Query implemented per-queue async limits; zero capacity means unsupported |
| `GPU_QUEUE_SUBMIT_ASYNC` | `0x476b` | `GpuQueueSubmitAsync`, 40 bytes | Copy and enqueue owned commands; return after acceptance, without waiting for GPU completion |

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
  A future SGFX adapter must propagate that uncertainty as a failed observation,
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

With the pinned `scarlet-rust-toolchain` (`scarlet-rust-nix` `2b4ddd55`):

- Kernel suites pass 1,176 RISC-V and 1,147 AArch64 tests. New deterministic cases
  cover read-only authority, producer loss, readiness races, bounded admission,
  partial acceptance, detached backing, failed response publication, full handle
  tables, and unretired-request quarantine.
- Both full builds pass in the clean verification tree; source copies preserve
  the user's modified project lock in the working checkout. Root formatting passes.
- `gpu-raw` checks and strict Clippy pass for both normal Scarlet **std** targets.
  Its two pure response-classification/request tests and all-target Clippy pass
  in an AArch64 Linux harness with Scarlet Rust. This is not a claim that its
  Scarlet syscalls run on Linux, or that the native crate supports x86 hosts.
- Kernel strict Clippy remains blocked by the same 1,110 existing diagnostics;
  the new code does not waive them. Native-only ELF assembly prevents running
  the `gpu-raw` crate's test harness on macOS without additional platform work.

Next: implement real VirtIO asynchronous control-queue ownership/completion,
then A618 staging/fence retirement, native SGFX receipts and chunk failure
tracking, and SWS/UI resource lifetime integration. Driver fault/reset and A618
hardware evidence remain required. Preserve the user's accepted current QEMU
runtime baseline; the historical debug-build delay is not reopened here.
