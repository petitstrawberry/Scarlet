# Native runtime services

Android integration added shared kernel facilities as well as an Android ABI
adapter. This guide describes those facilities at the source level, reviewed on
2026-11-14. They are available to native kernel components and, where wrappers
exist, native userspace. Their existence does not imply full Linux or Android
compatibility, nor a new runtime validation result.

## Ownership and authority

These objects have different responsibilities; creating one does not grant
control over the others.

| Object | Responsibility | Reference |
| --- | --- | --- |
| Environment and View | Select ABI filesystem roots and retain filesystem authority | [Execution environments](../abi/execution-environments.md) |
| PID namespace | Hierarchical task names and visibility | [Task namespaces](../container/task-namespace.md) |
| Mount namespace | Retain, split and join View correspondences | [Mount namespaces](mount-namespaces.md) |
| ProcessScope | Lifetime membership, explicit orphan reaper, termination | [Process scopes](../task/process-scopes.md) |
| ResourceGroup | Hierarchical process membership and CPU placement constraints | [Native wrapper](../../user/lib/scarlet-os/src/resource_group.rs) |
| Shield policy and subject | Authorization, credentials, delegated transitions and execution constraints | [Shield](../security/shield.md) |

For example, assigning PID 1 in a new PID namespace does not make that process
a scope reaper. Namespace visibility also does not confer process-memory or
mount-management authority. Native capability checks remain in force across
ABI changes.

## RPC transport and object lifetime

[ipc/rpc.rs](../../kernel/src/ipc/rpc.rs) implements bounded synchronous calls,
nested return routes, one-way deliveries and transferable dispatch objects.
This is a kernel transport used by adapters, not a public standalone RPC syscall
family. Binder owns its wire structures, handle translation and discovery in
[abi/android/binder](../../kernel/src/abi/android/binder/mod.rs).

- Reply capabilities are single-use. Dropping an unanswered reply completes its
  caller with `PeerClosed`.
- One-way deliveries serialize per dispatch object until the receiver releases
  the delivery. Other objects are not covered by that object's serialization.
- Messages retain dispatch objects, native capabilities with access metadata,
  and optional adapter-owned lifetime resources separately from serialized bytes.
- Sender process identity, active principal and Shield subject are captured
  from kernel task state. Sender-supplied bytes cannot replace them.
- A dispatch object can be revoked independently. Death watches do not keep its
  server alive; dropping the watch cancels the registration.
- Scoped priority donation uses independently revocable leases. It does not
  change the receiver's configured base priority permanently.

Current implementation bounds are 256 KiB per message, 64 combined object and
capability references, 256 in-flight requests per endpoint, and 32 nested call
levels. These are implementation limits, not a promise of unbounded Binder
compatibility. The module's tests exercise cancellation, lifetime, nesting,
one-way ordering and sender identity.

## Process membership, scheduling and admission

[ResourceGroup](../../kernel/src/task/resource_group.rs) supports independent
hierarchies. Threads share process membership; fork creates a new membership at
the parent's current groups, and exec retains membership. Moving a process moves
its threads together. Effective CPU placement intersects hierarchy constraints,
ancestor constraints, requested affinity and online CPUs. Updates validate live
tasks, including admitted deadline placement, before publishing a new mask.

Native userspace uses `scarlet_os::resource_group::ResourceGroup` to create,
enter, inspect, remove and leave groups or set CPU masks. Management operations
require authority; obtaining current membership is not an authority upgrade.
Linux cgroup and cpuset files are adapters over these facilities, not evidence
that every Linux cgroup controller exists.

The scheduler supports native fixed-priority FIFO in addition to fair and
deadline policies. FIFO priorities are 1 through 255; yielding rotates among
runnable peers at the same priority. The native wrapper retains fair fallback
controls when changing policy. See [scheduler policy](../architecture/scheduler.md)
and [fixed-priority leases](../../kernel/src/task/fixed_priority.rs).

[Task capacity](../../kernel/src/sched/task_capacity.rs) separates live user-task,
kernel-task and stack budgets from monotonically allocated task IDs. Defaults
derive from usable physical RAM and the virtual kernel-stack window; explicit
configuration is validated against both. Reclamation returns capacity without
reusing task identity. Task-limit control is not a Linux rlimit implementation.

## Filesystem and memory services

| Facility | Native behavior and adapter boundary |
| --- | --- |
| Mount restrictions | Read-only, no-exec and no-privilege-gain restrictions constrain operations through a View; mapping permission ceilings survive later `mprotect`. See [restrictions](mount-restrictions.md). |
| Mount propagation | Shared and downstream relationships propagate supported mount changes across namespace copies. See [propagation](mount-propagation.md). |
| Inode metadata | [VFS xattrs](../../kernel/src/fs/vfs_v2/xattr.rs), TmpFS and Ext2 provide bounded byte-valued attributes. [Ext2 tests](../../kernel/src/fs/vfs_v2/drivers/ext2/xattr_tests.rs) cover persistence. Storing ACL bytes does not establish ACL enforcement. |
| File labels | Shield can select labels at creation, authorize relabeling and persist symbolic labels in an explicit metadata domain. Numeric policy IDs are not stored as durable file identities. See [Shield](../security/shield.md). |
| Directory notifications | [VFS notification queues](../../kernel/src/fs/vfs_v2/notify.rs) retain inode identities and report bounded mutation events and overflow. Linux inotify translates supported directory events; it is not a complete file-access notification implementation. |
| Userspace filesystems | [userfs](../../kernel/src/fs/vfs_v2/userfs.rs) retains request/reply lifetime and bounds. The server capability owns connection lifetime; a mount cannot keep a departed server alive. [FUSE](../../kernel/src/abi/linux/device/fuse.rs) supplies a limited FUSE 7 codec, not every caching, locking or ioctl feature. |
| Mapping lifetime | [VM manager](../../kernel/src/vm/manager.rs) implements private-range relocation/growth, fork ownership and COW-aware memory access. [Layout policy](../../kernel/src/vm/layout.rs) controls randomized mmap placement while preserving fixed addresses. |
| Mapping inheritance | [Mapping owners](../../kernel/src/object/capability/memory_mapping/mod.rs) can control fork inheritance; shared backing offsets and permission ceilings belong to the mapping lifetime. |
| Shared-memory sealing | The Linux [memfd adapter](../../kernel/src/abi/linux/generic/memfd.rs) enforces supported seals against writes, resize and writable mappings, including future-write restrictions. |

[Memory pressure](../../kernel/src/mem/pressure.rs) measures bounded reclaim
attempts after allocation failure and scheduler-observed stalls. Linux
`/proc/pressure/memory` exposes those measurements and trigger readiness.
`full` is derived from all non-idle tasks being stalled with no ready task,
not copied from `some` and not a general measurement of every Linux stall source.

[Reclaim preference](../../kernel/src/task/reclaim_preference.rs) is a native
process attribute. Linux `oom_score_adj` translates it; raising protection
beyond the inherited allowance requires native authority. This does not by
itself implement Linux's OOM-killer selection policy. Clean-page reclaim and
retry are in the [page cache](../../kernel/src/mem/page_cache.rs).

## Network, input, time and diagnostics

The [network guide](../network/architecture.md) covers native IPv6 address,
base-header and static-routing support, which remains below full IPv6 sockets
and neighbor discovery. Linux rejects unsupported IPv6 socket creation instead
of silently constructing an IPv4 socket.

[Local datagrams](../../kernel/src/network/local_datagram.rs) preserve record
boundaries, sender credentials and transferred descriptor lifetime. Linux owns
the ancillary-message format. Local sequenced-packet sockets preserve their
type across accept. Socket deadlines bound blocking operations rather than
restarting a timeout after every wake.

[Link statistics](../../kernel/src/network/link.rs) use link identity and
incarnation checks so a replaced interface cannot satisfy a stale query.
[Traffic reservations](../../kernel/src/network/traffic.rs) account for queued
packets and return credit after failed or cancelled sends. Named
[packet meters](../../kernel/src/network/traffic_policy.rs) back supported Linux
quota views. These native mechanisms do not constitute a BPF execution engine.

[Virtual input channels](../../kernel/src/device/input/virtual_device.rs) return
separate producer and receiver capabilities. Only the producer injects events;
only the receiver consumes them. A manager decides whether to publish a channel
in a View. Keyboard, mouse and bounded single-contact touch channels share the
native event interface; Linux evdev translation stays in its ABI adapter.
[Device lifecycle queues](../../kernel/src/device/lifecycle.rs) similarly
provide native observations that the Linux uevent adapter translates.

[Timers](../../kernel/src/object/timer.rs) and [wake inhibition](../../kernel/src/system_power/wake.rs)
provide shared primitives. Linux POSIX timers translate supported notification
modes, including thread-targeted signals, and validate the receiving thread.
The signal adapter supplies process-pending queues, timed waits and architecture
signal frames; this is not an assertion of identical signal support on every
architecture.

[Native logs](../../kernel/src/log.rs) retain kernel/userspace origin metadata
in a bounded ring. [Linux log translation](../../kernel/src/abi/linux/generic/log.rs)
controls its own record format, privileges and cursor semantics. A userspace
message is not promoted to a kernel-origin record by writing kernel-like text.

## Validation and change ownership

The linked modules contain kernel `#[test_case]` coverage; use the normal kernel
test tasks described in the [kernel guide](README.md#tests-and-further-reading).
Native ABI byte contracts have host tests under
[scarlet-abi/tests](../../user/lib/scarlet-abi/tests/native_contract.rs).
These references locate tests; they do not claim that a new full suite was run
for this documentation change.

Keep generic mechanisms and their authority checks in the common subsystems.
Keep Linux and Android wire formats and errno translation in their adapters.
Android image preparation, graphics adapters, the session manager and frontend
are maintained in [scarlet-bundle-android](https://github.com/petitstrawberry/scarlet-bundle-android).
Scarlet consumes the released archives through its Android bundle manifest.
