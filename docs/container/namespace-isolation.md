# Scarlet Container System

## Overview

Scarlet implements a lightweight, flexible containerization system using the `CreateNamespace` syscall. Unlike traditional Unix approaches (like `unshare`), Scarlet uses a **smart, flag-based API** that allows combining multiple isolation types in a single syscall.

## Design Philosophy

Scarlet's containerization follows these principles:

1. **Cross-ABI Collaboration by Default**: Different ABIs (Linux, xv6, Scarlet) share resources by default
2. **Explicit Isolation**: Containers are created only when explicitly requested
3. **Smart API**: Flag-based syscall allows combining multiple isolations efficiently
4. **Future-Proof**: Extensible design supports adding new isolation types

## CreateNamespace Syscall

### Syscall Number
**92** (Process Management range)

### Syntax
```c
int create_namespace(unsigned long flags, const char *name);
```

### Flags
- `NS_CREATE_TASK` (0x01): Create separate task namespace (PIDs)
- `NS_CREATE_VFS` (0x02): Create separate VFS namespace (filesystem view)
- `NS_CREATE_NET` (0x04): Create separate network namespace (future)
- `NS_CREATE_IPC` (0x08): Create separate IPC namespace (future)

Flags can be combined with bitwise OR.

### Parameters
- `flags`: Bitfield specifying which namespaces to create
- `name`: Optional namespace name (NULL for auto-generated name)

### Return Value
- `0` on success
- `-1` (usize::MAX) on failure

## Usage Examples

### Example 1: Task Namespace Isolation

Creates a separate PID space:

```rust
use scarlet_std::syscall::{Syscall, syscall2};

const NS_CREATE_TASK: usize = 0x01;

fn create_container() {
    // Create separate task namespace
    let result = syscall2(
        Syscall::CreateNamespace,
        NS_CREATE_TASK,
        "my_container\0".as_ptr() as usize
    );
    
    if result == 0 {
        // Now in new task namespace
        // PIDs visible to this process are independent
        println!("Container created, PID: {}", getpid());
    }
}
```

### Example 2: Combined Isolation (Full Container)

Creates both task and VFS isolation:

```rust
const NS_CREATE_TASK: usize = 0x01;
const NS_CREATE_VFS: usize = 0x02;

fn create_full_container() {
    // Combine task and VFS namespaces
    let flags = NS_CREATE_TASK | NS_CREATE_VFS;
    
    let result = syscall2(
        Syscall::CreateNamespace,
        flags,
        "full_container\0".as_ptr() as usize
    );
    
    if result == 0 {
        // Now in isolated container with:
        // - Independent PID space
        // - Isolated filesystem view
        println!("Full container created!");
    }
}
```

### Example 3: VFS Isolation Only

Creates filesystem isolation without changing PID space:

```rust
const NS_CREATE_VFS: usize = 0x02;

fn create_chroot_alternative() {
    // Only isolate filesystem
    let result = syscall2(
        Syscall::CreateNamespace,
        NS_CREATE_VFS,
        core::ptr::null::<u8>() as usize  // Auto-generate name
    );
    
    if result == 0 {
        // Filesystem is now isolated
        // Can safely pivot root without affecting other processes
        println!("VFS namespace created");
    }
}
```

## How It Works

### Task Namespace Isolation

When `NS_CREATE_TASK` is specified:

1. A new `TaskNamespace` is created as a child of the current namespace
2. The task's namespace reference is updated
3. A new namespace-local ID is allocated
4. Future `fork()` calls will use the new namespace

**Result**: The process and its children will have independent PID space.

### VFS Namespace Isolation

When `NS_CREATE_VFS` is specified:

1. A new `VfsManager` instance is created
2. Current working directory is inherited (if any)
3. The task's VFS reference is updated
4. Future filesystem operations use the isolated VFS

**Result**: The process can mount/unmount filesystems independently.

### Combined Isolation

Multiple flags can be combined:

```rust
// Create task, VFS, and (future) network isolation
let flags = NS_CREATE_TASK | NS_CREATE_VFS | NS_CREATE_NET;
syscall2(Syscall::CreateNamespace, flags, name);
```

## Namespace Hierarchy

Namespaces form a parent-child hierarchy:

```
Root Namespace
├── Container1 Namespace
│   ├── Task A (PID 1 in container)
│   └── Task B (PID 2 in container)
└── Container2 Namespace
    ├── Task C (PID 1 in container)
    └── Task D (PID 2 in container)
```

Child namespaces inherit from parent but allocate independent IDs.

## Demo Program

The `container_demo` program demonstrates all isolation types:

```bash
# Run the demo
$ container_demo

===========================================
  Scarlet Container Demo
  Demonstrating Namespace Isolation
===========================================

=== Task Namespace Demo ===
Parent process PID: 5
Parent created child with PID: 6
Child before namespace: PID = 6
Child after namespace: PID = 1 (in new namespace)
Child: Successfully created isolated task namespace!
Parent: Child completed

=== VFS Namespace Demo ===
Current PID: 5
Successfully created isolated VFS namespace!
This process now has an independent filesystem view

=== Combined Namespace Demo (Task + VFS) ===
Process PID before: 5
Process PID after: 1 (in new namespace)
Successfully created isolated task AND VFS namespaces!
This process now has:
  - Independent PID space
  - Isolated filesystem view
  - Ready for containerized execution

===========================================
  Demo Complete!
===========================================
```

## Comparison with Traditional Approaches

### Traditional Unix (unshare)
```c
unshare(CLONE_NEWPID | CLONE_NEWNS);  // Two separate syscalls conceptually
```

### Scarlet Approach
```rust
create_namespace(NS_CREATE_TASK | NS_CREATE_VFS, "container");  // Single smart syscall
```

**Advantages**:
- Single syscall for multiple isolations
- Named namespaces for easier debugging
- Extensible flag design
- Clean API that scales with new isolation types

## Future Extensions

The flag-based design allows easy addition of new isolation types:

- **Network Namespace** (`NS_CREATE_NET`): Isolated network stack
- **IPC Namespace** (`NS_CREATE_IPC`): Isolated IPC objects
- **User Namespace**: User/group ID isolation
- **Time Namespace**: Independent time settings
- **Cgroup Namespace**: Resource limits and accounting

## Integration with Other Features

### With VFS Pivot Root

```rust
// 1. Create VFS namespace
create_namespace(NS_CREATE_VFS, "container");

// 2. Mount new root
sys_fs_mount("tmpfs", "/new_root", "tmpfs", 0, NULL);

// 3. Pivot to new root
sys_fs_pivot_root("/new_root", "/new_root/old_root");

// Now fully isolated with new root filesystem
```

### With Process Groups

```rust
// 1. Create task namespace
create_namespace(NS_CREATE_TASK, "job");

// 2. Create process group for job control
// (future implementation)
```

## Best Practices

1. **Name your namespaces**: Makes debugging easier
2. **Combine isolations when possible**: More efficient than multiple syscalls
3. **Check return values**: Handle errors gracefully
4. **Document isolation requirements**: Clear what each container needs

## Security Considerations

- Namespace isolation provides **process-level separation**
- Does not provide security boundaries by itself
- Should be combined with other security features (capabilities, seccomp, etc.)
- VFS isolation prevents mount pollution but not filesystem access control

## References

- Linux namespaces: man 7 namespaces
- Container demo source: user/bin/src/container_demo.rs
