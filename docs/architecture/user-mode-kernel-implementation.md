# User-mode kernel implementation plan

This plan implements the [user-mode kernel design](user-mode-kernel.md) using
ELF-loader SVC-to-BRK replacement. It is based on Scarlet commit
`dd6c3960535670b819ed35c639e0ed9b48f716b4`. It describes proposed changes;
the hosted runtime and the acceptance tests below have not been implemented
or executed.

The replacement boundary is the execution platform plus concrete device and
memory implementations. Scarlet retains its kernel objects, VFS, network
stack, IPC, scheduler, and wait protocols. Existing public constructors and
trait-object boundaries remain the entry points.

## Required first delivery

The first supported hosted system runs the existing full distribution with
multiple vCPUs. Configure `2..=MAX_NUM_CPUS`, with four vCPUs as the default,
matching the [AArch64 full runner](../../projects/aarch64-limine-full/tools/run_aarch64.sh).
All configured vCPUs have working CPU-local state, timers, IPI delivery, idle
Tasks, and scheduler entry before initial Task execution.

The distribution reference is the
[full project](../../projects/aarch64-limine-full/scarlet.toml) and the entire
[`bundles/full` dependency graph](../../bundles/full/bundle.toml):
desktop, Debian userspace, experimental applications, native Rust toolchain,
and rust-analyzer, including their architecture-filtered transitive bundles.
Keep the pinned application contents, enabled modules, normal init/service
configuration, and native/Linux ABI paths. Add host BSP configuration and the
hosted drivers without deleting services or reducing the bundle to boot it.

The first acceptance target includes storage, network/DNS, accelerated
graphics, display/input, audio, video, and the normal console/desktop sessions
operating together. The full service graph includes
[`sas -> scarlet-seat`](../../bundles/desktop/fs/etc/stemd.d/services/01-sws.toml);
its audio dependency must work. The desktop bundle also installs native
Vulkan, so the hosted SGFX driver must supply DriverApi as well as BackendApi.

Implementation work can be divided by dependency, but the acceptance target
is one complete SMP system. Console-only startup, a single running guest
thread per address space, or later delivery of required full-system devices
does not satisfy this target. Target-specific execution requirements remain
mandatory for each supported Linux/Darwin port.

## 1. Compilation and boot boundaries

Introduce a hosted build feature independently of the AArch64 ISA selection.
The AArch64 register format, ELF/ABI handling, and ordinary kernel algorithms
remain shared. The hosted build selects CPU binding, kernel context switching,
user entry, timers, interrupt masking, address translation, and executable
publication implementations at module boundaries.

The current [architecture facade](../../kernel/src/arch/mod.rs) selects both
the ISA and privileged execution by target architecture. A hosted AArch64
build must not reach TPIDR_EL1, DAIF, TTBR, VBAR, or exception-return operations.
Select the concrete implementations behind the existing facade; do not spread
host checks through syscall, IPC, or filesystem implementations.

| Existing boundary | Hosted implementation | Common behavior retained |
| --- | --- | --- |
| `arch::get_cpu`, `try_get_cpuid` | Kernel host-thread TLS holding a logical vCPU identity | `CpuLocal`, scheduler per-CPU state |
| `KernelContext`, `arch::switch::switch_to` | Guarded host stacks and host-ABI context save/restore | Stackful Task scheduling |
| `ArchTimer`, interrupt controllers | Monotonic deadlines and queue-backed controllers | Software timers, IRQ sources, deferred completion |
| `vm::addr`, page-table facade | Logical frame addresses, kernel aliases, worker mappings | VMA ownership, permissions, COW |
| `ipc::shared_memory::SharedMemory` | Compile-selected hosted concrete implementation | Constructor, `SharedMemoryObject`, kernel handle type |
| `BlockDevice` | Root-image block driver | ext2, page cache, VFS, bootstrap |
| `NetworkDevice` / `EthernetDevice` | Host frame transport driver | Scarlet sockets, TCP/IP, routing and configuration |
| `GpuBackend` / `GraphicsDevice` | Hosted GPU execution and display drivers | GPU handles, attachments, submissions, SWS |
| `AudioPlaybackDevice` | Host PCM playback driver | AudioCharDevice, SAS, audio client rings and completion |
| `VideoDecodeBackend` | Host decode-service driver | Video sessions, mapped buffers, image/frame delivery |

Keep winit and SGFX/wgpu dependencies in a hosted driver/BSP crate. The
kernel-facing driver objects implement Scarlet traits and forward owned host
requests. Host runtime primitives used by the core are isolated behind the
hosted modules. Select the allocator, panic integration, and initcall
collection for the host executable; the existing ELF linker-section
iteration cannot be assumed to work unchanged for a Mach-O executable.

Split [start_kernel](../../kernel/src/lib.rs) into platform memory preparation
and the common initialization sequence. Hosted preparation supplies the RAM
arena, kernel aliases, CPU bindings, and boot-image leases before common code
can allocate pages. It does not populate bare-metal physical fields with
dummy addresses and continue through the hardware MMU setup.

Add a hosted device-source variant carrying a BSP population callback. The
[DeviceManager](../../kernel/src/device/manager.rs) invokes that callback with
the existing priority list. This keeps dependency direction from BSP to
kernel, without making the kernel import the BSP or its graphics libraries.
Population is idempotent per driver and follows the current order:

1. Reserve and register the initial Task before any Scarlet driver worker.
2. Populate Critical interrupt controllers and initialize them per vCPU.
3. Initialize NetworkManager.
4. Populate Core/Standard/Late devices: block, serial, network, GPU, display,
   input, and entropy providers.
5. Run graphics discovery and the existing late initcalls, including TTY setup.
6. Mount initramfs, load init, and enter the existing scheduler startup.

Driver registration during early initcalls may register factories, but must
not create Scarlet worker Tasks before the init Task has reserved PID 1.

Prepare the HostCpu, ingress, IPI, and timer state for every configured vCPU
before releasing the secondary-CPU hook. Reuse the common BSP/AP startup
order: the BSP claims its initial Task before releasing APs; each AP installs
its TLS CPU identity, initializes its local controller/timer, creates its idle
Task, and enters the scheduler. Publish a vCPU as online only after its IPI
receive path and timer work. A host bootstrap readiness barrier holds initial
Task execution until all configured vCPUs are ready, without holding Scarlet
spinlocks. Per-CPU initialization must not clear another CPU's pending events.

## 2. Executable publication and trap delivery

### Patch at the common executable-publication boundary

The main ELF and interpreter both reach
`load_elf_segment_at_address` in the
[ELF loader](../../kernel/src/task/elf_loader/mod.rs). Patch after the complete
executable segment has been filled, before any worker can execute it. The
current chunk-copy helper is not the publication boundary: patching a partial
chunk can be overwritten by a later copy.

The same patcher is called when a mapping gains execute permission and when
an executable demand page is installed. The relevant VM hooks are
`protect_memory_map_range_inner`, executable-page synchronization, and the
lazy-page installation paths in the [VM manager](../../kernel/src/vm/manager.rs).
The userspace dynamic loader reaches this boundary through its
[protect operation](../../user/lib/scarlet-dl/src/platform.rs); loading only
the initial ELF would miss dynamically loaded libraries.

The publisher owns the sequence: prepare complete bytes, identify and replace
SVC instructions, publish patch-site metadata, perform host instruction-cache
synchronization, and expose execute permission. VFS continues to serve the
original file. The worker executes the patched image.

Use a reserved BRK encoding plus a patch-site table indexed by address-space
identity, image generation, and guest PC. Validate the expected patched word
and retain the original SVC immediate. A matching BRK immediate alone is not
proof of a syscall. Application breakpoints and other faults remain distinct
stop reasons. On aligned AArch64 instruction words, recognize SVC with
`(word & 0xffe0001f) == 0xd4000001` and replace it with an equal-size BRK.
The patcher must define its handling of embedded data in
executable ranges and invalidated/rewritten code; a blind byte search is not
an instruction-publication contract.

### Normalize to the existing Scarlet trap convention

For a recognized patched syscall, construct the AArch64 Trapframe with
`elr = brk_pc + 4` and the SVC exception class, instruction-length bit, and
original immediate in ESR. Existing
[Trapframe methods](../../kernel/src/arch/aarch64/mod.rs) expect SVC ELR to
point after the instruction: `increment_pc_next` is a no-op and
`get_current_pc` returns ELR minus four. Preserve the captured PC for a
breakpoint, memory fault, or asynchronous stop.

Capture all guest state before calling host code: integer registers, SP, PC,
status, guest TLS state, Q0-Q31, FPCR, and FPSR. The integer Trapframe is not a
complete snapshot; connect FP/SIMD state to the existing
[`Vcpu::fpu`](../../kernel/src/arch/aarch64/vcpu/mod.rs).

The worker's exception-entry assembly shim locates its preallocated control
block without relying on guest TLS, saves guest state, and establishes the
host ABI/TLS environment. The final resume trampoline performs the inverse
transition without calling host libraries after restoring guest state.
Validate the host's own signal/exception trampoline as part of this contract.

The exception handler only records the stop, notifies the control transport,
and parks. It does not allocate, acquire Scarlet locks, call Waker, or execute
syscalls inside the kernel process. A normal host receiver thread transfers
the notification into the vCPU ingress queue.

### Worker execution ownership

One worker process represents one guest address space. Register snapshots
and exclusive retry state belong to the guest Task. Each issued run leases
a worker pthread with its own exception stack and transport until the broker
receives the complete stop. Sequential runs can reuse that pthread across
Tasks and vCPUs. Dropping Task handles retires surplus idle pthreads. The kernel grants a run permit containing
Task identity/generation, address-space identity/generation, mapping
generation, logical vCPU, and run epoch.

Multiple threads of the same address space must execute concurrently on
different vCPUs. Run permits and register/stop slots are per guest thread;
there is no single running-thread slot or execution mutex for the worker
process. A short admission gate records new permits without being held
during guest execution.

Concurrent BRKs are routed to the respective vCPU ingress queues. Each vCPU
executes syscall dispatch on its current Task's kernel stack, using that
Task's ABI state. A host receiver only routes events; it does not serialize
kernel execution through a global or address-space-wide dispatcher.

The matching stop acknowledgement or host-confirmed termination of the
corresponding worker/thread generation ends that run permit. A disconnected
notification channel alone is not proof of termination. Deliver confirmed
death as a terminal event that resolves outstanding waits and mapping leases.
A stop request does not itself make the Task safe to migrate or its memory safe to revoke.
Preserve a synchronous BRK/fault that races with a timer stop, and dispatch it
exactly once. Reject stale stop/resume packets after exec, exit, or reuse of
an identity.

Confirming worker process death closes the address-space admission gate and
ends every permit for that worker generation. Fan out terminal events to
their owning vCPUs, which perform Task cleanup at safe boundaries. Enumerate
by address-space identity, not TGID: CLONE_VM and CLONE_THREAD are independent
in the existing Task model. Individual worker-thread termination and normal
guest exit_group retain their distinct semantics.

## 3. Memory objects and address-space transactions

### One backing model for kernel aliases and workers

Hosted `paddr` values identify stable Scarlet frame ranges. They are not host
physical addresses or file descriptors. A hosted backing registry resolves a
range to host shared extents and a contiguous kernel alias.

Register a shared RAM arena before initializing the existing PMM. Preserve
the buddy allocator and `ContiguousPages`; adapt
[`phys_to_virt` / `virt_to_phys`](../../kernel/src/vm/addr.rs) to the hosted
alias region. Translation must not wait on host I/O or worker RPC. PMM pages,
anonymous memory, ELF pages, [page cache](../../kernel/src/mem/page_cache.rs),
and [guest memfd](../../kernel/src/abi/linux/generic/memfd.rs) then use the same
frame resolver. Guest memfd retains its existing `MemfdBacking` /
`MemfdMapping` objects and demand-fault behavior.

Kernel usercopy and every worker mapping of a frame must observe the same
bytes. Scarlet implements private mappings and fork COW by allocating,
copying, and remapping frames. The kernel alias and worker both map the
selected frame as host-shared storage. Host-private COW would make the
worker's modified page invisible through the kernel alias.

### Compile-select SharedMemory, preserving its callers

Keep `SharedMemoryObject`, `SharedMemoryBacking`, and `SharedMemoryPin` in
[`ipc::shared_memory`](../../kernel/src/ipc/shared_memory.rs). Move the
current concrete implementation behind a bare-metal module and re-export
either it or the hosted implementation as `SharedMemory`.

The production constructor calls in native shared-memory creation,
[Binder Arena](../../kernel/src/abi/android/binder/arena.rs), and
[PolicyStatus](../../kernel/src/abi/android/policy_status.rs) keep the same
API. `KernelObject::SharedMemory(Arc<dyn SharedMemoryObject>)`, handle-table
operations, and socket handle transfer keep their existing representation.

The hosted concrete implementation reserves a stable logical address range
and contiguous kernel VA up to a configured object limit. It commits shared
extents only for allocated capacity. Growth appends extents and preserves the
prefix's backing, logical paddr, and kernel alias. Reservation failure or an
exceeded limit returns an allocation error through the existing Result.

This stability is required by current consumers. Binder and PolicyStatus
dereference `phys_to_virt(get_mapping_info().paddr)`; a zero placeholder is
not valid for this object. The [shared futex key](../../kernel/src/abi/linux/generic/futex.rs)
uses `translate_to_phys(uaddr)`, so two mappings of the same bytes must retain
the same logical paddr across growth.

| Existing operation | Hosted contract |
| --- | --- |
| `new`, `size`, `id`, `is_valid` | Preserve signatures; allocate zeroed backing, keep identity stable. |
| `get_mapping_info` | Check offset/length and return the stable range address and current permissions. |
| `mapping_owner_for_range` | Retain the backing Arc and the object offset independently of a guest VA. |
| `resolve_fault` | Resolve object offset plus page offset; do not allocate host storage or wait for worker commands here. |
| `pin_range` / `unpin_range` | Preserve the existing pin count and whole-object base/size result. |
| `resize` | Reject a changed rounded size while pinned; publish growth only after new extents are ready. |
| `invalidate` | Reject new mappings/pins/faults without freeing bytes still retained by existing owners. |
| Final release | Retire only after handles, queued transfers, VMA owners, pins, and worker mapping leases release their references. |

Preserve the existing logical-shrink contract: shrinking changes the admitted
size and does not by itself guarantee revocation of every resident tail PTE.
Do not truncate host storage still mapped by a worker. A range owner must
include the object offset in fault resolution and respect current object size
when the existing `can_extend_vma_on_fault` path is used; its original mapped
length must not incorrectly forbid supported growth.

The current physical implementation grows by allocating a new range, copying
the prefix, and retaining the old range in `stale_pages`. It does not update
all resident PTEs, so old and new mappings can cease sharing subsequent prefix
writes. The hosted implementation must use the stable-prefix contract above.

### Capture mapping information and ownership together

Add a defaulted preparation hook to
[`MemoryMappingOps`](../../kernel/src/object/capability/memory_mapping/mod.rs).
The proposed `MappingRequest` includes offset, length, sharing intent, and
the existing private-demand-path condition. A proposed `PreparedMapping`
contains optional initial mapping information, a range owner with its bound
offset, lifecycle notification target, and the admitted permission ceiling.

The default preserves the existing hooks and private demand-mapping behavior.
In particular, native `MAP_PRIVATE && !MAP_SHARED` currently creates an
owner-backed VMA without calling `get_mapping_info`; do not turn that into
an eager allocation or equate it unconditionally with `!shared`.

Objects with replaceable backing override preparation to capture information
and owner from one immutable snapshot. Native and Linux mmap call it once
and retain the result across placement retries. A retry must not combine an
old paddr with a newly fetched owner. VMA split and fork retain the admitted
owner and permissions.

Forward the hook through supported file/device wrappers, including
[`VfsFileObject`](../../kernel/src/fs/vfs_v2/core.rs) and
[`DevFileObject`](../../kernel/src/fs/vfs_v2/drivers/devfs.rs). The VFS wrapper
must still intersect the admitted ceiling with mount execution restrictions
and the open-file grant. This change does not enable mmap on currently
unsupported file types.

### Extend the existing sleepable VM transaction

Reuse the VM manager's `mapping_transaction: Arc<Mutex<()>>`. It already
serializes direct-map writes with fork. Extend it to map, unmap, protection,
relocation, lazy/COW publication, and usercopy mapping snapshots:

1. Prepare the object's backing/owner snapshot and release its driver lock.
2. Acquire the sleepable address-space transaction; collect the VM generation,
   old mappings, and backing leases under short VM/PTE locks.
3. Close the address-space guest-entry gate and capture its active permit set
   under the same short lock used to issue permits. Include RUN commands
   already issued but not delivered. Release the short locks.
4. Park or definitively cancel every captured permit. The worker rejects a
   delayed RUN for a cancelled run/stop epoch. Collect matching stop
   acknowledgements independently of syscall completion.
5. Apply the host mapping operation and collect its mapping acknowledgement.
   On failure restore the old host state before reopening admission; retain
   leases throughout rollback.
6. Commit Scarlet metadata and the new mapping generation under short locks.
   Retire old frames only after revocation and their leases have completed.
7. Reopen admission, notify gate waiters, and release the transaction.
   Eligible Tasks resume through the ordinary Scarlet scheduler.

Permit issuance and gate closure are linearized together; separate atomic
flag/count snapshots are insufficient. The gate does not limit the number
of concurrently running threads when open. A BRK/fault stop ends its active
guest permit before syscall dispatch. A Task already in a syscall, waiting
for the mapping mutex, or blocked on I/O is guest-stopped and is not an
outstanding stop-ACK target. Do not wait for all those kernel continuations
to finish, remove them from ready queues, or stop unrelated vCPUs.

Stop ACK proves that the guest cannot run; mapping ACK proves that the
worker's mappings, permissions, and required cache state match the committed
generation. They are separate obligations. A Task blocked at guest entry
uses the existing register/check/sleep wake protocol until admission reopens.

Host acknowledgements must be deliverable without acquiring the transaction
mutex being held by the waiter. Do not turn low-level PageTable operations
called under spinlocks into blocking host RPC.

Public VM entry points acquire the transaction once. Internal translation,
fault, COW, and retry helpers accept an already-held transaction token.
Blindly adding another lock inside `lazy_map_page_with` would deadlock the
existing `copy_to_user` and `madvise` paths.

Protect `copy_from_user_inner`, including disjoint copies, from translation
through memcpy using the same transaction. This fixes mapping/backing
lifetime, not concurrent application writes to the bytes.
[`usercopy`](../../kernel/src/library/std/usercopy.rs).

Audit the Linux ABI's direct translated-pointer paths as well. The current
single-page [read/write paths](../../kernel/src/abi/linux/generic/fs.rs)
pass raw kernel aliases into stream operations that can block; pipe and
socket result paths also perform direct user-memory accesses. Stage I/O
through kernel-owned buffers and use the transaction-protected copy helpers
at the user boundary. Hold the transaction only for translation/copy, not
while waiting for I/O: otherwise a reader can block the same address space's
writer from copying its input and deadlock. Ordinary copies need no guest
stop; close admission only when they require COW or mapping publication.

For fork, park the parent's guest writers before confirming the snapshot or
changing private mappings into COW sources. Publish parent protection and
child state before reopening their gates. Keep the current exec admission
rules for shared address spaces; prepare all host operations before its
existing guarded image-exchange commit.

Maintain lock order: brk transaction, then mapping transaction, then short
PTE/VM/allocation locks in their existing order. Host resource teardown is
queued for retirement rather than performed by a final Drop under a spinlock.

`BinderFile::mmap` needs a local control-flow change: it currently holds its
state spinlock across `Arena::new` and VM registration. Reserve the arena
creation under the lock, allocate/map outside it, then publish or cancel the
reservation. The constructor API itself remains unchanged.

For exec, prepare a new worker/address space and entry state before the
existing image-exchange commit. Failed preparation leaves the old image
running. Old-worker termination/revocation must complete before its frames
become reusable.

## 4. Scheduler, Waker, IPC, and interrupts

### Preserve kernel continuations

Use one kernel host thread per logical vCPU and a separate kernel stack per
Scarlet Task. Preserve `schedule`, `kernel_context_switch`, ready queues,
`running_cpu`, and `release_deferred_prev` in the
[scheduler](../../kernel/src/sched/scheduler.rs).

The hosted low-level context saves host-ABI callee-saved registers, including
the required FP/SIMD state. It does not restore the old host thread's TLS when
a Task migrates. Guest full-register state and kernel continuation state are
different objects.

The kernel vCPU threads execute independently from the first supported build.
Object locks protect shared state; a global host execution lock or an
address-space-wide syscall lock is not introduced. A blocked syscall on one
vCPU leaves other vCPUs free to execute user code and kernel work.

Each user Task enters a permanent hosted user run loop once. Kernel Tasks
retain their kernel entry and stackful scheduling; idle uses the hosted idle
loop. The user run loop grants the
worker a run permit and calls `run_guest_and_wait` on that Task's kernel
stack. After a matching stop acknowledgement, it calls a returning trap
dispatcher. A blocking syscall continues through the existing
`Waker -> schedule -> kernel_context_switch` path. When awakened, it resumes
the same syscall stack, including partial I/O progress.

Extract the shared dispatch body from the current nonreturning AArch64 trap
handler. Calling its existing tail entry from inside the hosted run loop
would nest another run loop on every trap.

Host mutex/condvar guards and thread-affine host runtime objects must be
released before entering Scarlet dispatch or scheduling.

### Keep Scarlet IPC in the kernel process

Retain [pipes](../../kernel/src/ipc/pipe.rs),
[local sockets](../../kernel/src/network/local.rs), credentials, kernel-object
transfer, and readiness subscriptions. A reader blocked on a pipe retains its
kernel continuation; a writer modifies the same kernel pipe object and wakes
it through the existing [Waker](../../kernel/src/sync/waker.rs). Usercopy moves
payload bytes through the canonical backing aliases.

Handle transfer continues to move references to the same KernelObject inside
the kernel process. Closing a sender's handle does not destroy a queued
transfer or a receiver's mapping. The host control transport carries worker
execution/mapping commands and replies; it does not replace guest IPC.

### Deliver host events through the existing IRQ core

Implement the existing `TimerController`,
`SoftwareInterruptController`, and `ExternalInterruptController`
[traits](../../kernel/src/interrupt/controllers.rs). Register them with
[InterruptManager](../../kernel/src/interrupt/mod.rs).

A host receiver enqueues owned completion records and raises a source's
pending state. At a vCPU safe point, use
`claim_and_handle_pending_external_interrupt` and the existing
`InterruptSource` / `MaskableInterruptSource` callbacks.
`DeferredInterruptCompletion` continues to mask/complete/unmask a deferred
source. Its longer work and token completion run in a Scarlet kernel Task.

The host external controller preserves mask, claim, acknowledgement, EOI, and
unmask semantics. Masking must retain queued records. Coalesced pending bits
are notifications; the source queue retains every completion. Unmasking a
nonempty source makes it pending again.

`InterruptManager::send_ipi` currently uses the external controller's
`send_ipi`. The hosted external controller delegates that method to the
shared IPI state, preserving `arch::send_reschedule_ipi`.
On receipt, acknowledge the controller source first, then call
`scheduler::acknowledge_reschedule_ipi`, and only afterwards process or defer
rescheduling. This clears the scheduler's coalescing flag without discarding
a newly arriving notification.

The hosted ArchTimer exposes a monotonic counter and absolute deadline.
Use that deadline in guest/idle waits and validate the arm generation after a
timeout. Keep deadline, arm generation, and pending state per vCPU.
On expiry, stop only the guest thread named by that vCPU's current permit and
obtain its acknowledgement, then
run `timer::handle_local_timer_irq` and
`scheduler::handle_timer_reschedule` on the vCPU. Keep the existing software
timer queue and callbacks.
Ordinary timer/IPI delivery does not suspend the worker process or its other
guest threads. If the target vCPU is already executing a syscall, record its
pending event for a safe boundary; other vCPUs continue to run.

### Guard and wakeup rules

Keep [IrqGuard](../../kernel/src/sync/irq_guard.rs), PreemptGuard, and CPU-local
state. Hosted interrupt save/restore manipulates a logical vCPU mask.
Restoring the mask does not itself execute callbacks or schedule.

This is required by the existing wake protocol: Waker marks an event wake in
progress, makes the waiter runnable, then completes the event publication.
Scheduling the waiter from an intermediate IrqGuard drop can suspend its
producer and leave the waiter spinning for completion on the same vCPU.
Dispatch and rescheduling occur after that operation has returned.

Safe points are worker-stop/trap boundaries, user-return boundaries, the idle
loop, and explicit kernel polls with no forbidden guards. Host callbacks
never interrupt kernel execution to call Waker or access CpuLocal. A signal
requests a worker stop; it is not a reentrant Scarlet interrupt handler.

Each vCPU ingress uses one host mutex/condition-variable predicate covering
queued events, stop acknowledgements, and timer state. The producer updates
the predicate and sequence before notification. The consumer checks and parks
under the same mutex, then releases it before invoking Scarlet code.
While awaiting a stop, wait for the matching ACK or new ingress generation,
rather than spinning on the already-pending interrupt bit.

The existing wake-before-context-save ownership rule remains intact. A Task
becomes runnable through Scarlet's queues, and its old `running_cpu` ownership
is released after the context switch has saved the stack. No separate host
Task queue bypasses this handoff.

## 5. Filesystems, network, and media drivers

### Root storage

Add a hosted [BlockDevice](../../kernel/src/device/block/mod.rs) implementation.
It exposes a configured root-image file as a named block device.
The existing [bootstrap](../../user/bin/src/bootstrap.rs) accepts an explicit
`root=` device and `rootfstype=`, so ext2 mounting, pivot_root, Environment,
and service startup follow their current paths.

Keep the caller-owned `submit_requests` batch contract: one result per input
request, in input order, with no consumption of another caller's queued work.
Translate sectors using checked bounds; handle short host reads/writes and
return an I/O error rather than successful partial block completion.
Host I/O workers return data/status packets; the kernel driver updates its
pending batch, then wakes the submitting Task through the IRQ path.
Partition probing can request synchronous reads before the scheduler is
available. Provide a boot submission path that waits on the host transport
and consumes its reply directly, without trying to wake or schedule a Scarlet
Task. Switch to the normal completion path after scheduler readiness; both
paths preserve the same batch and error contract.

The current block request API contains Read and Write but no flush operation.
For the hosted driver, a successful write batch includes the host's
durable-file synchronization step. Do not acknowledge a write while relying
on an unrepresented later host flush. This preserves the existing filesystem
API, including the ext2 page-cache writeback path.

VFS nodes, mount namespaces, file descriptions, guest credentials, labels,
page cache, and inode identity remain Scarlet-owned. A host file descriptor
for the disk image is an implementation detail of the block driver.

Direct mounting of a host directory is a separate filesystem capability:
implement [FileSystemDriver](../../kernel/src/fs/mod.rs),
[`FileSystemOperations` and `VfsNode`](../../kernel/src/fs/vfs_v2/core.rs),
and FileObject. Register it through FileSystemDriverManager. It is not needed
to boot the normal image. Such a mount must anchor resolution to its exported
directory and preserve open-file/inode lifetime; host paths and host process
credentials must not replace Scarlet's VFS authority checks. Unsupported
metadata/label operations return the existing unsupported result.

### Network

Add a hosted Ethernet driver implementing the existing
[NetworkDevice and EthernetDevice](../../kernel/src/device/network/mod.rs)
interfaces. `send_packet` queues an owned frame; `receive_packets` drains
completed RX frames without blocking. Preserve MTU, MAC, link status,
link-epoch reporting, and bounded queue/backpressure behavior.

After DeviceManager registration, construct the EthernetNetworkInterface and
register it with both NetworkManager and EthernetLayer, following the
existing [VirtIO network driver](../../kernel/src/drivers/network/virtio_net.rs).
NetworkManager registration does not perform the EthernetLayer registration.
The kernel RX completion Task uses the existing receive preparation/epoch
check and `NetworkManager::handle_received_packet` entry.

Use a user-mode host network/NAT service as the host transport. The
boundary presented to Scarlet remains Ethernet frames. The service consumes
TX frames and returns RX frames through the host completion ingress. Scarlet
continues to execute its socket, TCP/IP, address-configuration, and resolver
code. No guest socket is replaced by a host socket object.

### Audio

Implement [AudioPlaybackDevice](../../kernel/src/device/audio/mod.rs) and
register it with `register_playback_device_with_info` after PID 1 is reserved
and before normal services start. Preserve AudioCharDevice, SAS, and the
existing client/mixer protocol. SAS requires S16LE, 48 kHz stereo output;
the host adapter performs any conversion needed by the native output format.

Implement capabilities, configure, start, stop, release, submit_period,
process_completions, completion callback registration, and in-flight limits.
Configure prepares bounded period buffers and a stream epoch. submit_period
is called under the existing IRQ/ring lock, so it copies committed PCM into
preallocated transport storage and enqueues without waiting for host progress
or queue space.

The host audio callback consumes host-owned periods and reports actual
consumption. Return those records through virtual IRQ ingress; invoke the
installed completion callback on a kernel vCPU, then let the existing audio
completion worker advance pointers and wake writers. Queue acceptance alone
does not mean playback completion. Stop/release quiesce host access and reject
old-epoch completions before storage is reused.

The full configuration's scarlet-seat depends on SAS. SAS returns before
publishing readiness if audio output cannot be opened. Real playback,
refill/drain, and stop under concurrent desktop/network load are initial
acceptance requirements.

### Video

Implement [VideoDecodeBackend](../../kernel/src/device/video.rs) and use its
normal registration, sessions, mapped-buffer requests, and completion notifier.
The full [video-player build](../../bundles/experimental/bundle.toml) enables
device decode paths; supplying its installed binary without a usable decoder
does not establish full media operation.

Provide actual host decode operations for the formats required by the full
configuration. Retain session and buffer leases on the kernel driver side
through completion or proven cancellation; send owned coded data and session
identities to the host decode service. Install decoded output before reporting
frame readiness. Session destruction guarantees no remaining host access,
and concurrent sessions retain their independent identities and ordering.

## 6. Hosted GPU, display, and input

### Add concrete backends behind the current GPU objects

Register a hosted `GpuBackend` through the existing GPU control-device
registration and a hosted `GraphicsDevice` through DeviceManager/graphics
discovery. Preserve generic GpuImage/GpuBuffer, handles, attached-resource
authority, completion objects, and SWS.

| Existing GPU entry | Hosted work |
| --- | --- |
| `query_dialect`, `create_context` | Advertise the hosted wire dialect; create a context proxy. |
| `plan_image`, `create_image_with_layout`, `create_buffer` | Validate generic layout/backing and create host resource IDs. |
| `attach_image_pinned`, `attach_buffer_pinned` | Bind the resource to the context while preserving the generic pin and attachment authority. |
| CPU access methods | Implement the existing reservation contract and transfer between canonical CPU backing and the host GPU resource. |
| Queue `enqueue(GpuSubmission)` | Accept bounded owned work without waiting for GPU progress or a free slot. |
| Queue `submit` | Return only when submitted accesses have retired, as required by the existing synchronous contract. |
| `begin_gpu_resource_present` / connector variant | Retain presentation owners and return the existing wait ticket. |

The method contracts are in
[`gpu/backend.rs`](../../kernel/src/device/gpu/backend.rs) and
[`graphics/mod.rs`](../../kernel/src/device/graphics/mod.rs).
Busy/Rejected from enqueue returns the unchanged submission with no accepted
work. An accepted or partially failed submission remains retained until
accesses have actually retired.

The default CPU-access hooks do not implement an exclusion mechanism. The
hosted backend must stop new conflicting GPU admission for the same backing
across the whole device, including other contexts, wait for earlier accesses
to retire, and retain the reservation through CPU copies and transfer
callbacks. A transfer callback uses its existing reservation rather than
acquiring it recursively.

SharedMemory-backed GPU import follows the existing
`GpuImage::new_imported -> SharedMemoryPin -> create_image_with_layout`
path in [resource.rs](../../kernel/src/device/gpu/resource.rs).
Resolve its paddr through the backing registry. The separate
`import_shared_image` operation accepts a SharedImage lease and has its own
layout/color contract; do not conflate these entry points.

### Keep host graphics objects on their owning thread

The host main/event-loop thread owns winit, SGFX WindowContext, and the wgpu
resource table. Kernel-side concrete backend objects contain transport
handles and resource identities and satisfy the existing Send + Sync traits.
Do not force the current SGFX Rc-based resource table across threads.

The kernel driver retains `GpuSubmission`, `GpuBackingPin`, attachment
leases, and `PresentationOwners` in a pending table. The host thread receives
resource IDs/generations and owned command/payload bytes. It returns pure
completion records. A Scarlet completion Task consumes those records and
calls `complete` or `retire_failed`, then releases the kernel owners.
Their Drop paths must not run on an arbitrary host graphics callback thread.

Driver admission is multi-producer from the configured kernel vCPUs.
Assign device-wide request identities and device epochs, with a sequence for
each logical GPU queue. Admission fixes queue order, resource-table revision,
and immutable attachment authority before publishing to the host; arrival
order on a shared transport is not the queue-order contract. Concurrent
detach, handle close, and process exit preserve accepted work's owners.
Reserve device/queue capacity without waiting under IRQ/attachment locks.
Retire each completion once even when failure, teardown, and resize race.

Create one host GPU device/resource namespace for the instance. Client and
SWS contexts attach the same host texture through their independently
authorized Scarlet image handles. CPU SharedMemory imports use explicit
uploads/access synchronization; GPU-to-GPU composition does not require
readback through the worker process.

### Use owned wire commands and a sampled-image import operation

Add a dynamic SGFX backend library for the hosted kernel's advertised GPU
backend/dialect. Its guest-facing SGFX ABI remains the existing dynamic-backend
ABI. It converts recordings to a versioned wire format.

Implement both BackendApi and DriverApi in this first full-system driver.
The native Vulkan ICD installed by
[`scarlet-vulkan`](../../user/lib/scarlet-vulkan/bundle.toml) requires the
driver API. Cover resource creation/synchronization/retirement, buffer and
texture readback, shared-image mapping, queue submission, and receipt
cloning/release. Preserve programmable shaders, depth attachments, sampled
textures, and vertex/buffer operations used by the installed
[`vulkan-canvas-demo`](../../user/vulkan-canvas-demo/src/vulkan_cube.rs).
SGFX's host Resources materializes its logical textures/buffers; these do
not all need to become generic Scarlet GPU objects.

Retain the explicit `scarlet-hosted` backend identity and add it to SWS's
[`backend_is_accelerated`](../../user/std-bin/src/sws/animation_policy.rs)
recognition. This is a narrow backend-integration change: the existing
SWS ABI, service configuration, render, and resize flows remain in use.
Full acceptance verifies that accelerated composition and timed animations
are active alongside native Vulkan clients.

The wire format contains command headers, checked offsets/lengths into an
owned payload arena, and context attachment identities/generations. Validate
all spans, arithmetic, resource authority, format/usage, and negotiated limits
before acceptance. Existing SGFX IR includes borrowed pointer/length payloads;
copying only the outer command array does not make those pointers valid in the
kernel or host process.

The host SGFX integration needs an operation to bind a same-device texture as
a sampled image in another context. The current wgpu `map_image` operation
requires render-attachment/presentation usage, whereas
[SWS imports](../../user/std-bin/src/sgfx_ir_support.rs) require sampled usage.
The new operation validates device identity, usage, format, and extent and
retains the texture lease for the binding's lifetime. Preserve client/SWS
handle transfer and generic attachment checks.

Expose a WindowContext method that clones its existing Context for these
allocations and bindings. This shares the device, queue, and completion
tracker; constructing a new device for each Scarlet context would fail the
backend's device-identity checks.

The external hosted display driver also needs a public opaque
`GpuDisplayResource` constructor and identity accessors. The current opaque
constructor/accessors are crate-private. Add this small interface rather than
misrepresenting a GPU texture as a CPU-linear resource.

### Completion and resize

Submission, presentation submission, GPU source-read retirement, and CPU
readback completion are distinct events. SGFX's current WindowContext present
returns after submission/present plus a nonblocking poll. Add a completion
receipt that establishes when the source image is no longer read, and retain
the existing presentation owners until that receipt is completed. A window
close or device loss fails pending waits and retires resources only after
quiescence.

Window resize creates a new display-backing generation and updates the host
surface, published display dimensions, and input-coordinate transform.
Old mappings and pending presentations retain the old generation.
Use the same output epoch for this publication and incoming input events.
Tracked presentation must also accept an already-submitted old-size image
and blit it to the current host surface until it retires. The existing SGFX
present operation requires equal source/surface dimensions; a resize race
must not turn that generation mismatch into device loss or desktop GPU
fallback.

The existing [DisplayCharDevice](../../kernel/src/device/graphics/display_device.rs)
bypasses mapping-device pinning for connector zero. Its current separate
information/owner lookup is insufficient for replaceable backing. Use the
prepared-mapping hook from section 3 to return an immutable per-VMA backing
snapshot for every connector. Query operations report current dimensions;
old VMAs continue to reference their captured generation.

Do not permanently pin a generation merely per open handle. The existing
[framebuffer library](../../user/lib/framebuffer/src/lib.rs) unmaps and remaps
through the same handle when backing changes. Each new mapping must capture
the current generation. The current
[SWS compositor](../../user/std-bin/src/sws/compositor.rs) already has
`check_display_resize`, `refresh_mapping`, and GPU target resize handling.

Input events from winit enter the hosted input driver through the IRQ ingress.
Serial input enters the existing TTY, including foreground-process-group
Ctrl-C handling. Preserve Scarlet seat/device access checks and input events.

The external SGFX references for this plan are pinned to
[61e5ddd0](https://github.com/petitstrawberry/sgfx/tree/61e5ddd0f0e31309d2468ace3c4afe67d949fa72):
[wgpu backend](https://github.com/petitstrawberry/sgfx/blob/61e5ddd0f0e31309d2468ace3c4afe67d949fa72/crates/sgfx-backend-wgpu/src/lib.rs)
and [command ABI](https://github.com/petitstrawberry/sgfx/blob/61e5ddd0f0e31309d2468ace3c4afe67d949fa72/crates/sgfx-core/src/ir/abi/commands.rs).

## 7. First-release acceptance

The host execution contract must be verified on each actual target. In
particular, Darwin's guest TLS/x18 preservation, host exception entry,
4 KiB guest permission boundaries on a 16 KiB host, and executable-memory
write/execute restrictions remain unresolved port requirements. A successful
SVC patching demo does not establish those properties. Keep these checks in
the execution/VM port; do not silently widen guest permissions or claim
arbitrary existing binaries are supported before they pass.

All rows below are required together on the normal full image. The default
acceptance run uses four vCPUs; repeat SMP boundary tests with two vCPUs and
exercise the configured upper limit. CPU count, service dependencies, and
application capabilities are not reduced to obtain a passing result.

| Required area | First-release acceptance condition |
| --- | --- |
| Full distribution | Resolve the complete full bundle and enabled modules with pinned dependencies; boot normal init, stemd, login/seat and user-session configuration. Preserve native and Debian/Linux application environments. |
| SMP execution | All configured vCPUs are online before Task execution. The same address space has multiple simultaneously active guest run permits, independent progress, and concurrent syscall dispatch on different vCPUs. |
| Executable publication | Main ELF, interpreter, dynamic libraries and generated executable pages use SVC-to-BRK publication; real BRK remains distinct. Preserve full register/TLS state through traps, migration and asynchronous stops. |
| VM and objects | Concurrent mapping/COW/fork and usercopy preserve lifetime and permissions; SharedMemory alias/growth, handle transfer, offset mappings and shared futex identity remain coherent. |
| Storage and networking | Full-image application and compiler I/O works; writes survive normal shutdown/reopen. Normal netcfgd/resolverd, TCP/UDP and installed network services operate concurrently with desktop/media load. |
| Audio and desktop services | Real PCM consumption, refill/drain and stop work; SAS publishes readiness and scarlet-seat starts the normal SWS/greeter/session path without removing its dependencies. |
| GPU and native Vulkan | SWS, several SGFX clients and a native Vulkan client run together with programmable/depth/texture/buffer/readback operations, accelerated composition, image sharing, presentation and resize. |
| Full media and runtime workloads | Installed video/audio applications use working device paths. Native Rust/toolchain and Linux application workloads exercise threads, shared memory, dynamic loading and executable-memory publication where used. |
| Failure and shutdown | Worker death, device errors, window close and process exit resolve waits and retire resources without stopping unrelated vCPUs or reusing live backing. |

The concurrency suite runs alongside the full services and workloads:

- Assert overlapping valid run permits for multiple threads of the same
  address space and independent vCPU progress. A CPU-count report, host
  thread count, or guest barrier completed by time slicing is not sufficient
  evidence of this execution model.
- Block a pipe reader while another thread in the same address space writes
  and performs mapping operations. Exercise partial writes, concurrent
  syscalls, wake before context save, migration, and timeout/event races.
- Delay an address-space stop acknowledgement deliberately. Other address
  spaces, kernel vCPU threads, timers, network/audio completions, and GPU
  progress must continue.
- Race admission closure with RUN publication/delivery, BRK, COW, and worker
  termination. Inject delayed, duplicate, and stale-epoch notifications;
  dispatch synchronous traps once and reject cancelled RUN commands.
- Race fork snapshot creation with parent writes on another vCPU, and race
  unmap/protect with every usercopy path, including Linux ABI fast paths.
  Confirm failed updates preserve the previous usable image/mappings.
- Retain pending events under nested IRQ guards and at idle park boundaries.
  Schedule only after Waker completion publication; acknowledge reschedule
  IPIs at both the controller and scheduler layers.
- Exercise SharedMemory growth and handle transfer while peers map and use
  shared futexes. Keep a GPU import pinned and verify size-change rejection.
- Submit from SWS, native Vulkan, and other clients concurrently while
  detaching/closing resources and resizing the host window. New mappings
  capture the new display generation; old VMAs and accepted work retain the
  old one. Placement retries never mix information and owner generations.
- Run filesystem/compiler, network, animation, and audio/video activity
  together through login, normal use, and shutdown on the full configuration.

These are implementation acceptance criteria, not test results from this
source review.
