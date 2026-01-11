# Stem Daemon (stemd)

## Overview

Stem Daemon (`stemd`) is a systemd-like service manager for Scarlet OS. It provides centralized process management with dependency resolution, zombie process reaping, and IPC-based control.

## Architecture

stemd consists of three main components:

### 1. Main Thread
- Reads the TOML configuration file (`/etc/stemd.toml`)
- Parses service definitions and dependencies
- Resolves dependencies using topological sorting
- Launches services in dependency order

### 2. Wait Thread
- Continuously reaps zombie processes using `waitpid(-1, 0)`
- Prevents zombie process accumulation
- Logs process exit status for debugging

### 3. IPC Thread
- Listens on a Unix-domain socket at `/tmp/stemd.sock`
- Accepts control commands from clients
- Currently supports:
  - `status` - Returns daemon status
  - `help` - Lists available commands

## Configuration File Format

stemd reads TOML-based configuration files from the `/etc/stemd.d/` directory. All `.toml` files in this directory are loaded and processed in alphabetical order. If the directory doesn't exist or contains no configuration files, stemd falls back to reading `/etc/stemd.toml`.

### Example Configuration

Create configuration files in `/etc/stemd.d/`:

**`/etc/stemd.d/00-login.toml`:**
```toml
# Stem Daemon Configuration File
# Services are launched in dependency order

[service.login]
exec = "/system/scarlet/bin/login"
depends = []
```

**`/etc/stemd.d/10-services.toml`:**
```toml
[service.window_server]
exec = "/system/scarlet/bin/sws"
depends = []

[service.shell]
exec = "/system/scarlet/bin/sh"
depends = ["login"]
```

### Configuration Syntax

Each service is defined as a TOML section with the format `[service.name]`:

- `exec` (required): Full path to the executable to launch
- `depends` (optional): Array of service names that must start before this service

### Configuration Loading Order

1. **Primary**: Reads all `.toml` files from `/etc/stemd.d/` in alphabetical order
2. **Fallback**: If directory doesn't exist or is empty, reads `/etc/stemd.toml`
3. **Default**: If both fail, uses built-in configuration with login service

## Integration with init

The `init` process now launches `stemd` instead of directly launching `login`:

```rust
// In init.rs
println!("init: Starting stem daemon (stemd)...");
// ... fork and exec stemd ...
```

## Usage

### Running stemd

stemd is automatically started by the init process during system boot. It does not need to be manually invoked.

### Querying stemd Status

You can query stemd status via the IPC socket:

```bash
# Using a socket client (example)
echo "status" | nc -U /tmp/stemd.sock
```

## Implementation Details

### Service Dependencies

stemd uses a simple topological sort algorithm to resolve dependencies:

1. All services are loaded from the configuration file
2. Dependencies are resolved in order (depth-first traversal)
3. Services are launched sequentially with a brief delay between launches

### Process Management

- Each service is launched via `fork()` and `execve()`
- The main thread tracks launched service PIDs
- The wait thread handles process cleanup asynchronously

### TOML Parser

stemd includes a lightweight, no_std-compatible TOML parser that supports:
- Section headers: `[service.name]`
- Key-value pairs: `key = value`
- String values with optional quotes
- Arrays: `key = ["value1", "value2"]`
- Comments: `# comment`

## Limitations

Current implementation has the following limitations:

1. **No service restart**: If a service exits, it is not automatically restarted
2. **No service status tracking**: stemd does not track which services are running
3. **Sequential startup**: Services start one at a time, not in parallel
4. **Basic IPC**: Limited command set via IPC interface
5. **No service control**: Cannot stop or restart services via IPC

## Future Enhancements

Potential improvements for stemd:

- [ ] Service restart policies (always, on-failure, never)
- [ ] Parallel service startup for independent services
- [ ] Service health checks and monitoring
- [ ] Extended IPC commands (stop, restart, reload)
- [ ] Service logs collection and management
- [ ] Socket activation support
- [ ] Resource limits (CPU, memory) per service

## Source Code

- Implementation: `user/bin/src/stemd.rs`
- Configuration directory: `mkfs/initramfs/etc/stemd.d/`
- Default config: `mkfs/initramfs/etc/stemd.d/00-login.toml`
- Init integration: `user/bin/src/init.rs`

## Related Documentation

- [Container System](container_system.md)
- [Socket VFS Integration](socket_vfs_integration.md)
- [Task Namespace Design](task_namespace_design.md)
