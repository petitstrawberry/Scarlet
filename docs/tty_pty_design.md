# TTY / PTY Subsystem Design

This document defines the target design for Scarlet's TTY/PTY subsystem,
aligned with the Linux model (controlling terminal, sessions, process groups,
line discipline, pseudo-terminal pairs). It is the reference for the multi-PR
implementation tracked by issue #413.

The design is **forward-looking**: not all pieces exist yet. Each section
labels the current state (✅ implemented / ⚠️ partial / ❌ missing) and the
desired state. Implementation lands incrementally across phases (see the
phase list at the bottom). This document is updated as phases complete.

## 1. Goals

1. Bring the kernel TTY layer up to a Linux-comparable level:
   - Full `termios` (input/output/control/local flags, control characters).
   - Line discipline with canonical/raw modes, ECHO/ISIG/ICRNL/OPOST.
   - Sessions, process groups, controlling terminal, job-control signals.
2. Provide POSIX-style pseudo-terminals (`/dev/ptmx`, `/dev/pts/N`) so that
   terminal emulators (including Scarlet-native GUI apps) can host arbitrary
   shells without owning the physical UART.
3. Enable a GUI terminal application running on ScarletUI + framebuffer that
   talks to a PTY master and hosts `/bin/sh` (or any program) as its slave.
4. Keep the existing Scarlet-native `SCTL_TTY_*` control opcodes working as a
   backward-compatible facade over the new termios-centric model.
5. Keep all architecture-specific code under `kernel/src/arch/...`; the
   TTY/PTY core is arch-independent.

## 2. Layered architecture

```
   user space
      │  read/write/ioctl on FD
      ▼
   ┌────────────────────────────────────────────────────────────────┐
   │ VFS (CharDevice file objects)                                  │
   │   /dev/tty0 /dev/console /dev/tty /dev/ptmx /dev/pts/N         │
   └────────────────────────────────────────────────────────────────┘
      │
      ▼
   ┌────────────────────────────────────────────────────────────────┐
   │ TtyDevice (core)                                               │
   │   - termios state                                              │
   │   - line discipline (N_TTY)                                    │
   │   - read/write buffers + waker                                 │
   │   - job control: session, foreground PGID, controlling TTY     │
   └────────────────────────────────────────────────────────────────┘
      │ TtyDriver trait (push input from below, pull output to below)
      ▼
   ┌──────────────────────┬─────────────────────────────────────────┐
   │ UART driver          │ PTY driver                              │
   │  (pl011, virt UART)  │  (master ⇄ slave pair)                  │
   └──────────────────────┴─────────────────────────────────────────┘
```

- **`TtyDevice`** owns termios, line discipline state, and job-control fields.
  It is independent of the underlying transport.
- **`TtyDriver`** is the lower-edge trait that concrete transports
  (UART, PTY slave) implement. Output from line discipline calls
  `TtyDriver::write`; input from below is delivered via
  `TtyDevice::receive_buf` and then run through the line discipline.
- **PTY master** is *not* a `TtyDevice`. It is a `CharDevice` that exposes the
  raw byte stream of the pair and forwards window-size/ioctl operations to
  the slave's `TtyDevice`.

## 3. Module layout (target)

```
kernel/src/device/char/
  tty/
    mod.rs            // re-exports + late_initcall
    core.rs           // TtyDevice (driver-agnostic)
    termios.rs        // Linux-compatible termios struct + flag constants
    ldisc.rs          // N_TTY line discipline
    driver.rs         // TtyDriver trait
    job_control.rs    // session / pgid / controlling TTY helpers
  pty/
    mod.rs
    pair.rs           // PtyPair: shared buffers between master and slave
    master.rs         // PtyMaster: CharDevice
    slave.rs          // PtySlave: TtyDriver impl on top of TtyDevice
    ptmx.rs           // /dev/ptmx multiplexer

kernel/src/fs/vfs_v2/drivers/
  devpts.rs           // devpts filesystem (slave nodes only)
```

The existing `kernel/src/device/char/tty.rs` is moved into `tty/core.rs` +
`tty/ldisc.rs` etc. without changing the public re-exports (`TtyDevice`,
`tty_ctl::*`).

## 4. termios

`kernel/src/device/char/tty/termios.rs` is the **single source of truth** for
termios layout and flag constants. Both the Scarlet-native code paths and the
Linux ABI adapter (`kernel/src/abi/linux/device/tty.rs`) consume it. The
duplicated definition currently in the Linux ABI adapter is removed.

Layout matches Linux `asm-generic/termbits.h`:

```rust
#[repr(C)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line:  u8,
    pub c_cc:    [u8; NCCS],   // NCCS = 19
}
```

Supported flags (initial target — others reserved but ignored):

| Group | Flags                                                            |
|-------|------------------------------------------------------------------|
| iflag | `IGNBRK INLCR IGNCR ICRNL IXON`                                  |
| oflag | `OPOST ONLCR OCRNL ONLRET`                                       |
| cflag | `CSIZE CS8 CREAD HUPCL CLOCAL` (baud/parity stored, not honored) |
| lflag | `ISIG ICANON ECHO ECHOE ECHOK ECHONL IEXTEN TOSTOP NOFLSH`       |
| c_cc  | `VINTR VQUIT VERASE VKILL VEOF VEOL VMIN VTIME VSUSP VSTART VSTOP` |

## 5. Line discipline (N_TTY)

`tty/ldisc.rs` implements the canonical N_TTY behaviour:

- Cooked mode (`ICANON`): line-buffered, `VERASE`/`VKILL` editing, deliver on
  `\n`, `VEOF`, or `VEOL`.
- Raw mode: byte-at-a-time, honoring `VMIN`/`VTIME` exactly like Linux.
- `ECHO`/`ECHOE`/`ECHOK`/`ECHONL` echo policy.
- `ISIG` translates `VINTR`/`VQUIT`/`VSUSP` into SIGINT/SIGQUIT/SIGTSTP for
  the foreground process group (see §7).
- `ICRNL`/`INLCR`/`IGNCR` translate input line endings; `OPOST`/`ONLCR`
  translate output line endings.
- Foreground-PGID gating: a background process performing read/write while
  the terminal is its controlling TTY and `TOSTOP` (write) is set generates
  SIGTTIN/SIGTTOU.

## 6. ioctl map

The kernel TTY exposes ABI-neutral `ControlOps` opcodes. The Linux ABI
adapter translates the Linux numbers into them. Scarlet-native opcodes
(`SCTL_TTY_*`) remain for direct callers; they are now thin wrappers over
the same core operations.

| Linux ioctl     | Number  | Core operation                                | Scarlet-native opcode             |
|-----------------|---------|-----------------------------------------------|-----------------------------------|
| `TCGETS`        | 0x5401  | `Termios::get`                                | n/a (handled via termios path)    |
| `TCSETS`        | 0x5402  | `Termios::set(no_drain, no_flush)`            | n/a                               |
| `TCSETSW`       | 0x5403  | `Termios::set(drain, no_flush)`               | n/a                               |
| `TCSETSF`       | 0x5404  | `Termios::set(drain, flush)`                  | `SCTL_TTY_FLUSH_INPUT` + set      |
| `TIOCGWINSZ`    | 0x5413  | `get_winsize`                                 | `SCTL_TTY_GET_WINSIZE`            |
| `TIOCSWINSZ`    | 0x5414  | `set_winsize` + SIGWINCH to fg pgid           | `SCTL_TTY_SET_WINSIZE`            |
| `TIOCSCTTY`     | 0x540E  | acquire controlling TTY                       | (no direct opcode; via open path) |
| `TIOCNOTTY`     | 0x5422  | release controlling TTY                       | (no direct opcode)                |
| `TIOCGPGRP`     | 0x540F  | `get_foreground_pgid`                         | `SCTL_TTY_GET_FOREGROUND_GROUP`   |
| `TIOCSPGRP`     | 0x5410  | `set_foreground_pgid`                         | `SCTL_TTY_SET_FOREGROUND_GROUP`   |
| `TIOCGSID`      | 0x5429  | `get_session_id`                              | (new) `SCTL_TTY_GET_SESSION`      |
| `TIOCEXCL`      | 0x540C  | exclusive open                                | (new) `SCTL_TTY_SET_EXCL`         |
| `TIOCNXCL`      | 0x540D  | clear exclusive open                          | (new) `SCTL_TTY_CLR_EXCL`         |
| `TIOCPKT`       | 0x5420  | PTY master packet mode                        | n/a (master-only)                 |
| `TIOCSTI`       | 0x5412  | inject input byte                             | (new) `SCTL_TTY_INJECT_INPUT`     |
| `TIOCGPTN`      | 0x80045430 | get PTY slave number                       | (new) `SCTL_PTY_GET_NUMBER`       |
| `TIOCSPTLCK`    | 0x40045431 | lock/unlock PTY slave                      | (new) `SCTL_PTY_SET_LOCK`         |
| `KDGKBMODE`     | 0x4B44  | keyboard mode get                             | `SCTL_TTY_GET_KBMODE`             |
| `KDSKBMODE`     | 0x4B45  | keyboard mode set                             | `SCTL_TTY_SET_KBMODE`             |

The `SCTL_TTY_SET_FOREGROUND_GROUP` opcode now operates on the **process
group ID** (PGID) rather than the legacy `task_group_id`. The two are kept
in sync during Phase 1 so existing callers continue to work.

## 7. Sessions, process groups, controlling TTY

Each `Task` carries:

- `session_id: AtomicUsize` — Session ID (SID). 0 = uninitialised.
- `process_group_id: AtomicUsize` — Process Group ID (PGID).
- `controlling_tty: RwLock<Option<Weak<TtyDevice>>>` — controlling terminal,
  shared by all members of the session.
- `is_session_leader: AtomicBool` — true iff this task created its session.

Backward compatibility: `task_group_id` is **defined as an alias** for
`process_group_id`. `Task::get_task_group_id` / `set_task_group_id` continue
to compile and now read/write the same atomic. A later migration phase
renames all callers and removes the alias.

### Inheritance rules

| Event                          | SID                  | PGID                 | controlling_tty         |
|--------------------------------|----------------------|----------------------|-------------------------|
| `fork`/`clone` (non-thread)    | inherit              | inherit              | inherit (Weak clone)    |
| `clone(CLONE_THREAD)`          | inherit (same task)  | inherit              | inherit                 |
| `exec`                         | unchanged            | unchanged            | unchanged               |
| `setsid()`                     | new = caller PID     | new = caller PID     | cleared (None)          |
| `setpgid(pid, pgid)`           | unchanged (must = caller's SID) | set to `pgid` | unchanged               |
| `open(/dev/ttyN)` by session leader with no ctty and not `O_NOCTTY` | unchanged | unchanged | acquired (Arc::downgrade) |
| ioctl `TIOCSCTTY` (session leader, no ctty) | unchanged | unchanged | acquired               |
| ioctl `TIOCNOTTY`              | unchanged            | unchanged            | cleared on session leader; for others, cleared on calling task only |
| controlling TTY hangup (UART disconnect / PTY master close) | unchanged | unchanged | SIGHUP → session leader, then cleared |

### Foreground PGID

`TtyDevice::foreground_pgid: Mutex<Option<usize>>` identifies which process
group currently owns the terminal. Set via `TIOCSPGRP`/`TCSETPGRP` by a
process whose controlling TTY is this device and whose SID matches the
TTY's session. Read by `TIOCGPGRP`/`TCGETPGRP`.

### Namespace boundaries

All session/PGID identifiers are **global** task IDs internally. The Linux
ABI adapter translates to/from namespace-local IDs at the syscall boundary
using `TaskNamespace::resolve_local_id` / `resolve_global_id`. A process
group cannot span namespaces; `setpgid` rejects pgids outside the caller's
namespace.

## 8. Signals (job control)

Phase 2 adds POSIX-style signals on top of the existing event queue:

`SIGHUP SIGINT SIGQUIT SIGPIPE SIGCHLD SIGCONT SIGSTOP SIGTSTP SIGTTIN SIGTTOU SIGWINCH`.

Delivery API:

```rust
signal::send_to_task(task_id, sig);
signal::send_to_pgid(pgid, sig);
signal::send_to_session(sid, sig);
```

Default actions when no handler is registered:

| Signal     | Default                       |
|------------|-------------------------------|
| SIGINT/SIGQUIT/SIGTERM/SIGHUP/SIGPIPE | Terminate         |
| SIGTSTP/SIGTTIN/SIGTTOU       | Stop                          |
| SIGCONT                       | Continue (Stopped → Ready)    |
| SIGWINCH/SIGCHLD              | Ignore                        |

The TTY input path (`ldisc`) maps `VINTR/VQUIT/VSUSP` to
`send_to_pgid(foreground_pgid, SIG…)`. `TIOCSWINSZ` triggers
`send_to_pgid(foreground_pgid, SIGWINCH)`. Closing a PTY master sends
SIGHUP to the slave's session leader.

## 9. PTY architecture

`PtyPair` holds the bidirectional state for one master/slave pair:

```rust
struct PtyPair {
    id: usize,                        // pts number
    master_to_slave: Mutex<VecDeque<u8>>,  // master writes here, ldisc reads
    slave_to_master: Mutex<VecDeque<u8>>,  // ldisc writes here, master reads
    slave_tty:    Arc<TtyDevice>,     // owns termios + ldisc
    master_waker: Waker,
    locked: AtomicBool,               // TIOCSPTLCK
    packet_mode: AtomicBool,          // TIOCPKT
}
```

- The slave is a real `TtyDevice` with the same line discipline as a UART
  TTY. Its `TtyDriver` impl pushes output bytes into `slave_to_master` and
  wakes the master waker.
- The master is a `CharDevice` that:
  - On `read`, pops `slave_to_master`.
  - On `write`, pushes bytes into `master_to_slave`; the slave TTY's input
    path drains them through the line discipline.
  - Forwards `TIOCSWINSZ` to the slave (and the slave generates SIGWINCH).
  - Closing the master sends SIGHUP to the slave session and tears down the
    slave's `controlling_tty` references.

`/dev/ptmx` is registered as a special char device. `open()` on it:
1. Allocates a new pts number from a bitmap (`PtyAllocator`).
2. Creates the `PtyPair`.
3. Registers the slave node `/dev/pts/<n>` in the devpts filesystem.
4. Returns a file handle to the master.

devpts is a small dedicated VFS (`kernel/src/fs/vfs_v2/drivers/devpts.rs`)
that exposes only the live slaves; nodes appear/disappear with allocation.

## 10. User-space surface

Phase 5 adds to `user/lib/std`:

- `scarlet::tty` — `Termios`, `tcgetattr`, `tcsetattr`, `tcgetpgrp`,
  `tcsetpgrp`, `tcsetwinsize`, `isatty`, `ttyname`.
- `scarlet::pty` — `openpty()`, `forkpty()`.
- `scarlet::process` — `setsid`, `setpgid`, `getpgid`, `getsid`.

These wrap the kernel control opcodes (Scarlet-native ABI) and are mirrored
by the Linux ABI for binary compatibility with Linux userspace.

## 11. Scarlet Terminal (GUI)

Phase 7 introduces `user/bin/src/scarlet_terminal/`, an application that:

1. `openpty()` to allocate a master/slave pair.
2. `forkpty()` to spawn `/bin/sh` on the slave (setsid + TIOCSCTTY +
   dup2(slave) for stdin/out/err).
3. Renders the slave's output into a monospace cell grid (xterm-compatible
   CSI/SGR/ED/EL/CUP/SU/SD, OSC title).
4. Forwards keyboard events from EventDevice into the master (with ESC
   sequence encoding for arrows/function keys).
5. On window resize, issues `TIOCSWINSZ` on the master → kernel sends
   SIGWINCH to the foreground PGID.

## 12. Phase tracker (informational)

| Phase | Scope                                            | State                |
|------:|--------------------------------------------------|----------------------|
| 0     | This document                                    | ✅ delivered here    |
| 1     | Task: SID/PGID/ctty fields + setsid/setpgid      | ✅ delivered here    |
| 2     | Signals subsystem (job control set)              | ❌ not yet           |
| 3     | TTY refactor (`tty/` split, full termios)        | ❌ not yet           |
| 4     | PTY + devpts                                     | ❌ not yet           |
| 5     | `user/lib/std` tty/pty/process                   | ❌ not yet           |
| 6     | Shell/init/getty job control                     | ❌ not yet           |
| 7     | Scarlet Terminal GUI app                         | ❌ not yet           |
| 8     | ScarletUI components (TextGrid, ScrollView, …)   | ❌ not yet           |
| 9     | Integration tests                                | ❌ not yet           |
| 10    | Migrate `task_group_id` → `process_group_id`     | ❌ not yet           |

This file is updated at each phase boundary so reviewers can see the gap
between current behaviour and the target design.
