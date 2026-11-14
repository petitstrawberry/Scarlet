# Linux ABI Support Status

Scarlet includes a partial Linux ABI layer used for selected Buildroot-based
userlands, GUI applications, services, and KVM-oriented VMM workloads. This note
captures the current scope, known gaps, and the recommended way to exercise it.

## Summary

- **Architectures**: RISC-V 64-bit and AArch64 userlands built via Buildroot
- **Kernel interface**: Direct syscall translation layer into Scarlet kernel primitives
- **State**: Partial but actively used for selected workloads, including
  BusyBox-style userlands, Wayland bridge applications, Mozc, and `/dev/kvm`
  userspace VMMs
- **Demo**: See [demo.md](demo.md) for running instructions.

### Android ABI

The separate `android-aarch64` ABI composes the Linux implementation with
Android-specific device translations. Binder support lives in
`kernel/src/abi/android/binder/` and uses native RPC primitives.
Android userspace and its launcher are maintained in
[scarlet-bundle-android](https://github.com/petitstrawberry/scarlet-bundle-android).
Scarlet consumes the released runtime and native application archives through
`bundles/android/bundle.toml`.

### Exec argument limits (2026-10-19)

Debian trixie's `python3.13-minimal` 3.13.5-2+deb13u5 postinst invokes
`py_compile.py` with 285 library paths: 289 arguments and 11,190 string bytes
including NULs, before environment storage. Scarlet previously rejected the
invocation at its independent 256-string cap, leaving dpkg configuration
incomplete with `E2BIG`.

Exec now budgets argv and envp together, including string NULs and pointer
slots. The aggregate ceiling is 6 MiB, Linux's 3/4-of-8-MiB hard ceiling; each
string is limited to 32 pages (128 KiB on the supported 4-KiB-page targets),
including its NUL. Numerous short or empty strings consume that same budget.
The existing `prlimit64` stack-limit stub reports a value above the hard ceiling;
lowering a per-process stack rlimit is not implemented by this change. Linux's
usual 8-MiB stack soft limit would instead select a 2-MiB argument ceiling.
See [Linux execve limits](https://man7.org/linux/man-pages/man2/execve.2.html).

AArch64 and RISC-V Linux entry allocate larger initial stacks when argument
storage would exhaust the ordinary 1-MiB stack, retaining ordinary stack space
below the arguments and a guard page. Auxiliary-vector pairs are written with
page-aware usercopy: many arguments can place a pair across physically
noncontiguous stack pages. Exec remains transactional on preparation failure.

All 1,512 AArch64 kernel tests pass with the normal full-project features.
Coverage includes the Python postinst argument count, aggregate/pointer/env
budget boundaries, per-string limits, large-stack allocation, invalid user
pointers and an auxv pair crossing noncontiguous pages. The RISC-V Release
kernel library also passes `cargo check` with the same feature set.
The normal AArch64 Release boot image builds. After the user closed QEMU, the
boot partition was updated and the normal disk booted with HVF, eight vCPUs,
16 GiB RAM, VirGL, audio and the existing video backend. All desktop services
started. `dpkg --configure -a` in the Debian Linux view completed the previously
failing Python postinst; `dpkg-query -s python3.13-minimal` reports
`Status: install ok installed` for 3.13.5-2+deb13u5. The first serial invocation
needed a corrected Linux root PATH including `/usr/sbin` and `/sbin` for dpkg's
helper lookup. The root partition hash matched before/after the offline boot
update, and the GPT was unchanged.

## System Call Support Matrix

The following system calls are currently handled by the Linux ABI module.

### File System & I/O
| Syscall | Status | Notes |
|---------|--------|-------|
| `openat` | ⚠️ Partial | Maps supported flags to VFS and retains creation mode after umask. Full POSIX DAC and `O_TMPFILE` remain unsupported. |
| `close` | ✅ Supported | |
| `read`, `write` | ✅ Supported | |
| `readv`, `writev` | ✅ Supported | Vectored I/O. |
| `pread64`, `pwrite64` | ✅ Supported | |
| `lseek` | ✅ Supported | |
| `dup`, `dup3` | ✅ Supported | |
| `pipe2` | ✅ Supported | Supports `O_CLOEXEC` and `O_NONBLOCK`. |
| `ioctl` | ✅ Supported | Dispatches command to underlying device driver. |
| `fcntl` | ⚠️ Partial | Descriptor flags/duplication, `F_SETFL` (`O_NONBLOCK` only), memfd seals and supported byte-range locks. Sleeping lock acquisition under contention remains unsupported. |
| `getcwd` | ✅ Supported | |
| `chdir` | ✅ Supported | |
| `mkdirat` | ⚠️ Partial | Only `AT_FDCWD` supported. |
| `unlinkat` | ✅ Supported | Supports `AT_REMOVEDIR`. |
| `getdents64` | ✅ Supported | |
| `readlinkat` | ✅ Supported | |
| `newfstatat`, `newfstat` | ✅ Supported | |
| `umask` | ✅ Supported | Retains the process mask and returns the previous value; creation adapters apply it. |
| `fchmod`, `fchmodat` | ⚠️ Partial | Retain exact Unix mode bits for VFS-backed nodes in the Linux view; not full POSIX DAC enforcement. |
| `faccessat`, `faccessat2` | ⚠️ Partial | Validate supported flags and inspect metadata/read-only state; not complete Unix credential-based access evaluation. |
| `fsync` | ✅ Supported | Delegates to the file object's sync operation; durability depends on the backing filesystem. |
| `linkat` | ⚠️ Partial | Creates inode-preserving links relative to directory FDs; linking anonymous/unlinked FDs with `AT_EMPTY_PATH` remains unsupported. |
| `renameat2` | ⚠️ Partial | Supports directory FDs and atomic `RENAME_NOREPLACE`. Exchange/whiteout return `ENOSYS`. |
| `epoll_create1` | ✅ Supported | Allocates an ABI epoll descriptor and validates creation flags. |
| `epoll_ctl`, `epoll_wait` | ⚠️ Partial | Readiness registration and blocking waits are implemented; not all Linux epoll modes and signal-mask semantics are supported. |
| `pselect6`, `ppoll` | ✅ Supported | `sigmask` ignored. `pselect6` limited to 64 FDs. |
| `ftruncate` | ✅ Supported | |
| `fallocate` | ✅ Supported | |
| `flock` | ✅ Supported | |
| `inotify_init1`, `inotify_add_watch`, `inotify_rm_watch` | ⚠️ Partial | Bounded native directory-mutation watches, move events and overflow; not arbitrary file-access/modify watches. |
| `eventfd2` | ✅ Supported | |
| `timerfd_create` | ✅ Supported | |
| `timerfd_settime`, `timerfd_gettime` | ✅ Supported | |
| `statx` | ✅ Supported | |
| `getrandom` | ✅ Supported | |
| `memfd_create` | ⚠️ Partial | Anonymous shared backing and supported sealing operations, including future-write restrictions; unsupported creation flags are rejected. |
| `getxattr`, `setxattr`, `listxattr`, `removexattr` and `l*`/`f*` variants | ⚠️ Partial | Bounded VFS attributes; support depends on the filesystem. Reserved native Shield metadata is not exposed as a public xattr. |
| `mount`, `umount2` | ⚠️ Partial | View-local mount restrictions, supported bind/propagation operations and lazy detach; not every Linux mount type or flag. |
| `setns` (mount namespaces) | ⚠️ Partial | Retained native namespace capabilities with authority and View compatibility checks. See [mount namespaces](../../kernel/mount-namespaces.md). |

### Process Management
| Syscall | Status | Notes |
|---------|--------|-------|
| `execve` | ✅ Supported | ELF loading, argument/env passing. |
| `clone` | ✅ Supported | Threading flags (`CLONE_THREAD`, `CLONE_VM`, etc.) and TLS supported. |
| `exit`, `exit_group` | ✅ Supported | |
| `wait4` | ⚠️ Partial | Process-child exit collection, Linux status encoding and `WNOHANG`; resource-usage output remains incomplete. |
| `getpid`, `getppid`, `gettid` | ✅ Supported | |
| `uname` | ✅ Supported | Reports "Linux 6.1.0-scarlet...". |
| `brk` | ✅ Supported | Heap management. |
| `prlimit64` | 🚧 Stub | Returns success with fake high limits. |
| `set_tid_address` | ✅ Supported | |
| `set_robust_list` | ✅ Supported | Stores pointer. |
| `getuid`, `geteuid`, `getgid`, `getegid` | 🚧 Stub | Returns 0 (root). |
| `setuid`, `setgid`, `setpgid` | 🚧 Stub | Always succeeds. |
| `getpgid` | ✅ Supported | Returns task ID. |
| `membarrier` | 🚧 Stub | Always succeeds. |
| `unshare` | ✅ Supported | |
| `prctl` | ✅ Supported | |
| `sysinfo` | ✅ Supported | |
| `pidfd_open` | ✅ Supported | |
| `waitid` | ✅ Supported | |
| `sched_yield` | ✅ Supported | |
| `sched_getscheduler`, `sched_getparam` | ✅ Supported | |
| `sched_getaffinity` | ✅ Supported | |
| `setsid`, `getsid` | ✅ Supported | |
| `getresuid`, `getresgid` | ✅ Supported | |
| `sigaltstack` | ✅ Supported | |
| `kill`, `tkill`, `tgkill` | ✅ Supported | Signal delivery. |

### Memory Management
| Syscall | Status | Notes |
|---------|--------|-------|
| `mmap` | ✅ Supported | `MAP_ANONYMOUS`, `MAP_FIXED`, `MAP_SHARED`/`PRIVATE`, file-backed. |
| `munmap` | ✅ Supported | |
| `mprotect` | ✅ Supported | |
| `mremap` | ⚠️ Partial | Private anonymous growth and supported private-range relocation; `MREMAP_DONTUNMAP` and unsupported mapping/flag combinations are rejected. |
| `msync` | ✅ Supported | |
| `mlock`, `munlock` | ✅ Supported | |
| `mlockall`, `munlockall` | ✅ Supported | |
| `mlock2` | ✅ Supported | |
| `mincore` | ✅ Supported | |
| `madvise` | ✅ Supported | |

`/proc/self/maps` and `/proc/thread-self/maps` expose an open-time snapshot of
user virtual-memory ranges and permissions, including the native ELF
interpreter. This lets Box64 protect native mappings during Wine address
reservation. File offsets, device/inode identifiers and backing paths are not
reported. Numeric `/proc/<pid>/maps` is supported only when the caller shares
the target address space; PID visibility alone does not authorize inspection.

### Time & Timers
| Syscall | Status | Notes |
|---------|--------|-------|
| `clock_gettime` | ✅ Supported | |
| `nanosleep` | ✅ Supported | |
| `timer_create` | ⚠️ Partial | Supports `SIGEV_SIGNAL`, `SIGEV_NONE` and validated `SIGEV_THREAD_ID`; kernel-side `SIGEV_THREAD` is unsupported. |
| `timer_settime`, `timer_gettime` | ✅ Supported | |
| `timer_delete` | ✅ Supported | |
| `clock_getres` | 🚧 Stub | Returns 1ms resolution. |

### Signals & IPC
| Syscall | Status | Notes |
|---------|--------|-------|
| `rt_sigaction` | ⚠️ Partial | Retains handlers and action flags; custom delivery requires an architecture signal-frame implementation. AArch64 has handler frames and `rt_sigreturn`. |
| `rt_sigprocmask` | ✅ Supported | Updates thread masks while preserving unmaskable signals. |
| `sigaltstack`, `rt_sigtimedwait`, `rt_sigqueueinfo` | ⚠️ Partial | Alternate-stack state, pending-signal waits and queued process signals; architecture and target restrictions still apply. |
| `futex` | ⚠️ Partial | WAIT/WAKE, bitset variants and REQUEUE/CMP_REQUEUE with cancellation cleanup. PI futexes remain unsupported. |
| `seccomp` | ⚠️ Partial | Supported filters translate to inherited native execution constraints; TSYNC/listener flags are rejected. |

### Networking (Sockets)
Socket syscalls have real implementations backed by the kernel network stack.
| Syscall | Status | Notes |
|---------|--------|-------|
| `socket` | ✅ Supported | Creates socket via kernel network stack. |
| `socketpair` | ✅ Supported | |
| `bind`, `listen` | ✅ Supported | |
| `connect` | ✅ Supported | |
| `accept`, `accept4` | ✅ Supported | |
| `getsockname`, `getpeername` | ✅ Supported | |
| `sendto`, `recvfrom` | ✅ Supported | |
| `sendmsg`, `recvmsg` | ✅ Supported | |
| `setsockopt`, `getsockopt` | ✅ Supported | |
| `shutdown` | ✅ Supported | |

### Hypervisor / KVM
| Interface | Status | Notes |
|-----------|--------|-------|
| `/dev/kvm` | ✅ Supported | Linux KVM ioctl compatibility layer backed by SHV. |
| KVM VM/vCPU lifecycle | ✅ Supported | `KVM_CREATE_VM`, `KVM_CREATE_VCPU`, `KVM_RUN`. |
| KVM memory slots | ✅ Supported | `KVM_SET_USER_MEMORY_REGION`. |
| KVM register access | ✅ Supported | Architecture-specific RISC-V and AArch64 register paths. |
| KVM IRQ/device ioctls | ⚠️ Partial | Enough for current kvmtool/Firecracker-class workloads; not full KVM parity. |

## File System Implementation Notes

Filesystem compatibility is built on native VFS authority and metadata, not a
complete copy of Linux permission semantics:

- **Ownership**: Native principal/group ownership is retained and translated to
  Linux UID/GID. Ext2 persists full-width identities; files are not universally
  reported as root-owned.
- **Modes**: Creation and chmod adapters retain exact Unix modes for Linux stat
  results, and umask affects creation. This metadata does not imply complete
  POSIX DAC, ACL or set-ID semantics.
- **Authority**: View capabilities, read-only/no-exec/no-privilege-gain mount
  restrictions, and supported Shield creation/relabel/exec checks are enforced.
  See [mount restrictions](../../kernel/mount-restrictions.md) and
  [Shield](../../security/shield.md).
- **Attributes**: TmpFS and Ext2 support bounded extended attributes. Ext2 also
  persists native Shield labels in a protected symbolic metadata domain.
  Preserving ACL bytes is distinct from enforcing POSIX ACLs; filesystem quota
  enforcement is not established by the network quota adapter.

Implementation references: [filesystem syscalls](../../../kernel/src/abi/linux/generic/fs.rs),
[mode metadata](../../../kernel/src/abi/linux/generic/mode.rs),
[xattr translation](../../../kernel/src/abi/linux/generic/xattr.rs), and
[Ext2](../../../kernel/src/fs/vfs_v2/drivers/ext2/mod.rs).

## What Works Today

- Buildroot root filesystem generation (via `bundles/buildroot/tools/build_buildroot.sh`)
- Toolchain exports from Buildroot for building userspace
- Demo binaries `green`, `fbdoom`, and the Linux zathura PDF viewer built with
  the Buildroot toolchain
- Process launch, basic file I/O, framebuffer output, and Wayland bridge clients
  through Scarlet-managed devices

Refer to [userspace-artifacts.md](userspace-artifacts.md) for the exact build steps, and [demo.md](demo.md) for execution instructions.

## Known Limitations

- **Networking**: Socket options and edge cases remain partial. Native IPv6
  address/header/static-routing support does not supply full IPv6 sockets or
  neighbor discovery. Unsupported BPF operations are rejected.
- **Signals**: Custom handlers require architecture-specific signal frames;
  AArch64 support is not a promise of the same coverage on every ABI target.
- **Credentials**: UID/GID and supported capability changes use native Shield
  identities and delegated authority, not a complete Linux privilege model.
- **Filesystem**: Unix mode metadata is not full DAC/ACL enforcement. Anonymous
  FD hard linking and rename exchange/whiteout remain unsupported.
- **Readiness**: Epoll is implemented, but full Linux flag and signal-mask parity
  is not claimed. Device readiness and ioctls remain device-dependent.
- **Resource control**: Cgroup/cpuset, memory pressure and reclaim preferences
  translate native facilities; they do not implement every cgroup controller,
  Linux stall source or Linux OOM selection policy.

See [native runtime services](../../kernel/runtime-services.md) for the common
mechanisms behind these adapters. The capability and limitation updates above
were checked against source on 2026-11-14; historical runtime results elsewhere
on this page retain their original scope.

## Roadmap Highlights

1. Expand syscall surface (file descriptors, polling, networking)
2. Integrate device hotplug and block devices for broader filesystem coverage
3. Align ELF loader behaviour with glibc/musl expectations (TLS, auxv entries)
4. Harden tests to cover mixed Scarlet/Linux/xv6 pipeline scenarios

## Feedback

Issues and pull requests that improve Linux ABI coverage are welcome. Please include reproduction steps and note the Scarlet commit used when reporting problems.
