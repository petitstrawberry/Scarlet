# TTY and PTY Redesign

This document fixes the review target for issue #413: Scarlet's terminal layer
should converge on the Linux model while keeping the existing Scarlet-private
TTY control opcodes as compatibility shims.

## Target Layering

Linux names are shown on the left and Scarlet implementation names on the
right.

```text
process/session/job control
  task_struct::session/pgrp/tty + EventContent::ProcessControl
      -> Task::{session_id, process_group_id, controlling_tty}

tty_struct
      -> device::char::tty::core::TtyDevice

tty_ldisc / N_TTY
      -> device::char::tty::ldisc

tty_driver
      -> device::char::tty::driver::TtyDriver
      -> current transition: device::char::tty::TtyBackend

hardware or virtual endpoint
      -> UART CharDevice, PTY slave driver, future console backends
```

The current `kernel/src/device/char/tty.rs` is still a single module. The
migration target is:

```text
kernel/src/device/char/tty/
  mod.rs
  core.rs          # TtyDevice state and CharDevice glue
  ldisc.rs         # N_TTY-compatible line discipline
  termios.rs       # shared Linux-compatible termios constants and layout
  job_control.rs   # foreground PGID and controlling-tty ioctls
  driver.rs        # lower tty_driver-like trait
```

`kernel/src/abi/linux/device/tty.rs` must become a translation layer only. It
should not define a second source of truth for termios flags once
`tty/termios.rs` exists.

## PTY Data Flow

Unix98 PTY support is modeled as `/dev/ptmx` creating a master/slave pair and
`devpts` publishing the slave as `/dev/pts/N`.

```text
terminal emulator process
  write(master) ------------------------------------+
                                                    |
                                                    v
                                             PTY pair input
                                                    |
                                                    v
                                      slave TTY line discipline
                                                    |
                                                    v
                                            foreground shell

terminal emulator process
  read(master)  <------------------------------------+
                                                    |
                                                    v
                                      slave TTY output path
                                                    ^
                                                    |
                                            foreground shell
```

Master writes inject bytes into the slave TTY input path, so canonical mode,
echo, ISIG, ICRNL, and future UTF-8 handling are applied exactly once. Slave
writes bypass input processing and become master-readable output. Window size
is stored on the pair and reflected through both sides; `TIOCSWINSZ` must send
`ProcessControlType::WindowChange` to the slave TTY foreground PGID. The Linux
ABI projects that event to `SIGWINCH`.

Current Phase 4 foundation:

- `device::char::pty::PtyPair` owns the master/slave data path.
- `fs::vfs_v2::drivers::devpts::DevPtsFS` owns PTY number allocation and
  publishes `ptmx` plus active numeric slave entries.
- `devfs` exposes a stable `/dev/pts` mount point and a `/dev/ptmx` symlink
  whose target is relative (`pts/ptmx`), so it stays within the mounted `/dev`
  view instead of baking in a global absolute path.
- Slave endpoints start locked and are unlocked through `TIOCSPTLCK`, matching
  the Unix98 PTY slave-lock flow at the ioctl layer.
- Scarlet native user space uses `scarlet_std::pty::{PtyMaster, PtySlave,
  PtyPair}`. `PtyMaster::open()` opens `/dev/ptmx` and its methods call
  Scarlet-private `SCTL_PTY_*`/`SCTL_TTY_*` controls rather than adding Linux
  ioctl or POSIX wrapper names to the native ABI.
- `user/bin/pty_smoke` is a small manual integration check for opening a PTY and
  verifying basic master/slave I/O.

## Ownership Model

Scarlet uses global task IDs internally. Namespace-local PIDs/PGIDs/SIDs are
translated at ABI boundaries.

```text
Session (SID = leader task ID)
  owns zero or one controlling TTY
  contains process groups

Process group (PGID = leader task ID)
  is the unit for foreground/background terminal access
  is the target for terminal-generated process-control events

Task/thread group
  inherits SID, PGID, and controlling TTY across fork/clone
  may call setsid() if it is not already a process group leader
```

Current Phase 1 state:

- `Task::session_id` stores the global SID.
- `Task::process_group_id` stores the global PGID.
- `Task::task_group_id` is a migration mirror of PGID for older Scarlet
  controls.
- `Task::controlling_tty` is a `Weak<TtyDevice>` to avoid cycles.
- `Task::is_session_leader` records session leadership explicitly.
- `TtyDevice` stores its controlling SID and a weak self-reference so Linux
  `TIOCSCTTY`, `TIOCNOTTY`, and `TIOCGSID` can operate without adding a
  separate signal/session subsystem.

`setsid(2)` sets `SID = PGID = current task ID`, clears the controlling TTY,
and marks the caller as a session leader. `setpgid(2)` is constrained to the
caller or its direct child, the same session, and a target process group in the
same session.

Opening a concrete TTY such as `/dev/tty0` without `O_NOCTTY` attempts to
acquire it as the caller's controlling terminal when the caller is a session
leader and has no controlling TTY. `/dev/tty` is treated as the caller's
controlling terminal and fails with `ENXIO` when none is attached. A terminal
put into `TIOCEXCL` mode rejects later opens with `EBUSY` until `TIOCNXCL`
clears the exclusive flag.

`TIOCSPGRP` is accepted only on the caller's controlling TTY, and the target
PGID must name a process group leader in the caller's session.

## Job-Control State Machine

```text
keyboard input in foreground tty
  Ctrl-C  -> ProcessControlType::Interrupt    -> foreground PGID
  Ctrl-\  -> ProcessControlType::Quit         -> foreground PGID
  Ctrl-Z  -> ProcessControlType::TerminalStop -> foreground PGID

background read from controlling tty
  -> ProcessControlType::TerminalInput to caller PGID, unless ignored/blocked

background write with TOSTOP
  -> ProcessControlType::TerminalOutput to caller PGID, unless ignored/blocked

TerminalStop/TerminalInput/TerminalOutput default
  Running -> Stopped

Continue default
  Stopped -> Ready
```

Scarlet does not add a separate POSIX signal subsystem in kernel core. The
kernel-neutral representation is the existing Event path:
`EventContent::ProcessControl(ProcessControlType)`, delivered with
`EventDelivery::Group(GroupTarget::TaskGroup(pgid))`. ABI layers translate only
at their boundary. For example, the Linux ABI maps `TerminalStop` to `SIGTSTP`,
`TerminalInput` to `SIGTTIN`, `TerminalOutput` to `SIGTTOU`, and
`WindowChange` to `SIGWINCH`.

## Scarlet Control Opcode Mapping

Existing Scarlet-private controls remain stable and map to the shared termios
and job-control state.

| Scarlet control | Linux-facing equivalent | Shared state |
| --- | --- | --- |
| `SCTL_TTY_SET_ECHO` / `GET_ECHO` | `TCSETS` / `TCGETS`, `ECHO` | `termios.c_lflag` |
| `SCTL_TTY_SET_CANONICAL` / `GET_CANONICAL` | `TCSETS` / `TCGETS`, `ICANON` | `termios.c_lflag` |
| `SCTL_TTY_SET_SIGNAL_CHARS` / `GET_SIGNAL_CHARS` | `TCSETS` / `TCGETS`, `ISIG` | `termios.c_lflag` |
| `SCTL_TTY_SET_CRNL_INPUT` / `GET_CRNL_INPUT` | `TCSETS` / `TCGETS`, `ICRNL` | `termios.c_iflag` |
| `SCTL_TTY_SET_OUTPUT_POSTPROCESS` / `GET_OUTPUT_POSTPROCESS` | `TCSETS` / `TCGETS`, `OPOST` | `termios.c_oflag` |
| `SCTL_TTY_SET_EXTENDED_INPUT` / `GET_EXTENDED_INPUT` | `TCSETS` / `TCGETS`, `IEXTEN` | `termios.c_lflag` |
| TTY internal `tostop_enabled` | `TCSETS` / `TCGETS`, `TOSTOP` | `termios.c_lflag` |
| TTY internal control chars | `VINTR`, `VQUIT`, `VERASE`, `VEOF`, `VSUSP`, `VLNEXT` | `termios.c_cc` |
| `SCTL_TTY_SET_READ_POLICY` / `GET_READ_POLICY` | `VMIN` / `VTIME` | `termios.c_cc` |
| `SCTL_TTY_SET_WINSIZE` / `GET_WINSIZE` | `TIOCSWINSZ` / `TIOCGWINSZ` | TTY winsize |
| `SCTL_TTY_SET_KBMODE` / `GET_KBMODE` | `KDSKBMODE` / `KDGKBMODE` | console keyboard mode |
| `SCTL_TTY_SET_FOREGROUND_GROUP` / `GET_FOREGROUND_GROUP` | `TIOCSPGRP` / `TIOCGPGRP` | foreground PGID |
| `SCTL_TTY_FLUSH_INPUT` | `TCSETSF`, `TCFLSH` | ldisc input queue |
| TTY input/output queue queries | `TIOCINQ` / `FIONREAD`, `TIOCOUTQ` | ldisc queue lengths |
| TTY drain/flow no-op compatibility | `TCSBRK`, `TCXONC` | current TTY endpoint |
| `SCTL_TTY_SET_DEBUG` / `GET_DEBUG` | Scarlet-only | diagnostics |
| `SCTL_PTY_GET_NUMBER` | `TIOCGPTN` | PTY master pair number |
| `SCTL_PTY_SET_LOCKED` / `GET_LOCKED` | `TIOCSPTLCK` / `TIOCGPTLCK` | PTY slave lock state |

PTY master endpoints forward `SCTL_TTY_SET_WINSIZE` and
`SCTL_TTY_GET_WINSIZE` to their connected slave TTY. This keeps native
`PtyMaster::set_winsize` and Linux `TIOCSWINSZ` on the master endpoint on the
same code path as direct slave TTY updates, including
`ProcessControlType::WindowChange` delivery through the existing Event path.

Linux-only ioctls covered or planned for the job-control and PTY phases:

- covered now: `TIOCSCTTY`, `TIOCNOTTY`, `TIOCGSID`
- covered now: `TIOCEXCL`, `TIOCNXCL`, `TIOCSTI`
- covered now: `TIOCGPTN`, `TIOCSPTLCK`, `TIOCGPTLCK`
- `TIOCPKT`

## Merge Plan

1. Land the task SID/PGID/control-tty model and Linux `setsid`,
   `setpgid`, `getsid`, `getpgid`.
2. Extend `ProcessControlType` for terminal job-control events and make TTY
   input deliver to foreground PGIDs through `EventManager`.
3. Split `tty.rs` into the target module structure and move termios constants
   to a single shared definition.
4. Add PTY pair, `/dev/ptmx`, and `devpts`.
5. Add user-space wrappers and shell job-control behavior.
6. Build the ScarletUI terminal only after PTY and foreground PGID behavior are
   usable.
