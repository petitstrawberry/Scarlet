# Stem Daemon (stemd)

## Overview

Stem Daemon (`stemd`) is a systemd-like service manager for Scarlet OS. It provides centralized process management with dependency resolution, .desktop application registry, service readiness tracking, and IPC-based control via both Unix-domain socket and sbus.

## Architecture

```text
init
 └─> stemd
      ├── Config parser (TOML: /etc/stemd.d/*.toml)
      ├── Desktop file loader (/usr/share/applications/*.desktop)
      ├── Dependency resolver (topological sort)
      ├── Service launcher (fork/exec with TTY attachment)
      ├── IPC thread (Unix socket: /tmp/stemd.sock)
      ├── sbus listener (org.scarlet-os.stem method calls)
      └── Process reaper (waitpid)
```

stemd consists of these modules:

| Module | File | Responsibility |
|--------|------|----------------|
| Main | `user/bin/src/stemd/main.rs` | Config parsing, service lifecycle, process reaping |
| Protocol | `user/bin/src/stemd/protocol.rs` | IPC command definitions and payload builders |
| Desktop | `user/bin/src/stemd/desktop.rs` | XDG .desktop file parser and app registry |

## Configuration

stemd reads TOML-based configuration from `/etc/stemd.d/` (or `/system/scarlet/etc/stemd.d/` on deployed images). If the directory is absent, it falls back to `/etc/stemd.toml`.

### Service Definition

```toml
[service.login]
exec = "/system/scarlet/bin/login"
depends = []
after = ["window_server"]
tty = "/dev/tty0"

[service.sws]
exec = "/system/scarlet/bin/sws"
depends = []
ready_notify = true
ready_timeout_ms = 10000
```

| Field | Required | Description |
|-------|----------|-------------|
| `exec` | Yes | Full path to the executable |
| `depends` | No | Hard dependencies (must start first) |
| `after` | No | Ordering hints (soft, ignored if target missing) |
| `tty` | No | TTY device path for stdio attachment |
| `order` | No | Integer ordering hint (default 0) |
| `ready_notify` | No | Expect `SERVICE_READY` from this service (default false) |
| `ready_timeout_ms` | No | Timeout for readiness notification (default 5000 ms) |

## .desktop Application Registry

stemd loads XDG Desktop Entry files from `/usr/share/applications/` (or `/system/scarlet/usr/share/applications/`) at startup. Loaded apps are available for launch via the IPC `LAUNCH_OR_FOCUS` command.

```text
[Desktop Entry]
Name=Terminal
Exec=/bin/sh
Icon=terminal
Type=Application
```

## Service Lifecycle

1. **Config loading**: Parse all `.toml` files from config directory
2. **App loading**: Parse all `.desktop` files from applications directory
3. **Dependency resolution**: Topological sort respecting `depends`, `after`, and `order`
4. **Service startup**: Fork/exec in resolved order; attach TTY if configured
5. **Readiness wait**: If `ready_notify = true`, wait for `SERVICE_READY` IPC or timeout
6. **Process reaping**: Main thread calls `waitpid(-1, 0)` continuously
7. **App tracking**: Running apps/services tracked in global state for focus management

## IPC Protocol

stemd listens on `/tmp/stemd.sock`. All messages start with a 1-byte command type.

| Command | Code | Payload | Description |
|---------|------|---------|-------------|
| `STATUS` | 0x00 | empty | Query daemon status |
| `LAUNCH_OR_FOCUS` | 0x01 | app_id + exec_path | Launch app or focus existing window |
| `REGISTER_APP` | 0x02 | app definition | Register application dynamically |
| `UNREGISTER_APP` | 0x03 | app_id | Remove application from registry |
| `SHUTDOWN` | 0x04 | empty | Shut down stemd |
| `LAUNCH` | 0x05 | app_id + exec_path | Always start a new process |
| `SERVICE_READY` | 0x06 | service_name | Service notifies readiness |

### Launch Payload Format

Variable-length payload for `LAUNCH_OR_FOCUS` and `LAUNCH`:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 4 | `app_id_len` (u32 LE) |
| 4 | N | `app_id` (bytes) |
| 4+N | 4 | `exec_path_len` (u32 LE) |
| 8+N | M | `exec_path` (bytes) |

If `exec_path` is empty, stemd looks up the app from registered .desktop files.

## sbus Integration

stemd connects to sbus (Scarlet's D-Bus-like service bus) and registers as `org.scarlet-os.stem`. External processes can call methods over sbus for app launching and status queries.

## Manual Invocation

When run from an interactive shell, stemd forks once and the parent exits immediately. In this mode, stdio is detached to `/dev/null` and services with `tty` configured are skipped to avoid stealing the terminal.

## Source Code

- Main: `user/bin/src/stemd/main.rs`
- Protocol: `user/bin/src/stemd/protocol.rs`
- Desktop parser: `user/bin/src/stemd/desktop.rs`
- Init integration: `user/bin/src/init.rs`
- Default config: `mkfs/initramfs/system/scarlet/etc/stemd.d/`
