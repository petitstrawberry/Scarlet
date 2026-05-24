# TTY and PTY Redesign

This document fixes the review target for issue #413: Scarlet's terminal layer
should converge on the Linux model while keeping the existing Scarlet-private
TTY control opcodes as compatibility shims.

## Target Layering

Linux names are shown on the left and Scarlet implementation names on the
right.

```text
process/session/job control
  task_struct::signal/session/pgrp/tty
      -> Task::{session_id, process_group_id, controlling_tty}

tty_struct
      -> device::char::tty::core::TtyDevice

tty_ldisc / N_TTY
      -> device::char::tty::ldisc

tty_driver
      -> device::char::tty::driver::TtyDriver

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
`SIGWINCH` to the slave TTY foreground PGID.

## Ownership Model

Scarlet uses global task IDs internally. Namespace-local PIDs/PGIDs/SIDs are
translated at ABI boundaries.

```text
Session (SID = leader task ID)
  owns zero or one controlling TTY
  contains process groups

Process group (PGID = leader task ID)
  is the unit for foreground/background terminal access
  is the target for terminal-generated job-control signals

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

`setsid(2)` sets `SID = PGID = current task ID`, clears the controlling TTY,
and marks the caller as a session leader. `setpgid(2)` is constrained to the
caller or its direct child, the same session, and a target process group in the
same session.

## Job-Control State Machine

```text
keyboard input in foreground tty
  Ctrl-C  -> SIGINT  -> foreground PGID
  Ctrl-\  -> SIGQUIT -> foreground PGID
  Ctrl-Z  -> SIGTSTP -> foreground PGID

background read from controlling tty
  -> SIGTTIN to caller PGID, unless ignored/blocked

background write with TOSTOP
  -> SIGTTOU to caller PGID, unless ignored/blocked

SIGTSTP/SIGTTIN/SIGTTOU default
  Running -> Stopped

SIGCONT default
  Stopped -> Ready
```

Signal delivery should be centralized in `kernel/src/task/signal/` (or the
existing Linux generic signal module until that split lands). TTY code should
call a PGID-level helper instead of scanning tasks itself once that helper
exists.

## Scarlet Control Opcode Mapping

Existing Scarlet-private controls remain stable and map to the shared termios
and job-control state.

| Scarlet control | Linux-facing equivalent | Shared state |
| --- | --- | --- |
| `SCTL_TTY_SET_ECHO` / `GET_ECHO` | `TCSETS` / `TCGETS`, `ECHO` | `termios.c_lflag` |
| `SCTL_TTY_SET_CANONICAL` / `GET_CANONICAL` | `TCSETS` / `TCGETS`, `ICANON` | `termios.c_lflag` |
| `SCTL_TTY_SET_READ_POLICY` / `GET_READ_POLICY` | `VMIN` / `VTIME` | `termios.c_cc` |
| `SCTL_TTY_SET_WINSIZE` / `GET_WINSIZE` | `TIOCSWINSZ` / `TIOCGWINSZ` | TTY winsize |
| `SCTL_TTY_SET_KBMODE` / `GET_KBMODE` | `KDSKBMODE` / `KDGKBMODE` | console keyboard mode |
| `SCTL_TTY_SET_FOREGROUND_GROUP` / `GET_FOREGROUND_GROUP` | `TIOCSPGRP` / `TIOCGPGRP` | foreground PGID |
| `SCTL_TTY_FLUSH_INPUT` | `TCSETSF`, future `TCFLSH` | ldisc input queue |
| `SCTL_TTY_SET_DEBUG` / `GET_DEBUG` | Scarlet-only | diagnostics |

Linux-only ioctls planned for the job-control and PTY phases:

- `TIOCSCTTY`, `TIOCNOTTY`, `TIOCGSID`
- `TIOCEXCL`, `TIOCNXCL`
- `TIOCPKT`
- `TIOCSTI`
- `TIOCGPTN`, `TIOCSPTLCK`

## Merge Plan

1. Land the task SID/PGID/control-tty model and Linux `setsid`,
   `setpgid`, `getsid`, `getpgid`.
2. Move signal numbers and default actions into a kernel-neutral signal module,
   then make TTY input deliver to PGIDs through that module.
3. Split `tty.rs` into the target module structure and move termios constants
   to a single shared definition.
4. Add PTY pair, `/dev/ptmx`, and `devpts`.
5. Add user-space wrappers and shell job-control behavior.
6. Build the ScarletUI terminal only after PTY and foreground PGID behavior are
   usable.
