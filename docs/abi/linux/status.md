# Linux ABI Support Status

Scarlet includes an early Linux ABI layer focused on running selected RISC-V 64-bit userland binaries. This note captures the current scope, known gaps, and the recommended way to exercise the demo.

## Summary

- **Architecture**: RISC-V 64-bit userland built via Buildroot
- **Kernel interface**: Direct syscall translation layer into Scarlet kernel primitives
- **State**: Partial – intended for demos and experimentation rather than production workloads
- **Demo**: See [demo.md](demo.md) for running instructions.

## System Call Support Matrix

The following system calls are currently handled by the Linux ABI module (`linux-riscv64`).

### File System & I/O
| Syscall | Status | Notes |
|---------|--------|-------|
| `openat` | ⚠️ Partial | Maps flags to internal VFS. **Permissions/Modes are ignored.** |
| `close` | ✅ Supported | |
| `read`, `write` | ✅ Supported | |
| `readv`, `writev` | ✅ Supported | Vectored I/O. |
| `pread64`, `pwrite64` | ✅ Supported | |
| `lseek` | ✅ Supported | |
| `dup`, `dup3` | ✅ Supported | |
| `pipe2` | ✅ Supported | Supports `O_CLOEXEC` and `O_NONBLOCK`. |
| `ioctl` | ✅ Supported | Dispatches command to underlying device driver. |
| `fcntl` | ⚠️ Partial | `F_GETFD`, `F_SETFD`, `F_GETFL`, `F_SETFL` (O_NONBLOCK only). `F_DUPFD` and locking are missing. |
| `getcwd` | ✅ Supported | |
| `chdir` | ✅ Supported | |
| `mkdirat` | ⚠️ Partial | Only `AT_FDCWD` supported. |
| `unlinkat` | ✅ Supported | Supports `AT_REMOVEDIR`. |
| `getdents64` | ✅ Supported | |
| `readlinkat` | ✅ Supported | |
| `newfstatat`, `newfstat` | ✅ Supported | |
| `umask` | 🚧 Stub | Returns provided mask, does not affect creation. |
| `fchmod` | 🚧 Stub | Always succeeds. |
| `faccessat` | 🚧 Stub | Always succeeds. |
| `fsync` | 🚧 Stub | Always succeeds. |
| `linkat` | 🚧 Stub | Always succeeds (no-op). |
| `renameat2` | 🚧 Stub | Checks flags, returns success (no-op). |
| `epoll_create1` | 🚧 Stub | Returns dummy file descriptor. |
| `epoll_ctl`, `epoll_wait` | 🚧 Stub | Minimal/No-op implementation. |
| `pselect6`, `ppoll` | ✅ Supported | `sigmask` ignored. `pselect6` limited to 64 FDs. |

### Process Management
| Syscall | Status | Notes |
|---------|--------|-------|
| `execve` | ✅ Supported | ELF loading, argument/env passing. |
| `clone` | ✅ Supported | Threading flags (`CLONE_THREAD`, `CLONE_VM`, etc.) and TLS supported. |
| `exit`, `exit_group` | ✅ Supported | |
| `wait4` | ⚠️ Partial | Basic waiting supported; `WNOHANG` and other options ignored. |
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

### Memory Management
| Syscall | Status | Notes |
|---------|--------|-------|
| `mmap` | ✅ Supported | `MAP_ANONYMOUS`, `MAP_FIXED`, `MAP_SHARED`/`PRIVATE`, file-backed. |
| `munmap` | ✅ Supported | |
| `mprotect` | ✅ Supported | |

### Time & Timers
| Syscall | Status | Notes |
|---------|--------|-------|
| `clock_gettime` | ✅ Supported | |
| `nanosleep` | ✅ Supported | |
| `timer_create` | ✅ Supported | |
| `timer_settime`, `timer_gettime` | ✅ Supported | |
| `timer_delete` | ✅ Supported | |
| `clock_getres` | 🚧 Stub | Returns 1ms resolution. |

### Signals & IPC
| Syscall | Status | Notes |
|---------|--------|-------|
| `rt_sigaction` | 🚧 Stub | Ignores action; sets handler to Ignore/Default only. |
| `rt_sigprocmask` | 🚧 Stub | Always succeeds; ignores mask updates. |
| `futex` | ⚠️ Partial | `FUTEX_WAIT`, `FUTEX_WAKE` implemented. |

### Networking (Sockets)
**Note:** Networking is currently mocked to allow applications to start without hanging.
| Syscall | Status | Notes |
|---------|--------|-------|
| `socket` | 🚧 Mock | Creates a pipe to simulate a socket fd. |
| `bind`, `listen`, `connect` | 🚧 Mock | Always succeeds. |
| `accept` | 🚧 Mock | Returns a new pipe fd. |
| `getsockname` | 🚧 Mock | Returns `AF_UNIX`. |
| `setsockopt`, `getsockopt` | 🚧 Mock | Success / Dummy values. |

## File System Implementation Notes

While basic file operations work, the current implementation has significant deviations from standard Linux behavior:

- **Permissions & Ownership**: The VFS does not enforce file permissions (rwx) or ownership. All files appear as owned by root (UID 0/GID 0), and access checks (e.g., opening a read-only file for writing) may not be strictly enforced by the underlying filesystem drivers.
- **Creation Mode**: The `mode` argument in `openat` and `mkdirat` is currently ignored. Files are created with default internal attributes.
- **Path Resolution**: Symbolic link resolution is supported, but some complex path lookup behaviors (like `AT_EMPTY_PATH`) may be missing.
- **Filesystem Features**: Advanced features like extended attributes (`xattr`), access control lists (`ACL`), and quotas are not implemented.

## What Works Today

- Buildroot root filesystem assembled under `/opt/prebuilt/linux-riscv64.tar`
- Toolchain exports (under `/opt/buildroot/output/host`) for rebuilding userspace
- Demo binaries `green` and `fbdoom` built with the Buildroot toolchain
- Process launch, basic file I/O, and framebuffer output through Scarlet-managed devices

Refer to [userspace-artifacts.md](userspace-artifacts.md) for the exact build steps, and [demo.md](demo.md) for execution instructions.

## Known Limitations

- **Networking**: No real network stack integration yet; sockets are pipes.
- **Signals**: Signal delivery logic is basic; complex signal handling (stacks, nesting) is WIP.
- **User/Group**: Single-user (root) environment assumed.
- **Permissions**: File permissions, ownership, and access modes (e.g. read-only enforcement) are currently ignored.
- **Filesystem**: `linkat`, `renameat2` are stubs; hard links not fully supported in VFS v2.
- **Epoll**: Stubs only; event-driven I/O applications may not function correctly.
- **Device Support**: `ioctl` commands are device-dependent. Basic TTY support is available.

## Roadmap Highlights

1. Expand syscall surface (file descriptors, polling, networking)
2. Integrate device hotplug and block devices for broader filesystem coverage
3. Align ELF loader behaviour with glibc/musl expectations (TLS, auxv entries)
4. Harden tests to cover mixed Scarlet/Linux/xv6 pipeline scenarios

## Feedback

Issues and pull requests that improve Linux ABI coverage are welcome. Please include reproduction steps and note the Scarlet commit used when reporting problems.
