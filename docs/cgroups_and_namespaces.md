# Cgroups and Namespaces Implementation in Scarlet Linux ABI

## Overview

This document describes the implementation of cgroups (control groups) and namespace isolation features in the Scarlet Linux ABI module. These features enable containerization and resource management capabilities for Linux applications running on Scarlet.

## Components

### 1. Cgroups (Control Groups)

**Location**: `kernel/src/abi/linux/riscv64/cgroup.rs`

Cgroups provide a mechanism for organizing processes into hierarchical groups and controlling resource allocation. The current implementation provides:

#### Features
- **Stub Resource Controllers**: CPU, Memory, I/O, PIDs, and CPU set controllers
- **Cgroup Hierarchy**: Basic support for cgroup version 2 (unified hierarchy)
- **Compatibility Layer**: Applications can interact with cgroup interfaces without errors

#### Limitations
- Resource limits are accepted but **not enforced**
- Read/write operations succeed but don't affect actual resource usage
- This is intentional to maintain compatibility while core features are implemented

#### Controller Types
```rust
pub enum CgroupController {
    Cpu,      // CPU time allocation
    Memory,   // Memory limits
    Io,       // Disk I/O limits
    Pids,     // Process ID limits
    Cpuset,   // CPU affinity
}
```

### 2. Namespace Isolation (unshare/setns)

**Location**: `kernel/src/abi/linux/riscv64/unshare.rs`

Namespaces provide isolation for system resources, allowing processes to have independent views of various system aspects.

#### Implemented Namespaces

##### PID Namespace (`CLONE_NEWPID`)
- **Status**: ✅ Functional
- **Implementation**: Uses Scarlet's existing Task namespace infrastructure
- Creates a new task namespace for PID isolation
- New tasks spawned after unshare will use the new namespace
- Parent tasks retain their original PIDs

##### Mount Namespace (`CLONE_NEWNS`)
- **Status**: ✅ Functional
- **Implementation**: Uses Scarlet's existing VFS separation feature
- Creates a new isolated VFS instance
- Basic filesystem directories (/dev, /proc, /sys, /tmp) are automatically created
- Each namespace has independent mount points

##### Other Namespaces (Stub)
- **UTS** (`CLONE_NEWUTS`): Hostname/domain name isolation - **stub**
- **IPC** (`CLONE_NEWIPC`): System V IPC isolation - **stub**
- **Network** (`CLONE_NEWNET`): Network stack isolation - **stub**
- **User** (`CLONE_NEWUSER`): UID/GID isolation - **stub**
- **Cgroup** (`CLONE_NEWCGROUP`): Cgroup view isolation - **stub**

Stub namespaces accept the flags but don't perform actual isolation yet.

### 3. System Calls

#### sys_unshare (syscall 97)
Disassociate parts of the execution context.

**Signature**:
```c
int unshare(int flags);
```

**Arguments**:
- `flags`: Combination of `CLONE_NEW*` flags

**Returns**:
- 0 on success
- -EPERM if no current task context
- -ENOMEM if memory allocation fails (only for mount namespace)

**Usage Example**:
```c
// Create new PID and mount namespaces
unshare(CLONE_NEWPID | CLONE_NEWNS);
```

#### sys_setns (syscall 268)
Join an existing namespace.

**Signature**:
```c
int setns(int fd, int nstype);
```

**Status**: ⚠️ Stub - returns ENOSYS

**Future Implementation**: Requires /proc/[pid]/ns/* filesystem support

### 4. Syscall Table Integration

Added to `kernel/src/abi/linux/riscv64/mod.rs`:
```rust
syscall_table! {
    // ...
    Unshare = 97 => unshare::sys_unshare,
    // ...
    Setns = 268 => unshare::sys_setns,
    // ...
}
```

## Architecture

### Task Namespace Integration

The implementation leverages Scarlet's existing task namespace infrastructure:

```
┌─────────────────────────────────────┐
│   LinuxRiscv64Abi Instance          │
│                                     │
│  ┌──────────────────────────────┐  │
│  │  Task Namespace (Arc)        │  │
│  │  - next_task_id              │  │
│  │  - local_to_global mapping   │  │
│  │  - global_to_local mapping   │  │
│  │  - parent namespace ref      │  │
│  └──────────────────────────────┘  │
│                                     │
│  ┌──────────────────────────────┐  │
│  │  File Descriptor Table       │  │
│  │  VFS Integration              │  │
│  └──────────────────────────────┘  │
└─────────────────────────────────────┘
```

### VFS Integration

Mount namespace isolation uses per-task VFS instances:

```
┌────────────────────────────────────┐
│   Task Structure                   │
│                                    │
│  vfs: Option<Arc<VfsManager>>     │
│                                    │
│  ┌─────────────────────────────┐  │
│  │  VfsManager Instance        │  │
│  │  - mount_tree              │  │
│  │  - root filesystem         │  │
│  │  - independent mounts      │  │
│  └─────────────────────────────┘  │
└────────────────────────────────────┘
```

## Testing

The implementation includes basic unit tests:

### Cgroups Tests
- `test_cgroup_subsystem_creation`: Verify cgroup subsystem initialization
- `test_cgroup_operations_stub`: Verify stub operations don't panic

### Namespace Tests
- `test_unshare_flags_defined`: Verify flag values match Linux ABI

Additional integration tests should be added to verify:
- PID namespace isolation between parent and child processes
- Mount namespace independence
- Cgroup hierarchy operations

## Usage in Containers

This implementation provides the foundation for running containerized Linux applications on Scarlet:

1. **Create isolated namespace**: Call `unshare(CLONE_NEWPID | CLONE_NEWNS)`
2. **Setup container environment**: Mount filesystems, setup devices
3. **Launch container process**: Execute application in the isolated namespace

Example flow:
```c
// Container launcher code
unshare(CLONE_NEWPID | CLONE_NEWNS);  // Isolate PID and mount namespaces
// Setup container filesystem structure
// Execute container init process
execve("/container/init", ...);
```

## Future Enhancements

### Short-term
1. **Resource Enforcement**: Implement actual CPU and memory limits in cgroup controllers
2. **setns Implementation**: Support joining existing namespaces via /proc
3. **UTS Namespace**: Implement independent hostname/domainname per namespace

### Medium-term
4. **IPC Namespace**: Isolate System V IPC objects
5. **Cgroup Filesystem**: Mount cgroupfs for userspace interaction
6. **Network Namespace**: Basic network stack isolation

### Long-term
7. **User Namespace**: UID/GID mapping and privilege isolation
8. **Advanced Resource Controls**: I/O throttling, CPU pinning
9. **Cgroup v1 Compatibility**: Support both v1 and v2 hierarchies

## References

- Linux namespaces(7) man page
- Linux cgroups(7) man page
- Scarlet Task Namespace: `kernel/src/task/namespace.rs`
- Scarlet VFS: `kernel/src/fs/vfs_v2/manager.rs`

## Conclusion

This implementation provides a solid foundation for containerization in Scarlet while maintaining compatibility with Linux applications. The use of existing Scarlet features (Task namespace and VFS) ensures a clean integration, and stub implementations for unfinished features allow applications to run without modification.
