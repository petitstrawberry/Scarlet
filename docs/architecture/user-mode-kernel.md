# User-mode kernel

This is an implementation design, not a description of an available hosted
build. The [implementation plan](user-mode-kernel-implementation.md) specifies
the replacement boundaries, ownership rules, required common changes, and
acceptance tests against the current kernel.

The Scarlet user-mode kernel boots a full Scarlet system inside AArch64 Linux
or Apple Silicon macOS. From a host terminal, a user starts Scarlet, logs in
to its console or desktop, and runs applications in the guest filesystem.
The system supports arbitrary unmodified AArch64 Scarlet applications,
including binaries installed after boot. It runs on the host CPU without
requiring a hypervisor.

The first supported build must run the existing full distribution on multiple
vCPUs. The default is four vCPUs, matching the current AArch64 full runner.
The full bundle, normal service graph, native and Linux application paths,
network, accelerated graphics, input, audio, and media workloads form one
acceptance target. All configured vCPUs are ready before initial Task execution.
The same address space may execute several guest threads on different vCPUs
at the same time.

## System architecture

A running instance consists of the Scarlet kernel in one host process and
guest programs in separate worker processes. The kernel manages guest tasks,
virtual memory, filesystems, and devices. Workers execute the guest programs'
AArch64 instructions directly on the CPU. The port delivers system calls and
exceptions from those workers to the kernel and connects Scarlet devices to
host I/O. Linux and macOS each supply their own implementation of that port.

| Resource | Scarlet owns | The host provides |
| --- | --- | --- |
| Task | Identity, credentials, handles, Environment, ABI, exit status | Register save, resume, and notification of a stop |
| Address space | Address ranges, permissions, sharing, copy-on-write, mapping lifetime | Backing storage and mappings in the worker |
| vCPU | Task selection, affinity, accounting, wakeups, timer deadlines | Execution, interrupt delivery, and blocking waits |
| Device | Handles, access control, request ordering, and completion semantics | Block and packet I/O, terminal I/O, entropy, input, and GPU execution |

## Boot and application startup

Each host has a separate board support package (BSP) project with a `scarlet.toml` declaring its BSP,
runner, initramfs, and root filesystem built from the full bundle.
`cargo scarlet build` builds the BSP. `cargo scarlet run` composes the
project's declared images and starts its runner. The SDK owns image assembly
and dependency locking.
[Project layout](../build-system/README.md) and
[AArch64 full project](../../projects/aarch64-limine-full/scarlet.toml).

The BSP supplies boot information, RAM, vCPUs, timers, and devices. It
exposes the root image as a block device. Scarlet mounts the root filesystem
and constructs the Environment. Any bootstrap adaptation is confined to
accessing the boot medium. The distribution's init and service configuration
start the console and graphical sessions through the usual service manager,
login, seat, and SWS paths.
[Bootstrap](../../user/bin/src/bootstrap.rs) and [stemd](../services/stemd.md).

When a user launches an application, Scarlet opens its executable through
the guest filesystem, selects its ABI, and prepares its address space and
initial registers. The port installs that address space in a worker. Once
the Scarlet scheduler selects the new task, the worker enters guest code.
For example, a file read traps into Scarlet, which resolves the guest handle
and performs the VFS operation. A device completion can wake a blocked
reader, and the scheduler can then resume it in its worker.

## Task execution and memory

Each worker represents one guest address space. Threads that share a guest address space use the same
worker and retain separate register state. A vCPU runs a thread selected by
the Scarlet scheduler. A Scarlet task ID, a worker process ID, and a vCPU ID
identify different resources with independent lifetimes.
Each guest thread has its own exception stack, mailbox, and run epoch.
Concurrent syscalls execute on their owning kernel vCPU threads. A short
address-space admission gate prevents new guest entry during VM changes;
it does not serialize ordinary guest execution or syscall dispatch.

### User entry, traps, and ABI dispatch

Guest mappings exclude kernel memory. When publishing executable pages, the
port replaces AArch64 `SVC` instructions with `BRK` in the worker's execution
copy. The original bytes remain available through Scarlet VFS. The
host-specific exception entry captures registers, the guest PC, and the stop
reason. It distinguishes a translated syscall from an application breakpoint,
a memory fault, and an asynchronous vCPU interrupt.

The kernel resumes from a Scarlet trapframe. Its existing dispatcher recognizes
tagged Scarlet calls and resolves other calls through the ABI module for the
guest PC. That module supplies syscall results, exception and signal delivery,
and the guest state for resumption. The same entry serves binaries running
under Scarlet's other AArch64 ABI modules.
[ABI dispatch](../../kernel/src/abi/mod.rs) and
[execution environments](../abi/execution-environments.md).

### Address-space operations

A map, protection change, or unmap completes once Scarlet's VM state and the
worker's mappings agree. Conflicting updates synchronize within the affected
address space. Close its guest-entry gate, cancel or park all issued run
permits, and collect mapping acknowledgements before publishing the next
generation. Threads already handling syscalls are guest-stopped; the
transaction does not wait for their syscalls to finish. Other address spaces
and kernel vCPU threads continue to run. After `fork`, shared mappings retain the same backing and
private writes remain local to each process. For `exec`, the kernel prepares
the new address space, ABI, and entry registers before replacing the running
image. A failed preparation returns an error in the original process.

The main executable, ELF interpreter, dynamically loaded libraries, and pages
that acquire execute permission all use the same executable-mapping path.
On Darwin, guest regions with different permissions can fall within one
16 KiB host page. Preserving the guest's permissions at that boundary while
satisfying host write/execute restrictions is an open implementation issue.
The application contract includes existing binary layouts.
[ELF loader](../../kernel/src/task/elf_loader/mod.rs) and
[dynamic linker](../userspace/dynamic-linker.md).

### Kernel objects and shared memory

The BSP selects concrete implementations at build time or registers them at
device initialization. For example, both builds expose
`ipc::shared_memory::SharedMemory` and its constructor.
`KernelObject::SharedMemory` retains an `Arc<dyn SharedMemoryObject>`.
The user-mode implementation owns storage that can be mapped into workers.
The object implements the mapping, resize, pinning, and lifetime contracts
used by its callers. Closing a handle after mapping releases the handle's
reference. The mapping retains the backing until it is removed. This also
applies when another process received the handle over a Scarlet socket.
[Kernel objects](../../kernel/src/object/mod.rs) and
[shared memory](../../kernel/src/ipc/shared_memory.rs).

### Scheduling and interrupts

Scarlet owns the run queues and accounts time to the task executing on each
vCPU. The host delivers timer expiry, device completion, and cross-vCPU
rescheduling requests. A timer can interrupt a compute-bound guest thread
even if it never enters the kernel. Independent vCPUs can continue executing
while another handles a syscall. Revoking a mapping synchronizes the vCPUs
that can still access it.

An idle vCPU blocks until an event or the next timer deadline. Wait
registration and the final readiness check must cover a completion that
arrives as the vCPU goes to sleep. Worker termination wakes outstanding waits
and returns control to Scarlet's task teardown and exit handling.
[Scheduler](scheduler.md) and [timekeeping](../timekeeping.md).

## Devices

**Storage and networking.** The host supplies block reads and writes, network
frame transmission and reception, and completion notifications. Scarlet owns
the filesystem, socket semantics, network stack, address configuration, and
name resolution. Applications access these services through their usual
paths and socket APIs.
[BlockDevice](../../kernel/src/device/block/mod.rs) and
[NetworkDevice](../../kernel/src/device/network/mod.rs).

**Audio and media.** A host AudioPlaybackDevice provides the PCM output and
period-consumption completions used by the existing SAS service. SAS is a
dependency of scarlet-seat in the full distribution, so audio belongs to the
initial boot contract. A host VideoDecodeBackend serves the decoder device
API used by the installed video applications. Both drivers retain their
kernel buffer/session leases and deliver host completions through vCPU IRQ
ingress.
[Audio device](../../kernel/src/device/audio/mod.rs),
[video device](../../kernel/src/device/video.rs), and
[seat dependencies](../../bundles/desktop/fs/etc/stemd.d/services/01-sws.toml).

**Terminal and entropy.** Terminal input enters the Scarlet TTY. Ctrl-C is
delivered to the guest foreground process group. A separate host control
stops the instance even when the guest is unresponsive and restores the
host terminal settings. Optional diagnostics follow the terminal's line-ending
conventions. A registered host entropy source feeds Scarlet's random subsystem,
which serves the random devices and each ABI's random calls.
[TTY](../system/tty-pty.md) and [entropy sources](../../kernel/src/random.rs).

### Graphics and input

Applications draw into buffers and images owned through Scarlet handles. SWS
composites client content and submits the output image to the Scarlet display.
The host creates an SGFX `WindowContext` from a winit window. SGFX's wgpu
backend executes the GPU work and presents the result through Metal on macOS.
The existing ScarletUI winit integration establishes the lifetime relationship
between the native window, its SGFX context, and the mapped target session.
[Window initialization](https://github.com/petitstrawberry/scarlet-ui/blob/4802af3f0191dbd6a13d0293fc63670fcb346f61/crates/scarlet-ui-platform-winit/src/lib.rs)
and [session and presentation](https://github.com/petitstrawberry/scarlet-ui/blob/4802af3f0191dbd6a13d0293fc63670fcb346f61/crates/scarlet-ui-platform-winit/src/sgfx_renderer.rs).

Commands crossing a process boundary carry owned data, image handles, and
completion information. Borrowed spans in an SGFX recording have meaning
only in the submitting process. An exported client image must remain usable
as a sampled texture in the compositor's process. SGFX's wgpu
`map_image` currently requires render-attachment and presentation usage.
Sampled-image import therefore needs a distinct SGFX operation. Scarlet's
GPU device and display retain the image references until the corresponding
work completes. Composition and presentation keep GPU images on the GPU.
Explicit readback transfers pixels to the caller.
The hosted SGFX dynamic driver implements both BackendApi and DriverApi:
the latter is required by the native Vulkan ICD included in the full bundle.
Its programmable, depth, texture/buffer, and readback paths are part of the
first full-system acceptance target. Concurrent client/SWS/Vulkan submissions
retain per-queue ordering and attachment snapshots across vCPUs.
[SWS image import](../../user/std-bin/src/sgfx_ir_support.rs),
[SGFX wgpu](https://github.com/petitstrawberry/sgfx/blob/61e5ddd0f0e31309d2468ace3c4afe67d949fa72/crates/sgfx-backend-wgpu/src/lib.rs),
and [GPU completion](../graphics/gpu-async-submission.md).

A window resize or scale-factor change updates the physical output dimensions,
the SGFX surface, the Scarlet display mode, and the absolute pointer transform.
Replacing a framebuffer changes its backing identity. Existing mappings retain
the old allocation until released. The native window hides its cursor where
SWS draws the guest cursor. Closing the window wakes pending input and
presentation waits as part of instance shutdown.
[Display backing identity](../device/display-present.md) and
[SWS event waiting](../graphics/sws-event-waiting.md).
