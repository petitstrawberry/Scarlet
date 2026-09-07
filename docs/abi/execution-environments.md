# Execution Environments

An Environment is a sealed map from ABI names to filesystem views. A view owns
one root and its mount topology, but no working directory. Each task has its
own filesystem context (view plus cwd) and belongs to one Environment.

```text
Environment
  scarlet        → native view
  linux-aarch64  → Linux view

Task → Environment + active ABI + filesystem context
```

The native ABI name is `scarlet` on both supported architectures. Linux slots
are `linux-aarch64` or `linux-riscv64`; xv6 uses `xv6-riscv64`.
One Environment has at most one view per ABI. Use separate Environments for
two different roots of the same ABI.

## Execution and inheritance

Ordinary exec opens the executable in the calling task's view, detects its
ABI, and selects that ABI's slot in the **same** Environment. The loader
resolves `PT_INTERP` unchanged in the target view. Missing slots or interpreters
are errors; the kernel neither constructs a fallback root nor searches global
storage. Environment variables are passed unchanged.

Same-ABI exec preserves cwd. Cross-ABI exec starts at `/`. Fork shares the
Environment and mount views but copies cwd and the handle table. `CLONE_FS`
explicitly shares the filesystem context, including cwd. A requested VFS
namespace clone copies all ABI views together, preserving a coherent Environment.

Loading takes place in an unpublished process image. Failure leaves the live
memory mappings, ABI, Environment, cwd, handle table and user registers intact.
Successful ordinary exec closes close-on-exec handles. Open file descriptions
remain shared resources; loading through an already-open handle may move its
file position even when exec fails.

Exec currently rejects shared address spaces, shared handle tables, live
multithreaded processes and ABI zones before replacing the image. Coordinated
multithreaded exec is not implemented.

## Native API and authority

Use `scarlet_os::environment::{Environment, VfsView, HandleMapping}` (also
re-exported by the legacy `scarlet_std::environment` facade).

1. Create views, then compose their filesystems, overlays and non-recursive binds.
2. Create an Environment and register its ABI slots with `set_root`.
3. Call `seal`. Slot edits are now rejected; file writes and authorized mounts
   below the view root are not frozen.
4. Open the executable through a view handle and call `spawn` or `exec`.

Current-Environment and root queries return **use-only** handles. They allow
opening files and executing programs, not mount administration. View creation
requires bootstrap authority or an existing management view handle.
`current_admin` only duplicates authority the caller already holds.
Copying a handle preserves its rights; changing role metadata does not grant
management rights.
Use `from_handle` to adopt a capability explicitly transferred by a manager.

Mount, unmount and bind require destination-view management authority.
Root replacement requires a new view/Environment and an explicit transition,
not a bare chroot/enter operation. Legacy mount syscalls enforce the same
authority; pivot-root is restricted to bootstrap. Linux mount/unshare return
`ENOSYS` for unsupported operations instead of reporting fake success.

Cross-view composition rejects cyclic backing dependencies. Binds do not
recursively import source submounts. Dependency tracking is conservative:
unmounting an edge does not necessarily allow a reverse bind on those same views;
construct fresh views for a different composition.

Explicit spawn/exec takes an already-open executable, argv/envp, an absolute
cwd **in the target view**, and a source-to-target handle map. Nothing else is
inherited from the handle table, including standard streams. Duplicate target
slots and invalid source handles fail before replacement. Returned API handles
are close-on-exec; an explicit mapping retains a chosen handle for that
transition while preserving its close-on-exec metadata for later execs.

`exec_with_abi` and `spawn_with_abi` support explicit ABI selection for
ambiguous formats such as xv6 ELF. They still require that ABI's slot in the
selected sealed Environment. Spawn creates a child in the caller's task
namespace and inherits its job-control identity, terminal, nice value and
CPU-placement preferences.

The CLI utility `abi-run <abi> </executable> [args...]` opens the program in
that ABI's view, starts it at `/`, and passes the current environment variables
and standard streams. For example:

```sh
abi-run linux-aarch64 /bin/busybox ls /usr
abi-run linux-aarch64 /bin/busybox sh
```

The latter shell resolves its commands normally in the Linux view. Applications
must not reconstruct a global rootfs path to launch a program in another ABI.

## Syscall records

| IDs | Operations |
| --- | --- |
| 520–529 | View create, current, clone, open, mount, bind, overlay, unmount, mkdir, rooted-at |
| 1300–1305 | Environment create, set-root, remove-root, seal, current, get-root |
| 1306–1307 | Environment spawn and exec |

Spawn/exec arguments are Environment handle, executable handle, pointer to
`RawEnvironmentExec`, and optional ABI C-string pointer (zero for detection).
The 48-byte record contains `size: u32`, reserved-zero `flags: u32`, then
64-bit argv, envp, cwd and handle-map pointers and a handle count.
Each map entry is two `u32` values: source and target.

Strings are UTF-8, NUL-terminated, with a 4096-byte scan limit. argv/envp are
NUL-terminated pointer arrays with at most 256 entries each and 128 KiB of
combined string data. The handle map is bounded by the 1024-slot handle table.
Errors use the native `usize::MAX` convention. Spawn returns a namespace PID;
successful exec does not return to the old image.

## In-tree boot policy

The kernel starts the reserved PID 1 in the bootstrap view, loading `init=`
from the command line or `/init` by default. Only bootstrap may operate without
an Environment. The in-tree init selects backing storage using `root=` and
`rootfstype=`, constructs all available ABI views, seals the Environment, and
execs `/bin/stemd` while retaining PID 1. Failure leaves init in bootstrap.
Microvm init instead execs Firecracker in the Linux view.

The bundles use this **userspace-owned** backing layout:

```text
/init                         bootstrap program
/roots/<abi>/                 base filesystem data
/state/overlays/<abi>/        writable overlay data
/home, /shared, /tmp, /dev    explicitly shared resources
```

Each ABI sees its own overlay as `/`. The default views share `/home`,
`/shared`, `/tmp`, `/dev` and `/dev/pts`; Linux also shares the native
`/root`. Thus Mozc's launcher opens `/usr/lib/mozc/mozc_server` in the Linux
view, and both sides use `/root/.config/mozc` for its profile.

The default init also exposes a non-recursive backing-root bind at `/scarlet`
for explicit administrative access. It is not a kernel feature, an application
search path, or an ABI path-conversion convention. Applications use their
visible view, query another ABI view when launching its program, or explicitly
transfer open handles. PDFview, for example, opens a document in the caller's
view and passes it to zathura through stdin. Custom Environments may omit
`/scarlet` entirely.

Repository source directories such as `bundles/base/fs/system/scarlet` are
inputs to bundle layers; their names do not prescribe runtime paths.

Environment isolation covers filesystem views and explicit handle transfer.
It does not isolate networks, PIDs, IPC registries or users and is not a complete
security sandbox.
